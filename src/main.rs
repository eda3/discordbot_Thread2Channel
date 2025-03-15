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
    // 環境変数の一覧をデバッグ出力
    tracing::debug!("環境変数の一覧:");
    for (key, value) in env::vars() {
        if key.starts_with("THREAD_MAPPING_") {
            tracing::debug!("  環境変数: {}={}", key, value);
        }
    }

    let thread_mappings = env::vars()
        .filter(|(key, _)| key.starts_with("THREAD_MAPPING_"))
        .filter_map(|(_, value)| parse_thread_mapping_entry(&value))
        .collect::<HashMap<_, _>>();

    // マッピングの総数を出力
    tracing::info!(
        "読み込まれたスレッドマッピングの総数: {}",
        thread_mappings.len()
    );

    // すべてのマッピングを出力
    for (thread_id, info) in &thread_mappings {
        tracing::info!(
            "マッピング情報: スレッドID {} -> チャンネルID {}",
            thread_id,
            info.target_channel_id
        );
    }

    // 特定のスレッドIDが含まれているか確認（ユーザー指定のIDをチェック）
    let target_thread_id = 1_350_283_354_309_660_672_u64;
    let id = Id::new(target_thread_id);
    if thread_mappings.contains_key(&id) {
        tracing::info!(
            "指定されたスレッドID {}のマッピングが見つかりました。ターゲットチャンネル: {}",
            target_thread_id,
            thread_mappings.get(&id).unwrap().target_channel_id
        );
    } else {
        tracing::warn!(
            "指定されたスレッドID {}のマッピングが見つかりません",
            target_thread_id
        );
    }

    thread_mappings
}

/// 単一のスレッドマッピングエントリをパースする関数
///
/// # 引数
/// * `entry` - `thread_id:target_channel_id`形式の文字列
///
/// # 戻り値
/// * `パースに成功した場合はSome((thread_id, ThreadInfo))、失敗した場合はNone`
fn parse_thread_mapping_entry(entry: &str) -> Option<(Id<ChannelMarker>, ThreadInfo)> {
    tracing::debug!("スレッドマッピングエントリのパース: {}", entry);

    let parts: Vec<&str> = entry.split(':').collect();
    if parts.len() != 2 {
        tracing::warn!(
            "不正なマッピングフォーマット: {}（形式は thread_id:channel_id である必要があります）",
            entry
        );
        return None;
    }

    let thread_id_result = parts[0].parse::<u64>();
    let target_channel_id_result = parts[1].parse::<u64>();

    if let Err(e) = &thread_id_result {
        tracing::warn!("スレッドIDのパースに失敗: {} - エラー: {}", parts[0], e);
    }

    if let Err(e) = &target_channel_id_result {
        tracing::warn!(
            "ターゲットチャンネルIDのパースに失敗: {} - エラー: {}",
            parts[1],
            e
        );
    }

    let thread_id = thread_id_result.ok()?;
    let target_channel_id = target_channel_id_result.ok()?;

    let thread_id = Id::new(thread_id);
    let target_channel_id = Id::new(target_channel_id);

    tracing::info!(
        "スレッドマッピングを追加: {} -> {}",
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
    let formatted = format!("**{author_name}**: {content}");
    tracing::debug!("メッセージをフォーマット: {}", formatted);
    formatted
}

/// 添付ファイルのURLをフォーマットする
///
/// # 引数
/// * `attachments` - 添付ファイルのスライス
///
/// # 戻り値
/// * 添付ファイルURLを含むテキスト
fn format_attachments(attachments: &[twilight_model::channel::Attachment]) -> String {
    if attachments.is_empty() {
        tracing::debug!("添付ファイルなし");
        return String::new();
    }

    tracing::debug!("添付ファイル数: {}", attachments.len());

    let urls = attachments
        .iter()
        .fold(String::new(), |mut acc, attachment| {
            tracing::debug!("添付ファイル: {}", attachment.url);
            acc.push('\n');
            acc.push_str(&attachment.url);
            acc
        });

    tracing::debug!("添付ファイルURLをフォーマット: {}", urls);
    urls
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
        tracing::info!("BotStateを初期化中...");

        // HTTPクライアントの初期化
        let http = HttpClient::new(token);
        tracing::debug!("HTTP APIクライアントを初期化しました");

        // 環境変数からスレッドマッピングを読み込む
        let thread_mappings = parse_thread_mappings();
        tracing::info!(
            "スレッドマッピングを読み込みました ({}件)",
            thread_mappings.len()
        );

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
            "スレッドマッピングを追加: {} -> {}",
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
        tracing::debug!("スレッドID {}のターゲットチャンネルを検索中...", thread_id);

        // マップ内のすべてのキーを表示（デバッグ用）
        tracing::debug!("マップ内のスレッドID一覧:");
        for key in self.thread_mappings.keys() {
            tracing::debug!("  マップ内のスレッドID: {}", key);
        }

        let target = self
            .thread_mappings
            .get(&thread_id)
            .map(|info| info.target_channel_id);

        if let Some(channel_id) = target {
            tracing::info!(
                "スレッドID {}のターゲットチャンネルが見つかりました: {}",
                thread_id,
                channel_id
            );
            Some(channel_id)
        } else {
            tracing::warn!(
                "スレッドID {}のターゲットチャンネルが見つかりません",
                thread_id
            );
            None
        }
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
        tracing::debug!(
            "メッセージを送信します: スレッド {} -> チャンネル {}, 内容: {}",
            source_channel_id,
            target_channel_id,
            content
        );

        let result = self
            .http
            .create_message(target_channel_id)
            .content(content)
            .await;

        match &result {
            Ok(_) => {
                tracing::info!(
                    "メッセージを正常に転送しました: スレッド {} -> チャンネル {}",
                    source_channel_id,
                    target_channel_id
                );
            }
            Err(e) => {
                tracing::error!(
                    "メッセージ送信に失敗しました: スレッド {} -> チャンネル {}, エラー: {}",
                    source_channel_id,
                    target_channel_id,
                    e
                );
            }
        }

        result
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!("Failed to send message: {}", e))
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
        tracing::debug!(
            "メッセージを受信: チャンネル/スレッドID: {}, 作成者: {}, 内容: {}, ボット?: {}",
            msg.channel_id,
            msg.author.name,
            msg.content,
            msg.author.bot
        );

        // 対象の特定のスレッドIDかどうかを確認（デバッグ用）
        let target_thread_id = 1_350_283_354_309_660_672_u64;
        let id = Id::new(target_thread_id);
        if msg.channel_id == id {
            tracing::info!(
                "注目のスレッドIDからメッセージを受信: スレッドID {}, 作成者: {}, 内容: {}",
                msg.channel_id,
                msg.author.name,
                msg.content
            );
        }

        // ボットのメッセージは処理しない（無限ループ防止）
        if msg.author.bot {
            tracing::debug!("ボットからのメッセージは処理しません: {}", msg.author.name);
            return Ok(());
        }

        // スレッドマッピングに登録されているかチェック
        let Some(target_channel_id) = self.get_target_channel(msg.channel_id) else {
            tracing::debug!(
                "スレッドID {}はマッピングに登録されていません",
                msg.channel_id
            );
            return Ok(());
        };

        tracing::info!(
            "メッセージ転送処理を開始: スレッド {} -> チャンネル {}",
            msg.channel_id,
            target_channel_id
        );

        // メッセージのフォーマット
        let content = format_message_content(&msg.author.name, &msg.content);
        let attachment_urls = format_attachments(&msg.attachments);
        let full_content = format!("{content}{attachment_urls}");

        tracing::debug!("転送するメッセージ内容: {}", full_content);

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
            tracing::debug!(
                "MessageCreateイベントを受信: チャンネル/スレッドID: {}",
                msg.channel_id
            );

            // メッセージ作成イベントの処理
            if let Err(e) = bot_state.lock().await.handle_message(*msg).await {
                tracing::error!("メッセージ処理中にエラーが発生: {}", e);
            }
        }
        Event::Ready(_) => {
            // ボット準備完了イベントの処理
            tracing::info!("Botの準備が完了しました！");
        }
        // その他のイベントは無視
        _ => {
            tracing::trace!("その他のイベントを受信: {:?}", event.kind());
        }
    }
}

