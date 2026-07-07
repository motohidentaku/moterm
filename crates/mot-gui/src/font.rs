//! フォント管理。fontdb でシステムフォントを検索し、fontdue でグリフをラスタライズ、
//! (char, px量子化, bold) キーでグリフキャッシュに蓄積する。等幅前提。

use fontdue::{Font, FontSettings};
use std::collections::HashMap;

/// 既定の同梱フォント: HackGen Console（yuru7/HackGen, SIL OFL 1.1）。
/// 等幅＋日本語（JIS X 0208 1-4 水準＋かな）を含むため、端末の既定フォントかつ
/// CJK フォールバックとして使う。config.font 未指定でもこれが使われ、OS 登録は不要。
pub const HACKGEN_TTF: &[u8] = include_bytes!("../../../assets/fonts/HackGenConsole-Regular.ttf");

pub struct GlyphBitmap {
    pub w: usize,
    pub h: usize,
    pub data: Vec<u8>, // カバレッジ 0..=255
}

pub struct Glyph {
    pub bitmap: GlyphBitmap,
    /// ベアリング（左）
    pub left: i32,
    /// ベースラインからの上方向オフセット
    pub top: i32,
    pub advance: f32,
}

pub struct FontManager {
    primary: Font,
    /// CJK 等のフォールバック群（プライマリで欠けるグリフに使う）
    fallbacks: Vec<Font>,
    cache: HashMap<(char, u32, bool), Glyph>,
    /// px→セル幅（半角基準）のキャッシュ
    cell_w_cache: HashMap<u32, i32>,
}

impl FontManager {
    /// config.font / font_fallback を尊重してロードする。見つからなければ OS 既定 monospace。
    /// フォント探索は OS 非依存: 各 OS の代表的な等幅フォント名／CJK フォント名／
    /// システムフォントのファイルパスを順に試す。
    pub fn load(
        primary_name: Option<&str>,
        fallback_names: &[String],
    ) -> anyhow::Result<FontManager> {
        // --- プライマリ（等幅）---
        // config.font 未指定なら既定の同梱 HackGen Console（システムフォント走査なし＝起動が速い）。
        // 指定がある時だけ fontdb を走査し、引ければそれを使う。引けなければ OS 代表等幅 →
        // 汎用 → ファイル → 最終的に同梱 HackGen へフォールバック。
        let primary_data = match primary_name {
            Some(name) => {
                let mut db = fontdb::Database::new();
                db.load_system_fonts();
                let mut d = query_font(&db, Some(name), true);
                if d.is_none() {
                    for cand in PRIMARY_MONO_CANDIDATES {
                        if let Some(x) = query_font(&db, Some(cand), true) {
                            d = Some(x);
                            break;
                        }
                    }
                    d = d
                        .or_else(|| query_font(&db, None, true))
                        .or_else(|| load_first_existing(MONO_FONT_FILES));
                }
                d.unwrap_or_else(|| HACKGEN_TTF.to_vec())
            }
            None => HACKGEN_TTF.to_vec(),
        };
        let primary = Font::from_bytes(primary_data, FontSettings::default())
            .map_err(|e| anyhow::anyhow!("フォント解析失敗: {e}"))?;

        // フォールバック（CJK）は discover_cjk_fonts に集約。末尾に必ず同梱 HackGen が入るため、
        // config.font_fallback 未指定時はシステム走査を行わない。
        let fallbacks = discover_cjk_fonts(fallback_names);

        Ok(FontManager {
            primary,
            fallbacks,
            cache: HashMap::new(),
            cell_w_cache: HashMap::new(),
        })
    }

    fn font_for(&self, ch: char) -> &Font {
        if self.primary.lookup_glyph_index(ch) != 0 {
            return &self.primary;
        }
        for f in &self.fallbacks {
            if f.lookup_glyph_index(ch) != 0 {
                return f;
            }
        }
        &self.primary
    }

    /// セル幅（半角1文字ぶんの水平送り、ピクセル）。'M' の advance を基準にする。
    pub fn cell_width(&mut self, px: f32) -> i32 {
        let key = px.to_bits();
        if let Some(&w) = self.cell_w_cache.get(&key) {
            return w;
        }
        let m = self.primary.metrics('M', px);
        let w = (m.advance_width.ceil() as i32).max(1);
        self.cell_w_cache.insert(key, w);
        w
    }

    /// セル高さ（行送り）。px から概算（1.25 倍）。
    pub fn cell_height(&self, px: f32) -> i32 {
        (px * 1.30).ceil() as i32
    }

    /// ベースライン位置（セル上端からの距離）。
    pub fn baseline(&self, px: f32) -> i32 {
        (px * 1.05).round() as i32
    }

    pub fn glyph(&mut self, ch: char, px: f32, bold: bool) -> &Glyph {
        let key = (ch, px.to_bits(), bold);
        if !self.cache.contains_key(&key) {
            let font = self.font_for(ch);
            let (metrics, bitmap) = font.rasterize(ch, px);
            let g = Glyph {
                bitmap: GlyphBitmap {
                    w: metrics.width,
                    h: metrics.height,
                    data: bitmap,
                },
                left: metrics.xmin,
                top: metrics.height as i32 + metrics.ymin,
                advance: metrics.advance_width,
            };
            self.cache.insert(key, g);
        }
        self.cache.get(&key).unwrap()
    }
}

