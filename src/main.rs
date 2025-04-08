use dotenv::dotenv;
use serde_json::json;
use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use tokio::sync::RwLock;
use chrono::{DateTime, Utc, TimeZone};

use reqwest;
use twilight_gateway::{Event, Intents, Shard, ShardId};
use twilight_http::Client as HttpClient;
use twilight_model::channel::message::MessageType;
use twilight_model::gateway::payload::incoming::MessageCreate;
use twilight_model::id::{
    marker::{ChannelMarker, UserMarker, WebhookMarker},
    Id,
};

/// スレッド情報を保持する構造体
#[derive(Debug, Clone)]
struct ThreadInfo {
    /// メッセージのコピー先チャンネルID
    target_channel_id: Id<ChannelMarker>,
    /// 過去のメッセージを全て取得して転送するかどうか
    transfer_all_messages: bool,
    /// Webhook URL (オプション)
    webhook_url: Option<String>,
}

/// .env ファイルからスレッドマッピングを読み込む
fn load_thread_mappings_from_env() -> HashMap<Id<ChannelMarker>, ThreadInfo> {
    let mut thread_mappings = HashMap::new();

    // 環境変数をすべて走査
    for (key, value) in env::vars() {
        // THREAD_MAPPING_ で始まる環境変数を処理
        if key.starts_with("THREAD_MAPPING_") {
            let parts: Vec<&str> = value.split(':').collect();

            // フォーマット: thread_id:channel_id:webhook_url:all_flag
            if parts.len() >= 2 {
                // スレッドIDとチャンネルIDをパース
                if let (Ok(thread_id), Ok(channel_id)) = (parts[0].parse::<u64>(), parts[1].parse::<u64>()) {
                    let thread_id = Id::new(thread_id);
                    let channel_id = Id::new(channel_id);
                    
                    // 全メッセージ転送フラグを確認（デフォルトはfalse）
                    let transfer_all_messages = parts.iter().any(|&p| p == "all");
                    
                    // Webhook URLの取得（オプション）
                    // 第3パラメータがあり、"all"でない場合はWebhook URLとして扱う
                    let webhook_url = if parts.len() >= 3 && !parts[2].is_empty() && parts[2] != "all" {
                        // Webhook URLのバリデーション
                        let url = parts[2].to_string();
                        
                        if !url.starts_with("http://") && !url.starts_with("https://") {
                            println!("警告: 無効なWebhook URL ({}): URLはhttp://またはhttps://で始まる必要があります", key);
                            None
                        } else if !url.contains("discord.com/api/webhooks/") {
                            println!("警告: 無効なWebhook URLの形式 ({}): 正しいDiscord Webhook URLであることを確認してください", key);
                            None
                        } else {
                            Some(url)
                        }
                    } else {
                        None
                    };
                    
                    // WebhookがあるかどうかのフラグとURLの表示用文字列
                    let has_webhook = webhook_url.is_some();
                    
                    // スレッド情報を作成してマップに追加
                    thread_mappings.insert(
                        thread_id,
                        ThreadInfo {
                            target_channel_id: channel_id,
                            transfer_all_messages,
                            webhook_url,
                        },
                    );
                    
                    println!("マッピングを読み込みました: スレッド {} -> チャンネル {} (Webhook: {}, 全メッセージ転送: {})", 
                        thread_id, 
                        channel_id, 
                        has_webhook,
                        transfer_all_messages
                    );
                }
            }
        }
    }
    
    println!("合計 {} 個のスレッドマッピングを読み込みました", thread_mappings.len());
    thread_mappings
}

