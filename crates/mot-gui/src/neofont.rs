//! NEO-UI 用の同梱フォント＆アイコン描画。
//!
//! 端末の等幅 `FontManager` とは独立に、UI 用の**可変幅** Inter、値表示用の等幅
//! JetBrains Mono（Regular/Bold）、lucide アイコンフォントを**バイナリ埋め込み**で持つ。
//! いずれも `fontdue` でラスタライズし、既存の `Framebuffer::blit_coverage` で合成する。
//! （ライセンス: Inter=OFL, JetBrains Mono=OFL, lucide=ISC。assets/fonts に同梱）

use crate::font::{Glyph, GlyphBitmap};
use crate::render::Framebuffer;
use crate::theme::Pixel;
use fontdue::{Font, FontSettings};
use std::collections::HashMap;

const UI_TTF: &[u8] = include_bytes!("../../../assets/fonts/Inter-Variable.ttf");
const MONO_TTF: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");
const MONO_BOLD_TTF: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Bold.ttf");
const ICON_TTF: &[u8] = include_bytes!("../../../assets/fonts/lucide.ttf");

/// 使用するフェイス。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Face {
    /// Inter（可変幅UI）
    Ui,
    /// JetBrains Mono Regular（等幅・値/コード）
    Mono,
    /// JetBrains Mono Bold
    MonoBold,
    /// lucide アイコン
    Icon,
}

/// lucide アイコンのコードポイント（PUA）。名称は lucide のアイコン名に対応。
pub mod icon {
    pub const SERVER: char = '\u{e153}';
    pub const DATABASE: char = '\u{e0ad}';
    pub const GLOBE: char = '\u{e0e8}';
    pub const CHEVRON_DOWN: char = '\u{e06d}';
    pub const CHEVRON_RIGHT: char = '\u{e06f}';
    pub const STAR: char = '\u{e176}';
    pub const KEY: char = '\u{e0fd}';
    pub const SHIELD: char = '\u{e158}';
    pub const ZAP: char = '\u{e1b4}';
    pub const ARROW_UP: char = '\u{e04a}';
    pub const ARROW_DOWN: char = '\u{e042}';
    pub const HARD_DRIVE: char = '\u{e0ed}';
    pub const CPU: char = '\u{e0a9}';
    pub const WIFI: char = '\u{e1ae}';
    pub const ACTIVITY: char = '\u{e038}';
    pub const PLUS: char = '\u{e13d}';
    pub const X: char = '\u{e1b2}';
    pub const SEARCH: char = '\u{e151}';
    pub const SETTINGS: char = '\u{e154}';
    pub const TERMINAL: char = '\u{e181}';
    pub const PENCIL: char = '\u{e1f9}';
    pub const COPY: char = '\u{e09e}';
    pub const FOLDER: char = '\u{e0d7}';
    pub const FILE: char = '\u{e0c0}';
    pub const CORNER_UP: char = '\u{e0a4}';
    pub const UP_DOWN: char = '\u{e37d}';
    pub const ROTATE: char = '\u{e149}';
    pub const TRASH: char = '\u{e18e}';
    pub const FOLDER_PLUS: char = '\u{e0d9}';
    pub const UPLOAD: char = '\u{e19e}';
    pub const DOWNLOAD: char = '\u{e0b2}';
    pub const LOCK: char = '\u{e10b}';
    pub const UNLOCK: char = '\u{e10c}';
    pub const EYE: char = '\u{e0ba}';
    pub const EYE_OFF: char = '\u{e0bb}';
}

pub struct NeoFonts {
    ui: Font,
    mono: Font,
    mono_bold: Font,
    icons: Font,
    /// CJK フォールバック（Inter/JetBrains Mono に無い日本語グリフ用。システムから探索）。
    cjk: Vec<Font>,
    cache: HashMap<(Face, char, u32), Glyph>,
}

impl NeoFonts {
    /// fallback_names は config.font_fallback（CJK フォント名の明示指定。任意）。
    pub fn load(fallback_names: &[String]) -> anyhow::Result<NeoFonts> {
        let s = || FontSettings::default();
        let mk = |data: &[u8]| -> anyhow::Result<Font> {
            Font::from_bytes(data, s()).map_err(|e| anyhow::anyhow!("NEO フォント解析失敗: {e}"))
        };
        Ok(NeoFonts {
            ui: mk(UI_TTF)?,
            mono: mk(MONO_TTF)?,
            mono_bold: mk(MONO_BOLD_TTF)?,
            icons: mk(ICON_TTF)?,
            cjk: crate::font::discover_cjk_fonts(fallback_names),
            cache: HashMap::new(),
        })
    }

    fn font_of(&self, face: Face) -> &Font {
        match face {
            Face::Ui => &self.ui,
            Face::Mono => &self.mono,
            Face::MonoBold => &self.mono_bold,
            Face::Icon => &self.icons,
        }
    }

    /// face のプライマリにグリフが無ければ CJK フォールバックへ（アイコンは対象外）。
    fn font_for(&self, face: Face, ch: char) -> &Font {
        let primary = self.font_of(face);
        if face == Face::Icon || primary.lookup_glyph_index(ch) != 0 {
            return primary;
        }
        for f in &self.cjk {
            if f.lookup_glyph_index(ch) != 0 {
                return f;
            }
        }
        primary
    }

    fn glyph(&mut self, face: Face, ch: char, px: f32) -> &Glyph {
        let key = (face, ch, px.to_bits());
        if !self.cache.contains_key(&key) {
            let (metrics, bitmap) = self.font_for(face, ch).rasterize(ch, px);
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

    /// 可変幅テキスト（Inter 等）を baseline 基準で描画。返り値は描画後の右端 x。
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        fb: &mut Framebuffer,
        face: Face,
        text: &str,
        x: i32,
        baseline_y: i32,
        px: f32,
        color: Pixel,
    ) -> i32 {
        let mut cx = x as f32;
        for ch in text.chars() {
            let g = self.glyph(face, ch, px);
            let gx = cx.round() as i32 + g.left;
            let gy = baseline_y - g.top;
            fb.blit_coverage(gx, gy, &g.bitmap, color);
            cx += g.advance;
        }
        cx.round() as i32
    }

    /// テキストの描画幅（px、可変幅対応）。センタリング/右寄せの座標計算に使う。
    pub fn measure(&mut self, face: Face, text: &str, px: f32) -> i32 {
        let mut w = 0.0f32;
        for ch in text.chars() {
            w += self.glyph(face, ch, px).advance;
        }
        w.round() as i32
    }

    /// アイコンを (x,y) 左上基準・size_px 角で描画（lucide、単色）。
    pub fn draw_icon(
        &mut self,
        fb: &mut Framebuffer,
        ic: char,
        x: i32,
        y: i32,
        size_px: f32,
        color: Pixel,
    ) {
        let g = self.glyph(Face::Icon, ic, size_px);
        // lucide グリフはem内に収まる。上端 y からベースライン近似で配置。
        let baseline = y + (size_px * 0.82).round() as i32;
        let gx = x + g.left;
        let gy = baseline - g.top;
        fb.blit_coverage(gx, gy, &g.bitmap, color);
    }
}
