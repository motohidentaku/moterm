//! CPU フレームバッファと描画プリミティブ。softbuffer へ渡す 0RGB の Vec<u32>。

use crate::font::FontManager;
use crate::theme::{blend, pixel_rgb, rgb, Pixel};

pub struct Framebuffer {
    pub w: usize,
    pub h: usize,
    pub buf: Vec<u32>,
}

impl Framebuffer {
    pub fn new(w: usize, h: usize) -> Self {
        Framebuffer {
            w: w.max(1),
            h: h.max(1),
            buf: vec![0; w.max(1) * h.max(1)],
        }
    }

    pub fn resize(&mut self, w: usize, h: usize) {
        self.w = w.max(1);
        self.h = h.max(1);
        self.buf.resize(self.w * self.h, 0);
    }

    pub fn clear(&mut self, color: Pixel) {
        self.buf.iter_mut().for_each(|p| *p = color);
    }

    #[inline]
    pub fn put(&mut self, x: i32, y: i32, color: Pixel) {
        if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h {
            self.buf[y as usize * self.w + x as usize] = color;
        }
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: Pixel) {
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w).min(self.w as i32);
        let y1 = (y + h).min(self.h as i32);
        for yy in y0..y1 {
            let row = yy as usize * self.w;
            for xx in x0..x1 {
                self.buf[row + xx as usize] = color;
            }
        }
    }

    /// 枠線（塗りつぶさない矩形）
    pub fn stroke_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: Pixel) {
        self.fill_rect(x, y, w, 1, color);
        self.fill_rect(x, y + h - 1, w, 1, color);
        self.fill_rect(x, y, 1, h, color);
        self.fill_rect(x + w - 1, y, 1, h, color);
    }

    /// グリフのカバレッジビットマップを (x,y) 左上基準で前景色 fg で合成する。
    pub fn blit_coverage(&mut self, x: i32, y: i32, cov: &crate::font::GlyphBitmap, fg: Pixel) {
        for gy in 0..cov.h as i32 {
            let py = y + gy;
            if py < 0 || py as usize >= self.h {
                continue;
            }
            for gx in 0..cov.w as i32 {
                let px = x + gx;
                if px < 0 || px as usize >= self.w {
                    continue;
                }
                let a = cov.data[gy as usize * cov.w + gx as usize];
                if a == 0 {
                    continue;
                }
                let idx = py as usize * self.w + px as usize;
                let bg = self.buf[idx];
                self.buf[idx] = blend(bg, fg, a as f32 / 255.0);
            }
        }
    }

    /// 文字列を等幅で描画（UI テキスト用。セルグリッドは termview 側）。
    /// 返り値は描画後の x 位置。
    #[allow(clippy::too_many_arguments)]
    pub fn draw_text(
        &mut self,
        font: &mut FontManager,
        text: &str,
        x: i32,
        baseline_y: i32,
        px: f32,
        fg: Pixel,
    ) -> i32 {
        let mut cx = x;
        let cell_w = font.cell_width(px);
        for ch in text.chars() {
            let g = font.glyph(ch, px, false);
            let gx = cx + g.left;
            let gy = baseline_y - g.top;
            self.blit_coverage(gx, gy, &g.bitmap, fg);
            let adv = if unicode_width_ch(ch) == 2 {
                cell_w * 2
            } else {
                cell_w
            };
            cx += adv;
        }
        cx
    }

    /// 背景画像を cover スケール（アスペクト維持で画面全体を覆う）で敷く。
    /// 各ピクセルは `base`（テーマ背景）に画像を `opacity` で重ねた色にする
    /// （opacity=0.25 なら暗めに透ける）。事前に cover スケール済みの
    /// バッファ `scaled`（self.w×self.h の Pixel 配列）を受け取る。
    pub fn draw_background(&mut self, scaled: &[Pixel], base: Pixel, opacity: f32) {
        for (dst, &src) in self.buf.iter_mut().zip(scaled.iter()) {
            *dst = blend(base, src, opacity);
        }
    }

    /// 透過ウィンドウ用: テーマ背景と一致するピクセルの上位バイト（アルファ）を
    /// `bg_alpha` に、その他（文字・パネル等）を不透明(0xFF)にする。
    /// opacity<1.0 のときだけ present 直前に呼ぶ（既定 1.0 では呼ばない＝従来と同一）。
    pub fn apply_alpha(&mut self, bg_color: Pixel, bg_alpha: u8) {
        let bg_rgb = bg_color & 0x00FF_FFFF;
        let a = (bg_alpha as u32) << 24;
        let opaque = 0xFF00_0000u32;
        for p in self.buf.iter_mut() {
            if (*p & 0x00FF_FFFF) == bg_rgb {
                *p = (*p & 0x00FF_FFFF) | a;
            } else {
                *p = (*p & 0x00FF_FFFF) | opaque;
            }
        }
    }

    /// 画面全体を color 方向へ alpha だけ寄せて暗くする（モーダルの暗幕用）。
    pub fn overlay_dim(&mut self, color: Pixel, alpha: f32) {
        let a = alpha.clamp(0.0, 1.0);
        for p in self.buf.iter_mut() {
            *p = crate::theme::blend(*p, color, a);
        }
    }

    /// NEO-UI 描画基盤の橋渡し: tiny-skia で描いた `Pixmap`（straight RGBA, 上→下）を
    /// フレームバッファの (dx,dy) 左上へアルファ合成する。Pixmap のアルファに従って
    /// 既存ピクセルへ `blend` で重ねる（角丸/グラデ/グロー/すりガラス等を tiny-skia で
    /// 描いてから合成する用途。既存の高速プリミティブ＝端末セル塗り/glyph はそのまま）。
    pub fn blit_pixmap(&mut self, dx: i32, dy: i32, pm: &tiny_skia::Pixmap) {
        let (pw, ph) = (pm.width() as i32, pm.height() as i32);
        let px = pm.pixels();
        for sy in 0..ph {
            let py = dy + sy;
            if py < 0 || py as usize >= self.h {
                continue;
            }
            let row = py as usize * self.w;
            let srow = sy as usize * pw as usize;
            for sx in 0..pw {
                let qx = dx + sx;
                if qx < 0 || qx as usize >= self.w {
                    continue;
                }
                // tiny_skia::Pixmap::pixels() は premultiplied。demultiply して straight に。
                let c = px[srow + sx as usize];
                let a = c.alpha();
                if a == 0 {
                    continue;
                }
                let idx = row + qx as usize;
                if a == 255 {
                    self.buf[idx] = rgb(c.red(), c.green(), c.blue());
                } else {
                    // premultiplied → straight（0除算回避）
                    let inv = 255.0 / a as f32;
                    let sr = (c.red() as f32 * inv).min(255.0) as u8;
                    let sg = (c.green() as f32 * inv).min(255.0) as u8;
                    let sb = (c.blue() as f32 * inv).min(255.0) as u8;
                    self.buf[idx] = blend(self.buf[idx], rgb(sr, sg, sb), a as f32 / 255.0);
                }
            }
        }
    }

    /// PNG として保存（--screenshot 用）。
    pub fn save_png(&self, path: &str) -> std::io::Result<()> {
        let mut rgb_buf = Vec::with_capacity(self.w * self.h * 3);
        for &p in &self.buf {
            let (r, g, b) = pixel_rgb(p);
            rgb_buf.push(r);
            rgb_buf.push(g);
            rgb_buf.push(b);
        }
        image::save_buffer(
            path,
            &rgb_buf,
            self.w as u32,
            self.h as u32,
            image::ColorType::Rgb8,
        )
        .map_err(|e| std::io::Error::other(e.to_string()))
    }
}