/// ユーザーのアバターURLを取得する
fn get_user_avatar_url(user_id: Id<UserMarker>, avatar_hash: Option<&str>) -> String {
    if let Some(hash) = avatar_hash {
        // ユーザーがアバターを設定している場合は、そのアバターのURLを返す
        let url = format!("https://cdn.discordapp.com/avatars/{}/{}.webp?size=128", user_id, hash);
        println!("🖼️ アバターURL生成（カスタム）: {}", url);
        url
    } else {
        // アバターが設定されていない場合は、デフォルトのアバターURLを返す
        let default_avatar = user_id.get() % 5;
        let url = format!("https://cdn.discordapp.com/embed/avatars/{}.png", default_avatar);
        println!("🖼️ アバターURL生成（デフォルト）: {}", url);
        url
    }
}

/// Webhookを使用してメッセージを送信する
async fn send_webhook_message(
    _http: &HttpClient,
    webhook_url: &str,
    username: &str,
    avatar_url: &str,
    content: &str,
    attachments: &[twilight_model::channel::Attachment],
    timestamp: Option<&twilight_model::util::Timestamp>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Webhook URLのバリデーション
    if !webhook_url.starts_with("http://") && !webhook_url.starts_with("https://") {
        return Err(format!("無効なWebhook URL: URLはhttp://またはhttps://で始まる必要があります: {}", webhook_url).into());
    }

    println!("🚀 Webhookリクエスト送信開始:");
    println!("🔗 URL: {}", webhook_url);
    println!("👤 送信者名: \"{}\"", username);
    println!("🖼️ アバターURL: {}", avatar_url);
    
    let client = reqwest::Client::new();

    // タイムスタンプがある場合は追加
    let mut full_content = content.to_string();
    if let Some(ts) = timestamp {
        // タイムスタンプをUNIX時間として解釈し、JSTに変換
        let unix_timestamp = ts.as_secs();
        let dt = Utc.timestamp_opt(unix_timestamp as i64, 0).unwrap();
        let jst = dt + chrono::Duration::hours(9);
        full_content.push_str(&format!(" (`{}`)", jst.format("%Y/%m/%d %H:%M:%S")));
    }

    // 添付ファイルがある場合はリンクとして追加する
    if !attachments.is_empty() {
        full_content.push_str("\n\n**添付ファイル:**\n");
        for attachment in attachments {
            full_content.push_str(&format!("- {}\n", attachment.url));
        }
    }

    // WebhookにPOSTするJSONデータを作成
    let webhook_data = json!({
        "content": full_content,
        "username": username,
        "avatar_url": avatar_url,
        "allowed_mentions": {
            "parse": []  // メンションを無効化
        }
    });

    println!("📦 Webhookデータ:");
    println!("{}", serde_json::to_string_pretty(&webhook_data).unwrap_or_else(|_| webhook_data.to_string()));

    // WebhookにPOSTリクエストを送信
    let response = match client.post(webhook_url).json(&webhook_data).send().await {
        Ok(resp) => resp,
        Err(e) => {
            println!("❌ Webhookリクエスト送信エラー: {}", e);
            return Err(format!("Webhook送信失敗: {} - URL: {}", e, webhook_url).into());
        }
    };

    if !response.status().is_success() {
        // ステータス情報を保存
        let status = response.status();
        
        // エラーボディを取得
        let error_body = match response.text().await {
            Ok(body) => body,
            Err(_) => "レスポンスボディを取得できませんでした".to_string()
        };
        
        let error_msg = format!("❌ Webhookリクエスト失敗 ステータス: {} - URL: {} - レスポンス: {}", 
                               status, webhook_url, error_body);
        println!("{}", error_msg);
        return Err(error_msg.into());
    }

    println!("✅ Webhookリクエスト送信成功!");
    Ok(())
}

