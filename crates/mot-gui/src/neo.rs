//! NEO-UI 描画プリミティブとデザイントークン。
//!
//! tiny-skia で角丸/グラデ/グロー/ブラーを `Pixmap` に描き、`Framebuffer::blit_pixmap`
//! で既存バッファへα合成する。端末セル塗り/glyph 等の高速パスは render.rs のまま使い、
//! ここはクローム（サイドバー・タブ・パネル・枠・発光）専用。
//!
//! 配色（`color`）はクロム固定のネオン。端末**本文**の色は接続先の color_scheme に従う
//! （ここには持ち込まない）。Figma "Future-themed SSH Terminal UI" のパレット準拠。

use crate::render::Framebuffer;
use crate::theme::{pixel_rgb, Pixel};
use tiny_skia::{
    Color, FillRule, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap, Point,
    RadialGradient, Rect, Shader, SpreadMode, Stroke, Transform,
};

/// クローム固定のデザイントークン（値は 0x00RRGGBB。枠はα別指定）。
pub mod color {
    use crate::theme::Pixel;
    pub const BG: Pixel = 0x04_080F; // #04080F
    pub const BG_DEEP: Pixel = 0x02_0509; // #020509
    pub const PANEL: Pixel = 0x07_0D1A; // #070D1A
    pub const PANEL_HOVER: Pixel = 0x0B_1525; // #0B1525
    pub const CYAN: Pixel = 0x00_E5FF; // #00E5FF
    pub const VIOLET: Pixel = 0x7C_5CF6; // #7C5CF6
    pub const GREEN: Pixel = 0x00_FF88; // #00FF88
    pub const AMBER: Pixel = 0xF5_A623; // #F5A623
    pub const RED: Pixel = 0xFF_3860; // #FF3860
    pub const TEXT: Pixel = 0xB8_CDE0; // #B8CDE0
    pub const TEXT_SUB: Pixel = 0x60_7890; // #607890
    pub const TEXT_DIM: Pixel = 0x1E_3550; // #1E3550
    /// 枠の基準色（シアン）。α（例 0.09 / 0.22）は描画時に渡す。
    pub const BORDER: Pixel = CYAN;
}