fn unicode_width_ch(ch: char) -> usize {
    use unicode_width::UnicodeWidthChar;
    ch.width().unwrap_or(0)
}

/// UI の淡い前景（ヒント等）
pub fn dim(base: Pixel) -> Pixel {
    blend(rgb(0, 0, 0), base, 0.6)
}

/// cover スケール: 出力 (dst_w×dst_h) の座標 (dx,dy) に対応する
/// 入力画像 (src_w×src_h) のサンプル座標 (sx,sy) を返す。
/// アスペクト比を保ち、画面全体を覆う（はみ出しは中央クロップ）。
pub fn cover_sample_coord(
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    dx: u32,
    dy: u32,
) -> (u32, u32) {
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return (0, 0);
    }
    // scale = max(dst_w/src_w, dst_h/src_h) を分数のまま扱う。
    // 覆うために各軸の縮尺のうち大きい方を採用する。
    // sx = (dx - offx) / scale。offx は中央寄せのオフセット。
    let scale_num_w = dst_w as u64;
    let scale_den_w = src_w as u64;
    let scale_num_h = dst_h as u64;
    let scale_den_h = src_h as u64;
    // 幅基準スケールと高さ基準スケールを比較（dst_w/src_w vs dst_h/src_h）。
    let use_width_scale = scale_num_w * scale_den_h >= scale_num_h * scale_den_w;
    let (num, den) = if use_width_scale {
        (scale_num_w, scale_den_w) // scale = dst_w/src_w
    } else {
        (scale_num_h, scale_den_h) // scale = dst_h/src_h
    };
    // スケール後の画像サイズ
    let scaled_w = (src_w as u64 * num / den).max(1);
    let scaled_h = (src_h as u64 * num / den).max(1);
    let offx = (scaled_w.saturating_sub(dst_w as u64)) / 2;
    let offy = (scaled_h.saturating_sub(dst_h as u64)) / 2;
    // 出力座標 → スケール画像座標 → 元画像座標
    let sxs = dx as u64 + offx;
    let sys = dy as u64 + offy;
    let sx = (sxs * den / num).min(src_w as u64 - 1);
    let sy = (sys * den / num).min(src_h as u64 - 1);
    (sx as u32, sy as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_maps_corners_within_source() {
        // 100x100 の画像を 200x100 に cover（幅基準で 2x、縦は中央クロップ）
        let (sx, sy) = cover_sample_coord(100, 100, 200, 100, 0, 0);
        assert!(sx < 100 && sy < 100);
        // 右下端も範囲内
        let (sx2, sy2) = cover_sample_coord(100, 100, 200, 100, 199, 99);
        assert!(sx2 < 100 && sy2 < 100);
        // 中央付近は元画像の中央付近
        let (cx, _cy) = cover_sample_coord(100, 100, 200, 100, 100, 50);
        assert!((40..=60).contains(&cx));
    }

    #[test]
    fn cover_handles_degenerate() {
        assert_eq!(cover_sample_coord(0, 0, 10, 10, 5, 5), (0, 0));
        assert_eq!(cover_sample_coord(10, 10, 0, 0, 0, 0), (0, 0));
    }

    #[test]
    fn blit_pixmap_composites_opaque_and_alpha() {
        // 不透明: tiny-skia で塗った Pixmap がそのまま乗る
        let mut fb = Framebuffer::new(4, 4);
        fb.clear(rgb(0, 0, 0));
        let mut pm = tiny_skia::Pixmap::new(2, 2).unwrap();
        pm.fill(tiny_skia::Color::from_rgba8(0, 229, 255, 255));
        fb.blit_pixmap(1, 1, &pm);
        assert_eq!(fb.buf[4 + 1], rgb(0, 229, 255)); // (1,1) に合成
        assert_eq!(fb.buf[0], rgb(0, 0, 0)); // 範囲外は不変

        // 半透明: 50% 白を黒へ → 中間グレー（premul→straight→blend の往復確認）
        let mut fb2 = Framebuffer::new(1, 1);
        fb2.clear(rgb(0, 0, 0));
        let mut pm2 = tiny_skia::Pixmap::new(1, 1).unwrap();
        pm2.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 128));
        fb2.blit_pixmap(0, 0, &pm2);
        let (r, g, b) = pixel_rgb(fb2.buf[0]);
        assert!(
            (118..=138).contains(&r) && (118..=138).contains(&g) && (118..=138).contains(&b),
            "expected ~mid gray, got ({r},{g},{b})"
        );
    }

    #[test]
    fn apply_alpha_marks_background_translucent() {
        let mut fb = Framebuffer::new(2, 1);
        let bg = rgb(10, 20, 30);
        let text = rgb(200, 200, 200);
        fb.buf[0] = bg;
        fb.buf[1] = text;
        fb.apply_alpha(bg, 128);
        assert_eq!(fb.buf[0] >> 24, 128); // 背景は半透明
        assert_eq!(fb.buf[1] >> 24, 0xFF); // 文字は不透明
    }
}
