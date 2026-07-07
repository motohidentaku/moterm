//! 端末スクリーンモデル。パーサからのディスパッチを受けてグリッド状態を更新する。
//! xterm 系の挙動に準拠（プライマリ/代替スクリーン、スクロールリージョン、
//! スクロールバック、CJK 全角2セル、OSC 8/52/7、マウスレポート、ブラケットペースト）。

use crate::cell::{Attrs, Cell, Color, LinkId};
use crate::parser::{Params, VtHandler};
use std::collections::VecDeque;
use unicode_width::UnicodeWidthChar;

pub type Line = Vec<Cell>;

/// GUI へ通知するイベント（フレームごとに take_events で回収）
#[derive(Debug, Clone, PartialEq)]
pub enum TermEvent {
    Title(String),
    /// OSC 52: ローカルクリップボードへ設定すべきテキスト
    Clipboard(String),
    /// OSC 7: リモートの現在ディレクトリ（file://host/path の path 部）
    Cwd(String),
    Bell,
}

/// マウスレポートのモード（DEC private modes 9/1000/1002/1003）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseMode {
    #[default]
    Off,
    X10,    // 9: press のみ
    Normal, // 1000: press/release
    Button, // 1002: press/release + ドラッグ
    Any,    // 1003: 全モーション
}

/// マウスレポートのエンコーディング
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseEncoding {
    #[default]
    X10, // CSI M CbCxCy（+32 バイト値）
    Sgr, // 1006: CSI < b;x;y M/m
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
}

#[derive(Debug, Clone, Copy)]
pub struct CursorState {
    pub row: usize,
    pub col: usize,
    /// 行末到達後の遅延ラップ（xterm の wrap-pending 挙動）
    pending_wrap: bool,
}

