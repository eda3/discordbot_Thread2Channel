use std::env;
use std::sync::Arc;
use dotenv::dotenv;
use anyhow::Result;
use tokio::sync::Mutex;
use std::collections::HashMap;

use twilight_gateway::{Event, Intents, Shard, ShardId, StreamExt, EventTypeFlags};
use twilight_http::Client as HttpClient;
use twilight_model::gateway::payload::incoming::MessageCreate;
use twilight_model::id::{Id, marker::ChannelMarker};

// スレッド情報を保持する構造体
struct ThreadInfo {
    target_channel_id: Id<ChannelMarker>,
}

// ボットの状態を管理する構造体
struct BotState {
    http: HttpClient,
    // スレッドID -> ターゲットチャンネルIDのマッピング
    thread_mappings: HashMap<Id<ChannelMarker>, ThreadInfo>,
}

impl BotState {
    fn new(token: String) -> Self {
        // HTTPクライアントの初期化
        let http = HttpClient::new(token);
        
        // 初期スレッドマッピング
        let mut thread_mappings = HashMap::new();
        
        // 環境変数からスレッドIDとターゲットチャンネルIDのマッピングを読み込む
        // 形式: THREAD_MAPPING_1=thread_id:target_channel_id
        for (key, value) in env::vars() {
            if key.starts_with("THREAD_MAPPING_") {
                let parts: Vec<&str> = value.split(':').collect();
                if parts.len() == 2 {
                    if let (Ok(thread_id), Ok(target_channel_id)) = (
                        parts[0].parse::<u64>(),
                        parts[1].parse::<u64>(),
                    ) {
                        thread_mappings.insert(
                            Id::new(thread_id),
                            ThreadInfo {
                                target_channel_id: Id::new(target_channel_id),
                            },
                        );
                        tracing::info!("Added thread mapping: {} -> {}", thread_id, target_channel_id);
                    }
                }
            }
        }

        Self {
            http,
            thread_mappings,
        }
    }

    // 新しいスレッドマッピングを追加
    async fn add_thread_mapping(&mut self, thread_id: Id<ChannelMarker>, target_channel_id: Id<ChannelMarker>) {
        self.thread_mappings.insert(
            thread_id,
            ThreadInfo {
                target_channel_id,
            },
        );
        tracing::info!("Added thread mapping: {} -> {}", thread_id, target_channel_id);
    }

    // メッセージ処理
    async fn handle_message(&self, msg: MessageCreate) -> Result<()> {
        // スレッドからのメッセージならコピーする
        if let Some(thread_info) = self.thread_mappings.get(&msg.channel_id) {
            // メッセージの作成者がボット自身でないことを確認
            if !msg.author.bot {
                // メッセージをターゲットチャンネルにコピー
                let content = format!("**{}**: {}", msg.author.name, msg.content);
                
                let attachment_urls = msg.attachments.iter()
                    .map(|attachment| format!("\n{}", attachment.url))
                    .collect::<String>();
                
                let full_content = format!("{}{}", content, attachment_urls);
                
                // 直接メッセージを送信
                match self.http
                    .create_message(thread_info.target_channel_id)
                    .content(&full_content)
                    .await 
                {
                    Ok(_) => {},
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

#[tokio::main]
async fn main() -> Result<()> {
    // .envファイルから環境変数を読み込む
    dotenv().ok();
    
    // ロギングの初期化
    tracing_subscriber::fmt::init();
    
    // コマンドライン引数の解析
    let debug_mode = env::args().any(|arg| arg == "-debug");
    
    // トークンの取得
    let token = env::var("DISCORD_TOKEN").map_err(|_| anyhow::anyhow!("Missing DISCORD_TOKEN environment variable"))?;
    
    // ボットステートの初期化
    let bot_state = Arc::new(Mutex::new(BotState::new(token.clone())));
    
    // インテント（権限）の設定
    let intents = Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT | Intents::GUILD_MESSAGE_REACTIONS;
    
    // シャードの作成
    let mut shard = Shard::new(ShardId::ONE, token, intents);
    
    tracing::info!("Bot started!");
    if debug_mode {
        tracing::info!("Debug mode enabled");
    }
    
    // イベントループ
    loop {
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
        
        let bot_state_clone = Arc::clone(&bot_state);
        
        // イベント処理
        tokio::spawn(async move {
            match event {
                Event::MessageCreate(msg) => {
                    if let Err(e) = bot_state_clone.lock().await.handle_message(*msg).await {
                        tracing::error!("Error handling message: {}", e);
                    }
                }
                Event::Ready(_) => {
                    tracing::info!("Bot is ready!");
                }
                _ => {}
            }
        });
    }
    
    Ok(())
}