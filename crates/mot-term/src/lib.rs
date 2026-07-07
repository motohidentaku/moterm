//! mot-term — VT エスケープシーケンスパーサ＋端末スクリーンモデル。
//! WezTerm 系 crate を使わないゼロ実装。GUI/SSH 非依存で、バイト列 in → グリッド状態 out。

pub mod cell;
pub mod input;
pub mod parser;
pub mod screen;

pub use cell::{Attrs, Cell, Color};
pub use input::{encode_key, encode_paste, Key, Mods};
pub use screen::{Line, MouseButton, MouseEncoding, MouseMode, Screen, TermEvent};

use parser::Parser;

/// パーサとスクリーンを束ねた端末エミュレータ本体。
pub struct Terminal {
    parser: Parser,
    pub screen: Screen,
}

impl Terminal {
    pub fn new(cols: usize, rows: usize, scrollback_limit: usize) -> Self {
        Terminal {
            parser: Parser::new(),
            screen: Screen::new(cols, rows, scrollback_limit),
        }
    }

    /// リモートから受信したバイト列を反映する。
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.screen, bytes);
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.screen.resize(cols, rows);
    }
}