#[derive(Debug, Clone, Copy)]
struct SavedCursor {
    row: usize,
    col: usize,
    fg: Color,
    bg: Color,
    attrs: Attrs,
    origin_mode: bool,
    g1_active: bool,
    charsets: [CharSet; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum CharSet {
    #[default]
    Ascii,
    /// DEC Special Graphics（罫線）
    LineDrawing,
}

struct Grid {
    lines: Vec<Line>,
    cols: usize,
    rows: usize,
}

impl Grid {
    fn new(cols: usize, rows: usize) -> Self {
        Grid {
            lines: (0..rows).map(|_| blank_line(cols)).collect(),
            cols,
            rows,
        }
    }
}

fn blank_line(cols: usize) -> Line {
    vec![Cell::default(); cols]
}

pub struct Screen {
    grid: Grid,
    alt_grid: Grid,
    alt_active: bool,
    scrollback: VecDeque<Line>,
    scrollback_limit: usize,

    pub cursor: CursorState,
    saved_primary: Option<SavedCursor>,
    saved_alt: Option<SavedCursor>,

    // ペン（現在の SGR 状態）
    fg: Color,
    bg: Color,
    attrs: Attrs,
    cur_link: LinkId,

    // モード
    pub autowrap: bool,        // DECAWM (?7)
    pub cursor_visible: bool,  // DECTCEM (?25)
    pub app_cursor_keys: bool, // DECCKM (?1)
    pub app_keypad: bool,      // ESC = / ESC >
    pub bracketed_paste: bool, // ?2004
    pub mouse_mode: MouseMode,
    pub mouse_encoding: MouseEncoding,
    insert_mode: bool, // IRM (4)
    origin_mode: bool, // DECOM (?6)

    // スクロールリージョン（0-based、inclusive）
    scroll_top: usize,
    scroll_bottom: usize,

    tabs: Vec<bool>,

    // 文字集合（SO/SI + ESC ( / ESC )）
    charsets: [CharSet; 2],
    g1_active: bool,

    // OSC 8 ハイパーリンク表
    links: Vec<String>,

    pub title: String,
    /// OSC 7 で報告されたリモート cwd（SFTP アップロード先の解決に使う）
    pub remote_cwd: Option<String>,

    events: VecDeque<TermEvent>,
    responses: Vec<u8>,

    // --- OSC 133 シェル統合（コマンド境界） ---
    /// これまでにプライマリグリッド上端から押し出した総行数（プル戻しで減算）。
    /// 「絶対行」= scrolled_off + グリッド行。セッションを通して単調で、スクロールバック
    /// 溢れの影響を受けない番号付けの基準。
    scrolled_off: u64,
    /// OSC 133;A（プロンプト開始）で記録した絶対行の一覧（概ね昇順、上限あり）。
    prompt_marks: VecDeque<u64>,
    /// 直近コマンドの exit code（OSC 133;D;code）。
    last_exit_code: Option<i32>,
    /// 直近コマンドの出力開始絶対行（OSC 133;C）。将来のブロックコピー用に保持。
    last_cmd_start: Option<u64>,

    /// 表示内容の世代カウンタ（再描画判定用）
    pub generation: u64,
}

/// プロンプトマークの保持上限（古いものから捨てる）。
const PROMPT_MARK_CAP: usize = 2000;

impl Screen {
    pub fn new(cols: usize, rows: usize, scrollback_limit: usize) -> Self {
        let cols = cols.max(2);
        let rows = rows.max(1);
        Screen {
            grid: Grid::new(cols, rows),
            alt_grid: Grid::new(cols, rows),
            alt_active: false,
            scrollback: VecDeque::new(),
            scrollback_limit,
            cursor: CursorState {
                row: 0,
                col: 0,
                pending_wrap: false,
            },
            saved_primary: None,
            saved_alt: None,
            fg: Color::Default,
            bg: Color::Default,
            attrs: Attrs::empty(),
            cur_link: 0,
            autowrap: true,
            cursor_visible: true,
            app_cursor_keys: false,
            app_keypad: false,
            bracketed_paste: false,
            mouse_mode: MouseMode::Off,
            mouse_encoding: MouseEncoding::X10,
            insert_mode: false,
            origin_mode: false,
            scroll_top: 0,
            scroll_bottom: rows - 1,
            tabs: default_tabs(cols),
            charsets: [CharSet::Ascii; 2],
            g1_active: false,
            links: Vec::new(),
            title: String::new(),
            remote_cwd: None,
            events: VecDeque::new(),
            responses: Vec::new(),
            scrolled_off: 0,
            prompt_marks: VecDeque::new(),
            last_exit_code: None,
            last_cmd_start: None,
            generation: 0,
        }
    }

    pub fn cols(&self) -> usize {
        self.cur().cols
    }
    pub fn rows(&self) -> usize {
        self.cur().rows
    }
    pub fn scrollback_len(&self) -> usize {
        if self.alt_active {
            0
        } else {
            self.scrollback.len()
        }
    }
    pub fn alt_screen_active(&self) -> bool {
        self.alt_active
    }

    fn cur(&self) -> &Grid {
        if self.alt_active {
            &self.alt_grid
        } else {
            &self.grid
        }
    }
    fn cur_mut(&mut self) -> &mut Grid {
        if self.alt_active {
            &mut self.alt_grid
        } else {
            &mut self.grid
        }
    }

    /// ビューポートの i 行目（offset = スクロールバックを何行遡っているか）
    pub fn view_line(&self, i: usize, offset: usize) -> &Line {
        let offset = offset.min(self.scrollback_len());
        if i < offset {
            let sb_idx = self.scrollback.len() - offset + i;
            &self.scrollback[sb_idx]
        } else {
            &self.cur().lines[i - offset]
        }
    }

    // ------------------------------------------------------------------
    // OSC 133 シェル統合（絶対行の基準・プロンプトジャンプ）
    // ------------------------------------------------------------------
    /// これまでにグリッド上端から押し出した総行数（絶対行の基準）。
    /// グリッド行 r の絶対行 = pushed_total() + r。
    pub fn pushed_total(&self) -> u64 {
        self.scrolled_off
    }
    /// scroll_offset 行だけ遡ったビュー最上段の絶対行。
    pub fn top_visible_abs(&self, scroll_offset: usize) -> u64 {
        self.scrolled_off.saturating_sub(scroll_offset as u64)
    }
    /// OSC 133;A で記録したプロンプト開始の絶対行一覧。
    pub fn prompt_marks_abs(&self) -> &VecDeque<u64> {
        &self.prompt_marks
    }
    /// from_abs より前で最も近いプロンプト絶対行。
    pub fn prev_prompt_abs(&self, from_abs: u64) -> Option<u64> {
        self.prompt_marks
            .iter()
            .copied()
            .filter(|&mk| mk < from_abs)
            .max()
    }
    /// from_abs より後で最も近いプロンプト絶対行。
    pub fn next_prompt_abs(&self, from_abs: u64) -> Option<u64> {
        self.prompt_marks
            .iter()
            .copied()
            .filter(|&mk| mk > from_abs)
            .min()
    }
    /// 直近コマンドの exit code（OSC 133;D）。未受信/不明は None。
    pub fn last_exit_code(&self) -> Option<i32> {
        self.last_exit_code
    }
    /// 直近コマンドの出力開始絶対行（OSC 133;C）。
    pub fn last_cmd_start_abs(&self) -> Option<u64> {
        self.last_cmd_start
    }

    /// OSC 133;A: 現在のカーソル位置をプロンプト開始として記録する。
    fn mark_prompt(&mut self) {
        if self.alt_active {
            return;
        }
        let abs = self.scrolled_off + self.cursor.row as u64;
        if self.prompt_marks.back() == Some(&abs) {
            return; // 同一行の連続記録は無視
        }
        self.prompt_marks.push_back(abs);
        while self.prompt_marks.len() > PROMPT_MARK_CAP {
            self.prompt_marks.pop_front();
        }
    }

    /// OSC 8 リンク ID から URI を引く
    pub fn link_uri(&self, id: LinkId) -> Option<&str> {
        if id == 0 {
            None
        } else {
            self.links.get((id - 1) as usize).map(|s| s.as_str())
        }
    }

    pub fn take_events(&mut self) -> Vec<TermEvent> {
        self.events.drain(..).collect()
    }

    /// 端末からホストへの応答バイト列（DA/DSR 等）。PTY に書き戻すこと。
    pub fn take_responses(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.responses)
    }

    // ------------------------------------------------------------------
    // リサイズ
    // ------------------------------------------------------------------
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(2);
        let rows = rows.max(1);
        if cols == self.cols() && rows == self.rows() {
            return;
        }
        // プライマリ: 縮小時は上端行をスクロールバックへ、拡大時は引き戻す
        let cursor_row = self.cursor.row;
        let old_rows = self.grid.rows;
        if rows < old_rows {
            let excess = old_rows - rows;
            // カーソルより下の空行を先に削る
            let mut removed_from_top = 0;
            for _ in 0..excess {
                let last_used = self
                    .grid
                    .lines
                    .iter()
                    .rposition(line_has_content)
                    .unwrap_or(0);
                let can_trim_bottom =
                    self.grid.lines.len() > last_used + 1 && self.grid.lines.len() - 1 > cursor_row;
                if can_trim_bottom {
                    self.grid.lines.pop();
                } else {
                    let line = self.grid.lines.remove(0);
                    self.scrolled_off += 1;
                    self.push_scrollback(line);
                    removed_from_top += 1;
                }
            }
            self.cursor.row = self.cursor.row.saturating_sub(removed_from_top);
        } else if rows > old_rows {
            let mut needed = rows - old_rows;
            // スクロールバックから引き戻す
            while needed > 0 {
                if let Some(line) = self.scrollback.pop_back() {
                    let mut line = line;
                    line.resize(cols, Cell::default()); // 切り詰め済みの行を現在幅へ復元
                    self.grid.lines.insert(0, line);
                    self.cursor.row += 1;
                    self.scrolled_off = self.scrolled_off.saturating_sub(1);
                    needed -= 1;
                } else {
                    break;
                }
            }
            for _ in 0..needed {
                self.grid.lines.push(blank_line(cols));
            }
        }
        for line in &mut self.grid.lines {
            line.resize(cols, Cell::default());
        }
        self.grid.cols = cols;
        self.grid.rows = rows;

        // 代替スクリーンは単純に切り詰め/パディング
        self.alt_grid.lines.resize(rows, blank_line(cols));
        for line in &mut self.alt_grid.lines {
            line.resize(cols, Cell::default());
        }
        self.alt_grid.cols = cols;
        self.alt_grid.rows = rows;

        self.scroll_top = 0;
        self.scroll_bottom = rows - 1;
        self.cursor.row = self.cursor.row.min(rows - 1);
        self.cursor.col = self.cursor.col.min(cols - 1);
        self.cursor.pending_wrap = false;
        self.tabs = default_tabs(cols);
        self.generation += 1;
    }