/// fontdb からフォントデータを取得する。monospace_pref=true なら等幅を優先。
fn query_font(db: &fontdb::Database, name: Option<&str>, monospace_pref: bool) -> Option<Vec<u8>> {
    use fontdb::{Family, Query};
    let families: Vec<Family> = match name {
        Some(n) => vec![Family::Name(n)],
        None if monospace_pref => vec![Family::Monospace],
        None => vec![Family::SansSerif],
    };
    let query = Query {
        families: &families,
        ..Query::default()
    };
    let id = db.query(&query)?;
    db.with_face_data(id, |data, _index| data.to_vec())
}

fn load_font_file(path: &str) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

fn load_first_existing(paths: &[&str]) -> Option<Vec<u8>> {
    paths.iter().find_map(|p| load_font_file(p))
}

/// プライマリに使う等幅フォント名の候補（Windows / macOS / Linux をまたいで列挙）。
/// fontdb はシステムに存在する最初のものを解決する。
const PRIMARY_MONO_CANDIDATES: &[&str] = &[
    // Windows
    "Consolas",
    "Cascadia Mono",
    "Cascadia Code",
    // macOS
    "SF Mono",
    "Menlo",
    "Monaco",
    // Linux / 汎用
    "DejaVu Sans Mono",
    "Liberation Mono",
    "Noto Sans Mono",
];

/// CJK フォールバックに使うフォント名の候補（日本語優先）。
const CJK_FONT_CANDIDATES: &[&str] = &[
    // Windows（日本語 Windows に同梱）
    "Yu Gothic UI",
    "Yu Gothic",
    "Meiryo",
    "MS Gothic",
    "MS UI Gothic",
    "BIZ UDGothic",
    "Microsoft YaHei",
    // macOS
    "Hiragino Sans",
    "Hiragino Kaku Gothic ProN",
    "Apple SD Gothic Neo",
    // Linux
    "Noto Sans CJK JP",
    "Noto Sans CJK",
    "Noto Sans Mono CJK JP",
    "Noto Sans JP",
    "IPAGothic",
    "VL Gothic",
];

/// 名前で引けなかった場合に直読みする等幅フォントファイル。
const MONO_FONT_FILES: &[&str] = &[
    // Windows
    r"C:\Windows\Fonts\consola.ttf",
    // macOS
    "/System/Library/Fonts/Menlo.ttc",
    "/System/Library/Fonts/SFNSMono.ttf",
    // Linux
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
];

/// 名前で引けなかった場合に直読みする CJK フォントファイル。
const CJK_FONT_FILES: &[&str] = &[
    // Windows
    r"C:\Windows\Fonts\YuGothM.ttc",
    r"C:\Windows\Fonts\YuGothR.ttc",
    r"C:\Windows\Fonts\meiryo.ttc",
    r"C:\Windows\Fonts\msgothic.ttc",
    r"C:\Windows\Fonts\BIZ-UDGothicR.ttc",
    // macOS
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "/Library/Fonts/Arial Unicode.ttf",
    // Linux
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJKjp-Regular.otf",
];

/// システムから CJK フォールバックフォント群を探索する（FontManager と NeoFonts で共用）。
/// 明示指定 fallback_names → OS 代表 CJK 名 → 既定フォントファイル直読みの順。
pub fn discover_cjk_fonts(fallback_names: &[String]) -> Vec<Font> {
    let mut fallbacks = Vec::new();
    // config.font_fallback が明示指定された時だけシステムフォントを走査する。
    // 既定（未指定）は同梱 HackGen で日本語を賄えるため走査を省き、起動を速くする。
    if !fallback_names.is_empty() {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let mut names: Vec<String> = fallback_names.to_vec();
        for n in CJK_FONT_CANDIDATES {
            names.push((*n).to_string());
        }
        for n in &names {
            if let Some(data) = query_font(&db, Some(n), false) {
                if let Ok(f) = Font::from_bytes(
                    data,
                    FontSettings {
                        collection_index: 0,
                        ..Default::default()
                    },
                ) {
                    fallbacks.push(f);
                }
            }
        }
        // 日本語グリフ（あ）を持つフォールバックが無ければ、OS のフォントファイルを直読みする。
        if !fallbacks.iter().any(|f| f.lookup_glyph_index('あ') != 0) {
            for p in CJK_FONT_FILES {
                if let Some(data) = load_font_file(p) {
                    if let Ok(f) = Font::from_bytes(
                        data,
                        FontSettings {
                            collection_index: 0,
                            ..Default::default()
                        },
                    ) {
                        if f.lookup_glyph_index('あ') != 0 {
                            fallbacks.push(f);
                            break;
                        }
                    }
                }
            }
        }
    }
    // 最終保険: 同梱 HackGen（システムに日本語フォントが無くても必ず表示できる）。常に末尾へ。
    if let Ok(f) = Font::from_bytes(
        HACKGEN_TTF,
        FontSettings {
            collection_index: 0,
            ..Default::default()
        },
    ) {
        fallbacks.push(f);
    }
    fallbacks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_and_caches_glyph() {
        // コンテナには DejaVu / Noto CJK が導入済み
        let mut fm = FontManager::load(None, &[]).expect("load font");
        let w = fm.cell_width(16.0);
        assert!(w > 0);
        let g = fm.glyph('A', 16.0, false);
        assert!(g.bitmap.w > 0 && g.bitmap.h > 0);
        // 2 回目はキャッシュヒット（パニックしないこと）
        let _ = fm.glyph('A', 16.0, false);
        assert!(fm.cell_height(16.0) > w / 2);
    }

    #[test]
    fn cjk_glyph_available() {
        let mut fm = FontManager::load(None, &[]).expect("load font");
        let g = fm.glyph('あ', 16.0, false);
        assert!(g.bitmap.w > 0 && g.bitmap.h > 0, "CJK グリフが空");
    }
}
