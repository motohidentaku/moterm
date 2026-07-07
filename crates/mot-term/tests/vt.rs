//! mot-term の仕様固定テスト。
//! moterm README の端末仕様（色/属性、CJK 全角2セル、OSC 8/52、ブラケットペースト、
//! マウスレポート、スクロールバック、動的リサイズ）を代表ケースで検証する。

use mot_term::screen::{MouseButton, TermEvent};
use mot_term::{encode_key, encode_paste, Attrs, Color, Key, Mods, Terminal};

fn text_at(t: &Terminal, row: usize) -> String {
    let line = t.screen.view_line(row, 0);
    line.iter()
        .filter(|c| !c.is_wide_trailer())
        .map(|c| c.ch)
        .collect::<String>()
        .trim_end()
        .to_string()
}

#[test]
fn plain_text_and_newline() {
    let mut t = Terminal::new(20, 5, 100);
    t.feed(b"hello\r\nworld");
    assert_eq!(text_at(&t, 0), "hello");
    assert_eq!(text_at(&t, 1), "world");
    assert_eq!(t.screen.cursor.row, 1);
    assert_eq!(t.screen.cursor.col, 5);
}

#[test]
fn sgr_colors_and_attrs() {
    let mut t = Terminal::new(40, 5, 0);
    t.feed(b"\x1b[1;31mR\x1b[0m\x1b[38;5;196mX\x1b[m\x1b[38;2;1;2;3mT\x1b[4mU");
    let line = t.screen.view_line(0, 0);
    assert_eq!(line[0].fg, Color::Indexed(1));
    assert!(line[0].attrs.contains(Attrs::BOLD));
    assert_eq!(line[1].fg, Color::Indexed(196));
    assert_eq!(line[2].fg, Color::Rgb(1, 2, 3));
    assert!(line[3].attrs.contains(Attrs::UNDERLINE));
    // コロン形式 (38:2::r:g:b 相当の 38:2:r:g:b)
    let mut t2 = Terminal::new(10, 2, 0);
    t2.feed(b"\x1b[38:5:100mA");
    assert_eq!(t2.screen.view_line(0, 0)[0].fg, Color::Indexed(100));
}

#[test]
fn cjk_double_width() {
    let mut t = Terminal::new(10, 3, 0);
    t.feed("あiう".as_bytes());
    let line = t.screen.view_line(0, 0);
    assert_eq!(line[0].ch, 'あ');
    assert!(line[0].is_wide());
    assert!(line[1].is_wide_trailer());
    assert_eq!(line[2].ch, 'i');
    assert_eq!(line[3].ch, 'う');
    assert!(line[4].is_wide_trailer());
    assert_eq!(t.screen.cursor.col, 5);
    // 全角が行末1セルにかかるときは折り返す
    let mut t2 = Terminal::new(5, 3, 0);
    t2.feed("abcdあ".as_bytes());
    assert_eq!(text_at(&t2, 0), "abcd");
    assert_eq!(text_at(&t2, 1), "あ");
    // 全角の半分を上書きするとペアが消える
    let mut t3 = Terminal::new(10, 3, 0);
    t3.feed("あ\x1b[1;1Hx".as_bytes());
    let line = t3.screen.view_line(0, 0);
    assert_eq!(line[0].ch, 'x');
    assert_eq!(line[1].ch, ' ');
    assert!(!line[1].is_wide_trailer());
}

#[test]
fn cursor_movement_and_erase() {
    let mut t = Terminal::new(20, 5, 0);
    t.feed(b"0123456789\x1b[1;5Hx");
    assert_eq!(text_at(&t, 0), "0123x56789");
    // ED 0: カーソル以降を消去
    t.feed(b"\x1b[1;6H\x1b[J");
    assert_eq!(text_at(&t, 0), "0123x");
    // EL 2: 行全体
    t.feed(b"\x1b[2K");
    assert_eq!(text_at(&t, 0), "");
}