    fn push_scrollback(&mut self, mut line: Line) {
        if self.scrollback_limit == 0 {
            return;
        }
        // メモリ節約: 末尾の完全な空セル（Cell::default）を切り詰めて保存する。
        // 描画(termview)・選択・検索はいずれも「行が短ければ範囲外は空」として扱うので安全。
        // resize 拡大時の pull-back では現在幅へ resize して復元する。
        while line.last() == Some(&Cell::default()) {
            line.pop();
        }
        line.shrink_to_fit();
        if self.scrollback.len() >= self.scrollback_limit {
            self.scrollback.pop_front();
        }
        self.scrollback.push_back(line);
    }

    // ------------------------------------------------------------------
    // スクロール
    // ------------------------------------------------------------------
    fn scroll_up(&mut self, n: usize) {
        let (top, bottom) = (self.scroll_top, self.scroll_bottom);
        let n = n.min(bottom - top + 1);
        for _ in 0..n {
            let cols = self.cur().cols;
            let grid = self.cur_mut();
            let line = grid.lines.remove(top);
            grid.lines.insert(bottom, blank_line(cols));
            // プライマリ画面の最上段リージョンからのスクロールアウトのみ履歴へ
            if !self.alt_active && top == 0 {
                self.scrolled_off += 1;
                self.push_scrollback(line);
            }
        }
        self.generation += 1;
    }

    fn scroll_down(&mut self, n: usize) {
        let (top, bottom) = (self.scroll_top, self.scroll_bottom);
        let n = n.min(bottom - top + 1);
        for _ in 0..n {
            let cols = self.cur().cols;
            let grid = self.cur_mut();
            grid.lines.remove(bottom);
            grid.lines.insert(top, blank_line(cols));
        }
        self.generation += 1;
    }

    // ------------------------------------------------------------------
    // カーソル移動
    // ------------------------------------------------------------------
    fn clamp_cursor(&mut self) {
        self.cursor.row = self.cursor.row.min(self.rows() - 1);
        self.cursor.col = self.cursor.col.min(self.cols() - 1);
    }

    fn move_cursor(&mut self, row: i64, col: i64) {
        let (min_row, max_row) = if self.origin_mode {
            (self.scroll_top as i64, self.scroll_bottom as i64)
        } else {
            (0, self.rows() as i64 - 1)
        };
        let base = if self.origin_mode {
            self.scroll_top as i64
        } else {
            0
        };
        self.cursor.row = (base + row).clamp(min_row, max_row) as usize;
        self.cursor.col = col.clamp(0, self.cols() as i64 - 1) as usize;
        self.cursor.pending_wrap = false;
    }

    fn linefeed(&mut self) {
        if self.cursor.row == self.scroll_bottom {
            self.scroll_up(1);
        } else if self.cursor.row + 1 < self.rows() {
            self.cursor.row += 1;
        }
        self.cursor.pending_wrap = false;
    }

    fn reverse_linefeed(&mut self) {
        if self.cursor.row == self.scroll_top {
            self.scroll_down(1);
        } else if self.cursor.row > 0 {
            self.cursor.row -= 1;
        }
        self.cursor.pending_wrap = false;
    }

