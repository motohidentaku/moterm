//! セルとその属性。グリッド1マス分の状態。

/// bitflags crate を使わない最小のフラグ実装（依存削減）。
macro_rules! bitflags_lite {
    (
        $(#[$meta:meta])*
        pub struct $name:ident : $ty:ty {
            $( $(#[$fmeta:meta])* const $flag:ident = $val:expr; )*
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
        pub struct $name($ty);
        impl $name {
            $( $(#[$fmeta])* pub const $flag: $name = $name($val); )*
            pub const fn empty() -> Self { $name(0) }
            pub const fn bits(&self) -> $ty { self.0 }
            pub const fn contains(&self, other: $name) -> bool { self.0 & other.0 == other.0 }
            pub fn insert(&mut self, other: $name) { self.0 |= other.0; }
            pub fn remove(&mut self, other: $name) { self.0 &= !other.0; }
            pub fn set(&mut self, other: $name, on: bool) {
                if on { self.insert(other) } else { self.remove(other) }
            }
        }
        impl core::ops::BitOr for $name {
            type Output = $name;
            fn bitor(self, rhs: $name) -> $name { $name(self.0 | rhs.0) }
        }
    };
}

/// 256色＋トゥルーカラー＋デフォルトを表す色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    #[default]
    Default,
    /// ANSI 0-15 / 256色パレットのインデックス
    Indexed(u8),
    Rgb(u8, u8, u8),
}

bitflags_lite! {
    /// SGR 属性フラグ
    pub struct Attrs: u16 {
        const BOLD          = 1 << 0;
        const DIM           = 1 << 1;
        const ITALIC        = 1 << 2;
        const UNDERLINE     = 1 << 3;
        const BLINK         = 1 << 4;
        const REVERSE       = 1 << 5;
        const HIDDEN        = 1 << 6;
        const STRIKETHROUGH = 1 << 7;
        /// 全角文字の後続セル（描画スキップ用）
        const WIDE_TRAILER  = 1 << 8;
        /// 全角文字の先頭セル
        const WIDE          = 1 << 9;
    }
}

/// OSC 8 ハイパーリンクの ID（Screen 側の表に対するインデックス。0 = リンクなし）
pub type LinkId = u32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attrs,
    pub link: LinkId,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            ch: ' ',
            fg: Color::Default,
            bg: Color::Default,
            attrs: Attrs::empty(),
            link: 0,
        }
    }
}

impl Cell {
    pub fn blank_with_bg(bg: Color) -> Self {
        Cell {
            bg,
            ..Default::default()
        }
    }
    pub fn is_wide(&self) -> bool {
        self.attrs.contains(Attrs::WIDE)
    }
    pub fn is_wide_trailer(&self) -> bool {
        self.attrs.contains(Attrs::WIDE_TRAILER)
    }
}
