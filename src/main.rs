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
#[derive(Debug)]
struct ThreadInfo {
    /// メッセージのコピー先チャンネルID
    target_channel_id: Id<ChannelMarker>,
}

/// 環境変数からスレッドマッピング設定をパースする関数
///
/// 形式: `THREAD_MAPPING_*=thread_id:target_channel_id`
///
/// # 戻り値
/// * スレッドIDとターゲットチャンネル情報のハッシュマップ
fn parse_thread_mappings() -> HashMap<Id<ChannelMarker>, ThreadInfo> {
    env::vars()
        .filter(|(key, _)| key.starts_with("THREAD_MAPPING_"))
        .filter_map(|(_, value)| parse_thread_mapping_entry(&value))
        .collect()
}

/// 単一のスレッドマッピングエントリをパースする関数
///
/// # 引数
/// * `entry` - `thread_id:target_channel_id`形式の文字列
///
/// # 戻り値
/// * `パースに成功した場合はSome((thread_id, ThreadInfo))、失敗した場合はNone`
fn parse_thread_mapping_entry(entry: &str) -> Option<(Id<ChannelMarker>, ThreadInfo)> {
    let parts: Vec<&str> = entry.split(':').collect();
    if parts.len() != 2 {
        return None;
    }

    let thread_id = parts[0].parse::<u64>().ok()?;
    let target_channel_id = parts[1].parse::<u64>().ok()?;

    let thread_id = Id::new(thread_id);
    let target_channel_id = Id::new(target_channel_id);

    tracing::info!(
        "Added thread mapping: {} -> {}",
        thread_id,
        target_channel_id
    );

    Some((thread_id, ThreadInfo { target_channel_id }))
}

/// メッセージから送信者情報とコンテンツを含むフォーマット済みテキストを作成
///
/// # 引数
/// * `author_name` - 送信者の名前
/// * `content` - メッセージの内容
///
/// # 戻り値
/// * フォーマット済みテキスト
fn format_message_content(author_name: &str, content: &str) -> String {
    format!("**{author_name}**: {content}")
}

/// 添付ファイルのURLをフォーマットする
///
/// # 引数
/// * `attachments` - 添付ファイルのスライス
///
/// # 戻り値
/// * 添付ファイルURLを含むテキスト
fn format_attachments(attachments: &[twilight_model::channel::Attachment]) -> String {
    attachments
        .iter()
        .fold(String::new(), |mut acc, attachment| {
            acc.push('\n');
            acc.push_str(&attachment.url);
            acc
        })
}

/// ボットの状態を管理する構造体
/// HTTPクライアントとスレッドマッピング情報を保持します
#[derive(Debug)]
struct BotState {
    /// Discord HTTP APIクライアント
    http: HttpClient,
    /// スレッドID -> ターゲットチャンネルIDのマッピング
    /// キー: 監視対象のスレッドID、値: メッセージのコピー先チャンネル情報
    thread_mappings: HashMap<Id<ChannelMarker>, ThreadInfo>,
}

impl BotState {
    /// `BotState`構造体を作成し初期化する
    ///
    /// # 引数
    /// * `token` - Discord botのトークン
    ///
    /// # 戻り値
    /// * `初期化されたBotState構造体`
    fn new(token: String) -> Self {
        // HTTPクライアントの初期化
        let http = HttpClient::new(token);

        // 環境変数からスレッドマッピングを読み込む
        let thread_mappings = parse_thread_mappings();

        Self {
            http,
            thread_mappings,
        }
    }

    // 注: 現在使用されていない関数は残していますが、未使用の警告を抑制
    #[allow(dead_code)]
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

    /// スレッドID用のターゲットチャンネルを取得する
    ///
    /// # 引数
    /// * `thread_id` - 検索するスレッドID
    ///
    /// # 戻り値
    /// * `Option<Id<ChannelMarker>>` - ターゲットチャンネルIDが見つかった場合はSome、それ以外はNone
    fn get_target_channel(&self, thread_id: Id<ChannelMarker>) -> Option<Id<ChannelMarker>> {
        self.thread_mappings
            .get(&thread_id)
            .map(|info| info.target_channel_id)
    }

