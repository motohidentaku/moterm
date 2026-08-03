# moterm

[English](README.en.md) | **日本語**

Rust による SSH 専用ターミナルクライアント。
複数の環境へ SSH 接続して開発し、ローカルとの間でファイルをやり取りしつつ、Claude Code / Codex を扱う想定のアプリ。

| 接続中 | SFTP |
|---|---|
| ![neo](docs/images/neo.png) | ![neo-sftp](docs/images/neo-sftp.png) |

## 機能

- **認証・接続**: SSH エージェント / 公開鍵（OpenSSH・PEM・rsa-sha2・未暗号化 `.ppk` 自動変換）/ パスワード、
  TOFU ホスト鍵確認（`~/.ssh/known_hosts` 共有）、マスターパスワード式暗号ボールト（Argon2id + XChaCha20）、
  **踏み台 `proxy_jump`**（純 russh、外部 ssh 不要）/ `proxy_command`、キープアライブ、**自動再接続**（予期しない切断時にバックオフ再試行。`reconnect = { enabled, max_retries, backoff_sec }`）/ F5 手動再接続。
  パスワード/パスフレーズ/マスターパスワードの入力は neon モーダル（表示トグル・クリック送信）
- **SSH 実務機能**: 静的ポートフォワード -L/-R（F2 で端末右上に neon カードで状態表示）、
  `on_connect` 自動入力、セッションログ（生バイト＋プレーンテキスト）
- **SFTP（F3）**: 2ペイン FM（クリック/D&D 転送・進捗バー・ミラー同期 `m`・mkdir/rename/delete・
  列ソート・日付列）、ウィンドウへの D&D アップロード。**接続画面に埋め込み**
  （端末⇄SFTP のサブタブ切替・ツールバー・Transfer Queue。切断で自動クローズ）
- **端末**: SGR 全属性、256色/truecolor、CJK 全角、代替スクリーン、OSC 7/8/52、OSC 133 コマンド境界、
  ブラケットペースト、マウスレポート（vim/htop 等、Shift で抑止）、ウィンドウリサイズで行列が動的に追従
- **タブ/ペイン**: 可変幅タブ（ドラッグ並べ替え・ダブルクリック rename・右クリックメニュー）、
  二分木ペイン分割、ブロードキャスト入力（赤帯）
- **スクロールバック・コピペ**: ホイール/バー（サムドラッグ）/Shift+PageUp/Dn、Ctrl+Shift+F 検索
  （端末上部中央に neon 検索バー・件数表示）、ドラッグ/ダブル/トリプル選択、Alt 矩形、
  Ctrl+クリックで URL を開く
- **フォント**: 既定は同梱の **HackGen Console**（等幅＋日本語、SIL OFL）。OS に未インストールでも
  バイナリ埋め込みで動作し、既定構成ではシステムフォント走査を行わず**起動が速い**。
  `config.font` で別フォントを指定可（その場合も日本語は HackGen で補完）
- **外観・その他**: ウィンドウ透過・ぼかし（Windows は DWM）、組み込みカラースキーム4種、
  i18n（ja/en）、IME、HiDPI、`config.keys` キーバインド上書き、Ctrl+Shift+R 設定リロード

## 動作環境

- Linux（X11/Wayland）/ Windows 10+ / macOS。描画は CPU（winit + softbuffer + fontdue）で GPU 不要
- ウィンドウ透過・ぼかしはコンポジタ（Windows は DWM）有効時のみ実効
- プロトコルは SSH のみ・文字コードは UTF-8 のみ（telnet/シリアル/SJIS 非対応）

## ビルド

```sh
cargo build --release -p mot-gui        # バイナリ: target/release/moterm
```

- **Windows**: `scripts\windows-build.bat`（または `.ps1`）。前提は Rust(MSVC) + VS Build Tools(C++) のみ。
- **macOS**: `scripts/macos-bundle.sh` で Dock アイコン付き `.app` を生成。アイコンは `assets/icon.png`
  差し替えのみ
- この開発ホストのように Rust ツールチェーンが無い環境では Docker でビルドする:
  `docker build -t moterm-dev -f docker/dev.Dockerfile docker` の後 `./docker/run.sh cargo build -p mot-gui`

## 実行

```sh
moterm                    # 設定を自動探索して起動（無ければ既定値）
moterm path/to/moterm.lua # 設定ファイルを明示
```

