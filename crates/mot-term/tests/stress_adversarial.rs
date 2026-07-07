//! 敵対的 VT シーケンス stress。fzf / tmux display-popup 由来の異常入力（巨大/ゼロ/欠落/
//! 負の CSI パラメータ、DCS/APC/OSC 変種、代替画面・マウス・同期更新、極小端末）で
//! パーサが panic しないことを固定する回帰テスト。fzf が実際に吐く `\x1b[-1A` を含む。
//! パニックした場合は直前の eprintln が示すケースが犯人（RUST_BACKTRACE=1 で行特定）。
use mot_term::Terminal;

fn cases() -> Vec<(&'static str, Vec<u8>)> {
    let mut v: Vec<(&'static str, Vec<u8>)> = Vec::new();
    // 巨大/ゼロ/欠落 CSI パラメータ
    for (n, s) in [
        ("CUP huge", &b"\x1b[999999999;999999999H"[..]),
        ("CUP zero", b"\x1b[0;0H"),
        ("SU huge", b"\x1b[999999999S"),
        ("SD huge", b"\x1b[999999999T"),
        ("IL huge", b"\x1b[999999999L"),
        ("DL huge", b"\x1b[999999999M"),
        ("ICH huge", b"\x1b[999999999@"),
        ("DCH huge", b"\x1b[999999999P"),
        ("ECH huge", b"\x1b[999999999X"),
        ("REP huge", b"A\x1b[999999999b"),
        ("CBT huge", b"\x1b[999999999Z"),
        ("CHA huge", b"\x1b[999999999G"),
        ("VPA huge", b"\x1b[999999999d"),
        ("CUF huge", b"\x1b[999999999C"),
        ("CUB huge", b"\x1b[999999999D"),
        ("CUU huge", b"\x1b[999999999A"),
        ("CUD huge", b"\x1b[999999999B"),
        ("DECSTBM huge", b"\x1b[999999999;999999999r"),
        ("DECSTBM inverted", b"\x1b[20;1r"),
        ("TBC huge", b"\x1b[999999999g"),
        (
            "many params",
            b"\x1b[1;2;3;4;5;6;7;8;9;10;11;12;13;14;15;16;17m",
        ),
        ("empty params", b"\x1b[;;;;;H"),
        ("DECSTBM then LF flood", b"\x1b[5;3r\n\n\n\n\n\n\n\n\n\n"),
        ("scroll region + IL", b"\x1b[2;4r\x1b[3;1H\x1b[999L"),
    ] {
        v.push((n, s.to_vec()));
    }
    // 左右マージン / SCOSC 衝突 / DECRQM / DA / DSR / 同期更新 / カーソル形状
    for (n, s) in [
        ("DECLRMM+DECSLRM", &b"\x1b[?69h\x1b[5;40s"[..]),
        ("SCOSC/SCORC", b"\x1b[s\x1b[u"),
        ("DECRQM 2026", b"\x1b[?2026$p"),
        ("SYNC set/reset", b"\x1b[?2026h\x1b[?2026l"),
        ("DECSCUSR", b"\x1b[5 q"),
        ("Primary DA", b"\x1b[c"),
        ("DA2", b"\x1b[>c"),
        ("DSR cursor", b"\x1b[6n"),
        ("XTVERSION", b"\x1b[>q"),
        ("alt screen 1049", b"\x1b[?1049h\x1b[?1049l"),
        ("origin+wrap", b"\x1b[?6h\x1b[?7h"),
        ("mouse+sgr", b"\x1b[?1002h\x1b[?1006h\x1b[?1003h"),
    ] {
        v.push((n, s.to_vec()));
    }
    // 負パラメータ / 非標準パラメータバイト（fzf が実際に吐く \x1b[-1A を含む）
    for (n, s) in [
        ("CUU -1 (fzf 実物)", &b"\x1b[-1A"[..]),
        ("CUP -5;-3", b"\x1b[-5;-3H"),
        ("REP -1", b"A\x1b[-1b"),
        ("SU -1", b"\x1b[-1S"),
        ("DECSTBM -1", b"\x1b[-1;-9r"),
        ("CUD -1", b"\x1b[-1B"),
        ("fzf minimal render", b"\x1b[?1049h\x1b[?7l\x1b[?25l\x1b[?1000h\x1b[?1002h\x1b[?1006h\x1b[?2004h\x1b[-1A\x1b[G\x1b[K\x1b[1B\r\r\x1b[1A\r\r\r\x1b[?25h\x1b[?7h\x1b[?1049l"),
        ("plus param", b"\x1b[+3A"),
        ("dot param", b"\x1b[3.5H"),
    ] {
        v.push((n, s.to_vec()));
    }
    // OSC / DCS / APC / PM / SOS の変種（終端あり/なし・マルチバイト・空）
    for (n, s) in [
        ("OSC0 title empty", &b"\x1b]0;\x07"[..]),
        ("OSC0 multibyte + BEL", "\x1b]0;あいう日本語\x07".as_bytes()),
        ("OSC8 empty", b"\x1b]8;;\x07"),
        (
            "OSC8 uri + ST",
            b"\x1b]8;;http://x/\x1b\\text\x1b]8;;\x1b\\",
        ),
        (
            "OSC7 file url mb",
            "\x1b]7;file://host/home/ユーザ x\x1b\\".as_bytes(),
        ),
        ("OSC52 clipboard", b"\x1b]52;c;aGVsbG8=\x07"),
        (
            "OSC unterminated",
            b"\x1b]0;no-terminator-just-text-and-more",
        ),
        ("OSC percent trunc", b"\x1b]8;;http://x/%2\x07"),
        ("OSC percent mb", "\x1b]8;;http://x/%E3%81%\x07".as_bytes()),
        ("DCS + ST", b"\x1bPq;stuff\x1b\\"),
        ("DCS unterminated", b"\x1bP0;1|garbage no ST"),
        ("APC + ST", b"\x1b_Gfoo=bar\x1b\\"),
        ("PM + ST", b"\x1b^private\x1b\\"),
        ("SOS + ST", b"\x1bXstring\x1b\\"),
        (
            "APC unterminated flood",
            b"\x1b_this never ends and keeps going for a while....",
        ),
    ] {
        v.push((n, s.to_vec()));
    }
    v
}

fn run_on(cols: usize, rows: usize) {
    for (name, bytes) in cases() {
        eprintln!("[stress {cols}x{rows}] case: {name}");
        let mut t = Terminal::new(cols, rows, 100);
        t.feed(&bytes);
        // 追い打ち: 直後に通常テキストと改行で状態破壊を炙る
        t.feed(b"X\r\nY\r\n");
    }
    // 全部連結して一気に食わせる
    eprintln!("[stress {cols}x{rows}] case: ALL-concatenated");
    let mut t = Terminal::new(cols, rows, 100);
    for (_n, b) in cases() {
        t.feed(&b);
    }
}

#[test]
fn adversarial_sequences_do_not_panic() {
    for (c, r) in [(80usize, 24usize), (1, 1), (2, 2), (200, 50), (10, 3)] {
        run_on(c, r);
    }
}