    /// メッセージをターゲットチャンネルに送信する
    ///
    /// # 引数
    /// * `target_channel_id` - 送信先チャンネルID
    /// * `content` - 送信するコンテンツ
    /// * `source_channel_id` - 元のチャンネルID（ロギング用）
    ///
    /// # 戻り値
    /// * `Result<()>` - 処理結果
    async fn send_message_to_channel(
        &self,
        target_channel_id: Id<ChannelMarker>,
        content: &str,
        source_channel_id: Id<ChannelMarker>,
    ) -> Result<()> {
        match self
            .http
            .create_message(target_channel_id)
            .content(content)
            .await
        {
            Ok(_) => {
                tracing::info!(
                    "Copied message from thread {} to channel {}",
                    source_channel_id,
                    target_channel_id
                );
                Ok(())
            }
            Err(e) => Err(anyhow::anyhow!("Failed to send message: {}", e)),
        }
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
        // ボットのメッセージは処理しない（無限ループ防止）
        if msg.author.bot {
            return Ok(());
        }

        // スレッドマッピングに登録されているかチェック
        let Some(target_channel_id) = self.get_target_channel(msg.channel_id) else {
            return Ok(());
        };

        // メッセージのフォーマット
        let content = format_message_content(&msg.author.name, &msg.content);
        let attachment_urls = format_attachments(&msg.attachments);
        let full_content = format!("{content}{attachment_urls}");

        // ターゲットチャンネルにメッセージを送信
        self.send_message_to_channel(target_channel_id, &full_content, msg.channel_id)
            .await
    }
}

/// Discordイベントを処理する関数
///
/// # 引数
/// * `event` - 処理するイベント
/// * `bot_state` - ボットの状態
async fn handle_event(event: Event, bot_state: Arc<Mutex<BotState>>) {
    match event {
        Event::MessageCreate(msg) => {
            // メッセージ作成イベントの処理
            if let Err(e) = bot_state.lock().await.handle_message(*msg).await {
                tracing::error!("Error handling message: {}", e);
            }
        }
        Event::Ready(_) => {
            // ボット準備完了イベントの処理
            tracing::info!("Bot is ready!");
        }
        // その他のイベントは無視
        _ => {}
    }
}

/// Discordトークンを環境変数から取得する
///
/// # 戻り値
/// * `Result<String>` - トークン取得結果
fn get_discord_token() -> Result<String> {
    env::var("DISCORD_TOKEN")
        .map_err(|_| anyhow::anyhow!("Missing DISCORD_TOKEN environment variable"))
}

/// イベントループを実行する関数
///
/// # 引数
/// * `shard` - Discordシャード
/// * `bot_state` - ボットの状態
///
/// # 戻り値
/// * `Result<()>` - 処理結果
async fn run_event_loop(mut shard: Shard, bot_state: Arc<Mutex<BotState>>) -> Result<()> {
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
        tokio::spawn(async move {
            handle_event(event, bot_state_clone).await;
        });
    }

    Ok(())
}

/// デバッグモードが有効かチェックする
///
/// # 戻り値
/// * `bool` - デバッグモードが有効な場合はtrue
fn is_debug_mode() -> bool {
    env::args().any(|arg| arg == "-debug")
}

/// メインエントリーポイント
/// ボットの初期化とイベントループの実行を行います
#[tokio::main]
async fn main() -> Result<()> {
    // .envファイルから環境変数を読み込む
    dotenv().ok();

    // ロギングの初期化
    tracing_subscriber::fmt::init();

    // デバッグモードの確認
    let debug_mode = is_debug_mode();

    // Discordトークンの取得
    let token = get_discord_token()?;

    // ボットステートの初期化
    let bot_state = Arc::new(Mutex::new(BotState::new(token.clone())));

    // インテント（権限）の設定
    let intents =
        Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT | Intents::GUILD_MESSAGE_REACTIONS;

    // シャードの作成
    let shard = Shard::new(ShardId::ONE, token, intents);

    tracing::info!("Bot started!");
    if debug_mode {
        tracing::info!("Debug mode enabled");
    }

    // イベントループを実行
    run_event_loop(shard, bot_state).await
}