    // ------------------------------------------------------------------
    // 文字書き込み
    // ------------------------------------------------------------------
    fn put_char(&mut self, ch: char) {
        let ch = self.map_charset(ch);
        let width = ch.width().unwrap_or(0);
        if width == 0 {
            // 結合文字: 直前セルに合成（簡易: 直前セルの文字に付加）
            let r = self.cursor.row;
            let c = self.cursor.col.saturating_sub(1);
            let grid = self.cur_mut();
            if let Some(cell) = grid.lines[r].get_mut(c) {
                let mut s = cell.ch.to_string();
                s.push(ch);
                // 合成結果が1文字にならない場合はベース文字を維持
                if let Some(first) = s.chars().next() {
                    cell.ch = first;
                }
            }
            return;
        }

        let cols = self.cols();
        if self.cursor.pending_wrap && self.autowrap {
            self.cursor.col = 0;
            self.linefeed();
        }
        self.cursor.pending_wrap = false;

        // 全角が行末1セルに掛かる場合は先に折り返す
        if width == 2 && self.cursor.col + 1 >= cols {
            if self.autowrap {
                self.cursor.col = 0;
                self.linefeed();
            } else {
                self.cursor.col = cols.saturating_sub(2);
            }
        }

        if self.insert_mode {
            let (r, c) = (self.cursor.row, self.cursor.col);
            let grid = self.cur_mut();
            for _ in 0..width {
                grid.lines[r].insert(c, Cell::default());
                grid.lines[r].truncate(cols);
            }
        }

        let (r, c) = (self.cursor.row, self.cursor.col);
        let (fg, bg, attrs, link) = (self.fg, self.bg, self.attrs, self.cur_link);
        // 既存の全角文字を上書きする場合はペアを掃除
        self.clean_wide_at(r, c);
        if width == 2 {
            self.clean_wide_at(r, c + 1);
        }
        let grid = self.cur_mut();
        let line = &mut grid.lines[r];
        let mut cell_attrs = attrs;
        if width == 2 {
            cell_attrs.insert(Attrs::WIDE);
        }
        line[c] = Cell {
            ch,
            fg,
            bg,
            attrs: cell_attrs,
            link,
        };
        if width == 2 {
            let mut trailer = Cell {
                ch: ' ',
                fg,
                bg,
                attrs,
                link,
            };
            trailer.attrs.insert(Attrs::WIDE_TRAILER);
            line[c + 1] = trailer;
        }

        let next = c + width;
        if next >= cols {
            self.cursor.col = cols - 1;
            self.cursor.pending_wrap = true;
        } else {
            self.cursor.col = next;
        }
        self.generation += 1;
    }

    /// (r,c) が全角ペアの一部なら、ペア両方を空白化する
    fn clean_wide_at(&mut self, r: usize, c: usize) {
        let grid = self.cur_mut();
        let Some(cell) = grid.lines[r].get(c) else {
            return;
        };
        if cell.is_wide() {
            if let Some(t) = grid.lines[r].get_mut(c + 1) {
                *t = Cell::default();
            }
            grid.lines[r][c] = Cell::default();
        } else if cell.is_wide_trailer() && c > 0 {
            grid.lines[r][c - 1] = Cell::default();
            grid.lines[r][c] = Cell::default();
        }
    }

    fn map_charset(&self, ch: char) -> char {
        let active = self.charsets[if self.g1_active { 1 } else { 0 }];
        if active == CharSet::LineDrawing {
            line_drawing(ch)
        } else {
            ch
        }
    }

    // ------------------------------------------------------------------
    // 消去・編集
    // ------------------------------------------------------------------
    fn erase_line_range(&mut self, r: usize, c0: usize, c1: usize) {
        let bg = self.bg;
        let grid = self.cur_mut();
        let cols = grid.cols;
        for c in c0..c1.min(cols) {
            grid.lines[r][c] = Cell::blank_with_bg(bg);
        }
        self.generation += 1;
    }

    fn erase_display(&mut self, mode: u16) {
        let (r, c) = (self.cursor.row, self.cursor.col);
        let rows = self.rows();
        let cols = self.cols();
        match mode {
            0 => {
                self.erase_line_range(r, c, cols);
                for row in r + 1..rows {
                    self.erase_line_range(row, 0, cols);
                }
            }
            1 => {
                for row in 0..r {
                    self.erase_line_range(row, 0, cols);
                }
                self.erase_line_range(r, 0, c + 1);
            }
            2 => {
                for row in 0..rows {
                    self.erase_line_range(row, 0, cols);
                }
            }
            3 => {
                // xterm: スクロールバックも消去
                self.scrollback.clear();
                for row in 0..rows {
                    self.erase_line_range(row, 0, cols);
                }
            }
            _ => {}
        }
    }

    fn erase_line(&mut self, mode: u16) {
        let (r, c) = (self.cursor.row, self.cursor.col);
        let cols = self.cols();
        match mode {
            0 => self.erase_line_range(r, c, cols),
            1 => self.erase_line_range(r, 0, c + 1),
            2 => self.erase_line_range(r, 0, cols),
            _ => {}
        }
    }

    fn insert_lines(&mut self, n: usize) {
        if self.cursor.row < self.scroll_top || self.cursor.row > self.scroll_bottom {
            return;
        }
        let n = n.min(self.scroll_bottom - self.cursor.row + 1);
        let cols = self.cols();
        let (row, bottom) = (self.cursor.row, self.scroll_bottom);
        let grid = self.cur_mut();
        for _ in 0..n {
            grid.lines.remove(bottom);
            grid.lines.insert(row, blank_line(cols));
        }
        self.cursor.col = 0;
        self.cursor.pending_wrap = false;
        self.generation += 1;
    }

    fn delete_lines(&mut self, n: usize) {
        if self.cursor.row < self.scroll_top || self.cursor.row > self.scroll_bottom {
            return;
        }
        let n = n.min(self.scroll_bottom - self.cursor.row + 1);
        let cols = self.cols();
        let (row, bottom) = (self.cursor.row, self.scroll_bottom);
        let grid = self.cur_mut();
        for _ in 0..n {
            grid.lines.remove(row);
            grid.lines.insert(bottom, blank_line(cols));
        }
        self.cursor.col = 0;
        self.cursor.pending_wrap = false;
        self.generation += 1;
    }

    fn delete_chars(&mut self, n: usize) {
        let (r, c) = (self.cursor.row, self.cursor.col);
        let cols = self.cols();
        let n = n.min(cols - c);
        let bg = self.bg;
        let grid = self.cur_mut();
        let line = &mut grid.lines[r];
        for _ in 0..n {
            line.remove(c);
            line.push(Cell::blank_with_bg(bg));
        }
        self.generation += 1;
    }

