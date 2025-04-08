# Discord Thread2Channel Bot

Discordの特定のスレッド内のメッセージを指定したチャンネルにコピーするシンプルなボットです。元の送信者名とアバターを維持したWebhook転送にも対応しています。

## 機能

- 指定されたスレッド内のメッセージを自動的に別のチャンネルにコピー
- Webhook対応で元の送信者の名前とアバター画像を維持したメッセージ転送
- メッセージにタイムスタンプを追加（JST形式）
- 添付ファイルのURLも一緒にコピー
- 環境変数で複数のスレッド・チャンネルのペアを設定可能
- マッピング設定は動的に変更可能（コマンドでの設定）

## 必要条件

- Rust 1.74以上
- Discord Bot Token
- 適切な権限を持ったDiscordボット：
  - メッセージを読む (View Channels)
  - メッセージを送信 (Send Messages)
  - メッセージ履歴を読む (Read Message History)
  - Webhookを管理 (Manage Webhooks)

## セットアップ

1. リポジトリをクローン
```bash
git clone https://github.com/yourusername/discordbot_Thread2Channel.git
cd discordbot_Thread2Channel
```

2. `.env`ファイルを編集（`.env_example`をコピーして使用可能）
```
# Discord Bot Token
DISCORD_TOKEN=あなたのボットトークン

# スレッドとチャンネルのマッピング
# フォーマット: THREAD_MAPPING_番号=スレッドID:チャンネルID[:Webhook URL][:all]
THREAD_MAPPING_1=1122334455667788:9900112233445566
```

3. ビルドして実行
```bash
cargo build --release
cargo run --release
```

## 使い方

### 環境変数での設定

`.env`ファイルで以下のようにマッピングを設定できます：

```
# 基本形式: スレッドID:チャンネルID
THREAD_MAPPING_1=1122334455667788:9900112233445566

# Webhook URLを含む: スレッドID:チャンネルID:Webhook URL
THREAD_MAPPING_2=1122334455667788:9900112233445566:https://discord.com/api/webhooks/WEBHOOK_ID/WEBHOOK_TOKEN

# 過去メッセージ全転送フラグ(all)を含む: スレッドID:チャンネルID:all
THREAD_MAPPING_3=1122334455667788:9900112233445566:all

# Webhook URLと過去メッセージ全転送フラグ(all)を両方含む: スレッドID:チャンネルID:Webhook URL:all
THREAD_MAPPING_4=1122334455667788:9900112233445566:https://discord.com/api/webhooks/WEBHOOK_ID/WEBHOOK_TOKEN:all
```

### コマンドでの設定

以下のコマンドがスレッド内で使用できます：

- `!thread2channel <チャンネルID> [all]`
  - 現在のスレッドからメッセージを転送するチャンネルを設定します
  - `all`オプションを付けると過去のメッセージも含めて転送します

- `!set_webhook <webhook_url>`
  - Webhook URLを設定して、送信者のアバターと名前を維持したメッセージ転送を有効にします
  - Webhook名は自動的に空に設定されます（元の送信者名を表示するため）

- `!start`
  - 現在のスレッドの過去メッセージを一括で転送します
  - 事前に`!thread2channel`で転送先を設定しておく必要があります

### 動作の流れ

1. ボットをDiscordサーバーに招待します
2. `.env`ファイルで設定するか、コマンドでスレッドとチャンネルのマッピングを設定します
3. 設定したスレッドにメッセージが投稿されると、指定したチャンネルに自動的にコピーされます
4. WebhookモードではメッセージはWebhookを通じて送信され、元の送信者名とアバターが維持されます

## タイムスタンプ機能

転送されるメッセージには自動的にJST形式のタイムスタンプが追加されます：
```
メッセージ内容 (2023/06/15 12:34:56)
```

## その他の注意点

- Webhook名は空に設定する必要があります（空にしないと送信者名が上書きされます）
- メッセージ内のメンションは無効化されます（意図しないメンションを防ぐため）
- 添付ファイルはURLとして転送されます

## ライセンス

MITライセンス