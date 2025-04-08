// スレッドのメッセージを指定したチャンネルに転送するDiscord bot
// Thread2Channelは、特定のスレッドに投稿されたメッセージを指定した別のチャンネルに自動的にコピーします
use anyhow::Result;
use dotenv::dotenv;
use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::Duration;

// Discord APIとのインタラクションに必要なクレート
use twilight_gateway::{Event, EventTypeFlags, Intents, Shard, ShardId, StreamExt};
use twilight_http::request::channel::message::CreateMessage;
use twilight_http::Client as HttpClient;
use twilight_model::channel::message::embed::{Embed, EmbedAuthor, EmbedField, EmbedFooter};
use twilight_model::channel::message::MessageType;
use twilight_model::channel::ChannelType;
use twilight_model::gateway::payload::incoming::MessageCreate;
use twilight_model::id::{marker::ChannelMarker, Id};
use twilight_model::user::User;
use twilight_model::util::Timestamp;

/// スレッド情報を保持する構造体
/// 各スレッドがメッセージをコピーする先のターゲットチャンネルIDと転送設定を格納します
#[derive(Debug, Clone)]
struct ThreadInfo {
    /// メッセージのコピー先チャンネルID
    target_channel_id: Id<ChannelMarker>,
    /// 過去のメッセージを全て取得して転送するかどうか
    transfer_all_messages: bool,
}

/// `環境変数のキーが"THREAD_MAPPING_"`で始まるかどうかを判定する関数
///
/// # 引数
/// * `key` - チェックする環境変数キー
///
/// # 戻り値
/// * `bool` - キーがパターンにマッチする場合はtrue
fn is_thread_mapping_key(key: &str) -> bool {
    key.starts_with("THREAD_MAPPING_")
}

/// スレッドマッピング環境変数のデバッグ情報を出力する関数
///
/// # 引数
/// * `key` - 環境変数のキー
/// * `value` - 環境変数の値
fn log_thread_mapping_env(key: &str, value: &str) {
    tracing::debug!("  環境変数: {}={}", key, value);
}

/// スレッドマッピングの要約情報をログに出力する関数
///
/// # 引数
/// * `mappings` - スレッドマッピングのハッシュマップ
fn log_thread_mappings_summary(mappings: &HashMap<Id<ChannelMarker>, ThreadInfo>) {
    tracing::info!("読み込まれたスレッドマッピングの総数: {}", mappings.len());

    // すべてのマッピングを出力
    for (thread_id, info) in mappings {
        tracing::info!(
            "マッピング情報: スレッドID {} -> チャンネルID {}",
            thread_id,
            info.target_channel_id
        );
    }
}

/// 特定のスレッドIDがマッピングに含まれているか確認し、結果をログに出力する関数
///
/// # 引数
/// * `target_id` - 確認するスレッドID
/// * `mappings` - スレッドマッピングのハッシュマップ
fn check_target_thread_exists(target_id: u64, mappings: &HashMap<Id<ChannelMarker>, ThreadInfo>) {
    let id = Id::new(target_id);

    mappings.get(&id).map_or_else(
        || {
            tracing::warn!(
                "指定されたスレッドID {}のマッピングが見つかりません",
                target_id
            );
        },
        |info| {
            tracing::info!(
                "指定されたスレッドID {}のマッピングが見つかりました。ターゲットチャンネル: {}",
                target_id,
                info.target_channel_id
            );
        },
    );
}