/// ユーザーからのメッセージイベントを処理します
async fn handle_message_create(
    message: Box<MessageCreate>,
    http: Arc<HttpClient>,
    threads_info: Arc<RwLock<HashMap<Id<ChannelMarker>, ThreadInfo>>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // システムメッセージは処理しない
    if message.kind != MessageType::Regular && message.kind != MessageType::Reply {
        return Ok(());
    }

    // 対象のチャンネルがスレッドマッピングに登録されているか確認
    let thread_info = {
        let threads_info = threads_info.read().await;
        if let Some(info) = threads_info.get(&message.channel_id) {
            info.clone()
        } else {
            return Ok(());
        }
    };

    // メッセージ内容を準備
    let content = &message.content;
    let author = &message.author;
    let author_name = &author.name;
    // ImageHashからString形式のハッシュを取得
    let avatar_hash = author.avatar.as_ref().map(|hash| hash.to_string());
    let avatar_url = get_user_avatar_url(author.id, avatar_hash.as_deref());

    // メッセージの添付ファイルを取得
    let attachments = &message.attachments;
    
    // タイムスタンプを取得
    let timestamp = message.timestamp;

    // WebhookまたはRegularメッセージとして送信
    if let Some(webhook_url) = &thread_info.webhook_url {
        // Webhookを使用してメッセージを送信
        send_webhook_message(
            &http,
            webhook_url,
            author_name,
            &avatar_url,
            content,
            attachments,
            Some(&timestamp),
        )
        .await?;
    } else {
        // 旧方式：通常のメッセージとして送信
        let mut forward_message = format!("**{}**\n{}", author_name, content);
        
        // タイムスタンプを追加
        let unix_timestamp = timestamp.as_secs();
        let dt = Utc.timestamp_opt(unix_timestamp as i64, 0).unwrap();
        let jst = dt + chrono::Duration::hours(9);
        forward_message.push_str(&format!(" (`{}`)", jst.format("%Y/%m/%d %H:%M:%S")));

        // 添付ファイルがある場合はリンクを追加
        if !attachments.is_empty() {
            forward_message.push_str("\n\n**添付ファイル:**\n");
            for attachment in attachments {
                forward_message.push_str(&format!("- {}\n", attachment.url));
            }
        }

        http.create_message(thread_info.target_channel_id)
            .content(&forward_message)?
            .await?;
    }

    Ok(())
}

/// !thread2channelコマンドを処理します
async fn handle_thread2channel_command(
    message: Box<MessageCreate>,
    http: Arc<HttpClient>,
    threads_info: Arc<RwLock<HashMap<Id<ChannelMarker>, ThreadInfo>>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let content = &message.content;
    let parts: Vec<&str> = content.split_whitespace().collect();

    if parts.len() < 2 {
        // コマンドの使用方法を表示
        http.create_message(message.channel_id)
            .content("使用法: !thread2channel <target_channel_id> [all]")?
            .await?;
        return Ok(());
    }

    // ターゲットチャンネルIDを解析
    let target_channel_id = match parts[1].parse::<u64>() {
        Ok(id) => Id::new(id),
        Err(_) => {
            http.create_message(message.channel_id)
                .content("無効なチャンネルIDです。正しい数値IDを入力してください。")?
                .await?;
            return Ok(());
        }
    };

    // allオプションがあるかチェック
    let transfer_all_messages = parts.len() > 2 && parts[2] == "all";

    // スレッド情報をハッシュマップに追加
    {
        let mut threads_info = threads_info.write().await;
        threads_info.insert(
            message.channel_id,
            ThreadInfo {
                target_channel_id,
                transfer_all_messages,
                webhook_url: None,
            },
        );
    }

    // 設定完了メッセージを送信
    let response = if transfer_all_messages {
        format!(
            "このスレッドのメッセージを全てチャンネル <#{}>に転送します",
            target_channel_id
        )
    } else {
        format!(
            "このスレッドのメッセージをチャンネル <#{}>に転送します",
            target_channel_id
        )
    };

    http.create_message(message.channel_id)
        .content(&response)?
        .await?;

    Ok(())
}