#[test]
fn scroll_region_and_scrollback() {
    let mut t = Terminal::new(10, 3, 100);
    t.feed(b"a\r\nb\r\nc\r\nd\r\ne");
    // 3行画面に5行 → 2行がスクロールバックへ
    assert_eq!(t.screen.scrollback_len(), 2);
    assert_eq!(text_at(&t, 0), "c");
    assert_eq!(text_at(&t, 2), "e");
    // オフセット付きビュー
    let sb0: String = t.screen.view_line(0, 2).iter().map(|c| c.ch).collect();
    assert_eq!(sb0.trim_end(), "a");

    // DECSTBM: リージョン内スクロールは履歴に入らない
    let mut t2 = Terminal::new(10, 4, 100);
    t2.feed(b"\x1b[2;3rX\r\n");
    t2.feed(b"1\r\n2\r\n3\r\n4\r\n5");
    assert_eq!(t2.screen.scrollback_len(), 0);
}

#[test]
fn alt_screen() {
    let mut t = Terminal::new(10, 3, 100);
    t.feed(b"main1\r\nmain2");
    t.feed(b"\x1b[?1049h");
    assert!(t.screen.alt_screen_active());
    t.feed(b"ALT");
    assert_eq!(text_at(&t, 0), "ALT");
    assert_eq!(t.screen.scrollback_len(), 0); // 代替中はスクロールバックなし扱い
    t.feed(b"\x1b[?1049l");
    assert!(!t.screen.alt_screen_active());
    assert_eq!(text_at(&t, 0), "main1");
    assert_eq!(text_at(&t, 1), "main2");
    assert_eq!(t.screen.cursor.col, 5); // カーソル復元
}

#[test]
fn osc_title_cwd_hyperlink_osc52() {
    let mut t = Terminal::new(40, 5, 0);
    t.feed(b"\x1b]0;my-title\x07");
    assert_eq!(t.screen.title, "my-title");
    t.feed(b"\x1b]7;file://host/home/user%20x\x1b\\");
    assert_eq!(t.screen.remote_cwd.as_deref(), Some("/home/user x"));
    // 回帰: `%` の直後がマルチバイト文字だと percent_decode が char 境界外スライスで
    // panic していた（OSC 7 の cwd 通知経由でアプリ全体を巻き添えにし得た）。
    t.feed("\x1b]7;file://host/tmp/%あ\x1b\\".as_bytes());
    assert_eq!(t.screen.remote_cwd.as_deref(), Some("/tmp/%あ"));
    // OSC 8 ハイパーリンク
    t.feed(b"\x1b]8;;https://example.com\x1b\\LINK\x1b]8;;\x1b\\plain");
    let line = t.screen.view_line(0, 0);
    let id = line[0].link;
    assert_ne!(id, 0);
    assert_eq!(t.screen.link_uri(id), Some("https://example.com"));
    assert_eq!(line[4].link, 0);
    // OSC 52 → クリップボードイベント（"hello" の base64）
    t.feed(b"\x1b]52;c;aGVsbG8=\x07");
    let events = t.screen.take_events();
    assert!(events.contains(&TermEvent::Clipboard("hello".into())));
}

#[test]
fn bracketed_paste_and_key_encoding() {
    let mut t = Terminal::new(10, 3, 0);
    assert!(!t.screen.bracketed_paste);
    t.feed(b"\x1b[?2004h");
    assert!(t.screen.bracketed_paste);
    assert_eq!(
        encode_paste("a\nb", true),
        b"\x1b[200~a\rb\x1b[201~".to_vec()
    );
    assert_eq!(encode_paste("a\r\nb", false), b"a\rb".to_vec());

    // カーソルキー: 通常 / アプリケーションモード
    assert_eq!(encode_key(Key::Up, Mods::NONE, false).unwrap(), b"\x1b[A");
    assert_eq!(encode_key(Key::Up, Mods::NONE, true).unwrap(), b"\x1bOA");
    let ctrl = Mods {
        ctrl: true,
        ..Mods::NONE
    };
    assert_eq!(encode_key(Key::Up, ctrl, false).unwrap(), b"\x1b[1;5A");
    assert_eq!(encode_key(Key::Char('c'), ctrl, false).unwrap(), vec![3u8]); // Ctrl+C = ETX
    assert_eq!(
        encode_key(Key::F(5), Mods::NONE, false).unwrap(),
        b"\x1b[15~"
    );
}