/// 環境変数からスレッドマッピング設定をパースする関数
///
/// 形式: `THREAD_MAPPING_*=thread_id:target_channel_id[:all]`
/// `all` パラメータが指定されている場合、そのスレッドの全メッセージを転送します
///
/// # 戻り値
/// * スレッドIDとターゲットチャンネル情報のハッシュマップ
fn parse_thread_mappings() -> HashMap<Id<ChannelMarker>, ThreadInfo> {
    // 環境変数の一覧をデバッグ出力
    tracing::debug!("環境変数の一覧:");
    env::vars()
        .filter(|(key, _)| is_thread_mapping_key(key))
        .for_each(|(key, value)| log_thread_mapping_env(&key, &value));

    let thread_mappings = env::vars()
        .filter(|(key, _)| is_thread_mapping_key(key))
        .filter_map(|(_, value)| parse_thread_mapping_entry(&value))
        .collect::<HashMap<_, _>>();

    // マッピングの総数と詳細を出力
    log_thread_mappings_summary(&thread_mappings);

    // 特定のスレッドIDが含まれているか確認（ユーザー指定のIDをチェック）
    check_target_thread_exists(1_350_283_354_309_660_672_u64, &thread_mappings);

    thread_mappings
}

/// `エントリが有効なThreadMappingフォーマットかどうかをチェックする関数`
///
/// # 引数
/// * `parts` - スプリットされたエントリ部分のベクター
///
/// # 戻り値
/// * `bool` - フォーマットが正しい場合はtrue
fn is_valid_thread_mapping_format(parts: &[&str]) -> bool {
    !(parts.len() < 2 || parts.len() > 3)
}

/// スレッドIDとチャンネルIDをパースする関数
///
/// # 引数
/// * `thread_id_str` - スレッドIDの文字列
/// * `channel_id_str` - チャンネルIDの文字列
///
/// # 戻り値
/// * `Option<(u64, u64)>` - パースに成功した場合は数値のタプル、失敗した場合はNone
fn parse_ids(thread_id_str: &str, channel_id_str: &str) -> Option<(u64, u64)> {
    let thread_id_result = thread_id_str.parse::<u64>();
    let target_channel_id_result = channel_id_str.parse::<u64>();

    if let Err(e) = &thread_id_result {
        tracing::warn!(
            "スレッドIDのパースに失敗: {} - エラー: {}",
            thread_id_str,
            e
        );
    }

    if let Err(e) = &target_channel_id_result {
        tracing::warn!(
            "ターゲットチャンネルIDのパースに失敗: {} - エラー: {}",
            channel_id_str,
            e
        );
    }

    match (thread_id_result, target_channel_id_result) {
        (Ok(thread_id), Ok(channel_id)) => Some((thread_id, channel_id)),
        _ => None,
    }
}

