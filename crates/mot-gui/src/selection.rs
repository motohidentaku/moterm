//! テキスト選択の純ロジック。ドラッグ範囲/単語/行/矩形選択からテキストを抽出する。
//! 全角(WIDE)境界を尊重し、WIDE_TRAILER は無視する。

use mot_term::{Attrs, Screen};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelKind {
    /// 通常（行送りで連結）
    Linear,
    /// 矩形（Alt+ドラッグ）
    Block,
}

/// セル座標での選択（オフセット付きビュー座標系ではなく、絶対行 = scrollback+画面 の行インデックス）。
#[derive(Debug, Clone, Copy)]
pub struct Selection {
    pub kind: SelKind,
    pub anchor: (usize, usize), // (row, col)
    pub head: (usize, usize),
}

impl Selection {
    pub fn new(kind: SelKind, at: (usize, usize)) -> Self {
        Selection {
            kind,
            anchor: at,
            head: at,
        }
    }

    /// 正規化した (start, end)（row 昇順、同 row は col 昇順）。
    pub fn normalized(&self) -> ((usize, usize), (usize, usize)) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// あるセル (row,col) が選択に含まれるか。
    pub fn contains(&self, row: usize, col: usize) -> bool {
        let (s, e) = self.normalized();
        match self.kind {
            SelKind::Block => {
                let (c0, c1) = (s.1.min(e.1), s.1.max(e.1));
                row >= s.0 && row <= e.0 && col >= c0 && col <= c1
            }
            SelKind::Linear => {
                if row < s.0 || row > e.0 {
                    return false;
                }
                if s.0 == e.0 {
                    col >= s.1 && col <= e.1
                } else if row == s.0 {
                    col >= s.1
                } else if row == e.0 {
                    col <= e.1
                } else {
                    true
                }
            }
        }
    }
}

/// 選択範囲のテキストを抽出する。abs_line(row) は絶対行 → 表示行を返すクロージャで
/// スクリーンから行を取得する。ここでは Screen から絶対行を引くヘルパを使う。
pub fn extract_text(screen: &Screen, sel: &Selection) -> String {
    let (s, e) = sel.normalized();
    let mut out = String::new();
    for row in s.0..=e.0 {
        let (c0, c1) = match sel.kind {
            SelKind::Block => (s.1.min(e.1), s.1.max(e.1)),
            SelKind::Linear => {
                let start = if row == s.0 { s.1 } else { 0 };
                let end = if row == e.0 { e.1 } else { usize::MAX };
                (start, end)
            }
        };
        if let Some(line) = abs_line(screen, row) {
            let mut line_str = String::new();
            for (col, cell) in line.iter().enumerate() {
                if col < c0 || col > c1 {
                    continue;
                }
                if cell.attrs.contains(Attrs::WIDE_TRAILER) {
                    continue;
                }
                line_str.push(cell.ch);
            }
            // 行末の空白は削る（linear のみ）
            if sel.kind == SelKind::Linear {
                out.push_str(line_str.trim_end());
            } else {
                out.push_str(&line_str);
            }
        }
        if row != e.0 {
            out.push('\n');
        }
    }
    out
}

/// 絶対行インデックス（0 = 最古のスクロールバック行）→ その行のセル列。
pub fn abs_line(screen: &Screen, abs_row: usize) -> Option<&Vec<mot_term::Cell>> {
    let sb = screen.scrollback_len();
    let total = sb + screen.rows();
    if abs_row >= total {
        return None;
    }
    // view_line(i, offset=sb) は i を 0..(sb+rows) の絶対行として扱える
    Some(screen.view_line(abs_row, sb))
}

/// ダブルクリック: 単語境界を求める（英数と一部記号を語とみなす）。
pub fn word_at(screen: &Screen, row: usize, col: usize) -> Option<(usize, usize)> {
    let line = abs_line(screen, row)?;
    if col >= line.len() {
        return None;
    }
    let is_word = |c: char| c.is_alphanumeric() || "_-./:~".contains(c);
    if !is_word(line[col].ch) {
        return Some((col, col));
    }
    let mut start = col;
    while start > 0 && is_word(line[start - 1].ch) {
        start -= 1;
    }
    let mut end = col;
    while end + 1 < line.len() && is_word(line[end + 1].ch) {
        end += 1;
    }
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mot_term::Terminal;

    #[test]
    fn linear_selection_single_line() {
        let mut t = Terminal::new(20, 3, 10);
        t.feed(b"hello world");
        let mut sel = Selection::new(SelKind::Linear, (0, 0));
        sel.head = (0, 4);
        assert_eq!(extract_text(&t.screen, &sel), "hello");
    }

    #[test]
    fn linear_selection_multiline() {
        let mut t = Terminal::new(10, 3, 10);
        t.feed(b"abc\r\ndef");
        let mut sel = Selection::new(SelKind::Linear, (0, 1));
        sel.head = (1, 1);
        assert_eq!(extract_text(&t.screen, &sel), "bc\nde");
    }

    #[test]
    fn block_selection() {
        let mut t = Terminal::new(10, 3, 10);
        t.feed(b"abcd\r\nefgh\r\nijkl");
        let mut sel = Selection::new(SelKind::Block, (0, 1));
        sel.head = (2, 2);
        assert_eq!(extract_text(&t.screen, &sel), "bc\nfg\njk");
    }

    #[test]
    fn cjk_wide_not_duplicated() {
        let mut t = Terminal::new(10, 2, 10);
        t.feed("あい".as_bytes());
        let mut sel = Selection::new(SelKind::Linear, (0, 0));
        sel.head = (0, 3); // 2 全角 = 4 セル
        assert_eq!(extract_text(&t.screen, &sel), "あい");
    }

    #[test]
    fn word_boundary() {
        let mut t = Terminal::new(30, 2, 0);
        t.feed(b"foo bar-baz qux");
        assert_eq!(word_at(&t.screen, 0, 1), Some((0, 2))); // foo
        assert_eq!(word_at(&t.screen, 0, 5), Some((4, 10))); // bar-baz
    }

    #[test]
    fn selection_contains() {
        let mut sel = Selection::new(SelKind::Linear, (0, 2));
        sel.head = (1, 3);
        assert!(sel.contains(0, 5));
        assert!(!sel.contains(0, 1));
        assert!(sel.contains(1, 0));
        assert!(!sel.contains(1, 4));
    }
}
