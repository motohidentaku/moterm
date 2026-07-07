//! VT エスケープシーケンスパーサ。
//! Paul Williams の VT500 パーサ状態機械を基にした実装（DEC STD-070 相当のサブセット）。
//! バイト列を受け取り、ハンドラ（Screen）へ print / execute / csi / esc / osc をディスパッチする。

/// CSI パラメータ列。`;` 区切り。`:` 副パラメータは SGR の色指定でのみ意味を持つため、
/// 各パラメータを「主値＋副値列」として保持する。
#[derive(Debug, Default, Clone)]
pub struct Params {
    items: Vec<Vec<u16>>,
}

impl Params {
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    /// n 番目の主値（未指定・0 は default に置換）
    pub fn get(&self, n: usize, default: u16) -> u16 {
        match self.items.get(n).and_then(|v| v.first()) {
            Some(&v) if v != 0 => v,
            _ => default,
        }
    }
    /// n 番目の主値（未指定は default。0 はそのまま返す）
    pub fn get_raw(&self, n: usize, default: u16) -> u16 {
        match self.items.get(n).and_then(|v| v.first()) {
            Some(&v) => v,
            None => default,
        }
    }
    /// n 番目のパラメータ全体（副パラメータ込み）
    pub fn subparams(&self, n: usize) -> &[u16] {
        self.items.get(n).map(|v| v.as_slice()).unwrap_or(&[])
    }
    fn push_new(&mut self) {
        if self.items.len() < MAX_PARAMS {
            self.items.push(vec![0]);
        }
    }
    fn push_sub(&mut self) {
        if let Some(last) = self.items.last_mut() {
            if last.len() < MAX_SUBPARAMS {
                last.push(0);
            }
        }
    }
    fn digit(&mut self, d: u16) {
        if self.items.is_empty() {
            self.push_new();
        }
        if let Some(v) = self.items.last_mut().and_then(|p| p.last_mut()) {
            *v = v.saturating_mul(10).saturating_add(d);
        }
    }
    fn clear(&mut self) {
        self.items.clear();
    }
}

const MAX_PARAMS: usize = 32;
const MAX_SUBPARAMS: usize = 8;
/// OSC 52 のクリップボード転送を考慮した上限（xterm 相当）
const MAX_OSC_LEN: usize = 128 * 1024;
const MAX_INTERMEDIATES: usize = 4;

/// パーサからのディスパッチ先。
pub trait VtHandler {
    /// 表示可能文字
    fn print(&mut self, ch: char);
    /// C0 制御（BS/HT/LF/CR/BEL 等）
    fn execute(&mut self, byte: u8);
    /// CSI シーケンス。private_prefix は `?` `>` `<` `=` のいずれか。
    fn csi(
        &mut self,
        params: &Params,
        private_prefix: Option<u8>,
        intermediates: &[u8],
        final_byte: u8,
    );
    /// ESC シーケンス（CSI/OSC/DCS 以外）
    fn esc(&mut self, intermediates: &[u8], final_byte: u8);
    /// OSC 文字列（終端は含まない生バイト）
    fn osc(&mut self, data: &[u8]);
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    Ground,
    Escape,
    EscapeIntermediate,
    CsiEntry,
    CsiParam,
    CsiIntermediate,
    CsiIgnore,
    OscString,
    /// DCS / SOS / PM / APC — ST まで読み捨て
    IgnoreUntilSt,
}