/// 単一のスレッドマッピングエントリをパースする関数
///
/// # 引数
/// * `entry` - `thread_id:target_channel_id[:all]`形式の文字列
///
/// # 戻り値
/// * `パースに成功した場合はSome((thread_id, ThreadInfo))、失敗した場合はNone`
fn parse_thread_mapping_entry(entry: &str) -> Option<(Id<ChannelMarker>, ThreadInfo)> {
    tracing::debug!("スレッドマッピングエントリのパース: {}", entry);

    let parts: Vec<&str> = entry.split(':').collect();

    if !is_valid_thread_mapping_format(&parts) {
        tracing::warn!(
            "不正なマッピングフォーマット: {}（形式は thread_id:channel_id[:all] である必要があります）",
            entry
        );
        return None;
    }

    let (thread_id, target_channel_id) = parse_ids(parts[0], parts[1])?;

    // 全メッセージ転送フラグをチェック
    let transfer_all_messages = parts.len() == 3 && parts[2] == "all";

    let thread_id = Id::new(thread_id);
    let target_channel_id = Id::new(target_channel_id);

    tracing::info!(
        "スレッドマッピングを追加: {} -> {} (全メッセージ転送: {})",
        thread_id,
        target_channel_id,
        transfer_all_messages
    );

    Some((
        thread_id,
        ThreadInfo {
            target_channel_id,
            transfer_all_messages,
        },
    ))
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

/// 送信者情報、メッセージコンテンツ、添付ファイルから完全なメッセージコンテンツを作成する関数
///
/// # 引数
/// * `author_name` - 送信者の名前
/// * `content` - メッセージの内容
/// * `attachments` - 添付ファイルのスライス
///
/// # 戻り値
/// * フォーマット済みの完全なコンテンツ
fn create_full_message_content(
    author_name: &str,
    content: &str,
    attachments: &[twilight_model::channel::Attachment],
) -> String {
    let formatted_content = format_message_content(author_name, content);
    let attachment_urls = format_attachments(attachments);
    format!("{formatted_content}{attachment_urls}")
}

/// メッセージが通常のメッセージで、ボットからのものではないかを判定する関数
///
/// # 引数
/// * `message` - 判定するメッセージ
///
/// # 戻り値
/// * `bool` - 通常のユーザーメッセージの場合はtrue
fn is_regular_user_message(message: &twilight_model::channel::Message) -> bool {
    message.kind == MessageType::Regular && !message.author.bot
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
        self.thread_mappings.insert(
            thread_id,
            ThreadInfo {
                target_channel_id,
                transfer_all_messages: false,
            },
        );
        tracing::info!(
            "スレッドマッピングを追加: {} -> {}",
            thread_id,
            target_channel_id
        );
    }

    /// スレッドID用のスレッド情報を取得する
    ///
    /// # 引数
    /// * `thread_id` - 検索するスレッドID
    ///
    /// # 戻り値
    /// * `Option<&ThreadInfo>` - スレッド情報が見つかった場合はSome、それ以外はNone
    fn get_thread_info(&self, thread_id: Id<ChannelMarker>) -> Option<&ThreadInfo> {
        tracing::debug!("スレッドID {}の情報を検索中...", thread_id);

        // マップ内のすべてのキーを表示（デバッグ用）
        tracing::debug!("マップ内のスレッドID一覧:");
        self.thread_mappings.keys().for_each(|key| {
            tracing::debug!("  マップ内のスレッドID: {}", key);
        });

        // スレッド情報を取得し、ログ出力
        self.thread_mappings.get(&thread_id)
            .inspect(|info| {
                tracing::info!(
                    "スレッドID {}の情報が見つかりました: ターゲットチャンネル={}, 全メッセージ転送={}",
                    thread_id,
                    info.target_channel_id,
                    info.transfer_all_messages
                );
            })
            .or_else(|| {
                tracing::warn!("スレッドID {}の情報が見つかりません", thread_id);
                None
            })
    }

    /// スレッドID用のターゲットチャンネルを取得する
    ///
    /// # 引数
    /// * `thread_id` - 検索するスレッドID
    ///
    /// # 戻り値
    /// * `Option<Id<ChannelMarker>>` - ターゲットチャンネルIDが見つかった場合はSome、それ以外はNone
    #[allow(dead_code)]
    fn get_target_channel(&self, thread_id: Id<ChannelMarker>) -> Option<Id<ChannelMarker>> {
        self.get_thread_info(thread_id)
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

    /// チャンネルメッセージ取得を試行し、エラーならメッセージを送信して終了する
    ///
    /// # 引数
    /// * `thread_id` - メッセージを取得するスレッドID
    /// * `target_channel_id` - エラー報告先のチャンネルID
    ///
    /// # 戻り値
    /// * `Result<Vec<twilight_model::channel::Message>>` - 取得したメッセージまたはエラー
    async fn try_fetch_messages(
        &self,
        thread_id: Id<ChannelMarker>,
        target_channel_id: Id<ChannelMarker>,
    ) -> Result<Vec<twilight_model::channel::Message>> {
        // メッセージ履歴を取得
        let messages_result = self.http.channel_messages(thread_id).limit(100).await;

        if let Err(e) = &messages_result {
            tracing::error!("メッセージ履歴の取得に失敗: {}", e);
            let error_message = format!("❌ メッセージ履歴の取得に失敗しました: {e}");
            self.send_message_to_channel(target_channel_id, &error_message, thread_id)
                .await?;
            return Err(anyhow::anyhow!("Failed to fetch message history: {}", e));
        }

        // モデルを取得
        let model_result = messages_result.unwrap().model().await;

        if let Err(e) = &model_result {
            tracing::error!("メッセージモデルの取得に失敗: {}", e);
            let error_message = format!("❌ メッセージの処理に失敗しました: {e}");
            self.send_message_to_channel(target_channel_id, &error_message, thread_id)
                .await?;
            return Err(anyhow::anyhow!("Failed to model messages: {}", e));
        }

        Ok(model_result.unwrap())
    }

    /// メッセージを転送する処理を実行
    ///
    /// # 引数
    /// * `message` - 転送するメッセージ
    /// * `target_channel_id` - 転送先チャンネルID
    /// * `thread_id` - 元のスレッドID
    ///
    /// # 戻り値
    /// * `Result<()>` - 処理結果
    async fn transfer_single_message(
        &self,
        message: &twilight_model::channel::Message,
        target_channel_id: Id<ChannelMarker>,
        thread_id: Id<ChannelMarker>,
    ) -> Result<()> {
        // 埋め込みメッセージを作成
        let embed = MessageEmbedBuilder::new()
            .with_author(&message.author)
            .with_description(message.content.clone())
            .with_color(calculate_color(message.author.id.get()))
            .with_timestamp(message.timestamp);

        // 添付ファイルの処理
        let embed = message
            .attachments
            .iter()
            .fold(embed, |builder, attachment| {
                builder.add_field(
                    "添付ファイル".to_string(),
                    format!("[{}]({})", attachment.filename, attachment.url),
                    false,
                )
            });

        // 埋め込みメッセージを送信
        let embed = embed.build();

        match self
            .http
            .create_message(target_channel_id)
            .embeds(&[embed])
            .await
        {
            Ok(_) => {
                tracing::info!(
                    "メッセージを転送: スレッド {} -> チャンネル {} (埋め込み形式)",
                    thread_id,
                    target_channel_id
                );
                Ok(())
            }
            Err(e) => {
                tracing::error!("埋め込みメッセージの送信に失敗: {}", e);

                // フォールバック: テキスト形式でメッセージを送信
                let full_content = create_full_message_content(
                    &message.author.name,
                    &message.content,
                    &message.attachments,
                );

                self.send_message_to_channel(target_channel_id, &full_content, thread_id)
                    .await
            }
        }
    }

    /// 指定されたスレッドの全メッセージを取得して転送する
    ///
    /// # 引数
    /// * `thread_id` - メッセージを取得するスレッドID
    /// * `target_channel_id` - 転送先チャンネルID
    ///
    /// # 戻り値
    /// * `Result<()>` - 処理結果
    async fn fetch_and_transfer_all_messages(
        &self,
        thread_id: Id<ChannelMarker>,
        target_channel_id: Id<ChannelMarker>,
    ) -> Result<()> {
        tracing::info!(
            "スレッド {} の全メッセージの取得と転送を開始します",
            thread_id
        );

        // 最初に転送準備中のメッセージを送信
        let status_message = "🔄 このスレッドのメッセージを全て取得して転送しています...";
        self.send_message_to_channel(target_channel_id, status_message, thread_id)
            .await?;

        // メッセージ履歴を取得して処理
        let messages = self
            .try_fetch_messages(thread_id, target_channel_id)
            .await?;
        let total_messages = messages.len();

        tracing::info!("取得したメッセージ数: {}", total_messages);

        // 転送するメッセージの総数を通知
        let info_message =
            format!("ℹ️ このスレッドから {total_messages} 件のメッセージを転送します");
        self.send_message_to_channel(target_channel_id, &info_message, thread_id)
            .await?;

        // メッセージを古い順から処理し、転送（フィルタリング、変換、送信をパイプラインで処理）
        futures::future::try_join_all(
            messages
                .iter()
                .rev() // 古い順にするため逆順に
                .filter(|msg| is_regular_user_message(msg))
                .map(|msg| {
                    let target_id = target_channel_id;
                    let src_id = thread_id;
                    async move {
                        // メッセージ転送
                        self.transfer_single_message(msg, target_id, src_id).await?;

                        // レート制限を避けるために少し待機
                        tokio::time::sleep(Duration::from_millis(500)).await;

                        Ok::<_, anyhow::Error>(())
                    }
                }),
        )
        .await?;

        // 転送完了メッセージを送信
        let completion_message =
            format!("✅ スレッドからの {total_messages} 件のメッセージの転送が完了しました");
        self.send_message_to_channel(target_channel_id, &completion_message, thread_id)
            .await?;

        tracing::info!("スレッド {} の全メッセージの転送が完了しました", thread_id);
        Ok(())
    }

    /// 受信したコマンドを処理する
    ///
    /// # 引数
    /// * `command` - コマンド文字列
    /// * `thread_info` - スレッド情報
    /// * `thread_id` - スレッドID
    ///
    /// # 戻り値
    /// * `Option<Result<()>>` - コマンドを処理した場合は結果、コマンドではない場合はNone
    async fn handle_command(
        &self,
        command: &str,
        thread_info: &ThreadInfo,
        thread_id: Id<ChannelMarker>,
    ) -> Option<Result<()>> {
        let target_channel_id = thread_info.target_channel_id;
        let trimmed_command = command.trim();

        // コマンドを判別して適切な処理を行う
        match trimmed_command {
            "!all" => {
                tracing::info!("「!all」コマンドを検出、全メッセージの転送を開始します");
                Some(
                    self.fetch_and_transfer_all_messages(thread_id, target_channel_id)
                        .await,
                )
            }
            "!start" if thread_info.transfer_all_messages => {
                tracing::info!("全メッセージ転送設定が有効です。転送を開始します");
                Some(
                    self.fetch_and_transfer_all_messages(thread_id, target_channel_id)
                        .await,
                )
            }
            _ => None,
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
        let Some(thread_info) = self.get_thread_info(msg.channel_id) else {
            tracing::debug!(
                "スレッドID {}はマッピングに登録されていません",
                msg.channel_id
            );
            return Ok(());
        };

        let target_channel_id = thread_info.target_channel_id;

        tracing::info!(
            "メッセージ転送処理を開始: スレッド {} -> チャンネル {}",
            msg.channel_id,
            target_channel_id
        );

        // コマンド処理を試みる
        if let Some(result) = self
            .handle_command(&msg.content, thread_info, msg.channel_id)
            .await
        {
            return result;
        }

        // 通常のメッセージ処理
        let full_content =
            create_full_message_content(&msg.author.name, &msg.content, &msg.attachments);

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

            // メッセージ処理をスポーンして実行
            let bot_state_clone = Arc::clone(&bot_state);
            let msg_owned = *msg;
            tokio::spawn(async move {
                // ボット自身のメッセージは無視
                if msg_owned.author.bot {
                    return;
                }

                let state = bot_state_clone.lock().await;
                let http = &state.http;

                // スレッド内のメッセージかどうかを確認
                if let Ok(channel_result) = http.channel(msg_owned.channel_id).await {
                    if let Ok(channel) = channel_result.model().await {
                        if channel.kind == ChannelType::PublicThread
                            || channel.kind == ChannelType::PrivateThread
                        {
                            // スレッドマッピングに登録されているかチェック
                            if let Some(thread_info) = state.get_thread_info(msg_owned.channel_id) {
                                // コマンド処理のみ実行
                                let target_channel_id = thread_info.target_channel_id;
                                let content = msg_owned.content.trim();

                                // コマンドかどうかチェック
                                let is_all_command = content == "!all";
                                let is_start_command =
                                    content == "!start" && thread_info.transfer_all_messages;

                                if is_all_command || is_start_command {
                                    // 全メッセージ転送を実行
                                    tracing::info!(
                                        "コマンド「{}」を検出、全メッセージの転送を開始します",
                                        content
                                    );

                                    // ロックを解放してから処理を行う（デッドロック防止）
                                    drop(state);

                                    // 新しいスコープで再度ロックを取得
                                    let state = bot_state_clone.lock().await;
                                    if let Err(e) = state
                                        .fetch_and_transfer_all_messages(
                                            msg_owned.channel_id,
                                            target_channel_id,
                                        )
                                        .await
                                    {
                                        tracing::error!("全メッセージ転送に失敗: {}", e);
                                    }
                                }

                                // 通常のメッセージ転送は実行しない
                            }
                        }
                    }
                }
            });
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

/// イベントを処理する関数
///
/// # 引数
/// * `event_result` - イベント結果
/// * `shard_id` - シャードID
///
/// # 戻り値
/// * `Option<Event>` - 処理すべきイベント、またはNone（ループを抜ける場合）
fn process_event_result(
    event_result: Option<Result<Event, twilight_gateway::error::ReceiveMessageError>>,
    shard_id: ShardId,
) -> Option<Event> {
    match event_result {
        Some(Ok(event)) => {
            tracing::debug!("イベント受信: タイプ {:?}", event.kind());
            Some(event)
        }
        Some(Err(e)) => {
            tracing::error!("イベント受信中にエラーが発生: {}", e);
            None
        }
        None => {
            tracing::warn!("シャード {} のイベントストリームが終了しました", shard_id);
            None
        }
    }
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

        let event_result = shard.next_event(EventTypeFlags::all()).await;

        // イベント結果を処理
        if let Some(event) = process_event_result(event_result, shard.id()) {
            // 各イベント処理を別タスクで実行するためのボット状態のクローン
            let bot_state_clone = Arc::clone(&bot_state);

            // イベント処理を別スレッドで実行
            tokio::spawn(async move {
                handle_event(event, bot_state_clone).await;
            });
        }
        // エラーまたはNoneの場合はループの次のイテレーションへ
    }
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

    tracing::info!("埋め込みメッセージモードで起動しています");

    // イベントループを実行
    tracing::info!("イベントループを開始します...");
    run_event_loop(shard, bot_state).await
}

/// ユーザーIDから一意の色を生成する関数
fn calculate_color(user_id: u64) -> u32 {
    (user_id & 0xFFFFFF) as u32
}

/// 埋め込みメッセージの作成に使用する構造体
#[derive(Debug)]
struct MessageEmbedBuilder {
    author: Option<EmbedAuthor>,
    description: Option<String>,
    color: Option<u32>,
    timestamp: Option<Timestamp>,
    fields: Vec<EmbedField>,
    footer: Option<EmbedFooter>,
}

impl MessageEmbedBuilder {
    fn new() -> Self {
        Self {
            author: None,
            description: None,
            color: None,
            timestamp: None,
            fields: Vec::new(),
            footer: None,
        }
    }

    fn with_author(mut self, user: &User) -> Self {
        let avatar_url = if let Some(avatar_hash) = &user.avatar {
            format!(
                "https://cdn.discordapp.com/avatars/{}/{}.png",
                user.id.get(),
                avatar_hash
            )
        } else {
            // デフォルトのアバターURLを使用
            format!(
                "https://cdn.discordapp.com/embed/avatars/{}.png",
                (user.id.get() % 5) as u8 // ユーザーIDを5で割った余りでデフォルトアイコンを決定
            )
        };

        self.author = Some(EmbedAuthor {
            name: user.name.clone(),
            icon_url: Some(avatar_url),
            url: None,
            proxy_icon_url: None,
        });
        self
    }

    fn with_description(mut self, description: String) -> Self {
        self.description = Some(description);
        self
    }

    fn with_color(mut self, color: u32) -> Self {
        self.color = Some(color);
        self
    }

    fn with_timestamp(mut self, timestamp: Timestamp) -> Self {
        self.timestamp = Some(timestamp);
        self
    }

    fn add_field(mut self, name: String, value: String, inline: bool) -> Self {
        self.fields.push(EmbedField {
            name,
            value,
            inline,
        });
        self
    }

    fn build(self) -> Embed {
        Embed {
            author: self.author,
            color: self.color,
            description: self.description,
            fields: self.fields,
            footer: self.footer,
            image: None,
            kind: "rich".to_string(),
            provider: None,
            thumbnail: None,
            timestamp: self.timestamp,
            title: None,
            url: None,
            video: None,
        }
    }
}