    fn insert_chars(&mut self, n: usize) {
        let (r, c) = (self.cursor.row, self.cursor.col);
        let cols = self.cols();
        let n = n.min(cols - c);
        let bg = self.bg;
        let grid = self.cur_mut();
        let line = &mut grid.lines[r];
        for _ in 0..n {
            line.insert(c, Cell::blank_with_bg(bg));
        }
        line.truncate(cols);
        self.generation += 1;
    }

    // ------------------------------------------------------------------
    // SGR
    // ------------------------------------------------------------------
    fn sgr(&mut self, params: &Params) {
        if params.is_empty() {
            self.fg = Color::Default;
            self.bg = Color::Default;
            self.attrs = Attrs::empty();
            return;
        }
        let mut i = 0;
        while i < params.len() {
            let sub = params.subparams(i);
            let p = sub.first().copied().unwrap_or(0);
            match p {
                0 => {
                    self.fg = Color::Default;
                    self.bg = Color::Default;
                    self.attrs = Attrs::empty();
                }
                1 => self.attrs.insert(Attrs::BOLD),
                2 => self.attrs.insert(Attrs::DIM),
                3 => self.attrs.insert(Attrs::ITALIC),
                4 => {
                    // 4:0 は下線解除、4:x は下線バリエーション → 単純化して下線
                    if sub.len() >= 2 && sub[1] == 0 {
                        self.attrs.remove(Attrs::UNDERLINE);
                    } else {
                        self.attrs.insert(Attrs::UNDERLINE);
                    }
                }
                5 | 6 => self.attrs.insert(Attrs::BLINK),
                7 => self.attrs.insert(Attrs::REVERSE),
                8 => self.attrs.insert(Attrs::HIDDEN),
                9 => self.attrs.insert(Attrs::STRIKETHROUGH),
                21 | 22 => {
                    self.attrs.remove(Attrs::BOLD);
                    self.attrs.remove(Attrs::DIM);
                }
                23 => self.attrs.remove(Attrs::ITALIC),
                24 => self.attrs.remove(Attrs::UNDERLINE),
                25 => self.attrs.remove(Attrs::BLINK),
                27 => self.attrs.remove(Attrs::REVERSE),
                28 => self.attrs.remove(Attrs::HIDDEN),
                29 => self.attrs.remove(Attrs::STRIKETHROUGH),
                30..=37 => self.fg = Color::Indexed((p - 30) as u8),
                38 => {
                    let (color, consumed) = parse_extended_color(params, i);
                    if let Some(c) = color {
                        self.fg = c;
                    }
                    i += consumed;
                }
                39 => self.fg = Color::Default,
                40..=47 => self.bg = Color::Indexed((p - 40) as u8),
                48 => {
                    let (color, consumed) = parse_extended_color(params, i);
                    if let Some(c) = color {
                        self.bg = c;
                    }
                    i += consumed;
                }
                49 => self.bg = Color::Default,
                90..=97 => self.fg = Color::Indexed((p - 90 + 8) as u8),
                100..=107 => self.bg = Color::Indexed((p - 100 + 8) as u8),
                _ => {}
            }
            i += 1;
        }
    }

    // ------------------------------------------------------------------
    // モード
    // ------------------------------------------------------------------
    fn set_mode(&mut self, private: bool, params: &Params, on: bool) {
        for i in 0..params.len().max(1) {
            let p = params.get_raw(i, 0);
            if private {
                match p {
                    1 => self.app_cursor_keys = on,
                    6 => {
                        self.origin_mode = on;
                        self.move_cursor(0, 0);
                    }
                    7 => self.autowrap = on,
                    9 => self.mouse_mode = if on { MouseMode::X10 } else { MouseMode::Off },
                    12 => {} // カーソル点滅: 描画側の裁量
                    25 => self.cursor_visible = on,
                    1000 => {
                        self.mouse_mode = if on {
                            MouseMode::Normal
                        } else {
                            MouseMode::Off
                        }
                    }
                    1002 => {
                        self.mouse_mode = if on {
                            MouseMode::Button
                        } else {
                            MouseMode::Off
                        }
                    }
                    1003 => self.mouse_mode = if on { MouseMode::Any } else { MouseMode::Off },
                    1005 => {} // UTF-8 マウス: 未対応（SGR を推奨）
                    1006 => {
                        self.mouse_encoding = if on {
                            MouseEncoding::Sgr
                        } else {
                            MouseEncoding::X10
                        }
                    }
                    47 | 1047 => self.switch_alt(on, false),
                    1048 => {
                        if on {
                            self.save_cursor();
                        } else {
                            self.restore_cursor();
                        }
                    }
                    1049 => self.switch_alt(on, true),
                    2004 => self.bracketed_paste = on,
                    _ => {}
                }
            } else {
                match p {
                    4 => self.insert_mode = on,
                    20 => {} // LNM: 未対応
                    _ => {}
                }
            }
        }
        self.generation += 1;
    }

    fn switch_alt(&mut self, to_alt: bool, save_restore_cursor: bool) {
        if to_alt && !self.alt_active {
            if save_restore_cursor {
                self.save_cursor();
            }
            self.alt_active = true;
            // 代替スクリーンはクリアして開始（1049 挙動）
            let cols = self.cols();
            let rows = self.rows();
            self.alt_grid = Grid::new(cols, rows);
            self.cursor = CursorState {
                row: 0,
                col: 0,
                pending_wrap: false,
            };
            self.scroll_top = 0;
            self.scroll_bottom = rows - 1;
        } else if !to_alt && self.alt_active {
            self.alt_active = false;
            self.scroll_top = 0;
            self.scroll_bottom = self.rows() - 1;
            if save_restore_cursor {
                self.restore_cursor();
            }
            self.clamp_cursor();
        }
        self.generation += 1;
    }