#[test]
fn mouse_reporting() {
    let mut t = Terminal::new(80, 24, 0);
    // モードOFFではレポートしない
    assert!(t
        .screen
        .encode_mouse(MouseButton::Left, true, false, 0, 0, false, false)
        .is_none());
    // SGR 1006
    t.feed(b"\x1b[?1002h\x1b[?1006h");
    let press = t
        .screen
        .encode_mouse(MouseButton::Left, true, false, 4, 9, false, false)
        .unwrap();
    assert_eq!(press, b"\x1b[<0;5;10M".to_vec());
    let release = t
        .screen
        .encode_mouse(MouseButton::Left, false, false, 4, 9, false, false)
        .unwrap();
    assert_eq!(release, b"\x1b[<0;5;10m".to_vec());
    let wheel = t
        .screen
        .encode_mouse(MouseButton::WheelUp, true, false, 0, 0, false, false)
        .unwrap();
    assert_eq!(wheel, b"\x1b[<64;1;1M".to_vec());
}

#[test]
fn resize_dynamic() {
    let mut t = Terminal::new(10, 4, 100);
    t.feed(b"1\r\n2\r\n3\r\n4");
    t.resize(10, 2); // 縮小 → 上2行はスクロールバックへ
    assert_eq!(t.screen.rows(), 2);
    assert_eq!(t.screen.scrollback_len(), 2);
    assert_eq!(text_at(&t, 0), "3");
    t.resize(10, 4); // 拡大 → 引き戻し
    assert_eq!(text_at(&t, 0), "1");
    assert_eq!(t.screen.scrollback_len(), 0);
    t.resize(5, 4);
    assert_eq!(t.screen.cols(), 5);
}

#[test]
fn da_and_dsr_responses() {
    let mut t = Terminal::new(10, 3, 0);
    t.feed(b"\x1b[c");
    assert_eq!(t.screen.take_responses(), b"\x1b[?62;22c".to_vec());
    t.feed(b"\x1b[3;2H\x1b[6n");
    assert_eq!(t.screen.take_responses(), b"\x1b[3;2R".to_vec());
}

#[test]
fn line_drawing_charset() {
    let mut t = Terminal::new(10, 3, 0);
    t.feed(b"\x1b(0lqk\x1b(B a");
    let line = t.screen.view_line(0, 0);
    assert_eq!(line[0].ch, '┌');
    assert_eq!(line[1].ch, '─');
    assert_eq!(line[2].ch, '┐');
    assert_eq!(line[4].ch, 'a');
}

#[test]
fn wrap_pending_behavior() {
    // xterm の遅延ラップ: 最終桁に書いても即改行せず、次の文字で折り返す
    let mut t = Terminal::new(5, 3, 0);
    t.feed(b"abcde");
    assert_eq!(t.screen.cursor.row, 0);
    t.feed(b"f");
    assert_eq!(t.screen.cursor.row, 1);
    assert_eq!(text_at(&t, 0), "abcde");
    assert_eq!(text_at(&t, 1), "f");
    // 最終桁の後の CR は同じ行に留まる
    let mut t2 = Terminal::new(5, 3, 0);
    t2.feed(b"abcde\rX");
    assert_eq!(text_at(&t2, 0), "Xbcde");
}

#[test]
fn utf8_split_across_feeds() {
    // マルチバイト文字が feed 境界で分割されても正しく組み立てる
    let bytes = "日本語".as_bytes();
    let mut t = Terminal::new(10, 3, 0);
    t.feed(&bytes[..4]);
    t.feed(&bytes[4..]);
    assert_eq!(text_at(&t, 0), "日本語");
}

#[test]
fn insert_delete_lines_chars() {
    let mut t = Terminal::new(10, 4, 0);
    t.feed(b"aaa\r\nbbb\r\nccc\r\nddd");
    t.feed(b"\x1b[2;1H\x1b[L"); // 2行目に1行挿入
    assert_eq!(text_at(&t, 1), "");
    assert_eq!(text_at(&t, 2), "bbb");
    assert_eq!(text_at(&t, 3), "ccc"); // ddd は押し出される
    t.feed(b"\x1b[2;1H\x1b[M"); // 削除で戻す
    assert_eq!(text_at(&t, 1), "bbb");
    // DCH / ICH
    let mut t2 = Terminal::new(10, 2, 0);
    t2.feed(b"abcdef\x1b[1;2H\x1b[2P");
    assert_eq!(text_at(&t2, 0), "adef");
    t2.feed(b"\x1b[1;2H\x1b[2@");
    assert_eq!(text_at(&t2, 0), "a  def");
}

// ---- OSC 133 シェル統合（コマンド境界） ----