### macOS で「壊れているため開けません」と出る場合

Release の zip をブラウザで DL すると quarantine 属性が付き、Apple Silicon では未署名/ad-hoc
署名の `.app` が「壊れているため開けません」と表示されることがある。次のいずれかで解消する:

```sh
xattr -dr com.apple.quarantine /path/to/moterm.app   # quarantine を除去して開く
# もしくは Finder で .app を右クリック →「開く」→「開く」
```

正式な公証（Developer ID + notarization）は未対応のため、初回のみ上記の操作が必要。

設定ファイル `moterm.lua` の探索順: ①引数で指定したパス → ②実行ファイルと同じディレクトリ →
③ `~/.config/moterm/`（Windows は `%APPDATA%\moterm\`）。`profiles.json`・暗号ボールトも同じ場所に置かれる。

最小設定例（詳細は `examples/moterm.demo.lua`）:

```lua
local moterm = require "moterm"
local config = moterm.config()
config.profiles = {
  { name = "web1", group = "prod", host = "203.0.113.10", user = "admin",
    auth = { method = "publickey", key = "~/.ssh/id_ed25519" } },
  { name = "db1",  group = "prod", host = "10.0.0.5", user = "admin",
    proxy_jump = "admin@203.0.113.10",           -- 踏み台経由
    auth = { method = "agent" } },
}
config.groups = { { name = "prod", label = "本番環境" } }
return config
```

接続先の CPU / メモリ / ディスク使用率を右の情報パネルに出す（既定で有効、1 分ごと）:

```lua
config.metrics = {
  enabled = true,      -- false で採取を止める
  interval_min = 1,    -- 採取間隔（分）。0 でも停止
  panel = true,        -- 起動時に情報パネルを開くか（F6 で切替）
}
```

取得は対話シェルとは別の exec チャネルで行うため、画面やシェル履歴は汚れない。
`/proc` と `df` を使うので対象は Linux ホスト（取得できない場合はパネルにその旨を出し、
3 回連続で失敗したらそのタブでは採取をやめる）。パネルを閉じている間・非アクティブなタブでは
コマンドを送らない。

### 接続先で動く Claude Code の状況を表示する

情報パネルの AGENT セクションに、SSH 先で動いている [Claude Code](https://claude.com/claude-code)
のモデル・コンテキスト使用率・トークン・課金額・実行中のツールを出せる。**接続先に設定を1つ置く**
だけで、moterm 側の設定は要らない。

1. `scripts/moterm-claude.sh` を接続先の `~/.claude/moterm-claude.sh` へコピーして `chmod +x`
2. 接続先の `~/.claude/settings.json` に追記:

```json
{
  "statusLine": { "type": "command", "command": "~/.claude/moterm-claude.sh" },
  "hooks": {
    "SessionStart":     [{ "hooks": [{ "type": "command", "command": "~/.claude/moterm-claude.sh" }] }],
    "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "~/.claude/moterm-claude.sh" }] }],
    "PreToolUse":       [{ "hooks": [{ "type": "command", "command": "~/.claude/moterm-claude.sh" }] }],
    "PostToolUse":      [{ "hooks": [{ "type": "command", "command": "~/.claude/moterm-claude.sh" }] }],
    "Notification":     [{ "hooks": [{ "type": "command", "command": "~/.claude/moterm-claude.sh" }] }],
    "Stop":             [{ "hooks": [{ "type": "command", "command": "~/.claude/moterm-claude.sh" }] }],
    "SessionEnd":       [{ "hooks": [{ "type": "command", "command": "~/.claude/moterm-claude.sh" }] }]
  }
}
```

statusLine が渡してくる JSON を**加工せず** OSC 7777 で `/dev/tty` へ流すだけなので、
接続先に `jq` 等は要らない。Claude Code 側の画面には何も出ない。

**すでに statusLine を使っている場合**は既存コマンドを引数に渡す。同じ JSON をそのまま
流し込むので表示は変わらない（`statusLine` は1つしか設定できないためのラッパー方式）:

```json
"statusLine": { "type": "command", "command": "~/.claude/moterm-claude.sh ~/.claude/statusline.sh" }
```

**tmux 越しに使う場合**は接続先で passthrough を有効にする（tmux は未知の OSC を捨てるため）:

```sh
echo 'set -g allow-passthrough on' >> ~/.tmux.conf   # 次回の tmux 起動から
tmux set -g allow-passthrough on                     # 動作中の tmux へ即時反映
tmux show -g allow-passthrough                       # 確認
```

`set -g ...` は tmux のコマンド。シェルにそのまま打つと `set: -g: invalid option` になる
（`tmux` を前に付けるか、tmux 内で `Ctrl+b :` のプロンプトから実行する）。

複数の Claude Code を tmux で並べると 1 本の PTY に混ざって流れてくるが、moterm は
`session_id` で分けて更新の新しい順に最大3件（保持は8件）表示する。ただし **どの tmux pane を
見ているかは moterm には分からない**ため、表示されるのは「最後に動いたセッション」から順になる。

状態ドットの色: シアン = 応答生成中／ツール実行中、琥珀 = 権限プロンプト待ち、グレー = 入力待ち。

#### 出ないときの切り分け

上流から順に確かめる。

```sh
# 1) moterm 側が受け取れるか（接続先のシェルで直接叩く）
printf '\033]7777;-;{"session_id":"t","session_name":"manual test","model":{"display_name":"Test"}}\007'