    fn save_cursor(&mut self) {
        let saved = SavedCursor {
            row: self.cursor.row,
            col: self.cursor.col,
            fg: self.fg,
            bg: self.bg,
            attrs: self.attrs,
            origin_mode: self.origin_mode,
            g1_active: self.g1_active,
            charsets: self.charsets,
        };
        if self.alt_active {
            self.saved_alt = Some(saved);
        } else {
            self.saved_primary = Some(saved);
        }
    }

    fn restore_cursor(&mut self) {
        let saved = if self.alt_active {
            self.saved_alt
        } else {
            self.saved_primary
        };
        if let Some(s) = saved {
            self.cursor.row = s.row.min(self.rows() - 1);
            self.cursor.col = s.col.min(self.cols() - 1);
            self.cursor.pending_wrap = false;
            self.fg = s.fg;
            self.bg = s.bg;
            self.attrs = s.attrs;
            self.origin_mode = s.origin_mode;
            self.g1_active = s.g1_active;
            self.charsets = s.charsets;
        } else {
            self.cursor = CursorState {
                row: 0,
                col: 0,
                pending_wrap: false,
            };
        }
    }

    fn full_reset(&mut self) {
        let cols = self.cols();
        let rows = self.rows();
        let limit = self.scrollback_limit;
        *self = Screen::new(cols, rows, limit);
    }

    // ------------------------------------------------------------------
    // OSC
    // ------------------------------------------------------------------
    fn handle_osc(&mut self, data: &[u8]) {
        let s = String::from_utf8_lossy(data);
        let (num, rest) = match s.split_once(';') {
            Some((n, r)) => (n, r),
            None => (s.as_ref(), ""),
        };
        match num {
            "0" | "2" => {
                self.title = rest.to_string();
                self.events.push_back(TermEvent::Title(self.title.clone()));
            }
            "7" => {
                // file://hostname/path
                let path = rest
                    .strip_prefix("file://")
                    .map(|r| match r.find('/') {
                        Some(idx) => &r[idx..],
                        None => "/",
                    })
                    .unwrap_or(rest);
                let decoded = percent_decode(path);
                self.remote_cwd = Some(decoded.clone());
                self.events.push_back(TermEvent::Cwd(decoded));
            }
            "8" => {
                // OSC 8 ; params ; URI
                let uri = rest.split_once(';').map(|(_, u)| u).unwrap_or("");
                if uri.is_empty() {
                    self.cur_link = 0;
                } else {
                    self.links.push(uri.to_string());
                    self.cur_link = self.links.len() as LinkId;
                }
            }
            "52" => {
                // OSC 52 ; c ; base64data
                if let Some((_, b64)) = rest.split_once(';') {
                    if b64 != "?" {
                        use base64::Engine;
                        if let Ok(bytes) =
                            base64::engine::general_purpose::STANDARD.decode(b64.trim())
                        {
                            if let Ok(text) = String::from_utf8(bytes) {
                                self.events.push_back(TermEvent::Clipboard(text));
                            }
                        }
                    }
                }
            }
            "133" => self.handle_osc133(rest),
            _ => {}
        }
    }

    /// OSC 133（FinalTerm/iTerm2 シェル統合）。
    /// `A`=プロンプト開始 / `B`=コマンド入力開始 / `C`=出力開始 / `D[;code]`=コマンド終了。
    fn handle_osc133(&mut self, rest: &str) {
        match rest.as_bytes().first() {
            Some(b'A') => self.mark_prompt(),
            Some(b'C') => {
                if !self.alt_active {
                    self.last_cmd_start = Some(self.scrolled_off + self.cursor.row as u64);
                }
            }
            Some(b'D') => {
                // `;D` は code 無し（不明）、`;D;<n>` は exit code。
                self.last_exit_code = rest.split(';').nth(1).and_then(|s| s.trim().parse().ok());
            }
            _ => {} // B およびその他は現状無視
        }
    }

    // ------------------------------------------------------------------
    // マウスレポートのエンコード（GUI から呼ぶ）
    // ------------------------------------------------------------------
    /// セル座標 (col,row) 0-based。返り値 None はレポート不要。
    #[allow(clippy::too_many_arguments)]
    pub fn encode_mouse(
        &self,
        button: MouseButton,
        pressed: bool,
        is_motion: bool,
        col: usize,
        row: usize,
        shift: bool,
        ctrl: bool,
    ) -> Option<Vec<u8>> {
        match self.mouse_mode {
            MouseMode::Off => return None,
            MouseMode::X10 => {
                if !pressed || is_motion {
                    return None;
                }
            }
            MouseMode::Normal => {
                if is_motion {
                    return None;
                }
            }
            MouseMode::Button => {
                if is_motion && !pressed {
                    return None; // ボタンを押していないモーションは対象外
                }
            }
            MouseMode::Any => {}
        }
        let mut cb: u8 = match button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            MouseButton::WheelUp => 64,
            MouseButton::WheelDown => 65,
        };
        if is_motion {
            cb += 32;
        }
        if shift {
            cb += 4;
        }
        if ctrl {
            cb += 16;
        }
        match self.mouse_encoding {
            MouseEncoding::Sgr => {
                let m = if pressed { 'M' } else { 'm' };
                Some(format!("\x1b[<{};{};{}{}", cb, col + 1, row + 1, m).into_bytes())
            }
            MouseEncoding::X10 => {
                let cb = if !pressed
                    && !matches!(button, MouseButton::WheelUp | MouseButton::WheelDown)
                {
                    3 + (cb & !0b11) // release は 3
                } else {
                    cb
                };
                let cx = (col + 1).min(223) as u8 + 32;
                let cy = (row + 1).min(223) as u8 + 32;
                Some(vec![0x1b, b'[', b'M', cb + 32, cx, cy])
            }
        }
    }
}

