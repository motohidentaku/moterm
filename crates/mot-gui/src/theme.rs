//! 配色。ANSI 16 + 256 パレット + Rgb を u32(0RGB) へ変換。
//! `config.color_scheme` 名から組み込みパレットを選ぶ（無名は既定ダーク）。

use mot_term::Color;

/// 0x00RRGGBB 形式のピクセル。
pub type Pixel = u32;

#[inline]
pub fn rgb(r: u8, g: u8, b: u8) -> Pixel {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

#[inline]
pub fn pixel_rgb(p: Pixel) -> (u8, u8, u8) {
    (
        ((p >> 16) & 0xff) as u8,
        ((p >> 8) & 0xff) as u8,
        (p & 0xff) as u8,
    )
}

/// アルファ合成（0.0..=1.0）。前景 fg を背景 bg に alpha で重ねる。
pub fn blend(bg: Pixel, fg: Pixel, alpha: f32) -> Pixel {
    let a = alpha.clamp(0.0, 1.0);
    let (br, bg_, bb) = pixel_rgb(bg);
    let (fr, fg_, fb) = pixel_rgb(fg);
    let mix = |b: u8, f: u8| ((b as f32) * (1.0 - a) + (f as f32) * a).round() as u8;
    rgb(mix(br, fr), mix(bg_, fg_), mix(bb, fb))
}

#[derive(Debug, Clone)]
pub struct Theme {
    /// ANSI 0..=15
    pub ansi: [Pixel; 16],
    pub fg: Pixel,
    pub bg: Pixel,
    pub cursor: Pixel,
    /// 選択範囲のハイライト背景
    pub selection: Pixel,
    /// UI クローム（タブバー・パネル背景・枠）
    pub ui_bg: Pixel,
    pub ui_panel: Pixel,
    pub ui_fg: Pixel,
    pub ui_dim: Pixel,
    pub ui_accent: Pixel,
    /// ブロードキャスト帯・失敗表示など
    pub warn: Pixel,
}

impl Default for Theme {
    fn default() -> Self {
        Theme::builtin("default")
    }
}

impl Theme {
    /// 名前から組み込みテーマを返す（未知は default）。
    pub fn builtin(name: &str) -> Theme {
        match name
            .to_ascii_lowercase()
            .replace([' ', '-', '_'], "")
            .as_str()
        {
            "tokyonight" | "tokyo" => tokyo_night(),
            "solarizeddark" | "builtinsolarizeddark" => solarized_dark(),
            "gruvbox" | "gruvboxdark" => gruvbox_dark(),
            _ => default_dark(),
        }
    }

    /// mot_term の Color を前景色として解決する。
    pub fn resolve_fg(&self, c: Color) -> Pixel {
        match c {
            Color::Default => self.fg,
            Color::Indexed(i) => self.indexed(i),
            Color::Rgb(r, g, b) => rgb(r, g, b),
        }
    }

    pub fn resolve_bg(&self, c: Color) -> Pixel {
        match c {
            Color::Default => self.bg,
            Color::Indexed(i) => self.indexed(i),
            Color::Rgb(r, g, b) => rgb(r, g, b),
        }
    }

    /// 256 色パレット。0..=15 はテーマ、16..=231 は 6x6x6 キューブ、232..=255 はグレースケール。
    pub fn indexed(&self, i: u8) -> Pixel {
        match i {
            0..=15 => self.ansi[i as usize],
            16..=231 => {
                let i = i - 16;
                let r = i / 36;
                let g = (i % 36) / 6;
                let b = i % 6;
                let conv = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                rgb(conv(r), conv(g), conv(b))
            }
            232..=255 => {
                let v = 8 + (i - 232) * 10;
                rgb(v, v, v)
            }
        }
    }
}

fn default_dark() -> Theme {
    Theme {
        ansi: [
            rgb(0x1e, 0x22, 0x2a), // black
            rgb(0xe0, 0x60, 0x60), // red
            rgb(0x60, 0xc0, 0x70), // green
            rgb(0xd0, 0xb0, 0x50), // yellow
            rgb(0x50, 0x90, 0xd0), // blue
            rgb(0xb0, 0x70, 0xd0), // magenta
            rgb(0x50, 0xb0, 0xc0), // cyan
            rgb(0xc0, 0xc4, 0xcc), // white
            rgb(0x50, 0x56, 0x60), // bright black
            rgb(0xff, 0x80, 0x80),
            rgb(0x80, 0xe0, 0x90),
            rgb(0xf0, 0xd0, 0x70),
            rgb(0x70, 0xb0, 0xf0),
            rgb(0xd0, 0x90, 0xf0),
            rgb(0x70, 0xd0, 0xe0),
            rgb(0xf0, 0xf4, 0xfc),
        ],
        fg: rgb(0xc8, 0xcc, 0xd4),
        bg: rgb(0x16, 0x19, 0x20),
        cursor: rgb(0xd0, 0xd4, 0xdc),
        selection: rgb(0x33, 0x48, 0x66),
        ui_bg: rgb(0x10, 0x12, 0x18),
        ui_panel: rgb(0x1b, 0x1f, 0x28),
        ui_fg: rgb(0xd0, 0xd4, 0xdc),
        ui_dim: rgb(0x70, 0x76, 0x84),
        ui_accent: rgb(0x50, 0x90, 0xd0),
        warn: rgb(0xc0, 0x30, 0x30),
    }
}

fn tokyo_night() -> Theme {
    let mut t = default_dark();
    t.bg = rgb(0x1a, 0x1b, 0x26);
    t.ui_bg = rgb(0x16, 0x16, 0x1e);
    t.ui_panel = rgb(0x1f, 0x22, 0x35);
    t.fg = rgb(0xa9, 0xb1, 0xd6);
    t.ansi[4] = rgb(0x7a, 0xa2, 0xf7);
    t.ansi[5] = rgb(0xbb, 0x9a, 0xf7);
    t.ui_accent = rgb(0x7a, 0xa2, 0xf7);
    t
}

fn solarized_dark() -> Theme {
    let mut t = default_dark();
    t.bg = rgb(0x00, 0x2b, 0x36);
    t.ui_bg = rgb(0x00, 0x22, 0x2b);
    t.ui_panel = rgb(0x07, 0x36, 0x42);
    t.fg = rgb(0x83, 0x94, 0x96);
    t.ansi[1] = rgb(0xdc, 0x32, 0x2f);
    t.ansi[2] = rgb(0x85, 0x99, 0x00);
    t.ui_accent = rgb(0x26, 0x8b, 0xd2);
    t
}

fn gruvbox_dark() -> Theme {
    let mut t = default_dark();
    t.bg = rgb(0x28, 0x28, 0x28);
    t.ui_bg = rgb(0x1d, 0x20, 0x21);
    t.ui_panel = rgb(0x32, 0x30, 0x2f);
    t.fg = rgb(0xeb, 0xdb, 0xb2);
    t.ansi[1] = rgb(0xcc, 0x24, 0x1d);
    t.ansi[2] = rgb(0x98, 0x97, 0x1a);
    t.ui_accent = rgb(0x45, 0x85, 0x88);
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_pack_unpack() {
        assert_eq!(pixel_rgb(rgb(0x12, 0x34, 0x56)), (0x12, 0x34, 0x56));
    }

    #[test]
    fn indexed_cube_and_grayscale() {
        let t = default_dark();
        assert_eq!(t.indexed(16), rgb(0, 0, 0)); // キューブ原点
        assert_eq!(t.indexed(231), rgb(255, 255, 255)); // キューブ最大
        assert_eq!(t.indexed(232), rgb(8, 8, 8)); // グレー最小
        assert_eq!(t.indexed(255), rgb(238, 238, 238));
        assert_eq!(t.indexed(1), t.ansi[1]);
    }

    #[test]
    fn resolve_colors() {
        let t = default_dark();
        assert_eq!(t.resolve_fg(Color::Default), t.fg);
        assert_eq!(t.resolve_bg(Color::Default), t.bg);
        assert_eq!(t.resolve_fg(Color::Rgb(1, 2, 3)), rgb(1, 2, 3));
        assert_eq!(t.resolve_fg(Color::Indexed(2)), t.ansi[2]);
    }

    #[test]
    fn blend_endpoints() {
        assert_eq!(blend(rgb(0, 0, 0), rgb(255, 255, 255), 0.0), rgb(0, 0, 0));
        assert_eq!(
            blend(rgb(0, 0, 0), rgb(255, 255, 255), 1.0),
            rgb(255, 255, 255)
        );
        assert_eq!(
            blend(rgb(0, 0, 0), rgb(100, 100, 100), 0.5),
            rgb(50, 50, 50)
        );
    }

    #[test]
    fn named_themes_differ() {
        assert_ne!(
            Theme::builtin("Tokyo Night").bg,
            Theme::builtin("Gruvbox Dark").bg
        );
        assert_eq!(Theme::builtin("unknown-xyz").bg, default_dark().bg);
    }
}