# 2) スクリプト単体が動くか
echo '{"session_id":"s","session_name":"script test"}' | ~/.claude/moterm-claude.sh

# 3) Claude Code がスクリプトを呼んでいるか（settings.json の command を差し替えて実行）
#    "command": "env MOTERM_CLAUDE_DEBUG=/tmp/moterm-claude.log ~/.claude/moterm-claude.sh"
tail -f /tmp/moterm-claude.log

# 4) tmux 経由なら passthrough が有効か
tmux show -g allow-passthrough
```

1 で出るなら moterm 側は正常で、原因は Claude Code の設定側。moterm を `RUST_LOG=warn`
（既定）で起動しておくと、OSC は届いたが JSON を解釈できなかった場合に警告が出る。
`RUST_LOG=mot_gui=debug` にすると受信した1件ごとにログが出る。

Claude Code 側は `/hooks` で登録内容を確認でき、`claude --debug` で発火の様子が見られる。
`~/.claude/settings.json` の hooks はファイル監視で自動反映されるので**再起動は不要**
（statusLine は次にアシスタントの応答が返ったときに走る）。JSON の構文を間違えていると
設定ごと読まれないので、まず `/hooks` に出ているかを見るとよい。

### 主なキー操作（F1〜F6・タブ切替・コピペ等の16アクションは `config.keys` で変更可）

| キー | 動作 | キー | 動作 |
|---|---|---|---|
| F1 | サイドバー Filter（ホスト検索） | Ctrl+T | サイドバー選択（↑↓移動・Enter で接続） |
| F3 | SFTP ファイルマネージャ | F4 | タブを閉じる |
| F5 | 再接続 | Ctrl+PageUp/Dn | タブ切替 |
| Ctrl+Shift+C / V | コピー / 貼り付け | Ctrl+Shift+F | スクロールバック検索 |
| Ctrl+Shift+D | 選択パスを SFTP ダウンロード | Ctrl+Shift+R | 設定リロード |
| Shift+PageUp/Dn | スクロールバック | Ctrl+Shift+B | ブロードキャスト入力 |
| Ctrl+Shift+E / O | ペイン分割（横 / 縦） | Ctrl+Shift+X | ペインを閉じる |
| Ctrl+Shift+矢印 | ペインフォーカス移動 | Ctrl+1〜9 / Ctrl+Tab | タブ切替 |
| Ctrl+Shift+P / N | 前 / 次のプロンプトへ（OSC 133） | F2 | ポートフォワードパネル |
| F6 | 情報パネル（CPU/メモリ/ディスク） | | |


## 開発・テスト

検証の統一エントリポイントは `./check`（lint + typecheck + 全テスト。Docker 環境を自動検出）:

```sh
./check                      # lint + typecheck + 全テスト（完了条件）
./check fix                  # 自動整形
./docker/e2e.sh              # 実 sshd 統合テスト（pubkey / password+PF / SFTP / proxy_jump）
./docker/gui-e2e.sh          # GUI→実 sshd の end-to-end（Xvfb + xdotool、スクショ生成）
./docker/reconnect-e2e.sh    # 自動再接続（切断→sshd再起動→復帰）
./docker/neo-sftp-e2e.sh     # 接続画面に埋め込む SFTP の描画
```


## ライセンス

MIT（[LICENSE](LICENSE)）