// ----------------------------------------------------------------------
// VtHandler 実装（パーサからのディスパッチ）
// ----------------------------------------------------------------------
impl VtHandler for Screen {
    fn print(&mut self, ch: char) {
        self.put_char(ch);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x07 => self.events.push_back(TermEvent::Bell),
            0x08 => {
                // BS
                if self.cursor.col > 0 {
                    self.cursor.col -= 1;
                }
                self.cursor.pending_wrap = false;
            }
            0x09 => {
                // HT: 次のタブストップへ
                let cols = self.cols();
                let mut c = self.cursor.col + 1;
                while c < cols - 1 && !self.tabs.get(c).copied().unwrap_or(false) {
                    c += 1;
                }
                self.cursor.col = c.min(cols - 1);
            }
            0x0A..=0x0C => self.linefeed(),
            0x0D => {
                self.cursor.col = 0;
                self.cursor.pending_wrap = false;
            }
            0x0E => self.g1_active = true,  // SO
            0x0F => self.g1_active = false, // SI
            _ => {}
        }
        self.generation += 1;
    }

    fn csi(
        &mut self,
        params: &Params,
        private_prefix: Option<u8>,
        intermediates: &[u8],
        final_byte: u8,
    ) {
        let private = private_prefix == Some(b'?');
        if !intermediates.is_empty() {
            // 対応する intermediate 付き CSI は現状なし（DECSCUSR " q" 等は無視）
            return;
        }
        match final_byte {
            b'A' => {
                let n = params.get(0, 1) as usize;
                self.cursor.row =
                    self.cursor
                        .row
                        .saturating_sub(n)
                        .max(if self.cursor.row >= self.scroll_top {
                            self.scroll_top
                        } else {
                            0
                        });
                self.cursor.pending_wrap = false;
            }
            b'B' => {
                let n = params.get(0, 1) as usize;
                let limit = if self.cursor.row <= self.scroll_bottom {
                    self.scroll_bottom
                } else {
                    self.rows() - 1
                };
                self.cursor.row = (self.cursor.row + n).min(limit);
                self.cursor.pending_wrap = false;
            }
            b'C' => {
                let n = params.get(0, 1) as usize;
                self.cursor.col = (self.cursor.col + n).min(self.cols() - 1);
                self.cursor.pending_wrap = false;
            }
            b'D' => {
                let n = params.get(0, 1) as usize;
                self.cursor.col = self.cursor.col.saturating_sub(n);
                self.cursor.pending_wrap = false;
            }
            b'E' => {
                let n = params.get(0, 1) as usize;
                let limit = if self.cursor.row <= self.scroll_bottom {
                    self.scroll_bottom
                } else {
                    self.rows() - 1
                };
                self.cursor.row = (self.cursor.row + n).min(limit);
                self.cursor.col = 0;
                self.cursor.pending_wrap = false;
            }
            b'F' => {
                let n = params.get(0, 1) as usize;
                self.cursor.row = self.cursor.row.saturating_sub(n);
                self.cursor.col = 0;
                self.cursor.pending_wrap = false;
            }
            b'G' | b'`' => {
                let n = params.get(0, 1) as usize;
                self.cursor.col = (n - 1).min(self.cols() - 1);
                self.cursor.pending_wrap = false;
            }
            b'H' | b'f' => {
                let row = params.get(0, 1) as i64 - 1;
                let col = params.get(1, 1) as i64 - 1;
                self.move_cursor(row, col);
            }
            b'I' => {
                // CHT
                for _ in 0..params.get(0, 1) {
                    self.execute(0x09);
                }
            }
            b'J' => self.erase_display(params.get_raw(0, 0)),
            b'K' => self.erase_line(params.get_raw(0, 0)),
            b'L' => self.insert_lines(params.get(0, 1) as usize),
            b'M' => self.delete_lines(params.get(0, 1) as usize),
            b'P' => self.delete_chars(params.get(0, 1) as usize),
            b'S' => self.scroll_up(params.get(0, 1) as usize),
            b'T' => self.scroll_down(params.get(0, 1) as usize),
            b'X' => {
                let n = params.get(0, 1) as usize;
                let (r, c) = (self.cursor.row, self.cursor.col);
                let end = (c + n).min(self.cols());
                self.erase_line_range(r, c, end);
            }
            b'Z' => {
                // CBT: 前のタブストップへ
                for _ in 0..params.get(0, 1) {
                    let mut c = self.cursor.col;
                    while c > 0 {
                        c -= 1;
                        if self.tabs.get(c).copied().unwrap_or(false) {
                            break;
                        }
                    }
                    self.cursor.col = c;
                }
            }
            b'@' => self.insert_chars(params.get(0, 1) as usize),
            b'b' => {
                // REP: 直前の文字を繰り返す
                let (r, c) = (self.cursor.row, self.cursor.col);
                let prev = if c > 0 {
                    self.cur().lines[r][c - 1].ch
                } else {
                    ' '
                };
                for _ in 0..params.get(0, 1) {
                    self.put_char(prev);
                }
            }
            b'c' => {
                // DA: VT220 相当を名乗る
                if private_prefix.is_none() || private_prefix == Some(b'>') {
                    if private_prefix == Some(b'>') {
                        self.responses.extend_from_slice(b"\x1b[>1;10;0c");
                    } else {
                        self.responses.extend_from_slice(b"\x1b[?62;22c");
                    }
                }
            }
            b'd' => {
                // VPA
                let n = params.get(0, 1) as i64 - 1;
                let col = self.cursor.col as i64;
                self.move_cursor(n, col);
            }
            b'g' => {
                // TBC
                match params.get_raw(0, 0) {
                    0 => {
                        let c = self.cursor.col;
                        if let Some(t) = self.tabs.get_mut(c) {
                            *t = false;
                        }
                    }
                    3 => self.tabs.iter_mut().for_each(|t| *t = false),
                    _ => {}
                }
            }
            b'h' => self.set_mode(private, params, true),
            b'l' => self.set_mode(private, params, false),
            b'm' => {
                if private_prefix.is_none() {
                    self.sgr(params);
                }
            }
            b'n' => match params.get_raw(0, 0) {
                5 => self.responses.extend_from_slice(b"\x1b[0n"),
                6 => {
                    let r =
                        self.cursor.row + 1 - if self.origin_mode { self.scroll_top } else { 0 };
                    let c = self.cursor.col + 1;
                    self.responses
                        .extend_from_slice(format!("\x1b[{};{}R", r, c).as_bytes());
                }
                _ => {}
            },
            b'r' => {
                // DECSTBM
                let top = params.get(0, 1) as usize - 1;
                let bottom = params.get(1, self.rows() as u16) as usize - 1;
                if top < bottom && bottom < self.rows() {
                    self.scroll_top = top;
                    self.scroll_bottom = bottom;
                    self.move_cursor(0, 0);
                }
            }
            b's' => self.save_cursor(),
            b'u' => self.restore_cursor(),
            b't' => {} // window ops: 無視（安全側）
            _ => {
                log::trace!(
                    "unhandled CSI {:?} {:?} {}",
                    private_prefix,
                    params,
                    final_byte as char
                );
            }
        }
        self.generation += 1;
    }

    fn esc(&mut self, intermediates: &[u8], final_byte: u8) {
        match (intermediates.first(), final_byte) {
            (None, b'7') => self.save_cursor(),
            (None, b'8') => self.restore_cursor(),
            (None, b'D') => self.linefeed(),
            (None, b'E') => {
                self.linefeed();
                self.cursor.col = 0;
            }
            (None, b'H') => {
                let c = self.cursor.col;
                if let Some(t) = self.tabs.get_mut(c) {
                    *t = true;
                }
            }
            (None, b'M') => self.reverse_linefeed(),
            (None, b'c') => self.full_reset(),
            (None, b'=') => self.app_keypad = true,
            (None, b'>') => self.app_keypad = false,
            (Some(b'('), f) => self.charsets[0] = charset_for(f),
            (Some(b')'), f) => self.charsets[1] = charset_for(f),
            (Some(b'#'), b'8') => {
                // DECALN: 画面を E で埋める（vttest 用）
                let rows = self.rows();
                let cols = self.cols();
                let grid = self.cur_mut();
                for r in 0..rows {
                    for c in 0..cols {
                        grid.lines[r][c] = Cell {
                            ch: 'E',
                            ..Default::default()
                        };
                    }
                }
            }
            _ => {}
        }
        self.generation += 1;
    }

    fn osc(&mut self, data: &[u8]) {
        self.handle_osc(data);
        self.generation += 1;
    }
}