fn to_color(p: Pixel, alpha: f32) -> Color {
    let (r, g, b) = pixel_rgb(p);
    Color::from_rgba8(r, g, b, (alpha.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// 角丸矩形のパス（ローカル座標）。r は幅/高さの半分にクランプ。
fn round_rect_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.clamp(0.0, (w / 2.0).min(h / 2.0));
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    if r <= 0.0 {
        return Some(PathBuilder::from_rect(Rect::from_xywh(x, y, w, h)?));
    }
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish()
}

/// 角丸矩形の塗り（α合成）。radius=0 で通常矩形。
#[allow(clippy::too_many_arguments)]
pub fn fill_round_rect(
    fb: &mut Framebuffer,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    radius: f32,
    color: Pixel,
    alpha: f32,
) {
    if w <= 0 || h <= 0 || alpha <= 0.0 {
        return;
    }
    let Some(mut pm) = Pixmap::new(w as u32, h as u32) else {
        return;
    };
    if let Some(path) = round_rect_path(0.0, 0.0, w as f32, h as f32, radius) {
        let mut paint = Paint::default();
        paint.set_color(to_color(color, alpha));
        paint.anti_alias = true;
        pm.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    fb.blit_pixmap(x, y, &pm);
}

/// 角丸矩形の枠線（α合成）。width は px。
#[allow(clippy::too_many_arguments)]
pub fn stroke_round_rect(
    fb: &mut Framebuffer,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    radius: f32,
    color: Pixel,
    alpha: f32,
    width: f32,
) {
    if w <= 0 || h <= 0 || alpha <= 0.0 || width <= 0.0 {
        return;
    }
    let Some(mut pm) = Pixmap::new(w as u32, h as u32) else {
        return;
    };
    // 枠が矩形の内側に収まるよう半 width だけ内側へ寄せる。
    let inset = width / 2.0;
    if let Some(path) = round_rect_path(
        inset,
        inset,
        w as f32 - width,
        h as f32 - width,
        (radius - inset).max(0.0),
    ) {
        let mut paint = Paint::default();
        paint.set_color(to_color(color, alpha));
        paint.anti_alias = true;
        let stroke = Stroke {
            width,
            ..Default::default()
        };
        pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
    fb.blit_pixmap(x, y, &pm);
}

/// 縦方向の線形グラデ矩形（top→bottom、α合成）。
#[allow(clippy::too_many_arguments)]
pub fn gradient_rect_v(
    fb: &mut Framebuffer,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    top: Pixel,
    bottom: Pixel,
    alpha: f32,
) {
    if w <= 0 || h <= 0 || alpha <= 0.0 {
        return;
    }
    let Some(mut pm) = Pixmap::new(w as u32, h as u32) else {
        return;
    };
    let shader = LinearGradient::new(
        Point::from_xy(0.0, 0.0),
        Point::from_xy(0.0, h as f32),
        vec![
            GradientStop::new(0.0, to_color(top, alpha)),
            GradientStop::new(1.0, to_color(bottom, alpha)),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    )
    .unwrap_or_else(|| Shader::SolidColor(to_color(top, alpha)));
    let mut paint = Paint {
        shader,
        ..Default::default()
    };
    paint.anti_alias = true;
    if let Some(rect) = Rect::from_xywh(0.0, 0.0, w as f32, h as f32) {
        pm.fill_rect(rect, &paint, Transform::identity(), None);
    }
    fb.blit_pixmap(x, y, &pm);
}

/// 発光ドット: 中心 (cx,cy) に半径 core_r の実心円＋半径 glow_r の放射グロー。
/// 状態ドット・カーソル・アクセント点に使う。
pub fn glow_dot(fb: &mut Framebuffer, cx: i32, cy: i32, core_r: f32, color: Pixel, glow_r: f32) {
    let glow_r = glow_r.max(core_r).max(1.0);
    let d = (glow_r * 2.0).ceil() as u32;
    let Some(mut pm) = Pixmap::new(d.max(1), d.max(1)) else {
        return;
    };
    let center = Point::from_xy(glow_r, glow_r);
    // 放射グロー（中心 0.55α → 外周 透明）。start_radius=0, end_radius=glow_r の同心円。
    if let Some(sh) = RadialGradient::new(
        center,
        0.0,
        center,
        glow_r,
        vec![
            GradientStop::new(0.0, to_color(color, 0.55)),
            GradientStop::new(1.0, Color::TRANSPARENT),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    ) {
        let mut paint = Paint {
            shader: sh,
            ..Default::default()
        };
        paint.anti_alias = true;
        if let Some(p) = PathBuilder::from_circle(glow_r, glow_r, glow_r) {
            pm.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
        }
    }
    // 実心コア
    let mut paint = Paint::default();
    paint.set_color(to_color(color, 1.0));
    paint.anti_alias = true;
    if let Some(p) = PathBuilder::from_circle(glow_r, glow_r, core_r) {
        pm.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
    }
    fb.blit_pixmap(
        (cx as f32 - glow_r) as i32,
        (cy as f32 - glow_r) as i32,
        &pm,
    );
}

/// 分離ボックスブラー（premultiplied RGBA を平均）。グロー/影/すりガラス用。
/// radius が小さい前提の素朴実装（O(w*h*r)）。
pub fn box_blur(pm: &mut Pixmap, radius: usize) {
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    if radius == 0 || w == 0 || h == 0 {
        return;
    }
    let src = pm.data().to_vec(); // RGBA premultiplied
    let mut mid = vec![0u8; src.len()];
    // 水平パス
    for y in 0..h {
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(radius), (x + radius).min(w - 1));
            let mut acc = [0u32; 4];
            let n = (x1 - x0 + 1) as u32;
            for xx in x0..=x1 {
                let i = (y * w + xx) * 4;
                for (c, a) in acc.iter_mut().enumerate() {
                    *a += src[i + c] as u32;
                }
            }
            let o = (y * w + x) * 4;
            for (c, a) in acc.iter().enumerate() {
                mid[o + c] = (a / n) as u8;
            }
        }
    }
    // 垂直パス → pm へ書き戻し
    let dst = pm.data_mut();
    for x in 0..w {
        for y in 0..h {
            let (y0, y1) = (y.saturating_sub(radius), (y + radius).min(h - 1));
            let mut acc = [0u32; 4];
            let n = (y1 - y0 + 1) as u32;
            for yy in y0..=y1 {
                let i = (yy * w + x) * 4;
                for (c, a) in acc.iter_mut().enumerate() {
                    *a += mid[i + c] as u32;
                }
            }
            let o = (y * w + x) * 4;
            for (c, a) in acc.iter().enumerate() {
                dst[o + c] = (a / n) as u8;
            }
        }
    }
}

/// 角丸矩形のソフトシャドウ/外側グロー。要素の背後に blur したにじみを敷く。
/// spread=ブラー半径。要素矩形 (x,y,w,h) の周囲 spread ぶん外側までにじむ。
#[allow(clippy::too_many_arguments)]
pub fn glow_round_rect(
    fb: &mut Framebuffer,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    radius: f32,
    color: Pixel,
    alpha: f32,
    spread: usize,
) {
    if w <= 0 || h <= 0 || alpha <= 0.0 || spread == 0 {
        return;
    }
    let pad = spread as i32 + 2;
    let pw = (w + pad * 2) as u32;
    let ph = (h + pad * 2) as u32;
    let Some(mut pm) = Pixmap::new(pw, ph) else {
        return;
    };
    if let Some(path) = round_rect_path(pad as f32, pad as f32, w as f32, h as f32, radius) {
        let mut paint = Paint::default();
        paint.set_color(to_color(color, alpha));
        paint.anti_alias = true;
        pm.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    box_blur(&mut pm, spread);
    fb.blit_pixmap(x - pad, y - pad, &pm);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px_at(fb: &Framebuffer, x: usize, y: usize) -> (u8, u8, u8) {
        pixel_rgb(fb.buf[y * fb.w + x])
    }

    #[test]
    fn fill_round_rect_center_filled_corner_untouched() {
        let mut fb = Framebuffer::new(40, 40);
        fb.clear(0x000000);
        fill_round_rect(&mut fb, 0, 0, 40, 40, 12.0, color::CYAN, 1.0);
        // 中心はシアンで塗られる
        let (r, g, b) = px_at(&fb, 20, 20);
        assert!(g > 200 && b > 200 && r < 60, "center=({r},{g},{b})");
        // 角(0,0)は丸みで外れるので黒のまま
        assert_eq!(px_at(&fb, 0, 0), (0, 0, 0));
    }

    #[test]
    fn gradient_interpolates_top_to_bottom() {
        let mut fb = Framebuffer::new(10, 40);
        fb.clear(0x000000);
        // 上=緑(0,255,136) 下=紫(124,92,246)
        gradient_rect_v(&mut fb, 0, 0, 10, 40, color::GREEN, color::VIOLET, 1.0);
        let top = px_at(&fb, 5, 1);
        let bot = px_at(&fb, 5, 38);
        // 上は緑寄り(G高), 下は紫寄り(R/B高)
        assert!(top.1 > 180, "top green={:?}", top);
        assert!(bot.0 > 80 && bot.2 > 150, "bottom={:?}", bot);
    }

    #[test]
    fn glow_dot_core_bright_halo_fades() {
        let mut fb = Framebuffer::new(40, 40);
        fb.clear(0x000000);
        glow_dot(&mut fb, 20, 20, 4.0, color::GREEN, 12.0);
        let core = px_at(&fb, 20, 20);
        assert!(core.1 > 200, "core should be bright green: {:?}", core);
        // 外周付近はにじんで暗い（黒との中間）
        let edge = px_at(&fb, 20, 9);
        assert!(
            edge.1 < core.1,
            "halo should fade: edge={:?} core={:?}",
            edge,
            core
        );
    }

    #[test]
    fn box_blur_spreads_alpha() {
        let mut pm = Pixmap::new(9, 9).unwrap();
        // 中央1pxだけ不透明白
        pm.pixels_mut()[4 * 9 + 4] =
            tiny_skia::PremultipliedColorU8::from_rgba(255, 255, 255, 255).unwrap();
        box_blur(&mut pm, 2);
        // 中央は薄まり、隣接に広がる
        let center_a = pm.pixels()[4 * 9 + 4].alpha();
        let neighbor_a = pm.pixels()[4 * 9 + 5].alpha();
        assert!(center_a < 255 && center_a > 0);
        assert!(neighbor_a > 0, "blur should spread to neighbor");
    }
}