/// ウェブフックの名前を空に設定する
async fn clear_webhook_name(webhook_url: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // webhookのIDとトークンを抽出
    let parts: Vec<&str> = webhook_url.split('/').collect();
    if parts.len() >= 7 {
        let webhook_id = parts[5];
        let webhook_token = parts[6];
        
        println!("🔄 ウェブフック名を空に設定します: ID={}", webhook_id);
        
        // Webhookを更新するAPIリクエスト
        let client = reqwest::Client::new();
        let response = client.patch(&format!("https://discord.com/api/webhooks/{}/{}", webhook_id, webhook_token))
            .json(&json!({
                "name": ""  // 名前を空に設定
            }))
            .send()
            .await?;
            
        if response.status().is_success() {
            println!("✅ ウェブフック名を空に設定しました: ID={}", webhook_id);
            return Ok(());
        } else {
            let status = response.status();
            let error_body = match response.text().await {
                Ok(body) => body,
                Err(_) => "レスポンスボディを取得できませんでした".to_string()
            };
            
            let error_msg = format!("❌ ウェブフック名設定失敗: ステータス={}, レスポンス={}", status, error_body);
            println!("{}", error_msg);
            return Err(error_msg.into());
        }
    } else {
        return Err(format!("ウェブフックURLの形式が正しくありません: {}", webhook_url).into());
    }
}

/// !set_webhookコマンドを処理します
async fn handle_set_webhook_command(
    message: Box<MessageCreate>,
    http: Arc<HttpClient>,
    threads_info: Arc<RwLock<HashMap<Id<ChannelMarker>, ThreadInfo>>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let content = &message.content;
    let parts: Vec<&str> = content.split_whitespace().collect();

    if parts.len() < 2 {
        // コマンドの使用方法を表示
        http.create_message(message.channel_id)
            .content("使用法: !set_webhook <webhook_url>\nWebhook URLは完全なURL（https://discord.com/api/webhooks/...）である必要があります。\n\n**注意**: ウェブフック名は自動的に空に設定されます。")?
            .await?;
        return Ok(());
    }

    // Webhook URLを取得
    let webhook_url = parts[1].to_string();
    
    // Webhook URLのバリデーション
    if !webhook_url.starts_with("http://") && !webhook_url.starts_with("https://") {
        http.create_message(message.channel_id)
            .content("無効なWebhook URLです。URLはhttp://またはhttps://で始まる必要があります。")?
            .await?;
        return Ok(());
    }
    
    // Webhook URLにスペースや余分な文字が含まれている可能性があるため、URLの形式をチェック
    if webhook_url.contains(" ") || !webhook_url.contains("discord.com/api/webhooks/") {
        http.create_message(message.channel_id)
            .content("無効なWebhook URLの形式です。URLに空白が含まれていないか、正しいDiscord Webhook URLであることを確認してください。")?
            .await?;
        return Ok(());
    }

    // ウェブフック名を空に設定
    match clear_webhook_name(&webhook_url).await {
        Ok(_) => {
            println!("ウェブフック名の設定が完了しました");
        },
        Err(e) => {
            println!("ウェブフック名の設定中にエラーが発生しました: {}", e);
            // エラーがあっても処理は続行（警告として表示）
            http.create_message(message.channel_id)
                .content(&format!("⚠️ ウェブフック名の自動設定中にエラーが発生しました。ウェブフック自体は設定しますが、送信者名が正しく表示されない可能性があります。\nエラー: {}", e))?
                .await?;
        }
    }

    // スレッド情報がすでに存在するか確認
    {
        let mut threads_info = threads_info.write().await;
        if let Some(info) = threads_info.get_mut(&message.channel_id) {
            // 既存の設定にWebhook URLを追加
            info.webhook_url = Some(webhook_url.clone());
            
            println!("Webhookを設定しました: スレッド={}, URL={}", message.channel_id, webhook_url);

            // 設定完了メッセージを送信
            http.create_message(message.channel_id)
                .content("このスレッドにWebhookを設定しました！メッセージは元の送信者のアバターと名前で転送されます。ウェブフック名は自動的に空に設定されました。")?
                .await?;
        } else {
            // スレッド情報がまだ設定されていない場合
            http.create_message(message.channel_id)
                .content("まず !thread2channel コマンドでチャンネル転送を設定してください。")?
                .await?;
        }
    }

    Ok(())
}

