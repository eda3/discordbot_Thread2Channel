// スレッドのメッセージを指定したチャンネルに転送するDiscord bot
// Thread2Channelは、特定のスレッドに投稿されたメッセージを指定した別のチャンネルに自動的にコピーします
use anyhow::Result;
use dotenv::dotenv;
use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use tokio::sync::Mutex;

// Discord APIとのインタラクションに必要なクレート
use twilight_gateway::{Event, EventTypeFlags, Intents, Shard, ShardId, StreamExt};
use twilight_http::Client as HttpClient;
use twilight_model::gateway::payload::incoming::MessageCreate;
use twilight_model::id::{marker::ChannelMarker, Id};

/// スレッド情報を保持する構造体
/// 各スレッドがメッセージをコピーする先のターゲットチャンネルIDを格納します
struct ThreadInfo {
    /// メッセージのコピー先チャンネルID
    target_channel_id: Id<ChannelMarker>,
}

/// ボットの状態を管理する構造体
/// HTTPクライアントとスレッドマッピング情報を保持します
struct BotState {
    /// Discord HTTP APIクライアント
    http: HttpClient,
    /// スレッドID -> ターゲットチャンネルIDのマッピング
    /// キー: 監視対象のスレッドID、値: メッセージのコピー先チャンネル情報
    thread_mappings: HashMap<Id<ChannelMarker>, ThreadInfo>,
}

impl BotState {
    /// 新しいBotState構造体を作成し初期化する
    ///
    /// # 引数
    /// * `token` - Discord botのトークン
    ///
    /// # 戻り値
    /// * 初期化されたBotState構造体
    fn new(token: String) -> Self {
        // HTTPクライアントの初期化
        let http = HttpClient::new(token);

        // 初期スレッドマッピング
        let mut thread_mappings = HashMap::new();

        // 環境変数からスレッドIDとターゲットチャンネルIDのマッピングを読み込む
        // 形式: THREAD_MAPPING_1=thread_id:target_channel_id
        // 例: THREAD_MAPPING_1=123456789:987654321
        for (key, value) in env::vars() {
            if key.starts_with("THREAD_MAPPING_") {
                let parts: Vec<&str> = value.split(':').collect();
                if parts.len() == 2 {
                    if let (Ok(thread_id), Ok(target_channel_id)) =
                        (parts[0].parse::<u64>(), parts[1].parse::<u64>())
                    {
                        thread_mappings.insert(
                            Id::new(thread_id),
                            ThreadInfo {
                                target_channel_id: Id::new(target_channel_id),
                            },
                        );
                        tracing::info!(
                            "Added thread mapping: {} -> {}",
                            thread_id,
                            target_channel_id
                        );
                    }
                }
            }
        }

        Self {
            http,
            thread_mappings,
        }
    }

    /// 新しいスレッドマッピングを追加する
    ///
    /// # 引数
    /// * `thread_id` - 監視対象のスレッドID
    /// * `target_channel_id` - メッセージのコピー先チャンネルID
    async fn add_thread_mapping(
        &mut self,
        thread_id: Id<ChannelMarker>,
        target_channel_id: Id<ChannelMarker>,
    ) {
        self.thread_mappings
            .insert(thread_id, ThreadInfo { target_channel_id });
        tracing::info!(
            "Added thread mapping: {} -> {}",
            thread_id,
            target_channel_id
        );
    }

    /// メッセージを処理する
    /// スレッドからのメッセージを対応するターゲットチャンネルにコピーします
    ///
    /// # 引数
    /// * `msg` - 処理するメッセージ
    ///
    /// # 戻り値
    /// * `Result<()>` - 処理結果。エラーが発生した場合はエラー情報を含む
    async fn handle_message(&self, msg: MessageCreate) -> Result<()> {
        // スレッドからのメッセージならコピーする
        // スレッドIDが登録されているかチェック
        if let Some(thread_info) = self.thread_mappings.get(&msg.channel_id) {
            // メッセージの作成者がボット自身でないことを確認
            // ボットのメッセージをコピーすると無限ループが発生する可能性があるため
            if !msg.author.bot {
                // メッセージをターゲットチャンネルにコピー
                // 作成者の名前とメッセージ内容を含むフォーマットを作成
                let content = format!("**{}**: {}", msg.author.name, msg.content);

                // 添付ファイルがある場合、そのURLも含める
                let attachment_urls = msg
                    .attachments
                    .iter()
                    .map(|attachment| format!("\n{}", attachment.url))
                    .collect::<String>();

                let full_content = format!("{}{}", content, attachment_urls);

                // ターゲットチャンネルにメッセージを送信
                match self
                    .http
                    .create_message(thread_info.target_channel_id)
                    .content(&full_content)
                    .await
                {
                    Ok(_) => {}
                    Err(e) => {
                        return Err(anyhow::anyhow!("Failed to send message: {}", e));
                    }
                }

                tracing::info!(
                    "Copied message from thread {} to channel {}",
                    msg.channel_id,
                    thread_info.target_channel_id
                );
            }
        }

        Ok(())
    }
}

/// メインエントリーポイント
/// ボットの初期化とイベントループの実行を行います
#[tokio::main]
async fn main() -> Result<()> {
    // .envファイルから環境変数を読み込む
    // 環境変数にはDISCORD_TOKENとTHREAD_MAPPING_*が必要
    dotenv().ok();

    // ロギングの初期化
    // デバッグやエラー情報を出力するために使用
    tracing_subscriber::fmt::init();

    // コマンドライン引数の解析
    // -debugフラグでデバッグモードを有効化
    let debug_mode = env::args().any(|arg| arg == "-debug");

    // Discordトークンの取得
    // 環境変数からトークンを読み込み、見つからない場合はエラーを返す
    let token = env::var("DISCORD_TOKEN")
        .map_err(|_| anyhow::anyhow!("Missing DISCORD_TOKEN environment variable"))?;

    // ボットステートの初期化
    // スレッドセーフな共有参照を作成するためにArcとMutexを使用
    let bot_state = Arc::new(Mutex::new(BotState::new(token.clone())));

    // インテント（権限）の設定
    // ボットがDiscordから受け取るイベントの種類を指定
    let intents =
        Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT | Intents::GUILD_MESSAGE_REACTIONS;

    // シャードの作成
    // Discordゲートウェイへの接続を管理
    let mut shard = Shard::new(ShardId::ONE, token, intents);

    tracing::info!("Bot started!");
    if debug_mode {
        tracing::info!("Debug mode enabled");
    }

    // イベントループ
    // Discordから送られてくるイベントを永続的に処理
    loop {
        // 次のイベントを非同期に待機
        let event = match shard.next_event(EventTypeFlags::all()).await {
            Some(Ok(event)) => event,
            Some(Err(e)) => {
                tracing::error!("Error receiving event: {}", e);
                continue;
            }
            None => {
                tracing::warn!("Event stream ended");
                break;
            }
        };

        // 各イベント処理を別タスクで実行するためのボット状態のクローン
        let bot_state_clone = Arc::clone(&bot_state);

        // イベント処理を別スレッドで実行
        // 各イベントを非同期に処理し、メインループをブロックしない
        tokio::spawn(async move {
            match event {
                // メッセージ作成イベントの処理
                Event::MessageCreate(msg) => {
                    if let Err(e) = bot_state_clone.lock().await.handle_message(*msg).await {
                        tracing::error!("Error handling message: {}", e);
                    }
                }
                // ボット準備完了イベントの処理
                Event::Ready(_) => {
                    tracing::info!("Bot is ready!");
                }
                // その他のイベントは無視
                _ => {}
            }
        });
    }

    Ok(())
}
