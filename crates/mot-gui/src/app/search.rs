//! スクロールバック検索の純ロジック（A7）。行テキストのマッチ抽出とジャンプ先スクロール計算。

use mot_term::{Attrs, Cell};

/// 1行のセル列から、クエリ（小文字化済み）のマッチをセル列範囲 [c0, c1) で返す。
/// 全角の WIDE_TRAILER は読み飛ばし、文字とセル列の対応を保つ。大文字小文字は無視。
pub fn matches_in_line(line: &[Cell], query_lower: &str) -> Vec<(usize, usize)> {
    if query_lower.is_empty() {
        return Vec::new();
    }
    // 文字列と、各文字の開始セル列を作る
    let mut text = String::new();
    let mut char_col: Vec<usize> = Vec::new();
    for (c, cell) in line.iter().enumerate() {
        if cell.attrs.contains(Attrs::WIDE_TRAILER) {
            continue;
        }
        for lc in cell.ch.to_lowercase() {
            text.push(lc);
            char_col.push(c);
        }
    }
    // char_col[i] は text の i 番目の char のセル列。text は char 単位で走査する。
    let chars: Vec<char> = text.chars().collect();
    let q: Vec<char> = query_lower.chars().collect();
    let mut out = Vec::new();
    if q.len() > chars.len() {
        return out;
    }
    let mut i = 0;
    while i + q.len() <= chars.len() {
        if chars[i..i + q.len()] == q[..] {
            let c0 = char_col[i];
            let c1 = char_col[i + q.len() - 1] + 1;
            out.push((c0, c1));
            i += q.len();
        } else {
            i += 1;
        }
    }
    out
}

/// カレントマッチ行 abs_row を表示範囲に収めるための scroll オフセットを返す。
/// scroll=0 が最新（最下部）。scrollback 内の行はその行が上端付近に来るよう遡る。
pub fn scroll_to_row(abs_row: usize, scrollback_len: usize, rows: usize) -> usize {
    // 可視絶対行は [scrollback_len - scroll, scrollback_len - scroll + rows)
    if abs_row >= scrollback_len {
        // 現在画面内 → 最新表示で見える
        0
    } else {
        // 上端に少し余白を持たせて中央寄りに
        let margin = rows / 2;
        let target_top = abs_row.saturating_sub(margin);
        scrollback_len
            .saturating_sub(target_top)
            .min(scrollback_len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mot_term::Terminal;

    fn cells(s: &str, cols: usize) -> Vec<Cell> {
        let mut t = Terminal::new(cols, 2, 0);
        t.feed(s.as_bytes());
        t.screen.view_line(0, 0).clone()
    }

    #[test]
    fn finds_case_insensitive_matches() {
        let l = cells("Error: ERROR error", 40);
        let m = matches_in_line(&l, "error");
        assert_eq!(m.len(), 3);
        assert_eq!(m[0], (0, 5)); // "Error" は列0-4
    }

    #[test]
    fn cjk_columns() {
        // 全角を挟んでもセル列が正しい
        let l = cells("あkeyい", 20);
        let m = matches_in_line(&l, "key");
        // "あ"=列0-1, k=列2 → key は列2-4
        assert_eq!(m, vec![(2, 5)]);
    }

    #[test]
    fn empty_query_no_match() {
        let l = cells("anything", 20);
        assert!(matches_in_line(&l, "").is_empty());
        assert!(matches_in_line(&l, "zzz").is_empty());
    }

    #[test]
    fn scroll_target() {
        // 画面内(abs>=sb) は scroll 0
        assert_eq!(scroll_to_row(100, 100, 24), 0);
        assert_eq!(scroll_to_row(110, 100, 24), 0);
        // スクロールバック内は遡る（中央寄せ margin=12）
        // abs=50, sb=100, rows=24 → target_top=38 → scroll=62
        assert_eq!(scroll_to_row(50, 100, 24), 62);
        // 先頭付近
        assert_eq!(scroll_to_row(0, 100, 24), 100);
    }
}