/// Discordトークンを環境変数から取得する
///
/// # 戻り値
/// * `Result<String>` - トークン取得結果
fn get_discord_token() -> Result<String> {
    tracing::debug!("環境変数からDiscordトークンを取得中...");

    let token = env::var("DISCORD_TOKEN")
        .map_err(|_| anyhow::anyhow!("Missing DISCORD_TOKEN environment variable"))?;

    tracing::debug!("Discordトークンを取得しました (長さ: {}文字)", token.len());
    Ok(token)
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
    tracing::info!("イベントループを開始します");

    loop {
        // 次のイベントを非同期に待機
        tracing::debug!("次のイベントを待機中...");

        let event = match shard.next_event(EventTypeFlags::all()).await {
            Some(Ok(event)) => {
                tracing::debug!("イベント受信: タイプ {:?}", event.kind());
                event
            }
            Some(Err(e)) => {
                tracing::error!("イベント受信中にエラーが発生: {}", e);
                continue;
            }
            None => {
                tracing::warn!("イベントストリームが終了しました");
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

    tracing::info!("イベントループを終了します");
    Ok(())
}

/// デバッグモードが有効かチェックする
///
/// # 戻り値
/// * `bool` - デバッグモードが有効な場合はtrue
fn is_debug_mode() -> bool {
    let debug_mode = env::args().any(|arg| arg == "-debug");
    tracing::debug!("デバッグモード: {}", debug_mode);
    debug_mode
}

/// メインエントリーポイント
/// ボットの初期化とイベントループの実行を行います
#[tokio::main]
async fn main() -> Result<()> {
    // .envファイルから環境変数を読み込む
    dotenv().ok();
    tracing::debug!(".envファイルを読み込みました");

    // ロギングの初期化
    tracing_subscriber::fmt::init();
    tracing::info!("ロギングを初期化しました");

    // デバッグモードの確認
    let debug_mode = is_debug_mode();

    // Discordトークンの取得
    let token = get_discord_token()?;

    // ボットステートの初期化
    tracing::info!("ボットステートを初期化しています...");
    let bot_state = Arc::new(Mutex::new(BotState::new(token.clone())));
    tracing::info!("ボットステートの初期化が完了しました");

    // インテント（権限）の設定
    let intents =
        Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT | Intents::GUILD_MESSAGE_REACTIONS;
    tracing::debug!("インテントを設定しました: {:?}", intents);

    // シャードの作成
    tracing::info!("シャードを初期化しています...");
    let shard = Shard::new(ShardId::ONE, token, intents);
    tracing::info!("シャードの初期化が完了しました");

    tracing::info!("Botが起動しました！");
    if debug_mode {
        tracing::info!("デバッグモードが有効です");
    }

    // イベントループを実行
    tracing::info!("イベントループを開始します...");
    run_event_loop(shard, bot_state).await
}
