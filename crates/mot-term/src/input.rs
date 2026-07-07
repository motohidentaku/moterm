//! キーボード入力 → PTY へ送るバイト列のエンコード（xterm 互換）。
//! GUI ツールキット非依存の Key 型を定義し、モード（アプリケーションカーソル等）に応じて変換する。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F(u8), // 1..=12
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

impl Mods {
    pub const NONE: Mods = Mods {
        shift: false,
        ctrl: false,
        alt: false,
    };
    /// xterm 修飾キーコード（1 + Shift*1 + Alt*2 + Ctrl*4）
    fn code(&self) -> u8 {
        1 + (self.shift as u8) + (self.alt as u8) * 2 + (self.ctrl as u8) * 4
    }
    pub fn any(&self) -> bool {
        self.shift || self.ctrl || self.alt
    }
}

/// キーをエンコードする。None は「端末へ送らない（GUI 側ショートカット等）」ではなく
/// 「対応するシーケンスが無い」の意。GUI ショートカットの判定は呼び出し側で先に行うこと。
pub fn encode_key(key: Key, mods: Mods, app_cursor: bool) -> Option<Vec<u8>> {
    let out: Vec<u8> = match key {
        Key::Char(c) => {
            let mut buf = Vec::new();
            if mods.alt {
                buf.push(0x1b);
            }
            if mods.ctrl {
                // Ctrl+文字 → C0 制御
                let c = c.to_ascii_uppercase();
                match c {
                    '@'..='_' => buf.push((c as u8) & 0x1f),
                    'a'..='z' => buf.push(c as u8 - b'a' + 1),
                    ' ' => buf.push(0),
                    '?' => buf.push(0x7f),
                    _ => {
                        let mut b = [0u8; 4];
                        buf.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
                    }
                }
            } else {
                let mut b = [0u8; 4];
                buf.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
            }
            buf
        }
        Key::Enter => {
            if mods.alt {
                vec![0x1b, b'\r']
            } else {
                vec![b'\r']
            }
        }
        Key::Tab => {
            if mods.shift {
                b"\x1b[Z".to_vec()
            } else {
                vec![b'\t']
            }
        }
        Key::Backspace => {
            if mods.alt {
                vec![0x1b, 0x7f]
            } else {
                vec![0x7f]
            }
        }
        Key::Escape => vec![0x1b],
        Key::Up | Key::Down | Key::Right | Key::Left => {
            let ch = match key {
                Key::Up => b'A',
                Key::Down => b'B',
                Key::Right => b'C',
                _ => b'D',
            };
            if mods.any() {
                format!("\x1b[1;{}{}", mods.code(), ch as char).into_bytes()
            } else if app_cursor {
                vec![0x1b, b'O', ch]
            } else {
                vec![0x1b, b'[', ch]
            }
        }
        Key::Home | Key::End => {
            let ch = if key == Key::Home { b'H' } else { b'F' };
            if mods.any() {
                format!("\x1b[1;{}{}", mods.code(), ch as char).into_bytes()
            } else if app_cursor {
                vec![0x1b, b'O', ch]
            } else {
                vec![0x1b, b'[', ch]
            }
        }
        Key::PageUp | Key::PageDown | Key::Insert | Key::Delete => {
            let n = match key {
                Key::Insert => 2,
                Key::Delete => 3,
                Key::PageUp => 5,
                _ => 6,
            };
            if mods.any() {
                format!("\x1b[{};{}~", n, mods.code()).into_bytes()
            } else {
                format!("\x1b[{}~", n).into_bytes()
            }
        }
        Key::F(n) => {
            let seq: &[u8] = match n {
                1 => b"\x1bOP",
                2 => b"\x1bOQ",
                3 => b"\x1bOR",
                4 => b"\x1bOS",
                5 => b"\x1b[15~",
                6 => b"\x1b[17~",
                7 => b"\x1b[18~",
                8 => b"\x1b[19~",
                9 => b"\x1b[20~",
                10 => b"\x1b[21~",
                11 => b"\x1b[23~",
                12 => b"\x1b[24~",
                _ => return None,
            };
            if mods.any() {
                match n {
                    1..=4 => format!("\x1b[1;{}{}", mods.code(), *seq.last().unwrap() as char)
                        .into_bytes(),
                    _ => {
                        let num = std::str::from_utf8(&seq[2..seq.len() - 1]).unwrap();
                        format!("\x1b[{};{}~", num, mods.code()).into_bytes()
                    }
                }
            } else {
                seq.to_vec()
            }
        }
    };
    Some(out)
}

/// 貼り付けテキストのエンコード（ブラケットペーストモード対応）
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    // 改行は CR に正規化（端末の Enter と同じ）。ブラケット時は生のまま包む
    let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        let mut out = b"\x1b[200~".to_vec();
        out.extend_from_slice(normalized.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        normalized.into_bytes()
    }
}