/// 過去のメッセージを全て取得して転送する
async fn fetch_all_messages_and_transfer(
    http: &HttpClient,
    thread_id: Id<ChannelMarker>,
    thread_info: &ThreadInfo,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("スレッド {} の全メッセージ転送を開始します...", thread_id);

    // メッセージ取得の制限（Discordの制限に合わせて調整）
    let limit = 100;
    
    // まずは通知メッセージを送信
    let status_message = "🔍 過去のメッセージを検索して転送しています...";
    http.create_message(thread_info.target_channel_id)
        .content(status_message)?
        .await?;

    // メッセージ履歴を取得
    let messages_result = http.channel_messages(thread_id)
        .limit(limit)?
        .await?;
        
    let messages = messages_result.models().await?;
    let message_count = messages.len();
    
    println!("{} 件のメッセージを取得しました", message_count);
    
    // 転送開始メッセージ
    let start_message = format!("🚀 **{}件** のメッセージを転送します", message_count);
    http.create_message(thread_info.target_channel_id)
        .content(&start_message)?
        .await?;

    // メッセージを古い順に処理（取得したものを逆順にすると古→新になる）
    for message in messages.into_iter().rev() {
        // システムメッセージやボットのメッセージは除外
        if message.author.bot || message.kind != MessageType::Regular && message.kind != MessageType::Reply {
            continue;
        }
        
        // 転送処理
        let author_name = &message.author.name;
        let content = &message.content;
        let avatar_hash = message.author.avatar.as_ref().map(|hash| hash.to_string());
        let avatar_url = get_user_avatar_url(message.author.id, avatar_hash.as_deref());
        let attachments = &message.attachments;
        let timestamp = message.timestamp;
        
        if let Some(webhook_url) = &thread_info.webhook_url {
            // Webhook使用
            send_webhook_message(
                http,
                webhook_url,
                author_name,
                &avatar_url,
                content,
                attachments,
                Some(&timestamp),
            ).await?;
        } else {
            // 通常メッセージ
            let mut forward_message = format!("**{}**\n{}", author_name, content);
            
            // タイムスタンプを追加
            let unix_timestamp = timestamp.as_secs();
            let dt = Utc.timestamp_opt(unix_timestamp as i64, 0).unwrap();
            let jst = dt + chrono::Duration::hours(9);
            forward_message.push_str(&format!(" (`{}`)", jst.format("%Y/%m/%d %H:%M:%S")));
            
            // 添付ファイルがある場合はリンクを追加
            if !attachments.is_empty() {
                forward_message.push_str("\n\n**添付ファイル:**\n");
                for attachment in attachments {
                    forward_message.push_str(&format!("- {}\n", attachment.url));
                }
            }
            
            http.create_message(thread_info.target_channel_id)
                .content(&forward_message)?
                .await?;
        }
        
        // 短い待機を入れて、レート制限を避ける
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
    }
    
    // 転送完了メッセージ
    let complete_message = format!("✅ **{}件** のメッセージの転送が完了しました", message_count);
    http.create_message(thread_info.target_channel_id)
        .content(&complete_message)?
        .await?;
        
    println!("スレッド {} の全メッセージ転送が完了しました", thread_id);
    
    Ok(())
}

/// !startコマンドを処理します（全メッセージ転送を開始）
async fn handle_start_command(
    message: Box<MessageCreate>,
    http: Arc<HttpClient>,
    threads_info: Arc<RwLock<HashMap<Id<ChannelMarker>, ThreadInfo>>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // スレッド情報を取得
    let thread_info = {
        let threads_info = threads_info.read().await;
        if let Some(info) = threads_info.get(&message.channel_id) {
            info.clone()
        } else {
            // スレッド情報がない場合は設定を促す
            http.create_message(message.channel_id)
                .content("このスレッドは設定されていません。まず `!thread2channel <target_channel_id>` コマンドで設定してください。")?
                .await?;
            return Ok(());
        }
    };
    
    // 確認メッセージを送信
    http.create_message(message.channel_id)
        .content("🔄 このスレッドの過去メッセージの転送を開始します...")?
        .await?;
    
    // 全メッセージ転送処理を実行
    fetch_all_messages_and_transfer(&http, message.channel_id, &thread_info).await?;
    
    Ok(())
}

