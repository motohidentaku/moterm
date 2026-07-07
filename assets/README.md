# アプリアイコン

## 置き場所

**`assets/icon.png`** — このファイルを差し替えるだけで各プラットフォームのアイコンになります。

- 正方形の PNG（推奨 **256×256** 以上、512×512 が理想）
- 背景透過（アルファ）対応
- 現在は既定アイコン（ダークな角丸＋緑のプロンプト `>_`）が入っています。自分の PNG で上書きしてください。

## どこに使われるか

| プラットフォーム | 用途 | 生成方法 |
|---|---|---|
| **Windows** | exe 埋め込み（エクスプローラ・ピン留め・スタートメニュー） | ビルド時に `crates/mot-gui/build.rs` が `icon.png` → `.ico` に変換して `moterm.exe` へ埋め込む（自動） |
| **Windows** | 実行中アプリのタスクバー／タイトルバー／Alt+Tab | 起動時に `with_window_icon`/`with_taskbar_icon` へ設定（`icon.png` をコンパイル時に埋め込み） |
| **macOS** | Dock アイコン | `scripts/macos-bundle.sh` が `icon.png` → `.icns` に変換し `moterm.app` を生成 |