#[test]
fn osc133_prompt_marks_and_jump() {
    // 10x3。プロンプト(A)→出力を繰り返し、絶対行のマークと前後ジャンプを検証する。
    let mut t = Terminal::new(10, 3, 100);
    t.feed(b"\x1b]133;A\x07"); // row0 → abs 0
    t.feed(b"p1\r\n");
    t.feed(b"\x1b]133;A\x07"); // row1 → abs 1
    t.feed(b"p2\r\n");
    t.feed(b"\x1b]133;A\x07"); // row2 → abs 2
    t.feed(b"p3\r\n"); // ここで最上段が1行スクロールアウト（scrolled_off=1）
    t.feed(b"\x1b]133;A\x07"); // scrolled_off1 + row2 → abs 3

    let s = &t.screen;
    assert_eq!(s.pushed_total(), 1);
    let marks: Vec<u64> = s.prompt_marks_abs().iter().copied().collect();
    assert_eq!(marks, vec![0, 1, 2, 3]);
    // 現在ビュー最上段の絶対行は 1（1行スクロールアウト済み）
    assert_eq!(s.top_visible_abs(0), 1);
    // 前後ジャンプ
    assert_eq!(s.prev_prompt_abs(3), Some(2));
    assert_eq!(s.prev_prompt_abs(1), Some(0));
    assert_eq!(s.prev_prompt_abs(0), None);
    assert_eq!(s.next_prompt_abs(0), Some(1));
    assert_eq!(s.next_prompt_abs(3), None);
}

#[test]
fn osc133_exit_code_parsing() {
    let mut t = Terminal::new(10, 3, 0);
    t.feed(b"\x1b]133;D;1\x07");
    assert_eq!(t.screen.last_exit_code(), Some(1));
    t.feed(b"\x1b]133;D;0\x07");
    assert_eq!(t.screen.last_exit_code(), Some(0));
    t.feed(b"\x1b]133;D\x07"); // code 無し → 不明
    assert_eq!(t.screen.last_exit_code(), None);
}

#[test]
fn osc133_ignored_in_alt_screen() {
    let mut t = Terminal::new(10, 3, 100);
    t.feed(b"\x1b[?1049h"); // 代替スクリーンへ
    t.feed(b"\x1b]133;A\x07");
    assert!(t.screen.prompt_marks_abs().is_empty());
}

// ---- スクロールバック強化（大容量・末尾空セル切り詰め） ----

#[test]
fn scrollback_caps_and_survives_many_lines() {
    // 上限1000に対し5000行を流し込み、最新1000行が残り古い行が捨てられること。
    let mut t = Terminal::new(10, 3, 1000);
    for i in 0..5000u32 {
        t.feed(format!("L{i}\r\n").as_bytes());
    }
    assert_eq!(t.screen.scrollback_len(), 1000);
    // 端の view_line 参照でパニックしないこと（切り詰め行でも安全）
    let _ = t.screen.view_line(0, 1000);
    // 最古の保持行は L(4000+?) 付近。境界で落ちないことと、絶対行が単調であることを確認。
    assert!(t.screen.pushed_total() >= 4000);
}

#[test]
fn scrollback_accepts_very_large_limit() {
    // 極端な上限(20万)を指定しても生成・少量投入で壊れない。
    let mut t = Terminal::new(80, 24, 200_000);
    for i in 0..3000u32 {
        t.feed(format!("row{i}\r\n").as_bytes());
    }
    assert!(t.screen.scrollback_len() <= 200_000);
    assert!(t.screen.scrollback_len() >= 2000);
}

#[test]
fn scrollback_trims_trailing_blanks_but_keeps_text() {
    // 末尾空白は切り詰めるが、テキストは view_line で正しく読める。
    let mut t = Terminal::new(20, 2, 100);
    t.feed(b"hello\r\n"); // 1行目 "hello" + 末尾空セル → スクロールバックへ（幅20だが切り詰め）
    t.feed(b"world\r\n");
    t.feed(b"third");
    // 先頭のスクロールバック行を読む
    let sb = t.screen.scrollback_len();
    assert!(sb >= 1);
    let line0 = t.screen.view_line(0, sb); // 最古のスクロールバック行
    let txt: String = line0.iter().map(|c| c.ch).collect::<String>();
    assert_eq!(txt.trim_end(), "hello");
    // 切り詰めにより行長は元の cols(20) 未満
    assert!(line0.len() <= 5);
}
