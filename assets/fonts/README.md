# 同梱フォント / アイコン（NEO-UI）

NEO-UI（`feat/neo-ui`）はデザイン再現のため以下をバイナリに埋め込む
（`include_bytes!`）。いずれも再配布可能なライセンス。

| ファイル | 用途 | ライセンス | 出典 |
|---|---|---|---|
| `HackGenConsole-Regular.ttf` | **既定の端末フォント＋日本語(CJK)フォールバック**（等幅・JIS X 0208 1-4水準） | SIL Open Font License 1.1 (`HackGen-LICENSE.txt`) | yuru7/HackGen v2.10.0 |
| `Inter-Variable.ttf` | 可変幅UIフォント | SIL Open Font License 1.1 (`Inter-OFL.txt`) | rsms/Inter（Google Fonts 経由） |
| `JetBrainsMono-Regular.ttf` / `JetBrainsMono-Bold.ttf` | 等幅（値・コード） | SIL Open Font License 1.1 (`JetBrainsMono-OFL.txt`) | JetBrains/JetBrainsMono |
| `lucide.ttf` | アイコン（PUAコードポイント） | ISC License (`lucide-LICENSE.txt`) | lucide-icons/lucide (`lucide-static`) |

- 端末**本文**のフォントは従来どおり `config.font` / システムフォント（`FontManager`）。
  ここは NEO-UI クローム（サイドバー・タブ・パネル・情報パネル）専用。
- アイコンのコードポイント表は `crates/mot-gui/src/neofont.rs` の `mod icon`。
