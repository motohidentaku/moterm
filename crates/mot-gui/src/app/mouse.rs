//! マウス関連の純ロジック: 端末セル行からの URL 抽出（A6）、ブラウザ起動、
//! winit ボタン → mot_term ボタンの対応。

use mot_term::{Attrs, Cell, MouseButton as TMouse};
use winit::event::MouseButton;

/// winit のマウスボタンを mot_term のボタンへ対応づける（ホイールは on_scroll 側で扱う）。
pub fn map_button(b: MouseButton) -> Option<TMouse> {
    match b {
        MouseButton::Left => Some(TMouse::Left),
        MouseButton::Middle => Some(TMouse::Middle),
        MouseButton::Right => Some(TMouse::Right),
        _ => None,
    }
}

/// 端末1行のセル列と、クリックされたセル列から URL を抽出する（OSC 8 が無い場合の http(s) 自動検出）。
/// 全角（WIDE）の後続 WIDE_TRAILER セルは読み飛ばし、文字とその開始セル列を対応づけて判定する。
pub fn url_at_cells(line: &[Cell], click_col: usize) -> Option<String> {
    // 文字列を組み立てつつ、各バイト位置がどのセル列由来かを記録する。
    let mut text = String::new();
    let mut byte_col: Vec<usize> = Vec::new();
    for (c, cell) in line.iter().enumerate() {
        if cell.attrs.contains(Attrs::WIDE_TRAILER) {
            continue;
        }
        let before = text.len();
        text.push(cell.ch);
        for _ in before..text.len() {
            byte_col.push(c);
        }
    }
    for (b0, b1) in find_urls(&text) {
        // このURLが占めるセル列範囲 [col0, col1]
        let col0 = byte_col[b0];
        let col1 = byte_col[b1 - 1];
        if click_col >= col0 && click_col <= col1 {
            return Some(text[b0..b1].to_string());
        }
    }
    None
}

/// 文字列中の http(s):// トークンのバイト範囲を返す。末尾の句読点/閉じ括弧は除く。
fn find_urls(s: &str) -> Vec<(usize, usize)> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let rest = &s[i..];
        if rest.starts_with("http://") || rest.starts_with("https://") {
            // 空白（半角/全角）まで伸ばす
            let mut end = i;
            for (off, ch) in rest.char_indices() {
                if ch.is_whitespace() {
                    break;
                }
                end = i + off + ch.len_utf8();
            }
            // 末尾の句読点・閉じ括弧を削る
            let mut e = end;
            while e > i {
                let last = s[i..e].chars().next_back().unwrap();
                if TRAILING_TRIM.contains(last) {
                    e -= last.len_utf8();
                } else {
                    break;
                }
            }
            if e > i {
                out.push((i, e));
            }
            i = end.max(i + 1);
        } else {
            i += next_char_len(bytes, i);
        }
    }
    out
}

const TRAILING_TRIM: &str = ".,;:!?)]}>\"'）」』。、";

fn next_char_len(bytes: &[u8], i: usize) -> usize {
    match bytes[i] {
        b if b < 0x80 => 1,
        b if b >= 0xF0 => 4,
        b if b >= 0xE0 => 3,
        b if b >= 0xC0 => 2,
        _ => 1,
    }
}

/// URL を OS 既定ブラウザで開く。失敗は log のみ（UI は止めない）。
pub fn open_url(url: &str) {
    use std::process::Command;
    // http(s) のみ許可（任意コマンド実行を防ぐ）
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return;
    }
    // cmd はコンソールを持たない GUI プロセスから起動すると自前で1枚開く。
    // CREATE_NO_WINDOW で黒い窓が一瞬光るのを抑える（ブラウザ自体は別プロセス）。
    #[cfg(target_os = "windows")]
    let res = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        Command::new("cmd")
            .args(["/C", "start", "", url])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
    };
    #[cfg(target_os = "macos")]
    let res = Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let res = Command::new("xdg-open").arg(url).spawn();
    if let Err(e) = res {
        log::warn!("URL を開けませんでした: {e}");
    }
}

/// ファイルをテキストエディタで開く（設定アイコン / NEW CONNECTION 用）。
/// 優先順: Sublime Text → Notepad（Windows）/ OS 既定（macOS=`open -t`, Linux=xdg-open）。
pub fn open_in_editor(path: &std::path::Path) {
    use std::process::Command;
    // 1) Sublime Text（PATH 上の subl/sublime_text と Windows の既定インストール先）。
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut subl: Vec<&str> = vec!["subl", "sublime_text"];
    #[cfg(windows)]
    subl.extend([
        r"C:\Program Files\Sublime Text\subl.exe",
        r"C:\Program Files\Sublime Text\sublime_text.exe",
        r"C:\Program Files\Sublime Text 3\sublime_text.exe",
    ]);
    for c in &subl {
        if Command::new(c).arg(path).spawn().is_ok() {
            return;
        }
    }
    // 2) フォールバック: Notepad / OS 既定。
    #[cfg(windows)]
    let fb = Command::new("notepad.exe").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let fb = Command::new("open").arg("-t").arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let fb = Command::new("xdg-open").arg(path).spawn();
    if let Err(e) = fb {
        log::warn!("設定ファイルを開けませんでした: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mot_term::Terminal;

    fn line_cells(s: &str, cols: usize) -> Vec<Cell> {
        let mut t = Terminal::new(cols, 2, 0);
        t.feed(s.as_bytes());
        t.screen.view_line(0, 0).clone()
    }

    #[test]
    fn detects_url_under_cursor() {
        let cells = line_cells("see https://example.com/page for info", 60);
        // "https" は列4から始まる
        assert_eq!(
            url_at_cells(&cells, 10).as_deref(),
            Some("https://example.com/page")
        );
        // URL 外（"see" の上）は None
        assert_eq!(url_at_cells(&cells, 1), None);
    }

    #[test]
    fn trims_trailing_punctuation() {
        let cells = line_cells("(https://example.com).", 40);
        let u = url_at_cells(&cells, 5).unwrap();
        assert_eq!(u, "https://example.com");
    }

    #[test]
    fn cjk_offset_mapping() {
        // 全角2セルを挟んでも列→URL 対応が崩れないこと
        let cells = line_cells("あ https://ex.com x", 40);
        // "あ"=列0-1, 空白=2, URL は列3から
        let u = url_at_cells(&cells, 6).unwrap();
        assert_eq!(u, "https://ex.com");
        assert_eq!(url_at_cells(&cells, 0), None); // 全角文字の上
    }

    #[test]
    fn no_url_returns_none() {
        let cells = line_cells("just plain text here", 40);
        assert_eq!(url_at_cells(&cells, 5), None);
    }

    #[test]
    fn button_mapping() {
        assert_eq!(map_button(MouseButton::Left), Some(TMouse::Left));
        assert_eq!(map_button(MouseButton::Right), Some(TMouse::Right));
        assert_eq!(map_button(MouseButton::Forward), None);
    }
}
