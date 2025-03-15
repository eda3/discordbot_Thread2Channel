# Discord Thread2Channel Bot

Discordの特定のスレッド内の全メッセージを指定したチャンネルにコピーするシンプルなボットです。

## 機能

- 指定されたスレッド内のメッセージを自動的に別のチャンネルにコピー
- 添付ファイルのURLも一緒にコピー
- 環境変数で複数のスレッド・チャンネルのペアを設定可能
- メッセージの作成者名を表示

## 必要条件

- Rust 1.74以上
- Discord Bot Token
- 適切な権限を持ったDiscordボット（メッセージ閲覧・送信権限）

## セットアップ

1. リポジトリをクローン
```bash
git clone https://github.com/yourusername/discordbot_Thread2Channel.git
cd discordbot_Thread2Channel
```

2. `.env`ファイルを編集
```
# Discord Bot Token
DISCORD_TOKEN=あなたのボットトークン

# スレッドとチャンネルのマッピング
# フォーマット: THREAD_MAPPING_番号=スレッドID:ターゲットチャンネルID
THREAD_MAPPING_1=1122334455667788:9900112233445566
# 必要に応じて追加のマッピングを設定
# THREAD_MAPPING_2=別のスレッドID:別のコピー先チャンネルID
```

3. ビルドして実行
```bash
cargo build --release
cargo run --release
```

## 使い方

1. ボットをDiscordサーバーに招待します（必要な権限: ボット、メッセージ閲覧、メッセージ送信）
2. `.env`ファイルにスレッドIDとターゲットチャンネルIDを設定します
3. ボットを起動します
4. 設定したスレッドにメッセージが投稿されると、指定したチャンネルに自動的にコピーされます

## デバッグモード

デバッグ情報を表示するには、`--debug`フラグを使用します：

```bash
cargo run -- --debug
```

## ライセンス

MITライセンス