pub struct Parser {
    state: State,
    params: Params,
    private_prefix: Option<u8>,
    intermediates: Vec<u8>,
    osc_buf: Vec<u8>,
    /// OSC 文字列の途中で ESC を読んだ（次が `\` なら ST 終端）
    osc_pending: bool,
    /// UTF-8 デコード途中のバッファ
    utf8_buf: [u8; 4],
    utf8_len: usize,
    utf8_need: usize,
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser {
    pub fn new() -> Self {
        Parser {
            state: State::Ground,
            params: Params::default(),
            private_prefix: None,
            intermediates: Vec::new(),
            osc_buf: Vec::new(),
            osc_pending: false,
            utf8_buf: [0; 4],
            utf8_len: 0,
            utf8_need: 0,
        }
    }

    pub fn advance<H: VtHandler>(&mut self, handler: &mut H, bytes: &[u8]) {
        for &b in bytes {
            self.byte(handler, b);
        }
    }

    fn clear_seq(&mut self) {
        self.params.clear();
        self.private_prefix = None;
        self.intermediates.clear();
    }

    fn byte<H: VtHandler>(&mut self, h: &mut H, b: u8) {
        // UTF-8 継続処理は Ground でのみ発生する
        if self.utf8_need > 0 && self.state == State::Ground {
            if (0x80..0xC0).contains(&b) {
                self.utf8_buf[self.utf8_len] = b;
                self.utf8_len += 1;
                if self.utf8_len == self.utf8_need {
                    let ch = core::str::from_utf8(&self.utf8_buf[..self.utf8_len])
                        .ok()
                        .and_then(|s| s.chars().next())
                        .unwrap_or(char::REPLACEMENT_CHARACTER);
                    h.print(ch);
                    self.utf8_need = 0;
                    self.utf8_len = 0;
                }
                return;
            }
            // 不正な継続バイト: 置換文字を出して通常処理を続ける
            h.print(char::REPLACEMENT_CHARACTER);
            self.utf8_need = 0;
            self.utf8_len = 0;
        }

        match self.state {
            State::Ground => self.ground(h, b),
            State::Escape => self.escape(h, b),
            State::EscapeIntermediate => match b {
                0x20..=0x2F => self.collect(b),
                0x30..=0x7E => {
                    h.esc(&self.intermediates, b);
                    self.state = State::Ground;
                }
                _ => self.anywhere_control(h, b),
            },
            State::CsiEntry => match b {
                0x30..=0x39 | 0x3B => {
                    self.params_byte(b);
                    self.state = State::CsiParam;
                }
                0x3A => self.state = State::CsiIgnore,
                0x3C..=0x3F => {
                    self.private_prefix = Some(b);
                    self.state = State::CsiParam;
                }
                0x20..=0x2F => {
                    self.collect(b);
                    self.state = State::CsiIntermediate;
                }
                0x40..=0x7E => self.csi_dispatch(h, b),
                _ => self.anywhere_control(h, b),
            },
            State::CsiParam => match b {
                0x30..=0x3B => self.params_byte(b),
                0x3C..=0x3F => self.state = State::CsiIgnore,
                0x20..=0x2F => {
                    self.collect(b);
                    self.state = State::CsiIntermediate;
                }
                0x40..=0x7E => self.csi_dispatch(h, b),
                _ => self.anywhere_control(h, b),
            },
            State::CsiIntermediate => match b {
                0x20..=0x2F => self.collect(b),
                0x30..=0x3F => self.state = State::CsiIgnore,
                0x40..=0x7E => self.csi_dispatch(h, b),
                _ => self.anywhere_control(h, b),
            },
            State::CsiIgnore => match b {
                0x40..=0x7E => self.state = State::Ground,
                _ => self.anywhere_control(h, b),
            },
            State::OscString => match b {
                0x07 => {
                    // BEL 終端
                    let buf = core::mem::take(&mut self.osc_buf);
                    h.osc(&buf);
                    self.state = State::Ground;
                }
                0x1B => {
                    // ESC \ (ST) 終端は escape() 側で処理
                    self.osc_pending = true;
                    self.state = State::Escape;
                }
                0x00..=0x06 | 0x08..=0x1A | 0x1C..=0x1F => {} // その他 C0 は無視
                _ => {
                    if self.osc_buf.len() < MAX_OSC_LEN {
                        self.osc_buf.push(b);
                    }
                }
            },
            State::IgnoreUntilSt => match b {
                0x1B => self.state = State::Escape,
                0x07 => self.state = State::Ground, // xterm は BEL 終端も許容
                _ => {}
            },
        }
    }

    fn ground<H: VtHandler>(&mut self, h: &mut H, b: u8) {
        match b {
            0x1B => {
                self.clear_seq();
                self.state = State::Escape;
            }
            0x00..=0x1F | 0x7F => h.execute(b),
            0x20..=0x7E => h.print(b as char),
            0xC2..=0xDF => {
                self.utf8_start(b, 2);
            }
            0xE0..=0xEF => {
                self.utf8_start(b, 3);
            }
            0xF0..=0xF4 => {
                self.utf8_start(b, 4);
            }
            _ => h.print(char::REPLACEMENT_CHARACTER), // 不正な先頭バイト
        }
    }

    fn utf8_start(&mut self, b: u8, need: usize) {
        self.utf8_buf[0] = b;
        self.utf8_len = 1;
        self.utf8_need = need;
    }

    fn escape<H: VtHandler>(&mut self, h: &mut H, b: u8) {
        // OSC 途中から来た ESC: `\` なら ST 終端、それ以外は OSC を破棄して通常処理
        let from_osc = core::mem::take(&mut self.osc_pending);
        if from_osc && b != b'\\' {
            self.osc_buf.clear();
        }
        match b {
            b'[' => {
                self.clear_seq();
                self.state = State::CsiEntry;
            }
            b']' => {
                self.osc_buf.clear();
                self.state = State::OscString;
            }
            b'P' | b'X' | b'^' | b'_' => {
                self.state = State::IgnoreUntilSt;
            }
            b'\\' => {
                // ST: OSC 終端として処理（それ以外の文脈では no-op）
                if from_osc {
                    let buf = core::mem::take(&mut self.osc_buf);
                    h.osc(&buf);
                }
                self.state = State::Ground;
            }
            0x20..=0x2F => {
                self.intermediates.clear();
                self.collect(b);
                self.state = State::EscapeIntermediate;
            }
            0x30..=0x7E => {
                h.esc(&[], b);
                self.state = State::Ground;
            }
            0x18 | 0x1A => self.state = State::Ground, // CAN/SUB
            0x1B => {}                                 // ESC ESC → Escape のまま
            _ => {
                h.execute(b);
            }
        }
    }

    /// CSI 系状態での C0 / CAN / SUB / ESC の共通処理
    fn anywhere_control<H: VtHandler>(&mut self, h: &mut H, b: u8) {
        match b {
            0x1B => {
                self.clear_seq();
                self.state = State::Escape;
            }
            0x18 | 0x1A => self.state = State::Ground,
            0x00..=0x1F => h.execute(b), // CSI 中の C0 は即時実行
            _ => {}                      // 0x7F ほかは無視
        }
    }

    fn collect(&mut self, b: u8) {
        if self.intermediates.len() < MAX_INTERMEDIATES {
            self.intermediates.push(b);
        }
    }

    fn params_byte(&mut self, b: u8) {
        match b {
            b'0'..=b'9' => self.params.digit((b - b'0') as u16),
            b';' => self.params.push_new(),
            b':' => self.params.push_sub(),
            _ => {}
        }
    }

    fn csi_dispatch<H: VtHandler>(&mut self, h: &mut H, final_byte: u8) {
        // パラメータが一度も始まっていない場合は空のまま渡す（get の default で処理）
        h.csi(
            &self.params,
            self.private_prefix,
            &self.intermediates,
            final_byte,
        );
        self.state = State::Ground;
    }
}