/// イベントを処理します
async fn handle_event(
    event: Event,
    http: Arc<HttpClient>,
    threads_info: Arc<RwLock<HashMap<Id<ChannelMarker>, ThreadInfo>>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match event {
        Event::MessageCreate(message) => {
            // コマンドの処理
            if message.content.starts_with("!thread2channel") {
                handle_thread2channel_command(message, http.clone(), threads_info.clone()).await?;
            }
            // webhookの設定コマンド
            else if message.content.starts_with("!set_webhook") {
                handle_set_webhook_command(message, http.clone(), threads_info.clone()).await?;
            }
            // 全メッセージ転送開始コマンド
            else if message.content.starts_with("!start") {
                handle_start_command(message, http.clone(), threads_info.clone()).await?;
            }
            // 通常メッセージの転送処理
            else {
                handle_message_create(message, http.clone(), threads_info.clone()).await?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// 指定されたDISCORD_TOKENでBOTを起動する
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // .envファイルから環境変数を読み込む
    dotenv().ok();

    // BOTトークンを環境変数から取得
    let token = env::var("DISCORD_TOKEN").expect("Expected a token in the environment");

    // インテントを設定し、何のイベントを受け取るかを指定
    let intents = Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT;

    // HTTPクライアントを作成
    let http = Arc::new(HttpClient::new(token.clone()));

    // 新しいシャードを作成してゲートウェイに接続
    let mut shard = Shard::new(ShardId::ONE, token, intents);

    // .envファイルからスレッドマッピングを読み込む
    let initial_mappings = load_thread_mappings_from_env();

    // 各ウェブフックの名前を空に設定
    for (_thread_id, thread_info) in &initial_mappings {
        if let Some(webhook_url) = &thread_info.webhook_url {
            println!("環境変数から読み込んだWebhookの名前をクリアします");
            if let Err(e) = clear_webhook_name(webhook_url).await {
                println!("環境変数のWebhook名クリア中にエラー: {}", e);
            }
        }
    }

    // スレッド情報を保持するハッシュマップを作成
    let threads_info = Arc::new(RwLock::new(initial_mappings));

    println!("Botを起動しました！");
    println!("Webhook機能を使用して送信者のアバターと名前を複製します");
    println!(".envファイルから設定を読み込みました");
    println!("コマンドでの設定も引き続き利用可能です");

    // イベントループ開始前に、全メッセージ転送フラグが設定されているマッピングを処理
    {
        let mappings = threads_info.read().await;
        let http_clone = Arc::clone(&http);
        
        for (thread_id, info) in mappings.iter() {
            if info.transfer_all_messages {
                println!("スレッド {} の全メッセージ転送を開始します...", thread_id);
                
                // 全メッセージ転送処理を実行
                match fetch_all_messages_and_transfer(&http_clone, *thread_id, info).await {
                    Ok(_) => println!("スレッド {} の全メッセージ転送が完了しました", thread_id),
                    Err(e) => eprintln!("スレッド {} の全メッセージ転送中にエラーが発生しました: {}", thread_id, e),
                }
            }
        }
    }

    // イベントループ
    loop {
        let event = match shard.next_event().await {
            Ok(event) => event,
            Err(e) => {
                eprintln!("Error receiving event: {:?}", e);
                continue;
            }
        };

        // 受信したイベントを処理
        if let Err(e) = handle_event(event, Arc::clone(&http), Arc::clone(&threads_info)).await {
            eprintln!("Error handling event: {:?}", e);
        }
    }
}