// ----------------------------------------------------------------------
// ヘルパ
// ----------------------------------------------------------------------
fn default_tabs(cols: usize) -> Vec<bool> {
    (0..cols).map(|c| c % 8 == 0 && c != 0).collect()
}

fn line_has_content(line: &Line) -> bool {
    line.iter().any(|c| c.ch != ' ' || c.bg != Color::Default)
}

fn charset_for(f: u8) -> CharSet {
    match f {
        b'0' => CharSet::LineDrawing,
        _ => CharSet::Ascii,
    }
}

/// DEC Special Graphics → Unicode 罫線
fn line_drawing(ch: char) -> char {
    match ch {
        'j' => '┘',
        'k' => '┐',
        'l' => '┌',
        'm' => '└',
        'n' => '┼',
        'q' => '─',
        't' => '├',
        'u' => '┤',
        'v' => '┴',
        'w' => '┬',
        'x' => '│',
        'a' => '▒',
        '`' => '◆',
        'f' => '°',
        'g' => '±',
        'y' => '≤',
        'z' => '≥',
        '{' => 'π',
        '|' => '≠',
        '}' => '£',
        '~' => '·',
        _ => ch,
    }
}

/// SGR 38/48 の拡張色を解釈。(色, 追加で消費した「;区切りパラメータ」数)
fn parse_extended_color(params: &Params, i: usize) -> (Option<Color>, usize) {
    let sub = params.subparams(i);
    if sub.len() >= 2 {
        // コロン形式: 38:5:n / 38:2:[cs:]r:g:b
        match sub[1] {
            5 if sub.len() >= 3 => return (Some(Color::Indexed(sub[2] as u8)), 0),
            2 if sub.len() >= 5 => {
                // 38:2:r:g:b または 38:2:cs:r:g:b（色空間 ID 付き）
                let (r, g, b) = if sub.len() >= 6 {
                    (sub[3], sub[4], sub[5])
                } else {
                    (sub[2], sub[3], sub[4])
                };
                return (Some(Color::Rgb(r as u8, g as u8, b as u8)), 0);
            }
            _ => return (None, 0),
        }
    }
    // セミコロン形式: 38;5;n / 38;2;r;g;b
    match params.get_raw(i + 1, 0) {
        5 => (Some(Color::Indexed(params.get_raw(i + 2, 0) as u8)), 2),
        2 => (
            Some(Color::Rgb(
                params.get_raw(i + 2, 0) as u8,
                params.get_raw(i + 3, 0) as u8,
                params.get_raw(i + 4, 0) as u8,
            )),
            4,
        ),
        _ => (None, 0),
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // `%XX`（XX は ASCII 16進2桁）だけをデコードする。ここで str スライス
        // `&s[i+1..i+3]` を使うと、`%` の直後がマルチバイト文字だと char 境界外スライスで
        // panic する（例: OSC 7 の cwd 通知 `file://h/%<多バイト>`）。バイトから直接読む。
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
