//! NEO-UI（3カラム未来的UI）のレンダリング。
//!
//! 常設サイドバー（ホスト一覧）＋中央端末＋右情報パネル＋タイトルバー＋ステータスバー。
//! クローム配色は neo::color 固定、端末本文色はホスト依存。
//!
//! **スケーリング**: 文字サイズ・レイアウト寸法はすべて `config.font_size`（既定14）から
//! 求めた `scale` に比例する。基準は 14pt=scale1.0。描画とヒットテストは同じ scale と
//! レイアウト式（NeoLayout / sidebar_rows）を共有してズレを防ぐ。

use super::*;
use crate::neo::{self, color as nc};
use crate::neofont::{icon, Face, NeoFonts};
use crate::render::Framebuffer;
use crate::theme::{blend, Pixel};

/// UI スケールの基準フォントサイズ。config.font_size がこの値のとき scale=1.0。
/// 端末本文（= font_size）より小さく取ることで、クローム文字を本文とほぼ同大まで
/// 相対的に大きくする（font_size=14 なら scale≈1.27）。
const BASE_FONT: f32 = 11.0;

/// 画面矩形。
#[derive(Clone, Copy)]
struct R {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

/// マスターパスワードモーダルの構成矩形。
struct MasterRects {
    card: R,
    input: R,
    eye: R,
    button: R,
}

/// パスワード系モーダル（Master/Password/Passphrase）の表示情報。
struct PwModalInfo {
    icon: char,
    title: String,
    sub1: String,
    sub2: String,
    placeholder: String,
    btn_label: String,
    footer: Option<String>,
    input: String,
}

/// NEO-UI のレイアウト（描画・ヒットテストで共有）。
struct NeoLayout {
    scale: f32,
    title: R,
    sidebar: R,
    tabstrip: R,
    /// SFTP セッションが開いている時の端末/SFTP 切替サブタブ帯。
    subtab: Option<R>,
    /// 中央ペイン（端末 or SFTP の描画領域）。subtab がある時はその下。
    terminal: R,
    /// 右の情報パネル（ホスト情報 + メトリクス）。畳んでいる時は None。
    info: Option<R>,
    status: R,
}

// 基準寸法（scale=1.0 のときの px）。
const TITLE_H: f32 = 32.0;
const STATUS_H: f32 = 24.0;
const SIDEBAR_W: f32 = 224.0;
const INFO_W: f32 = 258.0;
/// 情報パネルを出しても中央にこれだけの幅が残らなければ畳む（端末が潰れるのを防ぐ）。
const CENTER_MIN_W: f32 = 320.0;
const TABSTRIP_H: f32 = 36.0;
const TERM_HEADER_H: f32 = 30.0;
const SUBTAB_H: f32 = 30.0;

/// 長さを scale 倍して丸める。
#[inline]
fn si(n: f32, scale: f32) -> i32 {
    (n * scale).round() as i32
}

impl NeoLayout {
    fn compute(w: i32, h: i32, scale: f32, sftp_open: bool, info_open: bool) -> NeoLayout {
        let title_h = si(TITLE_H, scale);
        let status_h = si(STATUS_H, scale);
        let tabstrip_h = si(TABSTRIP_H, scale);
        let subtab_h = if sftp_open { si(SUBTAB_H, scale) } else { 0 };
        let sidebar_w = si(SIDEBAR_W, scale).min(w / 3).max(0);
        // 情報パネルを出すと中央が最小幅を割る場合は畳む（狭いウィンドウ対策）。
        let info_w = si(INFO_W, scale).min(w / 3).max(0);
        let info_open = info_open && (w - sidebar_w - info_w) >= si(CENTER_MIN_W, scale);
        let info_w = if info_open { info_w } else { 0 };
        let body_top = title_h;
        let body_bot = (h - status_h).max(body_top);
        let center_x = sidebar_w;
        let center_w = (w - sidebar_w - info_w).max(0);
        let content_top = body_top + tabstrip_h + subtab_h;
        NeoLayout {
            scale,
            title: R {
                x: 0,
                y: 0,
                w,
                h: title_h,
            },
            sidebar: R {
                x: 0,
                y: body_top,
                w: sidebar_w,
                h: body_bot - body_top,
            },
            tabstrip: R {
                x: center_x,
                y: body_top,
                w: center_w,
                h: tabstrip_h,
            },
            subtab: if sftp_open {
                Some(R {
                    x: center_x,
                    y: body_top + tabstrip_h,
                    w: center_w,
                    h: subtab_h,
                })
            } else {
                None
            },
            terminal: R {
                x: center_x,
                y: content_top,
                w: center_w,
                h: (body_bot - content_top).max(0),
            },
            info: if info_open {
                Some(R {
                    x: center_x + center_w,
                    y: body_top,
                    w: info_w,
                    h: body_bot - body_top,
                })
            } else {
                None
            },
            status: R {
                x: 0,
                y: body_bot,
                w,
                h: status_h,
            },
        }
    }

    /// 端末ペインヘッダの高さ（px）。
    fn term_header(&self) -> i32 {
        si(TERM_HEADER_H, self.scale)
    }
}

/// 1px の水平線（base 色に cyan を alpha で載せた枠色）。
fn hline(fb: &mut Framebuffer, x: i32, y: i32, w: i32, base: Pixel, alpha: f32) {
    fb.fill_rect(x, y, w, 1, blend(base, nc::CYAN, alpha));
}
fn vline(fb: &mut Framebuffer, x: i32, y: i32, h: i32, base: Pixel, alpha: f32) {
    fb.fill_rect(x, y, 1, h, blend(base, nc::CYAN, alpha));
}

fn status_color(status: &TabStatus) -> Pixel {
    match status {
        TabStatus::Connected => nc::GREEN,
        TabStatus::Connecting => nc::AMBER,
        TabStatus::Failed => nc::RED,
    }
}

/// アニメーション位相（0..1 の三角波、約1.1秒周期）。
fn anim_phase() -> f32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let t = (ms % 1100) as f32 / 1100.0;
    if t < 0.5 {
        t * 2.0
    } else {
        (1.0 - t) * 2.0
    }
}

/// 状態ドット（接続中は phase でグローがパルスする）。scale で拡縮。
fn status_dot(fb: &mut Framebuffer, cx: i32, cy: i32, status: &TabStatus, phase: f32, scale: f32) {
    let col = status_color(status);
    let glow = match status {
        TabStatus::Connecting => (5.0 + phase * 7.0) * scale,
        _ => 5.0 * scale,
    };
    neo::glow_dot(fb, cx, cy, 3.0 * scale, col, glow);
}

impl App {
    /// config.font_size から UI スケールを求める（14pt=1.0、0.6..3.0 でクランプ）。
    pub(super) fn neo_scale(&self) -> f32 {
        (self.px / BASE_FONT).clamp(0.6, 3.0)
    }

    /// タイトルバー右側のウィンドウ操作（設定/最小化/最大化/閉じる）とドラッグ移動。
    /// 右から close, maximize, minimize（各 46*scale 幅）、その左に設定アイコン。
    fn neo_titlebar_press(&mut self, mx: i32, scale: f32, title_w: i32) {
        let btn = si(46.0, scale);
        let set_r = title_w - btn * 3; // 設定ゾーン右端（= minimize の左端）
        let set_l = set_r - si(52.0, scale); // 設定ゾーン左端
        if mx >= title_w - btn {
            self.quit_requested = true;
        } else if mx >= title_w - btn * 2 {
            if let Some(w) = &self.window {
                w.set_maximized(!w.is_maximized());
            }
        } else if mx >= set_r {
            if let Some(w) = &self.window {
                w.set_minimized(true);
            }
        } else if mx >= set_l {
            // 設定アイコン → 設定ファイルをテキストエディタで開く。
            crate::app::mouse::open_in_editor(&self.config_file);
        } else {
            // それより左（ワードマーク等の空き領域）はウィンドウ移動。
            if let Some(w) = &self.window {
                let _ = w.drag_window();
            }
        }
    }

    /// 現在の状態に基づく NEO-UI レイアウト（SFTP セッションの有無を反映）。
    /// SFTP はアクティブタブの接続に紐づく時だけサブタブ帯を出し、表示中は
    /// 2ペインに幅を割くため情報パネルを畳む。
    fn neo_layout(&self) -> NeoLayout {
        let belongs = self.fm_belongs_to_active();
        NeoLayout::compute(
            self.fb.w as i32,
            self.fb.h as i32,
            self.neo_scale(),
            belongs,
            // SFTP は 2 ペインに幅を要するため、その間は情報パネルを畳む。
            self.neo_info_visible && !belongs,
        )
    }

    /// NEO-UI 全体を描画する。
    pub(super) fn render_neo(&mut self) {
        let scale = self.neo_scale();
        let lay = self.neo_layout();
        let active_tab = self.active_tab;
        // アクティブタブのフォーカスペインを端末領域のセル数へ合わせる（SIGWINCH）。
        self.relayout_neo(&lay);
        let m = self.cell_metrics();
        let theme = self.theme.clone();
        let selection = self.selection; // Copy
        let tab_rename = self.tab_rename.clone();
        let sftp_view = self.mode == Mode::Sftp && self.fm_belongs_to_active();
        let lang = self.lang;
        let filter_focus = self.neo_filter_focus;
        let sidebar_sel = self.neo_sidebar_sel;
        let broadcast = self.broadcast;
        let ime_preedit = self.ime_preedit.clone();
        // 検索ハイライト用に (マッチ範囲, カレント) を複製（self 分解前に借用を切る）。
        let search_data = self.search.as_ref().map(|s| (s.matches.clone(), s.current));
        let tab_title = self
            .tabs
            .get(active_tab)
            .map(|t| t.title.clone())
            .unwrap_or_default();
        let App {
            fb,
            font,
            neo_fonts,
            launcher,
            tabs,
            neo_collapsed,
            fm,
            ..
        } = self;

        fb.clear(nc::BG);
        draw_dot_grid(fb, scale);

        let phase = anim_phase();
        draw_titlebar(fb, neo_fonts, &lay);
        draw_sidebar(
            fb,
            neo_fonts,
            &lay,
            launcher,
            neo_collapsed,
            filter_focus,
            sidebar_sel,
        );
        draw_tabstrip(
            fb,
            neo_fonts,
            &lay,
            tabs,
            active_tab,
            phase,
            tab_rename.as_ref(),
        );

        // 中央: SFTP セッションがあればサブタブ帯＋（SFTP or 端末）、無ければ端末。
        if let Some(sub) = lay.subtab {
            draw_subtab_strip(fb, neo_fonts, &sub, &tab_title, sftp_view, scale);
        }
        if sftp_view {
            if let Some(fmr) = fm.as_mut() {
                draw_neo_sftp(fb, neo_fonts, font, &m, &lay, fmr, scale, lang);
            }
        } else {
            let search_hl =
                search_data
                    .as_ref()
                    .map(|(matches, current)| crate::termview::SearchHl {
                        matches,
                        current: *current,
                    });
            draw_terminal(
                fb,
                neo_fonts,
                font,
                &theme,
                &m,
                &lay,
                tabs.get(active_tab),
                selection.as_ref(),
                search_hl,
            );
        }

        // IME 変換中テキスト（preedit）を端末カーソル位置へインライン描画。
        if !ime_preedit.is_empty() && !sftp_view {
            if let Some((cx, cy)) = neo_terminal_cursor_px(&lay, &m, scale, tabs.get(active_tab)) {
                draw_preedit(fb, font, &theme, &m, cx, cy, &ime_preedit);
            }
        }

        // ブロードキャスト入力中は端末領域上端に赤帯＋ラベルを出す。
        if broadcast && !sftp_view {
            draw_broadcast_bar(fb, neo_fonts, &lay, scale);
        }

        draw_info_panel(fb, neo_fonts, &lay, tabs.get(active_tab));
        draw_statusbar(fb, neo_fonts, &lay, tabs, active_tab, phase);
    }

    /// IME 候補ウィンドウの表示位置を端末カーソルへ合わせるよう OS へ通知する。
    /// これを呼ばないと変換候補が既定位置（左上など）に出てしまう。
    pub(super) fn update_ime_cursor_area(&mut self) {
        if !self.config.use_ime || self.window.is_none() {
            return;
        }
        let lay = self.neo_layout();
        let m = self.cell_metrics();
        let scale = self.neo_scale();
        let cursor = neo_terminal_cursor_px(&lay, &m, scale, self.tabs.get(self.active_tab));
        if let (Some((cx, cy)), Some(window)) = (cursor, &self.window) {
            window.set_ime_cursor_area(
                winit::dpi::PhysicalPosition::new(cx, cy),
                winit::dpi::PhysicalSize::new((m.cw * 2).max(1) as u32, m.ch.max(1) as u32),
            );
        }
    }

    /// NEO-UI: アクティブタブのフォーカスペインを端末領域（ヘッダ分を除く）へリサイズ。
    fn relayout_neo(&mut self, lay: &NeoLayout) {
        let m = self.cell_metrics();
        // 桁/行は「実際に本文を描く可視領域」から算出する。draw_terminal/draw_screen は
        // 左に si(6) の余白、右に si(12) のスクロールバー、上に +2 のインセットを取るため、
        // それらを差し引く。全幅/全高から出すと報告サイズが可視領域より大きくなり、
        // 全画面アプリ(vim/htop/tmux)の最終列・行がスクロールバー下や画面外へはみ出して
        // 「右端・下端に変な文字列」として見える（#表示崩れ）。
        let cw = (lay.terminal.w - si(6.0, lay.scale) - si(12.0, lay.scale)).max(1);
        let ch = (lay.terminal.h - lay.term_header() - 2).max(1);
        let cols = (cw / m.cw).max(1) as u16;
        let rows = (ch / m.ch).max(1) as u16;
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            let focus = tab.focus;
            if let Some(p) = tab.panes.get_mut(&focus) {
                if p.cols != cols || p.rows != rows {
                    p.cols = cols;
                    p.rows = rows;
                    p.terminal.resize(cols as usize, rows as usize);
                    p.handle.resize(cols, rows);
                }
            }
        }
    }
}

fn in_r(r: &R, mx: i32, my: i32) -> bool {
    mx >= r.x && mx < r.x + r.w && my >= r.y && my < r.y + r.h
}

impl App {
    /// NEO-UI のマウス処理（押下/解放・左右）。
    pub(super) fn neo_mouse(&mut self, state: ElementState, button: MouseButton) {
        let (mx, my) = (self.mouse.0 as i32, self.mouse.1 as i32);

        // 解放: ドラッグ状態＋マウス転送を解除。
        if state == ElementState::Released {
            if button == MouseButton::Left {
                self.dragging_sel = false;
                self.tab_drag = None;
                self.scrollbar_drag = None;
            }
            self.neo_mouse_report_release(mx, my, button);
            return;
        }
        // タブメニュー表示中は最優先で項目処理。
        if self.tab_menu.is_some() {
            self.handle_tab_menu_click(button);
            return;
        }
        // Filter 入力のフォーカスは押下でいったん外す（フィルタ枠クリックで再取得）。
        self.neo_filter_focus = false;

        let scale = self.neo_scale();
        let lay = self.neo_layout();

        // 枠なし時、ウィンドウ端の押下は OS のリサイズドラッグへ委譲。
        if button == MouseButton::Left {
            if let Some(dir) = self.edge_resize_dir(mx, my) {
                if let Some(win) = &self.window {
                    let _ = win.drag_resize_window(dir);
                }
                return;
            }
        }

        // タイトルバー: 最小化/最大化/閉じる＋ドラッグ移動（左のみ）。
        if in_r(&lay.title, mx, my) {
            if button == MouseButton::Left {
                self.neo_titlebar_press(mx, scale, lay.title.w);
            }
            return;
        }

        // タブ帯
        let ts = lay.tabstrip;
        if my >= ts.y && my < ts.y + ts.h && mx >= ts.x {
            self.neo_tabstrip_press(mx, button, &ts);
            return;
        }

        // SFTP セッションのサブタブ帯（端末⇄SFTP 切替）。
        if let Some(sub) = lay.subtab {
            if my >= sub.y && my < sub.y + sub.h && mx >= sub.x {
                if button == MouseButton::Left {
                    self.neo_subtab_click(mx, my, &sub);
                }
                return;
            }
        }

        // 端末領域: URL / マウス転送 / スクロールバー / テキスト選択。
        if in_r(&lay.terminal, mx, my) {
            // Ctrl+左クリックで URL を開く（OSC8 → http(s) 検出）。
            if button == MouseButton::Left && self.mods.control_key() {
                if let Some(url) = self.neo_url_at(mx, my) {
                    crate::app::mouse::open_url(&url);
                    return;
                }
            }
            // マウス対応TUI（vim/htop 等）へプロトコル転送（Shift非押下 & mode有効）。
            if !self.mods.shift_key() {
                if let Some((pid, col, row, mode)) = self.neo_pane_cell_at(mx, my) {
                    if mode != mot_term::MouseMode::Off {
                        if let Some(mb) = crate::app::mouse::map_button(button) {
                            self.send_mouse(pid, mb, true, false, col, row);
                            self.mouse_report_btn = Some(mb);
                            return;
                        }
                    }
                }
            }
            // 右クリックで貼り付け（マウスモードが Off のとき）。
            if button == MouseButton::Right {
                self.paste_clipboard();
                return;
            }
            if button == MouseButton::Left {
                if self.neo_scrollbar_press(mx, my, &lay) {
                    return;
                }
                // テキスト選択開始
                if let Some((col, view_row)) = self.neo_term_cell(mx, my) {
                    self.bump_click_count();
                    self.start_selection_at(col, view_row);
                }
            }
            return;
        }

        // サイドバー（左のみ）
        if button == MouseButton::Left && in_r(&lay.sidebar, mx, my) {
            self.neo_sidebar_click(mx, my, &lay);
        }
    }

    /// 端末領域の (mx,my) → フォーカスペインの (pid, col, row, mouse_mode)。マウス転送用。
    pub(super) fn neo_pane_cell_at(
        &mut self,
        mx: i32,
        my: i32,
    ) -> Option<(u64, usize, usize, mot_term::MouseMode)> {
        let lay = self.neo_layout();
        if !in_r(&lay.terminal, mx, my) {
            return None;
        }
        let (col, view_row) = self.neo_term_cell(mx, my)?;
        let tab = self.tabs.get(self.active_tab)?;
        let pid = tab.focus;
        let mode = tab.panes.get(&pid)?.terminal.screen.mouse_mode;
        Some((pid, col, view_row, mode))
    }

    /// 端末領域の (mx,my) が指すセルの URL（OSC8 リンク優先、無ければ行から http(s) 検出）。
    fn neo_url_at(&mut self, mx: i32, my: i32) -> Option<String> {
        let (col, view_row) = self.neo_term_cell(mx, my)?;
        let tab = self.tabs.get(self.active_tab)?;
        let p = tab.panes.get(&tab.focus)?;
        let line = p.terminal.screen.view_line(view_row, p.scroll);
        if let Some(cell) = line.get(col) {
            if let Some(uri) = p.terminal.screen.link_uri(cell.link) {
                return Some(uri.to_string());
            }
        }
        crate::app::mouse::url_at_cells(line, col)
    }

    /// マウス転送中のボタン解放を対象ペインへ報告する。
    fn neo_mouse_report_release(&mut self, mx: i32, my: i32, button: MouseButton) {
        let Some(mb) = self.mouse_report_btn else {
            return;
        };
        if crate::app::mouse::map_button(button) != Some(mb) {
            return;
        }
        self.mouse_report_btn = None;
        if let Some((pid, col, row, mode)) = self.neo_pane_cell_at(mx, my) {
            if mode != mot_term::MouseMode::Off {
                self.send_mouse(pid, mb, false, false, col, row);
            }
        }
    }

    /// タブ帯の押下（切替/ドラッグ開始/×閉じ/ダブルクリックrename/右クリックメニュー）。
    fn neo_tabstrip_press(&mut self, mx: i32, button: MouseButton, ts: &R) {
        let scale = self.neo_scale();
        let (geom, plus_x0, plus_x1) = self.neo_tab_geometry();
        for (i, x0, x1) in geom {
            if mx >= x0 && mx < x1 {
                match button {
                    MouseButton::Right => {
                        self.tab_menu = Some(TabMenu {
                            index: i,
                            x: x0,
                            y: ts.y + ts.h,
                        });
                    }
                    MouseButton::Left => {
                        // × ボタン領域（タブ右端）
                        if mx >= x1 - si(18.0, scale) {
                            self.active_tab = i;
                            self.close_active_tab();
                            self.last_tab_click = None;
                            return;
                        }
                        // ダブルクリック → rename
                        let now = std::time::Instant::now();
                        let dbl = double_click(
                            self.last_tab_click.map(|(li, _)| li),
                            self.last_tab_click
                                .map(|(_, t)| now.duration_since(t).as_millis()),
                            i,
                            DOUBLE_CLICK_MS,
                        );
                        if dbl {
                            self.tab_rename = Some((i, self.tabs[i].title.clone()));
                            self.last_tab_click = None;
                            return;
                        }
                        // 単発: 切替＋ドラッグ開始候補
                        self.active_tab = i;
                        self.tab_drag = Some(TabDrag {
                            index: i,
                            press_x: mx,
                            moved: false,
                        });
                        self.last_tab_click = Some((i, now));
                    }
                    _ => {}
                }
                return;
            }
        }
        // 「＋」領域（左）: 未接続なら何もしない（接続はサイドバーから）
        let _ = (plus_x0, plus_x1);
    }

    /// neo タブ帯のジオメトリ (i, x0, x1) と「＋」ボタンの x 範囲。描画/ヒット/ドラッグで共有。
    pub(super) fn neo_tab_geometry(&mut self) -> (Vec<(usize, i32, i32)>, i32, i32) {
        let scale = self.neo_scale();
        let lay = self.neo_layout();
        let titles: Vec<String> = self.tabs.iter().map(|t| t.title.clone()).collect();
        let mut x = lay.tabstrip.x + si(4.0, scale);
        let mut out = Vec::with_capacity(titles.len());
        for (i, title) in titles.iter().enumerate() {
            let tw = self.neo_fonts.measure(Face::Mono, title, 11.0 * scale) + si(44.0, scale);
            out.push((i, x, x + tw));
            x += tw + 1;
        }
        let plus_x0 = x + si(2.0, scale);
        let plus_x1 = plus_x0 + si(24.0, scale);
        (out, plus_x0, plus_x1)
    }

    /// 端末領域の (mx,my) → フォーカスペインのビューセル (col, view_row)。
    /// 端末コンテンツ原点は draw_screen と一致させる（左端 x+6・上端 header 下+2）。
    pub(super) fn neo_term_cell(&mut self, mx: i32, my: i32) -> Option<(usize, usize)> {
        let scale = self.neo_scale();
        let lay = self.neo_layout();
        let m = self.cell_metrics();
        let t = lay.terminal;
        let ox = t.x + si(6.0, scale);
        let oy = t.y + lay.term_header() + 2;
        if mx < ox || my < oy || m.cw <= 0 || m.ch <= 0 {
            return None;
        }
        let col = ((mx - ox) / m.cw).max(0) as usize;
        let view_row = ((my - oy) / m.ch).max(0) as usize;
        Some((col, view_row))
    }

    /// 端末スクロールバーのサム/トラックを押下したらドラッグ開始（true で消費）。
    fn neo_scrollbar_press(&mut self, mx: i32, my: i32, lay: &NeoLayout) -> bool {
        let scale = lay.scale;
        let t = lay.terminal;
        let oy = t.y + lay.term_header() + 2;
        let track_h = (t.h - lay.term_header() - 2).max(1);
        let hit_l = t.x + t.w - si(12.0, scale);
        if mx < hit_l || mx >= t.x + t.w || my < oy || my >= oy + track_h {
            return false;
        }
        let (pid, total, rows, scroll) = {
            let Some(tab) = self.tabs.get(self.active_tab) else {
                return false;
            };
            let Some(p) = tab.panes.get(&tab.focus) else {
                return false;
            };
            (
                tab.focus,
                p.terminal.screen.scrollback_len() + p.terminal.screen.rows(),
                p.rows as usize,
                p.scroll,
            )
        };
        let Some((thumb_top, thumb_h)) =
            crate::termview::scrollbar_thumb(track_h, total, rows, scroll)
        else {
            return false;
        };
        let thumb_top_abs = oy + thumb_top;
        let grab_dy = if my >= thumb_top_abs && my < thumb_top_abs + thumb_h {
            my - thumb_top_abs
        } else {
            thumb_h / 2
        };
        self.scrollbar_drag = Some(ScrollbarDrag { pid, grab_dy });
        self.neo_on_scrollbar_drag();
        true
    }

    /// neo スクロールバードラッグ: マウス y からフォーカスペインの scroll を逆算。
    pub(super) fn neo_on_scrollbar_drag(&mut self) {
        let Some(drag) = self.scrollbar_drag else {
            return;
        };
        let lay = self.neo_layout();
        let t = lay.terminal;
        let oy = t.y + lay.term_header() + 2;
        let track_h = (t.h - lay.term_header() - 2).max(1);
        let my = self.mouse.1 as i32;
        let _ = drag.pid;
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            let focus = tab.focus;
            if let Some(p) = tab.panes.get_mut(&focus) {
                let total = p.terminal.screen.scrollback_len() + p.terminal.screen.rows();
                let thumb_top = my - oy - drag.grab_dy;
                p.scroll = crate::termview::scroll_from_thumb_top(
                    track_h,
                    total,
                    p.rows as usize,
                    thumb_top,
                );
            }
        }
    }

    /// サイドバークリック: NEW CONNECTION ボタン / ホスト行 → 接続 / グループ → 折りたたみ。
    /// Filter 入力のキー処理（フォーカス時）。query を編集し、Enter で先頭ホストへ接続。
    pub(super) fn neo_filter_key(&mut self, event: &winit::event::KeyEvent) {
        use winit::keyboard::{Key as WKey, NamedKey};
        match &event.logical_key {
            WKey::Named(NamedKey::Escape) => {
                self.neo_filter_focus = false;
                self.launcher.query.clear();
            }
            WKey::Named(NamedKey::Enter) => {
                let first = self
                    .launcher
                    .model
                    .filter(&self.launcher.query)
                    .into_iter()
                    .next()
                    .cloned();
                self.neo_filter_focus = false;
                if let Some(p) = first {
                    self.connect_profile(p);
                }
            }
            WKey::Named(NamedKey::Backspace) => {
                self.launcher.query.pop();
            }
            _ => {
                if let Some(t) = &event.text {
                    if !t.chars().any(|c| c.is_control()) {
                        self.launcher.query.push_str(t);
                    }
                }
            }
        }
    }

    /// サイドバーのキーボード選択モードのキー処理（Ctrl+T で開始）。
    /// ↑↓ で行移動、Enter で接続（Host→その接続先 / Group→一括接続）、Esc で解除。
    /// 行数はクエリ有無・折りたたみで変わるため、毎回 sidebar_rows を取り直して index をクランプする。
    /// 選択モード中は上記以外のキーも消費し、端末へは渡さない（on_key 側で return）。
    pub(super) fn neo_sidebar_nav_key(&mut self, event: &winit::event::KeyEvent) {
        use winit::keyboard::{Key as WKey, NamedKey};
        let lay = self.neo_layout();
        let rows = sidebar_rows(
            &self.launcher.model,
            lay.sidebar,
            &self.launcher.query,
            &self.neo_collapsed,
            lay.scale,
        );
        if rows.is_empty() {
            // 行が無ければ Esc だけ受け付けて解除。
            if matches!(event.logical_key, WKey::Named(NamedKey::Escape)) {
                self.neo_sidebar_sel = None;
            }
            return;
        }
        let last = rows.len() - 1;
        let cur = self.neo_sidebar_sel.unwrap_or(0).min(last);
        match &event.logical_key {
            WKey::Named(NamedKey::Escape) => {
                self.neo_sidebar_sel = None;
            }
            WKey::Named(NamedKey::ArrowUp) => {
                self.neo_sidebar_sel = Some(cur.saturating_sub(1));
            }
            WKey::Named(NamedKey::ArrowDown) => {
                self.neo_sidebar_sel = Some((cur + 1).min(last));
            }
            WKey::Named(NamedKey::Enter) => {
                // rows は self を不変借用しているため、接続対象を先に owned へ取り出してから
                // self を可変借用する（connect_profile / neo_connect_group は &mut self）。
                enum Nav {
                    Host(Box<Profile>),
                    Group(String),
                }
                let nav = match &rows[cur].row {
                    NeoRow::Host { profile } => Nav::Host(profile.clone()),
                    NeoRow::Group { name, .. } => Nav::Group(name.clone()),
                };
                self.neo_sidebar_sel = None;
                match nav {
                    Nav::Host(p) => self.connect_profile(*p),
                    Nav::Group(g) => self.neo_connect_group(&g),
                }
            }
            // 選択モード中は他キーも消費（何もしない）。
            _ => {}
        }
    }

    fn neo_sidebar_click(&mut self, mx: i32, my: i32, lay: &NeoLayout) {
        let scale = lay.scale;
        let s = lay.sidebar;
        // Filter 入力ボックス（最上部）をクリック → フォーカス取得（キーを query へ）。
        let fx = s.x + si(10.0, scale);
        let fy = s.y + si(10.0, scale);
        let fw = s.w - si(20.0, scale);
        let fh = si(26.0, scale);
        if mx >= fx && mx < fx + fw && my >= fy && my < fy + fh {
            self.neo_filter_focus = true;
            return;
        }
        // NEW CONNECTION ボタン（最下部）→ 設定ファイルをエディタで開く（ホスト追加用）。
        let by = s.y + s.h - si(38.0, scale);
        if my >= by
            && my < by + si(28.0, scale)
            && mx >= s.x + si(10.0, scale)
            && mx < s.x + s.w - si(10.0, scale)
        {
            crate::app::mouse::open_in_editor(&self.config_file);
            return;
        }
        let rows = sidebar_rows(
            &self.launcher.model,
            s,
            &self.launcher.query,
            &self.neo_collapsed,
            scale,
        );
        let Some(pr) = rows.iter().find(|r| my >= r.y && my < r.y + r.h) else {
            return;
        };
        match &pr.row {
            NeoRow::Host { profile } => {
                let p = (**profile).clone();
                self.connect_profile(p);
            }
            NeoRow::Group { name, count, .. } => {
                // 右端の ⚡ ゾーン（"All" 以外・ホストあり）はグループ一括接続。
                let zap_l = s.x + s.w - si(46.0, scale);
                let zap_r = s.x + s.w - si(30.0, scale);
                if name != "All" && *count > 0 && mx >= zap_l && mx < zap_r {
                    let g = name.clone();
                    self.neo_connect_group(&g);
                } else {
                    // それ以外は折りたたみトグル。
                    let n = name.clone();
                    if !self.neo_collapsed.remove(&n) {
                        self.neo_collapsed.insert(n);
                    }
                }
            }
        }
    }

    /// タブ右クリックメニュー（neon スタイル）。項目矩形は tab_menu_geometry と共有。
    /// マスターパスワードモーダルの矩形（カード/入力/目/ボタン）。描画とヒットで共有。
    fn neo_master_rects(&self) -> MasterRects {
        let scale = self.neo_scale();
        let (fw, fh) = (self.fb.w as i32, self.fb.h as i32);
        let cw = si(360.0, scale)
            .min(fw - si(40.0, scale))
            .max(si(240.0, scale));
        let chh = si(316.0, scale);
        let cx = (fw - cw) / 2;
        let cy = (fh - chh) / 2;
        let inx = cx + si(28.0, scale);
        let inw = cw - si(56.0, scale);
        // 入力の y は アイコン箱(28+54)＋タイトル/サブ 領域の下。
        let iny = cy + si(28.0 + 54.0 + 26.0 + 24.0 + 34.0, scale);
        let inh = si(38.0, scale);
        let eye_w = si(30.0, scale);
        let by = iny + inh + si(30.0, scale);
        let bh = si(40.0, scale);
        MasterRects {
            card: R {
                x: cx,
                y: cy,
                w: cw,
                h: chh,
            },
            input: R {
                x: inx,
                y: iny,
                w: inw,
                h: inh,
            },
            eye: R {
                x: inx + inw - eye_w,
                y: iny,
                w: eye_w,
                h: inh,
            },
            button: R {
                x: inx,
                y: by,
                w: inw,
                h: bh,
            },
        }
    }

    /// マスターパスワードのクリック処理（目のトグル / Unlock ボタン）。
    pub(super) fn neo_master_modal_mouse(&mut self) {
        let (mx, my) = (self.mouse.0 as i32, self.mouse.1 as i32);
        let r = self.neo_master_rects();
        if in_r(&r.eye, mx, my) {
            self.master_show = !self.master_show;
        } else if in_r(&r.button, mx, my) {
            self.submit_pw_modal();
        }
    }

    /// ボタン相当（Enter と同じく入力を送って閉じる）。Master/Password/Passphrase 共通。
    fn submit_pw_modal(&mut self) {
        match self.modal.as_ref() {
            Some(Modal::Master { input, reply, .. })
            | Some(Modal::Password { input, reply, .. })
            | Some(Modal::Passphrase { input, reply, .. }) => {
                let _ = reply.send(Some(input.clone()));
            }
            _ => {}
        }
        self.close_modal();
    }

    /// パスワード系モーダルの種別ごとの表示情報。
    fn neo_pw_modal_info(&self) -> Option<PwModalInfo> {
        let target = |conn| {
            self.tabs
                .iter()
                .find(|t| t.conn == conn)
                .and_then(|t| t.profile.as_ref())
                .map(|p| {
                    format!(
                        "{}@{}:{}",
                        p.user.as_deref().unwrap_or("user"),
                        p.host,
                        p.port
                    )
                })
                .unwrap_or_else(|| "SSH connection".to_string())
        };
        match self.modal.as_ref()? {
            Modal::Master { is_new, input, .. } => Some(PwModalInfo {
                icon: icon::LOCK,
                title: if *is_new {
                    "Set Master Password"
                } else {
                    "Enter Master Password"
                }
                .into(),
                sub1: if *is_new {
                    "Create a password to encrypt your vault.".into()
                } else {
                    "moterm vault is locked.".into()
                },
                sub2: "Credentials are encrypted at rest.".into(),
                placeholder: "Master password".into(),
                btn_label: if *is_new {
                    "Set Password"
                } else {
                    "Unlock Vault"
                }
                .into(),
                footer: Some("AES-256-GCM · Argon2id".into()),
                input: input.clone(),
            }),
            Modal::Password { conn, input, .. } => Some(PwModalInfo {
                icon: icon::KEY,
                title: "SSH Password".into(),
                sub1: target(*conn),
                sub2: "Enter the account password to authenticate.".into(),
                placeholder: "Password".into(),
                btn_label: "Connect".into(),
                footer: None,
                input: input.clone(),
            }),
            Modal::Passphrase { conn, input, .. } => Some(PwModalInfo {
                icon: icon::KEY,
                title: "Key Passphrase".into(),
                sub1: target(*conn),
                sub2: "Unlock the private key to authenticate.".into(),
                placeholder: "Passphrase".into(),
                btn_label: "Unlock Key".into(),
                footer: None,
                input: input.clone(),
            }),
            _ => None,
        }
    }

    /// Figma 準拠のネオン・パスワード系モーダル（Master/Password/Passphrase）を描画。
    pub(super) fn draw_neo_master_modal(&mut self) {
        let Some(info) = self.neo_pw_modal_info() else {
            return;
        };
        let input = info.input.clone();
        let scale = self.neo_scale();
        let show = self.master_show;
        let r = self.neo_master_rects();
        let card = r.card;
        let (cx, cy, cw) = (card.x, card.y, card.w);
        let App { fb, neo_fonts, .. } = self;

        // 暗幕
        fb.overlay_dim(nc::BG_DEEP, 0.72);
        // カード（影＋角丸＋シアン枠）
        neo::glow_round_rect(
            fb,
            cx,
            cy,
            cw,
            card.h,
            10.0 * scale,
            nc::BG_DEEP,
            0.75,
            (14.0 * scale) as usize,
        );
        neo::fill_round_rect(fb, cx, cy, cw, card.h, 10.0 * scale, 0x060C18, 0.99);
        neo::stroke_round_rect(fb, cx, cy, cw, card.h, 10.0 * scale, nc::CYAN, 0.22, 1.0);

        // アイコン箱（ロック）
        let ib = si(54.0, scale);
        let ibx = cx + (cw - ib) / 2;
        let iby = cy + si(28.0, scale);
        neo::fill_round_rect(fb, ibx, iby, ib, ib, 12.0 * scale, nc::CYAN, 0.06);
        neo::stroke_round_rect(fb, ibx, iby, ib, ib, 12.0 * scale, nc::CYAN, 0.25, 1.0);
        let is = si(26.0, scale);
        neo_fonts.draw_icon(
            fb,
            info.icon,
            ibx + (ib - is) / 2,
            iby + (ib - is) / 2,
            is as f32,
            nc::CYAN,
        );

        // タイトル
        let ty = iby + ib + si(30.0, scale);
        let tw = neo_fonts.measure(Face::Ui, &info.title, 16.0 * scale);
        neo_fonts.draw(
            fb,
            Face::Ui,
            &info.title,
            cx + (cw - tw) / 2,
            ty,
            16.0 * scale,
            nc::TEXT,
        );
        // サブタイトル（2行・中央）
        let s1w = neo_fonts.measure(Face::Ui, &info.sub1, 11.5 * scale);
        neo_fonts.draw(
            fb,
            Face::Ui,
            &info.sub1,
            cx + (cw - s1w) / 2,
            ty + si(20.0, scale),
            11.5 * scale,
            nc::TEXT_SUB,
        );
        let s2w = neo_fonts.measure(Face::Ui, &info.sub2, 11.5 * scale);
        neo_fonts.draw(
            fb,
            Face::Ui,
            &info.sub2,
            cx + (cw - s2w) / 2,
            ty + si(36.0, scale),
            11.5 * scale,
            nc::TEXT_SUB,
        );

        // 入力ボックス
        let inp = r.input;
        let has_val = !input.is_empty();
        neo::fill_round_rect(
            fb,
            inp.x,
            inp.y,
            inp.w,
            inp.h,
            6.0 * scale,
            nc::BG_DEEP,
            0.7,
        );
        let (bcol, ba) = if has_val {
            (nc::CYAN, 0.4)
        } else {
            (nc::BORDER, 0.14)
        };
        neo::stroke_round_rect(fb, inp.x, inp.y, inp.w, inp.h, 6.0 * scale, bcol, ba, 1.0);
        let baseline = inp.y + inp.h / 2 + si(4.0, scale);
        if input.is_empty() {
            neo_fonts.draw(
                fb,
                Face::Mono,
                &info.placeholder,
                inp.x + si(12.0, scale),
                baseline,
                12.5 * scale,
                nc::TEXT_DIM,
            );
        } else if show {
            neo_fonts.draw(
                fb,
                Face::Mono,
                &input,
                inp.x + si(12.0, scale),
                baseline,
                12.5 * scale,
                nc::TEXT,
            );
        } else {
            // ドット（• を文字数ぶん、間隔を少し空けて）
            let mut dx = inp.x + si(12.0, scale);
            for _ in 0..input.chars().count() {
                dx = neo_fonts.draw(fb, Face::Mono, "•", dx, baseline, 12.5 * scale, nc::TEXT)
                    + si(2.0, scale);
            }
        }
        // 目のトグル
        let eye_ic = if show { icon::EYE_OFF } else { icon::EYE };
        neo_fonts.draw_icon(
            fb,
            eye_ic,
            r.eye.x + (r.eye.w - si(15.0, scale)) / 2,
            inp.y + (inp.h - si(15.0, scale)) / 2,
            15.0 * scale,
            nc::TEXT_SUB,
        );

        // Unlock ボタン
        let btn = r.button;
        neo::fill_round_rect(fb, btn.x, btn.y, btn.w, btn.h, 6.0 * scale, nc::CYAN, 0.12);
        neo::stroke_round_rect(
            fb,
            btn.x,
            btn.y,
            btn.w,
            btn.h,
            6.0 * scale,
            nc::CYAN,
            0.35,
            1.0,
        );
        let blw = neo_fonts.measure(Face::Ui, &info.btn_label, 13.0 * scale);
        let ic_gap = si(20.0, scale);
        let group_w = ic_gap + blw;
        let gx = btn.x + (btn.w - group_w) / 2;
        let bcy = btn.y + btn.h / 2;
        neo_fonts.draw_icon(
            fb,
            icon::UNLOCK,
            gx,
            bcy - si(7.0, scale),
            14.0 * scale,
            nc::CYAN,
        );
        neo_fonts.draw(
            fb,
            Face::Ui,
            &info.btn_label,
            gx + ic_gap,
            bcy + si(4.0, scale),
            13.0 * scale,
            nc::CYAN,
        );

        // フッタ（vault のみ）
        if let Some(foot) = &info.footer {
            let fw2 = neo_fonts.measure(Face::Mono, foot, 10.0 * scale);
            neo_fonts.draw(
                fb,
                Face::Mono,
                foot,
                cx + (cw - fw2) / 2,
                btn.y + btn.h + si(24.0, scale),
                10.0 * scale,
                nc::TEXT_DIM,
            );
        }
    }

    /// ポートフォワードパネル（F2）。端末右上に浮かぶ neon カード。
    pub(super) fn draw_neo_pf_panel(&mut self) {
        use mot_core::model::ForwardType;
        let scale = self.neo_scale();
        let lay = self.neo_layout();
        // アクティブタブの実フォワード状態を収集（最大 8 行）。
        let rows: Vec<(String, String, bool)> = self
            .tabs
            .get(self.active_tab)
            .and_then(|t| t.session.as_ref())
            .map(|s| {
                s.forwards()
                    .iter()
                    .take(8)
                    .map(|f| {
                        let dir = match f.kind {
                            ForwardType::Local => "-L",
                            ForwardType::Remote => "-R",
                        };
                        (format!("{dir}  {}", f.listen), f.target.clone(), f.ok)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let t = lay.terminal;
        let pw = si(320.0, scale)
            .min(t.w - si(24.0, scale))
            .max(si(180.0, scale));
        let rowh = si(22.0, scale);
        let head = si(38.0, scale);
        let body_rows = rows.len().max(1) as i32;
        let ph = head + rowh * body_rows + si(10.0, scale);
        let px0 = t.x + t.w - pw - si(12.0, scale);
        let py0 = t.y + lay.term_header() + si(12.0, scale);
        let App { fb, neo_fonts, .. } = self;
        neo::glow_round_rect(
            fb,
            px0,
            py0,
            pw,
            ph,
            8.0 * scale,
            nc::BG_DEEP,
            0.6,
            (10.0 * scale) as usize,
        );
        neo::fill_round_rect(fb, px0, py0, pw, ph, 8.0 * scale, 0x060C18, 0.98);
        neo::stroke_round_rect(fb, px0, py0, pw, ph, 8.0 * scale, nc::CYAN, 0.2, 1.0);
        neo_fonts.draw(
            fb,
            Face::Ui,
            "PORT FORWARDING",
            px0 + si(14.0, scale),
            py0 + si(22.0, scale),
            10.0 * scale,
            nc::TEXT_SUB,
        );
        hline(
            fb,
            px0 + si(12.0, scale),
            py0 + head - si(6.0, scale),
            pw - si(24.0, scale),
            nc::BG,
            0.12,
        );
        let mut y = py0 + head;
        if rows.is_empty() {
            neo_fonts.draw(
                fb,
                Face::Ui,
                "No active forwards",
                px0 + si(16.0, scale),
                y + si(14.0, scale),
                11.0 * scale,
                nc::TEXT_DIM,
            );
        } else {
            for (src, tgt, ok) in &rows {
                let cy = y + rowh / 2;
                let col = if *ok { nc::GREEN } else { nc::RED };
                neo::glow_dot(fb, px0 + si(18.0, scale), cy, 3.0 * scale, col, 5.0 * scale);
                let x2 = neo_fonts.draw(
                    fb,
                    Face::Mono,
                    src,
                    px0 + si(30.0, scale),
                    cy + si(4.0, scale),
                    10.5 * scale,
                    nc::CYAN,
                );
                let x3 = neo_fonts.draw(
                    fb,
                    Face::Mono,
                    "  →  ",
                    x2,
                    cy + si(4.0, scale),
                    10.5 * scale,
                    nc::TEXT_SUB,
                );
                neo_fonts.draw(
                    fb,
                    Face::Mono,
                    tgt,
                    x3,
                    cy + si(4.0, scale),
                    10.5 * scale,
                    nc::TEXT,
                );
                y += rowh;
            }
        }
    }

    /// 検索バー（Ctrl+Shift+F）。端末上部中央に浮かぶ neon バー。
    pub(super) fn draw_neo_search(&mut self) {
        let scale = self.neo_scale();
        let lay = self.neo_layout();
        let (query, cur, total) = match &self.search {
            Some(s) => (s.query.clone(), s.current, s.matches.len()),
            None => return,
        };
        let t = lay.terminal;
        let bw = si(360.0, scale)
            .min(t.w - si(24.0, scale))
            .max(si(200.0, scale));
        let bh = si(38.0, scale);
        let bx = t.x + (t.w - bw) / 2;
        let by = t.y + lay.term_header() + si(12.0, scale);
        let App { fb, neo_fonts, .. } = self;
        neo::glow_round_rect(
            fb,
            bx,
            by,
            bw,
            bh,
            8.0 * scale,
            nc::BG_DEEP,
            0.55,
            (8.0 * scale) as usize,
        );
        neo::fill_round_rect(fb, bx, by, bw, bh, 8.0 * scale, 0x060C18, 0.98);
        neo::stroke_round_rect(fb, bx, by, bw, bh, 8.0 * scale, nc::CYAN, 0.3, 1.0);
        let cy = by + bh / 2;
        neo_fonts.draw_icon(
            fb,
            icon::SEARCH,
            bx + si(12.0, scale),
            cy - si(7.0, scale),
            14.0 * scale,
            nc::CYAN,
        );
        let tx = bx + si(34.0, scale);
        let base = cy + si(4.0, scale);
        let end = if query.is_empty() {
            neo_fonts.draw(
                fb,
                Face::Ui,
                "Search…",
                tx,
                base,
                12.0 * scale,
                nc::TEXT_DIM,
            )
        } else {
            neo_fonts.draw(fb, Face::Mono, &query, tx, base, 12.0 * scale, nc::TEXT)
        };
        // カーソル
        fb.fill_rect(
            end + si(2.0, scale),
            by + si(9.0, scale),
            1.max(si(1.0, scale)),
            bh - si(18.0, scale),
            nc::CYAN,
        );
        // マッチ件数（右）
        let cnt = if total == 0 {
            "0/0".to_string()
        } else {
            format!("{}/{}", cur + 1, total)
        };
        let cwm = neo_fonts.measure(Face::Mono, &cnt, 11.0 * scale);
        let ccol = if total == 0 { nc::RED } else { nc::TEXT_SUB };
        neo_fonts.draw(
            fb,
            Face::Mono,
            &cnt,
            bx + bw - cwm - si(14.0, scale),
            base,
            11.0 * scale,
            ccol,
        );
    }

    pub(super) fn draw_neo_tab_menu(&mut self) {
        let Some(items) = self.tab_menu_geometry() else {
            return;
        };
        if items.is_empty() {
            return;
        }
        let sc = self.neo_scale();
        let lang = self.lang;
        let (mx, my) = (self.mouse.0 as i32, self.mouse.1 as i32);
        let x0 = items[0].1;
        let y0 = items[0].2;
        let x1 = items[0].3;
        let y1 = items.last().unwrap().4;
        let (mw, mh) = (x1 - x0, y1 - y0);
        let App { fb, neo_fonts, .. } = self;

        // 背後のソフトシャドウ＋角丸パネル＋シアン枠。
        neo::glow_round_rect(
            fb,
            x0,
            y0,
            mw,
            mh,
            6.0 * sc,
            nc::BG_DEEP,
            0.6,
            (8.0 * sc) as usize,
        );
        neo::fill_round_rect(fb, x0, y0, mw, mh, 6.0 * sc, nc::PANEL, 0.98);
        neo::stroke_round_rect(fb, x0, y0, mw, mh, 6.0 * sc, nc::BORDER, 0.22, 1.0);

        for (id, ix0, iy0, ix1, iy1) in &items {
            let (ix0, iy0, ix1, iy1) = (*ix0, *iy0, *ix1, *iy1);
            let ih = iy1 - iy0;
            let hover = mx >= ix0 && mx < ix1 && my >= iy0 && my < iy1;
            let is_close = *id == "close";
            // close の上に区切り線（破壊的操作を分離）。
            if is_close {
                hline(
                    fb,
                    ix0 + si(6.0, sc),
                    iy0,
                    ix1 - ix0 - si(12.0, sc),
                    nc::PANEL,
                    0.12,
                );
            }
            if hover {
                let hl = if is_close { nc::RED } else { nc::CYAN };
                neo::fill_round_rect(
                    fb,
                    ix0 + si(4.0, sc),
                    iy0 + si(2.0, sc),
                    (ix1 - ix0) - si(8.0, sc),
                    ih - si(4.0, sc),
                    4.0 * sc,
                    hl,
                    0.10,
                );
            }
            let ic = match *id {
                "rename" => icon::PENCIL,
                "duplicate" => icon::COPY,
                "sftp" => icon::FOLDER,
                _ => icon::X,
            };
            let col = if is_close {
                if hover {
                    nc::RED
                } else {
                    0xE0_6070
                }
            } else if hover {
                nc::CYAN
            } else {
                nc::TEXT
            };
            neo_fonts.draw_icon(
                fb,
                ic,
                ix0 + si(12.0, sc),
                iy0 + ih / 2 - si(6.0, sc),
                12.0 * sc,
                col,
            );
            let label = tab_menu_label(lang, id);
            neo_fonts.draw(
                fb,
                Face::Ui,
                label,
                ix0 + si(34.0, sc),
                iy0 + ih / 2 + si(4.0, sc),
                11.0 * sc,
                col,
            );
        }
    }

    /// サブタブ（端末/SFTP）のクリック。true=消費。
    fn neo_subtab_click(&mut self, mx: i32, my: i32, sub: &R) -> bool {
        if my < sub.y || my >= sub.y + sub.h {
            return false;
        }
        let scale = self.neo_scale();
        let title = self
            .tabs
            .get(self.active_tab)
            .map(|t| t.title.clone())
            .unwrap_or_default();
        let (tx0, tx1, sx0, sx1) = subtab_ranges(&mut self.neo_fonts, sub, &title, scale);
        if mx >= tx0 && mx < tx1 {
            self.mode = Mode::Terminal;
        } else if mx >= sx0 && mx < sx1 {
            self.mode = Mode::Sftp;
        }
        true
    }

    /// NEO-UI SFTP（mode==Sftp）のマウス処理。
    pub(super) fn neo_sftp_mouse(&mut self, state: ElementState, button: MouseButton) {
        if state != ElementState::Pressed {
            return;
        }
        let (mx, my) = (self.mouse.0 as i32, self.mouse.1 as i32);
        if self.tab_menu.is_some() {
            self.handle_tab_menu_click(button);
            return;
        }
        self.neo_filter_focus = false;
        let scale = self.neo_scale();
        let lay = self.neo_layout();

        // エッジリサイズ
        if button == MouseButton::Left {
            if let Some(dir) = self.edge_resize_dir(mx, my) {
                if let Some(win) = &self.window {
                    let _ = win.drag_resize_window(dir);
                }
                return;
            }
        }
        // タイトルバー
        if in_r(&lay.title, mx, my) {
            if button == MouseButton::Left {
                self.neo_titlebar_press(mx, scale, lay.title.w);
            }
            return;
        }
        // メインタブ帯（接続タブの切替・操作）
        let ts = lay.tabstrip;
        if my >= ts.y && my < ts.y + ts.h && mx >= ts.x {
            self.neo_tabstrip_press(mx, button, &ts);
            return;
        }
        // サブタブ帯
        if let Some(sub) = lay.subtab {
            if my >= sub.y && my < sub.y + sub.h && mx >= sub.x {
                if button == MouseButton::Left {
                    self.neo_subtab_click(mx, my, &sub);
                }
                return;
            }
        }
        // サイドバー（接続追加/切替）
        if button == MouseButton::Left && in_r(&lay.sidebar, mx, my) {
            self.neo_sidebar_click(mx, my, &lay);
            return;
        }
        if button != MouseButton::Left {
            return;
        }
        // SFTP 本体: ツールバー / ペイン
        let sl = neo_sftp_layout(&lay.terminal, scale);
        if my >= sl.toolbar.y && my < sl.toolbar.y + sl.toolbar.h {
            let btns = sftp_toolbar_layout(&mut self.neo_fonts, &sl.toolbar, scale);
            for (id, _, _, _, x0, x1) in btns {
                if mx >= x0 && mx < x1 {
                    self.neo_sftp_toolbar_action(id);
                    return;
                }
            }
            return;
        }
        self.neo_sftp_pane_click(mx, my, &sl);
    }

    /// ツールバーのボタン動作。
    fn neo_sftp_toolbar_action(&mut self, id: &str) {
        use crate::sftpview::{InputKind, Side};
        match id {
            "upload" => self.neo_sftp_transfer(Side::Local),
            "download" => self.neo_sftp_transfer(Side::Remote),
            "refresh" => self.fm_reload_active(),
            "delete" => self.fm_begin_delete(),
            "mkdir" => self.fm_begin_input(InputKind::Mkdir),
            _ => {}
        }
    }

    /// 指定ペインの選択を反対側へ転送（Upload=Local, Download=Remote）。
    /// マークがあれば全マーク、無ければ現在の選択1件。
    fn neo_sftp_transfer(&mut self, from: crate::sftpview::Side) {
        let names = {
            let Some(fm) = self.fm.as_mut() else {
                return;
            };
            fm.active = from;
            fm.pane(from).action_targets()
        };
        self.fm_request_transfer_many(from, names);
    }

    /// ペインのクリック（列見出し=ソート / 行=選択、選択済みディレクトリ再クリックで移動）。
    fn neo_sftp_pane_click(&mut self, mx: i32, my: i32, sl: &SftpLayout) {
        use crate::sftpview::{fm_hit, FmHit, PaneRect, Side};
        let m = self.cell_metrics();
        let (lh, cw) = (m.ch, m.cw);
        let (side, rect) = if mx >= sl.remote.x {
            (Side::Remote, sl.remote)
        } else {
            (Side::Local, sl.local)
        };
        let (scroll, total) = {
            let Some(fm) = self.fm.as_ref() else {
                return;
            };
            let p = fm.pane(side);
            (p.scroll, p.entries.len())
        };
        let pr = PaneRect {
            side,
            x: rect.x,
            y: rect.y,
            w: rect.w,
            bottom: rect.y + rect.h,
        };
        match fm_hit(&pr, lh, cw, scroll, total, mx, my) {
            FmHit::Header(key) => {
                if let Some(fm) = self.fm.as_mut() {
                    fm.active = side;
                    fm.pane_mut(side).set_sort(key);
                }
            }
            FmHit::Row(idx) => {
                // Ctrl+クリックはマークのトグル（ファイルのみ・移動や open はしない）。
                if self.mods.control_key() {
                    if let Some(fm) = self.fm.as_mut() {
                        fm.active = side;
                        fm.pane_mut(side).sel = idx;
                        fm.pane_mut(side).toggle_mark_at(idx);
                    }
                    return;
                }
                let (already, is_dir) = {
                    let Some(fm) = self.fm.as_ref() else {
                        return;
                    };
                    let p = fm.pane(side);
                    let already = fm.active == side && p.sel == idx;
                    let is_dir = p
                        .entries
                        .get(idx)
                        .map(|e| e.is_dir || e.parent)
                        .unwrap_or(false);
                    (already, is_dir)
                };
                if let Some(fm) = self.fm.as_mut() {
                    fm.active = side;
                    fm.pane_mut(side).sel = idx;
                }
                if already && is_dir {
                    self.fm_enter();
                }
            }
            FmHit::None => {
                if let Some(fm) = self.fm.as_mut() {
                    fm.active = side;
                }
            }
        }
    }

    /// グループ内の全ホストへ一括接続（各ホストを別タブで開く）。
    fn neo_connect_group(&mut self, group: &str) {
        let hosts: Vec<Profile> = self
            .launcher
            .model
            .hosts(Some(group))
            .into_iter()
            .cloned()
            .collect();
        for p in hosts {
            self.connect_profile(p);
        }
    }
}

/// 淡いドットグリッド背景（間隔も scale で拡縮）。
fn draw_dot_grid(fb: &mut Framebuffer, scale: f32) {
    let step = si(28.0, scale).max(8);
    let mut y = 0;
    while y < fb.h as i32 {
        let mut x = 0;
        while x < fb.w as i32 {
            fb.put(x, y, nc::TEXT_DIM);
            x += step;
        }
        y += step;
    }
}

fn draw_titlebar(fb: &mut Framebuffer, nf: &mut NeoFonts, lay: &NeoLayout) {
    let sc = lay.scale;
    let t = lay.title;
    let cy = t.y + t.h / 2; // 縦中央
    fb.fill_rect(t.x, t.y, t.w, t.h, nc::PANEL);
    hline(fb, t.x, t.y + t.h - 1, t.w, nc::PANEL, 0.09);
    // ワードマーク
    nf.draw_icon(
        fb,
        icon::TERMINAL,
        si(12.0, sc),
        cy - si(8.0, sc),
        16.0 * sc,
        nc::CYAN,
    );
    let x = nf.draw(
        fb,
        Face::Ui,
        "moterm",
        si(36.0, sc),
        cy + si(5.0, sc),
        14.0 * sc,
        nc::CYAN,
    );
    nf.draw(
        fb,
        Face::Mono,
        concat!("v", env!("CARGO_PKG_VERSION")),
        x + si(8.0, sc),
        cy + si(5.0, sc),
        9.0 * sc,
        nc::TEXT_DIM,
    );
    // 右: 設定＋ウィンドウ操作（最小化/最大化/×）
    let btn = si(46.0, sc);
    nf.draw_icon(
        fb,
        icon::SETTINGS,
        t.w - btn * 3 - si(40.0, sc),
        cy - si(7.0, sc),
        14.0 * sc,
        nc::TEXT_SUB,
    );
    // 各ボタン中央にアイコンを置く
    let bc = |i: i32| t.w - btn * (3 - i) + btn / 2;
    fb.fill_rect(
        bc(0) - si(5.0, sc),
        cy,
        si(10.0, sc),
        1.max(si(1.0, sc)),
        nc::TEXT_SUB,
    );
    neo::stroke_round_rect(
        fb,
        bc(1) - si(5.0, sc),
        cy - si(5.0, sc),
        si(10.0, sc),
        si(10.0, sc),
        1.0,
        nc::TEXT_SUB,
        0.8,
        1.0 * sc,
    );
    nf.draw_icon(
        fb,
        icon::X,
        bc(2) - si(5.0, sc),
        cy - si(5.0, sc),
        10.0 * sc,
        nc::TEXT_SUB,
    );
}

/// サイドバー行（描画とヒットテストで共有）。
enum NeoRow {
    Group {
        name: String,
        label: String,
        count: usize,
    },
    Host {
        profile: Box<Profile>,
    },
}

struct PlacedRow {
    y: i32,
    h: i32,
    row: NeoRow,
}

/// フィルタ空ならグループ＋ホスト（折りたたみ反映）、非空ならフラットな絞り込み一覧。
/// 行高・オフセットは scale で拡縮。
fn sidebar_rows(
    model: &mot_core::launcher::LauncherModel,
    s: R,
    query: &str,
    collapsed: &std::collections::HashSet<String>,
    scale: f32,
) -> Vec<PlacedRow> {
    let group_h = si(22.0, scale);
    let host_h = si(30.0, scale);
    let mut rows = Vec::new();
    let mut y = s.y + si(48.0, scale);
    let bottom = s.y + s.h - si(44.0, scale);
    if !query.trim().is_empty() {
        for p in model.filter(query) {
            if y > bottom {
                break;
            }
            rows.push(PlacedRow {
                y,
                h: host_h,
                row: NeoRow::Host {
                    profile: Box::new(p.clone()),
                },
            });
            y += host_h;
        }
        return rows;
    }
    for g in model.groups() {
        if y > bottom {
            break;
        }
        rows.push(PlacedRow {
            y,
            h: group_h,
            row: NeoRow::Group {
                name: g.name.clone(),
                label: g.label.clone(),
                count: g.count,
            },
        });
        y += group_h;
        if g.name == "All" || collapsed.contains(&g.name) {
            continue;
        }
        for host in model.hosts(Some(&g.name)) {
            if y > bottom {
                break;
            }
            rows.push(PlacedRow {
                y,
                h: host_h,
                row: NeoRow::Host {
                    profile: Box::new(host.clone()),
                },
            });
            y += host_h;
        }
    }
    rows
}

/// 使用率に応じた色（Figma 準拠: 70% 超で赤、50% 超で琥珀、それ以下は基準色）。
fn gauge_color(pct: f32, base: Pixel) -> Pixel {
    if pct > 70.0 {
        nc::RED
    } else if pct > 50.0 {
        nc::AMBER
    } else {
        base
    }
}

/// 使用率バー 1 本。高さ 2px の溝に色を敷き、値の分だけ塗る。
fn draw_metric_bar(fb: &mut Framebuffer, x: i32, y: i32, w: i32, pct: f32, color: Pixel, sc: f32) {
    let h = si(2.0, sc).max(1);
    fb.fill_rect(x, y, w, h, nc::TEXT_DIM);
    let fill = ((w as f32) * (pct.clamp(0.0, 100.0) / 100.0)).round() as i32;
    if fill > 0 {
        fb.fill_rect(x, y, fill, h, color);
    }
}

/// kB を人が読める単位へ（メモリ/ディスクの補助表示用）。
fn human_kb(kb: u64) -> String {
    const UNITS: [(&str, f64); 3] = [
        ("T", 1024.0 * 1024.0 * 1024.0),
        ("G", 1024.0 * 1024.0),
        ("M", 1024.0),
    ];
    let v = kb as f64;
    for (u, div) in UNITS {
        if v >= div {
            return format!("{:.1}{u}", v / div);
        }
    }
    format!("{kb}K")
}

/// 右の情報パネル。アクティブタブのホスト情報とシステムメトリクスを縦に積む。
fn draw_info_panel(fb: &mut Framebuffer, nf: &mut NeoFonts, lay: &NeoLayout, tab: Option<&Tab>) {
    let Some(s) = lay.info else {
        return;
    };
    if s.w <= 0 {
        return;
    }
    let sc = lay.scale;
    fb.fill_rect(s.x, s.y, s.w, s.h, nc::PANEL);
    vline(fb, s.x, s.y, s.h, nc::PANEL, 0.09);

    let pad = si(14.0, sc);
    let x = s.x + pad;
    let inner_w = s.w - pad * 2;

    let Some(tab) = tab else {
        nf.draw(
            fb,
            Face::Ui,
            "No session selected",
            x,
            s.y + si(60.0, sc),
            11.0 * sc,
            nc::TEXT_SUB,
        );
        return;
    };

    // ── ホストヘッダ ───────────────────────────────────────────────
    let mut y = s.y + si(14.0, sc);
    let badge = si(28.0, sc);
    neo::fill_round_rect(fb, x, y, badge, badge, 6.0 * sc, nc::CYAN, 0.07);
    neo::stroke_round_rect(fb, x, y, badge, badge, 6.0 * sc, nc::CYAN, 0.18, 1.0);
    nf.draw_icon(
        fb,
        icon::SERVER,
        x + si(8.0, sc),
        y + si(8.0, sc),
        13.0 * sc,
        nc::CYAN,
    );
    let tx = x + badge + si(8.0, sc);
    nf.draw(
        fb,
        Face::Ui,
        &tab.title,
        tx,
        y + si(12.0, sc),
        12.0 * sc,
        nc::CYAN,
    );
    if let Some(p) = &tab.profile {
        let addr = format!("{}@{}:{}", p.effective_user(), p.host, p.port);
        nf.draw(
            fb,
            Face::Mono,
            &addr,
            tx,
            y + si(24.0, sc),
            9.5 * sc,
            nc::TEXT_SUB,
        );
    }
    y += badge + si(12.0, sc);

    // ステータス（ドット＋大文字ラベル）
    let col = status_color(&tab.status);
    let dot = si(6.0, sc).max(2);
    fb.fill_rect(x, y, dot, dot, col);
    let label = match tab.status {
        TabStatus::Connected => "CONNECTED",
        TabStatus::Connecting => "CONNECTING",
        TabStatus::Failed => "DISCONNECTED",
    };
    nf.draw(
        fb,
        Face::Mono,
        label,
        x + dot + si(6.0, sc),
        y + si(6.0, sc),
        9.5 * sc,
        col,
    );
    y += si(18.0, sc);
    hline(fb, s.x, y, s.w, nc::PANEL, 0.09);
    y += si(12.0, sc);

    // ── SYSTEM ────────────────────────────────────────────────────
    let heading = match (tab.metrics_unavailable, tab.metrics_at) {
        (true, _) => "SYSTEM · UNAVAILABLE".to_string(),
        (false, Some(at)) => format!("SYSTEM · {}s AGO", at.elapsed().as_secs()),
        (false, None) => "SYSTEM · ...".to_string(),
    };
    nf.draw(fb, Face::Ui, &heading, x, y, 9.0 * sc, nc::TEXT_SUB);
    y += si(12.0, sc);

    if let Some(m) = tab.metrics {
        // 取得から間が空いた値は薄く出す（1 分間隔なので「今」ではないことを示す）。
        let stale = tab
            .metrics_at
            .is_some_and(|at| at.elapsed() > std::time::Duration::from_secs(150));
        let rows: [(char, &str, f32, Pixel, String); 3] = [
            (
                icon::CPU,
                "CPU",
                m.cpu_pct,
                gauge_color(m.cpu_pct, nc::CYAN),
                String::new(),
            ),
            (
                icon::ACTIVITY,
                "Memory",
                m.mem.used_pct(),
                gauge_color(m.mem.used_pct(), nc::VIOLET),
                format!(
                    "{} / {}",
                    human_kb(m.mem.used_kb()),
                    human_kb(m.mem.total_kb)
                ),
            ),
            (
                icon::HARD_DRIVE,
                "Disk",
                m.disk.used_pct(),
                gauge_color(m.disk.used_pct(), nc::GREEN),
                format!(
                    "{} / {}",
                    human_kb(m.disk.used_kb),
                    human_kb(m.disk.total_kb)
                ),
            ),
        ];
        for (ic, label, pct, color, sub) in rows {
            let color = if stale { nc::TEXT_SUB } else { color };
            nf.draw_icon(fb, ic, x, y, 9.0 * sc, nc::TEXT_SUB);
            nf.draw(
                fb,
                Face::Ui,
                label,
                x + si(14.0, sc),
                y + si(8.0, sc),
                10.0 * sc,
                nc::TEXT_SUB,
            );
            let val = format!("{pct:.0}%");
            let vw = nf.measure(Face::Mono, &val, 10.5 * sc);
            nf.draw(
                fb,
                Face::Mono,
                &val,
                x + inner_w - vw,
                y + si(8.0, sc),
                10.5 * sc,
                color,
            );
            y += si(15.0, sc);
            draw_metric_bar(fb, x, y, inner_w, pct, color, sc);
            y += si(6.0, sc);
            if !sub.is_empty() {
                let sw = nf.measure(Face::Mono, &sub, 9.0 * sc);
                nf.draw(
                    fb,
                    Face::Mono,
                    &sub,
                    x + inner_w - sw,
                    y + si(9.0, sc),
                    9.0 * sc,
                    nc::TEXT_SUB,
                );
                y += si(12.0, sc);
            }
            y += si(8.0, sc);
        }
    } else {
        let msg = if tab.metrics_unavailable {
            "このホストでは取得できません"
        } else if tab.status == TabStatus::Connected {
            "取得中..."
        } else {
            "未接続"
        };
        nf.draw(
            fb,
            Face::Ui,
            msg,
            x,
            y + si(8.0, sc),
            10.0 * sc,
            nc::TEXT_DIM,
        );
        y += si(20.0, sc);
    }

    y += si(6.0, sc);
    hline(fb, s.x, y, s.w, nc::PANEL, 0.09);
    y += si(12.0, sc);

    // ── AUTHENTICATION ────────────────────────────────────────────
    nf.draw(fb, Face::Ui, "AUTHENTICATION", x, y, 9.0 * sc, nc::TEXT_SUB);
    y += si(14.0, sc);
    if let Some(p) = &tab.profile {
        let (ic, text, color) = match &p.auth {
            mot_core::model::AuthMethod::Publickey { key } => (icon::KEY, key.clone(), nc::AMBER),
            mot_core::model::AuthMethod::Agent => {
                (icon::SHIELD, "ssh-agent".to_string(), nc::GREEN)
            }
            mot_core::model::AuthMethod::Password { .. } => {
                (icon::LOCK, "password".to_string(), nc::TEXT_SUB)
            }
        };
        nf.draw_icon(fb, ic, x, y, 9.0 * sc, color);
        // 鍵パスは長いので右から詰めて末尾（ファイル名側）を残す。
        let avail = inner_w - si(14.0, sc);
        let text = ellipsize_left(nf, &text, avail, 10.0 * sc);
        nf.draw(
            fb,
            Face::Mono,
            &text,
            x + si(14.0, sc),
            y + si(8.0, sc),
            10.0 * sc,
            color,
        );
    }
}

/// 幅に収まらない文字列を先頭側から削り `…path` の形にする（末尾を残す）。
fn ellipsize_left(nf: &mut NeoFonts, text: &str, max_w: i32, px: f32) -> String {
    if nf.measure(Face::Mono, text, px) <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    for skip in 1..chars.len() {
        let s: String = std::iter::once('…')
            .chain(chars[skip..].iter().copied())
            .collect();
        if nf.measure(Face::Mono, &s, px) <= max_w {
            return s;
        }
    }
    "…".to_string()
}

fn draw_sidebar(
    fb: &mut Framebuffer,
    nf: &mut NeoFonts,
    lay: &NeoLayout,
    launcher: &LauncherState,
    collapsed: &std::collections::HashSet<String>,
    filter_focus: bool,
    sidebar_sel: Option<usize>,
) {
    let sc = lay.scale;
    let s = lay.sidebar;
    if s.w <= 0 {
        return;
    }
    fb.fill_rect(s.x, s.y, s.w, s.h, nc::PANEL);
    vline(fb, s.x + s.w - 1, s.y, s.h, nc::PANEL, 0.09);

    // フィルタボックス（フォーカス時はシアン枠＋カーソル）
    let fx = s.x + si(10.0, sc);
    let fy = s.y + si(10.0, sc);
    let fw = s.w - si(20.0, sc);
    let fh = si(26.0, sc);
    neo::fill_round_rect(fb, fx, fy, fw, fh, 4.0 * sc, nc::BG_DEEP, 0.5);
    let (bcol, ba) = if filter_focus {
        (nc::CYAN, 0.4)
    } else {
        (nc::BORDER, 0.09)
    };
    neo::stroke_round_rect(fb, fx, fy, fw, fh, 4.0 * sc, bcol, ba, 1.0);
    nf.draw_icon(
        fb,
        icon::SEARCH,
        fx + si(8.0, sc),
        fy + si(7.0, sc),
        12.0 * sc,
        if filter_focus { nc::CYAN } else { nc::TEXT_SUB },
    );
    let base = fy + fh - si(8.0, sc);
    let tx = fx + si(28.0, sc);
    if launcher.query.is_empty() && !filter_focus {
        nf.draw(
            fb,
            Face::Ui,
            "Filter hosts...",
            tx,
            base,
            11.0 * sc,
            nc::TEXT_SUB,
        );
    } else {
        let end = nf.draw(fb, Face::Ui, &launcher.query, tx, base, 11.0 * sc, nc::TEXT);
        if filter_focus {
            // カーソル（テキスト右端に細い縦棒）
            fb.fill_rect(
                end + si(1.0, sc),
                fy + si(6.0, sc),
                1.max(si(1.0, sc)),
                fh - si(12.0, sc),
                nc::CYAN,
            );
        }
    }

    // グループ＋ホスト（共有レイアウト）
    let rows = sidebar_rows(&launcher.model, s, &launcher.query, collapsed, sc);
    // 選択モードのカーソル行（行数変動に備えクランプ）。
    let sel = sidebar_sel
        .filter(|_| !rows.is_empty())
        .map(|i| i.min(rows.len() - 1));
    for (i, pr) in rows.iter().enumerate() {
        let y = pr.y;
        // キーボード選択中の行に淡いシアンの背景ハイライトを敷く。
        if Some(i) == sel {
            neo::fill_round_rect(
                fb,
                s.x + si(4.0, sc),
                pr.y - si(1.0, sc),
                s.w - si(8.0, sc),
                pr.h,
                4.0 * sc,
                nc::CYAN,
                0.12,
            );
        }
        match &pr.row {
            NeoRow::Group { name, label, count } => {
                nf.draw_icon(
                    fb,
                    group_icon(name),
                    s.x + si(10.0, sc),
                    y,
                    10.0 * sc,
                    nc::TEXT_DIM,
                );
                let text = format!("{}  ({})", label.to_uppercase(), count);
                nf.draw(
                    fb,
                    Face::Ui,
                    &text,
                    s.x + si(26.0, sc),
                    y + si(10.0, sc),
                    10.0 * sc,
                    nc::TEXT_SUB,
                );
                // グループ一括接続アイコン（"All" 以外・ホストがある時）。クリックで全接続。
                if name != "All" && *count > 0 {
                    nf.draw_icon(
                        fb,
                        icon::ZAP,
                        s.x + s.w - si(40.0, sc),
                        y,
                        9.0 * sc,
                        nc::CYAN,
                    );
                }
                let chev = if collapsed.contains(name) {
                    icon::CHEVRON_RIGHT
                } else {
                    icon::CHEVRON_DOWN
                };
                nf.draw_icon(
                    fb,
                    chev,
                    s.x + s.w - si(20.0, sc),
                    y,
                    9.0 * sc,
                    nc::TEXT_DIM,
                );
            }
            NeoRow::Host { profile } => {
                neo::glow_dot(
                    fb,
                    s.x + si(18.0, sc),
                    y + si(9.0, sc),
                    3.0 * sc,
                    nc::TEXT_DIM,
                    4.0 * sc,
                );
                nf.draw(
                    fb,
                    Face::Ui,
                    &profile.name,
                    s.x + si(30.0, sc),
                    y + si(8.0, sc),
                    11.0 * sc,
                    nc::TEXT,
                );
                nf.draw(
                    fb,
                    Face::Mono,
                    &format!("{}@{}", profile.effective_user(), profile.host),
                    s.x + si(30.0, sc),
                    y + si(21.0, sc),
                    9.0 * sc,
                    nc::TEXT_DIM,
                );
            }
        }
    }

    // NEW CONNECTION ボタン
    let bh = si(28.0, sc);
    let by = s.y + s.h - si(38.0, sc);
    let bx = s.x + si(10.0, sc);
    let bw = s.w - si(20.0, sc);
    hline(fb, s.x, by - si(8.0, sc), s.w, nc::PANEL, 0.09);
    neo::fill_round_rect(fb, bx, by, bw, bh, 6.0 * sc, nc::CYAN, 0.05);
    neo::stroke_round_rect(fb, bx, by, bw, bh, 6.0 * sc, nc::CYAN, 0.2, 1.0);
    let btext = "NEW CONNECTION";
    let btw = nf.measure(Face::Ui, btext, 10.5 * sc);
    nf.draw_icon(
        fb,
        icon::PLUS,
        bx + bw / 2 - btw / 2 - si(18.0, sc),
        by + si(7.0, sc),
        12.0 * sc,
        nc::CYAN,
    );
    nf.draw(
        fb,
        Face::Ui,
        btext,
        bx + bw / 2 - btw / 2,
        by + bh - si(9.0, sc),
        10.5 * sc,
        nc::CYAN,
    );
}

fn group_icon(name: &str) -> char {
    let n = name.to_lowercase();
    if n.contains("db") || n.contains("data") {
        icon::DATABASE
    } else if n.contains("dev") || n.contains("web") || n.contains("stg") || n.contains("検証") {
        icon::GLOBE
    } else {
        icon::SERVER
    }
}

fn draw_tabstrip(
    fb: &mut Framebuffer,
    nf: &mut NeoFonts,
    lay: &NeoLayout,
    tabs: &[Tab],
    active: usize,
    phase: f32,
    tab_rename: Option<&(usize, String)>,
) {
    let sc = lay.scale;
    let t = lay.tabstrip;
    let cy = t.y + t.h / 2;
    fb.fill_rect(t.x, t.y, t.w, t.h, blend(nc::BG, nc::PANEL, 0.7));
    hline(fb, t.x, t.y + t.h - 1, t.w, nc::BG, 0.09);
    let mut x = t.x + si(4.0, sc);
    for (i, tab) in tabs.iter().enumerate() {
        let is_active = i == active;
        // rename 中はバッファをラベルに（幅算出も rename 文字列で）
        let renaming = tab_rename
            .filter(|(ri, _)| *ri == i)
            .map(|(_, s)| s.clone());
        let label = renaming.clone().unwrap_or_else(|| tab.title.clone());
        let lw = nf.measure(Face::Mono, &label, 11.0 * sc);
        let tw = lw + si(44.0, sc);
        if is_active {
            fb.fill_rect(x, t.y, tw, t.h, blend(nc::BG, nc::CYAN, 0.06));
            fb.fill_rect(
                x,
                t.y + t.h - si(2.0, sc).max(1),
                tw,
                si(2.0, sc).max(1),
                nc::CYAN,
            );
        }
        status_dot(fb, x + si(12.0, sc), cy, &tab.status, phase, sc);
        let col = if renaming.is_some() {
            nc::TEXT
        } else if is_active {
            nc::CYAN
        } else {
            nc::TEXT_SUB
        };
        let shown = if renaming.is_some() {
            format!("{label}_")
        } else {
            label.clone()
        };
        nf.draw(
            fb,
            Face::Mono,
            &shown,
            x + si(22.0, sc),
            cy + si(4.0, sc),
            11.0 * sc,
            col,
        );
        // × は全タブに（アクティブは明るめ）
        let xcol = if is_active {
            nc::TEXT_SUB
        } else {
            nc::TEXT_DIM
        };
        nf.draw_icon(
            fb,
            icon::X,
            x + tw - si(16.0, sc),
            cy - si(5.0, sc),
            9.0 * sc,
            xcol,
        );
        x += tw + 1;
    }
    // ＋ 追加（装飾）
    nf.draw_icon(
        fb,
        icon::PLUS,
        x + si(8.0, sc),
        cy - si(6.0, sc),
        11.0 * sc,
        nc::TEXT_SUB,
    );
}

/// ブロードキャスト入力中（Ctrl+Shift+B）の警告表示。端末領域の上端に赤帯＋ラベル。
fn draw_broadcast_bar(fb: &mut Framebuffer, nf: &mut NeoFonts, lay: &NeoLayout, scale: f32) {
    let t = lay.terminal;
    let sc = scale;
    let bar_h = si(3.0, sc).max(2);
    // 上端の赤帯（全幅）。
    fb.fill_rect(t.x, t.y, t.w, bar_h, nc::RED);
    // 右寄せの「BROADCAST」ラベル（赤地の pill）。
    let label = "BROADCAST";
    let lw = nf.measure(Face::Ui, label, 10.0 * sc) + si(20.0, sc);
    let lh = si(17.0, sc);
    let lx = t.x + t.w - lw - si(8.0, sc);
    let ly = t.y + bar_h + si(4.0, sc);
    neo::fill_round_rect(fb, lx, ly, lw, lh, 3.0 * sc, nc::RED, 0.16);
    neo::stroke_round_rect(fb, lx, ly, lw, lh, 3.0 * sc, nc::RED, 0.5, 1.0);
    nf.draw(
        fb,
        Face::Ui,
        label,
        lx + si(10.0, sc),
        ly + si(4.0, sc),
        10.0 * sc,
        nc::RED,
    );
}

/// フォーカスペインの端末カーソルのピクセル位置（ウィンドウ座標・セル左上）を返す。
/// draw_terminal / draw_screen と同じ ox/oy 計算。スクロール中・カーソル不可視・
/// セッション無しは None。preedit 描画と IME 候補窓の位置指定で共用する。
fn neo_terminal_cursor_px(
    lay: &NeoLayout,
    m: &crate::termview::CellMetrics,
    scale: f32,
    tab: Option<&Tab>,
) -> Option<(i32, i32)> {
    let tab = tab?;
    let pane = tab.panes.get(&tab.focus)?;
    if pane.scroll != 0 || !pane.terminal.screen.cursor_visible {
        return None;
    }
    let t = lay.terminal;
    let cur = pane.terminal.screen.cursor;
    let cx = t.x + si(6.0, scale) + cur.col as i32 * m.cw;
    let cy = t.y + lay.term_header() + 2 + cur.row as i32 * m.ch;
    Some((cx, cy))
}

/// IME 変換中テキスト（preedit）をカーソル位置へインライン描画する。
/// 変換中と分かるよう選択色の背景帯＋アクセント下線を付ける。
fn draw_preedit(
    fb: &mut Framebuffer,
    font: &mut crate::font::FontManager,
    theme: &crate::theme::Theme,
    m: &crate::termview::CellMetrics,
    cx: i32,
    cy: i32,
    text: &str,
) {
    use unicode_width::UnicodeWidthChar;
    let cols: i32 = text.chars().map(|c| c.width().unwrap_or(1) as i32).sum();
    let w = (cols * m.cw).max(m.cw);
    fb.fill_rect(cx, cy, w, m.ch, theme.selection);
    let mut px = cx;
    for ch in text.chars() {
        if ch != ' ' {
            let g = font.glyph(ch, m.px, false);
            fb.blit_coverage(px + g.left, cy + m.baseline - g.top, &g.bitmap, theme.fg);
        }
        px += ch.width().unwrap_or(1) as i32 * m.cw;
    }
    // 変換中を示す下線（アクセント色）。
    fb.fill_rect(cx, cy + m.ch - 2, w, 2, theme.ui_accent);
}

#[allow(clippy::too_many_arguments)]
fn draw_terminal(
    fb: &mut Framebuffer,
    nf: &mut NeoFonts,
    font: &mut crate::font::FontManager,
    theme: &crate::theme::Theme,
    m: &crate::termview::CellMetrics,
    lay: &NeoLayout,
    tab: Option<&Tab>,
    sel: Option<&crate::selection::Selection>,
    search: Option<crate::termview::SearchHl>,
) {
    let sc = lay.scale;
    let t = lay.terminal;
    let hh = lay.term_header();
    fb.fill_rect(t.x, t.y, t.w, t.h, theme.bg);
    // ペインヘッダ
    fb.fill_rect(t.x, t.y, t.w, hh, blend(nc::BG_DEEP, 0x000000, 0.35));
    hline(fb, t.x, t.y + hh, t.w, nc::BG_DEEP, 0.06);
    nf.draw_icon(
        fb,
        icon::TERMINAL,
        t.x + si(12.0, sc),
        t.y + hh / 2 - si(6.0, sc),
        12.0 * sc,
        blend(nc::BG, nc::CYAN, 0.35),
    );
    let label = tab.map(|t| t.title.as_str()).unwrap_or("no session");
    nf.draw(
        fb,
        Face::Mono,
        label,
        t.x + si(32.0, sc),
        t.y + hh / 2 + si(4.0, sc),
        10.0 * sc,
        blend(nc::BG, nc::CYAN, 0.35),
    );

    // 本文: アクティブタブのフォーカスペインの端末画面（本文色はホスト由来・端末フォント）。
    let content_y = t.y + hh + 2;
    if let Some(pane) = tab.and_then(|tb| tb.panes.get(&tb.focus)) {
        crate::termview::draw_screen(
            fb,
            font,
            theme,
            &pane.terminal.screen,
            m,
            t.x + si(6.0, sc),
            content_y,
            pane.scroll,
            sel,
            search,
        );
        // スクロールバー（履歴がある時）。右端にサムを描画。
        let track_h = (t.h - hh - 2).max(1);
        let total = pane.terminal.screen.scrollback_len() + pane.terminal.screen.rows();
        if let Some((thumb_top, thumb_h)) =
            crate::termview::scrollbar_thumb(track_h, total, pane.rows as usize, pane.scroll)
        {
            let bx = t.x + t.w - si(4.0, sc);
            fb.fill_rect(
                bx,
                content_y + thumb_top,
                si(3.0, sc).max(2),
                thumb_h,
                theme.ui_dim,
            );
        }
    } else {
        nf.draw(
            fb,
            Face::Ui,
            "Select a host from the sidebar to connect.",
            t.x + si(18.0, sc),
            content_y + si(20.0, sc),
            13.0 * sc,
            nc::TEXT_SUB,
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────
// NEO-UI SFTP（接続画面に埋め込む2ペインFM）。既存 FileManager/操作を再利用し、
// 描画のみ neon 化。ペインのヒットは classic の fm_hit と同じ幾何に合わせる。
// ─────────────────────────────────────────────────────────────────────────

/// SFTP 中央領域の内訳。
struct SftpLayout {
    toolbar: R,
    local: R,
    remote: R,
    queue: R,
    divider_x: i32,
}

fn neo_sftp_layout(t: &R, scale: f32) -> SftpLayout {
    let toolbar_h = si(34.0, scale);
    let queue_h = si(82.0, scale);
    let panes_y = t.y + toolbar_h;
    let panes_h = (t.h - toolbar_h - queue_h).max(si(40.0, scale));
    let mid = t.x + t.w / 2;
    let qy = panes_y + panes_h;
    SftpLayout {
        toolbar: R {
            x: t.x,
            y: t.y,
            w: t.w,
            h: toolbar_h,
        },
        local: R {
            x: t.x,
            y: panes_y,
            w: mid - t.x - 1,
            h: panes_h,
        },
        remote: R {
            x: mid + 1,
            y: panes_y,
            w: t.x + t.w - mid - 1,
            h: panes_h,
        },
        queue: R {
            x: t.x,
            y: qy,
            w: t.w,
            h: (t.y + t.h - qy).max(0),
        },
        divider_x: mid,
    }
}

/// ツールバーのボタン (id, icon, label, color, x0, x1)。描画とヒットで共有。
fn sftp_toolbar_layout(
    nf: &mut NeoFonts,
    tb: &R,
    scale: f32,
) -> Vec<(&'static str, char, &'static str, Pixel, i32, i32)> {
    let items: [(&str, char, &str, Pixel); 5] = [
        ("upload", icon::UPLOAD, "Upload", nc::CYAN),
        ("download", icon::DOWNLOAD, "Download", nc::VIOLET),
        ("refresh", icon::ROTATE, "Refresh", nc::TEXT_SUB),
        ("delete", icon::TRASH, "Delete", nc::RED),
        ("mkdir", icon::FOLDER_PLUS, "New Folder", nc::TEXT_SUB),
    ];
    let mut x = tb.x + si(10.0, scale);
    let mut out = Vec::with_capacity(items.len());
    for (id, ic, label, col) in items {
        let bw = si(20.0, scale) + nf.measure(Face::Ui, label, 10.5 * scale) + si(16.0, scale);
        out.push((id, ic, label, col, x, x + bw));
        x += bw + si(5.0, scale);
    }
    out
}

/// 端末/SFTP サブタブの x 範囲 (term_x0, term_x1, sftp_x0, sftp_x1)。描画とヒットで共有。
fn subtab_ranges(nf: &mut NeoFonts, r: &R, tab_title: &str, scale: f32) -> (i32, i32, i32, i32) {
    let tx0 = r.x + si(8.0, scale);
    let tw = nf.measure(Face::Mono, tab_title, 10.5 * scale) + si(24.0, scale);
    let tx1 = tx0 + tw;
    let sx0 = tx1;
    let sw = si(26.0, scale) + nf.measure(Face::Ui, "SFTP", 10.5 * scale) + si(16.0, scale);
    (tx0, tx1, sx0, sx0 + sw)
}

fn draw_subtab_strip(
    fb: &mut Framebuffer,
    nf: &mut NeoFonts,
    r: &R,
    tab_title: &str,
    sftp_view: bool,
    scale: f32,
) {
    fb.fill_rect(r.x, r.y, r.w, r.h, blend(nc::BG_DEEP, 0x000000, 0.2));
    hline(fb, r.x, r.y + r.h - 1, r.w, nc::BG, 0.09);
    let cy = r.y + r.h / 2;
    let ul = si(2.0, scale).max(1);
    let (tx0, tx1, sx0, sx1) = subtab_ranges(nf, r, tab_title, scale);
    // 端末サブタブ
    if !sftp_view {
        fb.fill_rect(tx0, r.y, tx1 - tx0, r.h, blend(nc::BG, nc::CYAN, 0.05));
        fb.fill_rect(tx0, r.y + r.h - ul, tx1 - tx0, ul, nc::CYAN);
    }
    let tcol = if !sftp_view { nc::CYAN } else { nc::TEXT_SUB };
    nf.draw(
        fb,
        Face::Mono,
        tab_title,
        tx0 + si(12.0, scale),
        cy + si(4.0, scale),
        10.5 * scale,
        tcol,
    );
    // SFTP サブタブ
    if sftp_view {
        fb.fill_rect(sx0, r.y, sx1 - sx0, r.h, blend(nc::BG, nc::CYAN, 0.05));
        fb.fill_rect(sx0, r.y + r.h - ul, sx1 - sx0, ul, nc::CYAN);
    }
    let scol = if sftp_view { nc::CYAN } else { nc::TEXT_SUB };
    nf.draw_icon(
        fb,
        icon::UP_DOWN,
        sx0 + si(10.0, scale),
        cy - si(6.0, scale),
        11.0 * scale,
        scol,
    );
    nf.draw(
        fb,
        Face::Ui,
        "SFTP",
        sx0 + si(26.0, scale),
        cy + si(4.0, scale),
        10.5 * scale,
        scol,
    );
}

fn draw_sftp_toolbar(fb: &mut Framebuffer, nf: &mut NeoFonts, tb: &R, scale: f32) {
    fb.fill_rect(tb.x, tb.y, tb.w, tb.h, blend(nc::BG_DEEP, 0x000000, 0.4));
    hline(fb, tb.x, tb.y + tb.h - 1, tb.w, nc::BG, 0.09);
    let cy = tb.y + tb.h / 2;
    let btn_y = tb.y + si(5.0, scale);
    let btn_h = tb.h - si(10.0, scale);
    for (_, ic, label, col, x0, x1) in sftp_toolbar_layout(nf, tb, scale) {
        neo::fill_round_rect(fb, x0, btn_y, x1 - x0, btn_h, 3.0 * scale, nc::PANEL, 0.5);
        neo::stroke_round_rect(
            fb,
            x0,
            btn_y,
            x1 - x0,
            btn_h,
            3.0 * scale,
            nc::BORDER,
            0.12,
            1.0,
        );
        nf.draw_icon(
            fb,
            ic,
            x0 + si(7.0, scale),
            cy - si(6.0, scale),
            11.0 * scale,
            col,
        );
        nf.draw(
            fb,
            Face::Ui,
            label,
            x0 + si(22.0, scale),
            cy + si(4.0, scale),
            10.5 * scale,
            col,
        );
    }
    let rl = "SFTP · SSH";
    let rw = nf.measure(Face::Mono, rl, 10.0 * scale);
    nf.draw(
        fb,
        Face::Mono,
        rl,
        tb.x + tb.w - rw - si(10.0, scale),
        cy + si(4.0, scale),
        10.0 * scale,
        nc::TEXT_SUB,
    );
}

/// 末尾優先で文字列を max 文字に丸める（先頭に … を付ける）。
fn trunc_tail(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    let tail: String = chars[chars.len() - max.saturating_sub(1)..]
        .iter()
        .collect();
    format!("…{tail}")
}

#[allow(clippy::too_many_arguments)]
fn draw_neo_pane(
    fb: &mut Framebuffer,
    font: &mut crate::font::FontManager,
    nf: &mut NeoFonts,
    m: &crate::termview::CellMetrics,
    pane: &mut crate::sftpview::PaneFm,
    r: R,
    is_active: bool,
    accent: Pixel,
    scale: f32,
    lang: Lang,
) {
    use crate::sftpview::{
        col_bounds, col_label, fm_header_row_y, fm_list_top, fmt_mtime, fmt_size, scroll_follow,
        Side, SortKey,
    };
    let (x, y, w) = (r.x, r.y, r.w);
    let bottom = r.y + r.h;
    let (lh, cw, px) = (m.ch, m.cw, m.px);
    let bg = if is_active { nc::PANEL } else { nc::BG_DEEP };
    neo::fill_round_rect(fb, x, y, w, r.h, 5.0 * scale, bg, 0.7);
    let (bcol, ba) = if is_active {
        (accent, 0.5)
    } else {
        (nc::BORDER, 0.12)
    };
    neo::stroke_round_rect(fb, x, y, w, r.h, 5.0 * scale, bcol, ba, 1.0);

    // ヘッダ: ラベル + パス + N items
    let label = match pane.side {
        Side::Local => "LOCAL",
        Side::Remote => "REMOTE",
    };
    let base1 = y + lh - si(3.0, scale);
    let lx = nf.draw(
        fb,
        Face::Ui,
        label,
        x + si(10.0, scale),
        base1,
        9.5 * scale,
        accent,
    );
    let path = trunc_tail(&pane.cwd, 30);
    nf.draw(
        fb,
        Face::Mono,
        &path,
        lx + si(8.0, scale),
        base1,
        10.0 * scale,
        blend(bg, accent, 0.75),
    );
    let n_items = pane.entries.iter().filter(|e| !e.parent).count();
    let n_marked = pane.marked.len();
    let items = if n_marked > 0 {
        format!("{n_marked} marked / {n_items}")
    } else {
        n_items.to_string()
    };
    let icol = if n_marked > 0 {
        nc::AMBER
    } else {
        nc::TEXT_SUB
    };
    let iw = nf.measure(Face::Mono, &items, 9.5 * scale);
    nf.draw(
        fb,
        Face::Mono,
        &items,
        x + w - iw - si(10.0, scale),
        base1,
        9.5 * scale,
        icol,
    );

    // 列見出し（クリックでソート、fm_hit と同じ x）
    let (size_left, date_left, _right) = col_bounds(x, w, cw);
    let hdr_y = fm_header_row_y(y, lh);
    let hdr_base = hdr_y + lh - si(4.0, scale);
    let arrow = |k: SortKey| -> &'static str {
        if pane.sort_key == k {
            if pane.sort_asc {
                " ^"
            } else {
                " v"
            }
        } else {
            ""
        }
    };
    let hcol = |k: SortKey| {
        if pane.sort_key == k {
            accent
        } else {
            nc::TEXT_SUB
        }
    };
    nf.draw(
        fb,
        Face::Ui,
        &format!("{}{}", col_label(lang, SortKey::Name), arrow(SortKey::Name)),
        x + si(10.0, scale),
        hdr_base,
        9.5 * scale,
        hcol(SortKey::Name),
    );
    nf.draw(
        fb,
        Face::Ui,
        &format!("{}{}", col_label(lang, SortKey::Size), arrow(SortKey::Size)),
        size_left,
        hdr_base,
        9.5 * scale,
        hcol(SortKey::Size),
    );
    nf.draw(
        fb,
        Face::Ui,
        &format!("{}{}", col_label(lang, SortKey::Date), arrow(SortKey::Date)),
        date_left,
        hdr_base,
        9.5 * scale,
        hcol(SortKey::Date),
    );
    hline(
        fb,
        x + si(6.0, scale),
        hdr_y + lh,
        w - si(12.0, scale),
        nc::BG,
        0.09,
    );

    // 一覧
    let list_top = fm_list_top(y, lh);
    let total = pane.entries.len();
    let visible = ((bottom - list_top) / lh).max(1) as usize;
    pane.scroll = scroll_follow(pane.sel, pane.scroll, visible, total);
    if pane.entries.is_empty() {
        if let Some(note) = &pane.note {
            nf.draw(
                fb,
                Face::Mono,
                note,
                x + si(10.0, scale),
                list_top + lh - si(4.0, scale),
                10.0 * scale,
                nc::TEXT_DIM,
            );
        }
    }
    let name_x = x + si(24.0, scale);
    let name_max = ((size_left - name_x) / cw).max(1) as usize;
    for row in 0..visible {
        let idx = pane.scroll + row;
        if idx >= total {
            break;
        }
        let e = &pane.entries[idx];
        let ry = list_top + row as i32 * lh;
        let marked = pane.marked.contains(&e.name);
        if marked {
            // マーク行はアンバーで薄く塗り、左に太バーを出す（選択と別色で複数を可視化）。
            fb.fill_rect(
                x + si(4.0, scale),
                ry,
                w - si(8.0, scale),
                lh,
                blend(bg, nc::AMBER, 0.12),
            );
        }
        if idx == pane.sel {
            let a = if is_active { 0.14 } else { 0.06 };
            fb.fill_rect(
                x + si(4.0, scale),
                ry,
                w - si(8.0, scale),
                lh,
                blend(bg, accent, a),
            );
            fb.fill_rect(x + si(2.0, scale), ry, si(2.0, scale).max(1), lh, accent);
        }
        if marked {
            fb.fill_rect(x + si(2.0, scale), ry, si(2.0, scale).max(1), lh, nc::AMBER);
        }
        let (ic, iccol) = if e.parent {
            (icon::CORNER_UP, nc::TEXT_SUB)
        } else if e.is_dir {
            (icon::FOLDER, accent)
        } else if marked {
            (icon::FILE, nc::AMBER)
        } else {
            (icon::FILE, nc::TEXT_SUB)
        };
        nf.draw_icon(
            fb,
            ic,
            x + si(8.0, scale),
            ry + (lh - si(11.0, scale)) / 2,
            11.0 * scale,
            iccol,
        );
        let name = elide_cols(&e.name, name_max);
        let ncol = if e.parent {
            nc::TEXT_SUB
        } else if e.is_dir {
            accent
        } else if marked {
            nc::AMBER
        } else if is_active {
            nc::TEXT
        } else {
            nc::TEXT_SUB
        };
        fb.draw_text(font, &name, name_x, ry + lh - si(4.0, scale), px, ncol);
        let s = if e.is_dir {
            "-".to_string()
        } else {
            fmt_size(e.size)
        };
        let sw = s.chars().count() as i32 * cw;
        fb.draw_text(
            font,
            &s,
            date_left - sw - si(8.0, scale),
            ry + lh - si(4.0, scale),
            px,
            nc::TEXT_SUB,
        );
        if e.mtime > 0 {
            fb.draw_text(
                font,
                &fmt_mtime(e.mtime),
                date_left,
                ry + lh - si(4.0, scale),
                px,
                nc::TEXT_DIM,
            );
        }
    }
}

/// 列幅（cols）に収まるよう末尾を … で丸める。
fn elide_cols(s: &str, max_cols: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_cols {
        return s.to_string();
    }
    if max_cols <= 1 {
        return "…".to_string();
    }
    let head: String = chars[..max_cols - 1].iter().collect();
    format!("{head}…")
}

#[allow(clippy::too_many_arguments)]
fn draw_neo_sftp(
    fb: &mut Framebuffer,
    nf: &mut NeoFonts,
    font: &mut crate::font::FontManager,
    m: &crate::termview::CellMetrics,
    lay: &NeoLayout,
    fm: &mut crate::sftpview::FileManager,
    scale: f32,
    lang: Lang,
) {
    use crate::sftpview::Side;
    let t = lay.terminal;
    let sl = neo_sftp_layout(&t, scale);
    fb.fill_rect(t.x, t.y, t.w, t.h, nc::BG_DEEP);
    draw_sftp_toolbar(fb, nf, &sl.toolbar, scale);
    let active = fm.active;
    draw_neo_pane(
        fb,
        font,
        nf,
        m,
        &mut fm.local,
        sl.local,
        active == Side::Local,
        nc::VIOLET,
        scale,
        lang,
    );
    draw_neo_pane(
        fb,
        font,
        nf,
        m,
        &mut fm.remote,
        sl.remote,
        active == Side::Remote,
        nc::CYAN,
        scale,
        lang,
    );
    // 中央の縦グラデ区切り
    fb.fill_rect(
        sl.divider_x,
        sl.local.y,
        1,
        sl.local.h,
        blend(nc::BG, nc::CYAN, 0.18),
    );
    draw_sftp_queue(fb, nf, &sl.queue, fm, scale);
}

fn draw_sftp_queue(
    fb: &mut Framebuffer,
    nf: &mut NeoFonts,
    r: &R,
    fm: &crate::sftpview::FileManager,
    scale: f32,
) {
    use crate::sftpview::{fmt_size, InputKind};
    fb.fill_rect(r.x, r.y, r.w, r.h, blend(nc::BG_DEEP, 0x000000, 0.45));
    hline(fb, r.x, r.y, r.w, nc::BG, 0.09);
    nf.draw(
        fb,
        Face::Ui,
        "TRANSFER QUEUE",
        r.x + si(12.0, scale),
        r.y + si(14.0, scale),
        9.0 * scale,
        nc::TEXT_SUB,
    );
    let row_y = r.y + si(24.0, scale);
    let tx = r.x + si(12.0, scale);
    let tb = row_y + si(13.0, scale);
    if let Some(dp) = &fm.drive_picker {
        // ドライブ選択オーバーレイ（Windows）。選択中を [] とアクセント色で強調し、
        // 横幅に収まらなければ選択中が必ず見えるよう左を詰めて描画する。
        let mut x = nf.draw(fb, Face::Ui, "DRIVE  ", tx, tb, 10.0 * scale, nc::TEXT_SUB);
        let dpx = 11.0 * scale;
        let end = r.x + r.w - si(12.0, scale);
        // "C:\\" → "C:"。選択中は "[C:]"、非選択は " C: "。
        let texts: Vec<String> = dp
            .drives
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let l = d.trim_end_matches('\\');
                if i == dp.sel {
                    format!("[{l}]")
                } else {
                    format!(" {l} ")
                }
            })
            .collect();
        let widths: Vec<i32> = texts
            .iter()
            .map(|s| nf.measure(Face::Mono, s, dpx))
            .collect();
        let mut start = 0usize;
        while start < dp.sel {
            let used: i32 = widths[start..=dp.sel].iter().sum();
            if x + used <= end {
                break;
            }
            start += 1;
        }
        for i in start..texts.len() {
            if x + widths[i] > end {
                break;
            }
            let color = if i == dp.sel {
                nc::VIOLET
            } else {
                nc::TEXT_DIM
            };
            x = nf.draw(fb, Face::Mono, &texts[i], x, tb, dpx, color);
        }
        return;
    }
    if let Some(inp) = &fm.input {
        let label = match inp.kind {
            InputKind::Mkdir => "New folder",
            InputKind::Rename => "Rename",
        };
        nf.draw(
            fb,
            Face::Mono,
            &format!("{label}: {}_", inp.buffer),
            tx,
            tb,
            11.0 * scale,
            nc::CYAN,
        );
        return;
    }
    if let Some(cf) = &fm.confirm {
        nf.draw(
            fb,
            Face::Mono,
            &format!("{}  (y/n)", cf.message),
            tx,
            tb,
            11.0 * scale,
            nc::AMBER,
        );
        return;
    }
    if let Some(p) = &fm.progress {
        let pct = (p.done * 100).checked_div(p.total).unwrap_or(0).min(100);
        draw_queue_row(
            fb,
            nf,
            r,
            row_y,
            &p.name,
            &fmt_size(p.total),
            pct,
            false,
            scale,
        );
    } else if let Some(st) = &fm.status {
        nf.draw(fb, Face::Mono, st, tx, tb, 10.5 * scale, nc::GREEN);
    } else {
        nf.draw(
            fb,
            Face::Ui,
            "No active transfers",
            tx,
            tb,
            10.0 * scale,
            nc::TEXT_DIM,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_queue_row(
    fb: &mut Framebuffer,
    nf: &mut NeoFonts,
    r: &R,
    ry: i32,
    name: &str,
    size: &str,
    pct: u64,
    done: bool,
    scale: f32,
) {
    let cy = ry + si(8.0, scale);
    let col = if done { nc::GREEN } else { nc::AMBER };
    neo::glow_dot(fb, r.x + si(16.0, scale), cy, 3.0 * scale, col, 5.0 * scale);
    nf.draw(
        fb,
        Face::Mono,
        name,
        r.x + si(28.0, scale),
        cy + si(4.0, scale),
        10.5 * scale,
        nc::TEXT,
    );
    let pct_s = format!("{pct}%");
    let px1 = r.x + r.w - si(12.0, scale);
    let pw = nf.measure(Face::Mono, &pct_s, 10.0 * scale);
    nf.draw(
        fb,
        Face::Mono,
        &pct_s,
        px1 - pw,
        cy + si(4.0, scale),
        10.0 * scale,
        col,
    );
    let bar_w = si(90.0, scale);
    let bar_x = px1 - pw - si(8.0, scale) - bar_w;
    let bar_h = si(3.0, scale).max(2);
    fb.fill_rect(bar_x, cy - bar_h / 2, bar_w, bar_h, nc::TEXT_DIM);
    let fillw = (bar_w as f32 * (pct as f32 / 100.0)) as i32;
    if fillw > 0 {
        fb.fill_rect(bar_x, cy - bar_h / 2, fillw, bar_h, col);
    }
    let sw = nf.measure(Face::Mono, size, 10.0 * scale);
    nf.draw(
        fb,
        Face::Mono,
        size,
        bar_x - si(8.0, scale) - sw,
        cy + si(4.0, scale),
        10.0 * scale,
        nc::TEXT_SUB,
    );
}

fn draw_statusbar(
    fb: &mut Framebuffer,
    nf: &mut NeoFonts,
    lay: &NeoLayout,
    tabs: &[Tab],
    active: usize,
    phase: f32,
) {
    let sc = lay.scale;
    let s = lay.status;
    fb.fill_rect(s.x, s.y, s.w, s.h, nc::BG_DEEP);
    hline(fb, s.x, s.y, s.w, nc::BG_DEEP, 0.09);
    let cy = s.y + s.h / 2;
    // 左: アクティブホストの状態＋名前
    let mut lx = s.x + si(12.0, sc);
    if let Some(tab) = tabs.get(active) {
        status_dot(fb, lx, cy, &tab.status, phase, sc);
        let col = status_color(&tab.status);
        lx = nf.draw(
            fb,
            Face::Mono,
            &tab.title,
            lx + si(10.0, sc),
            cy + si(4.0, sc),
            10.0 * sc,
            col,
        ) + si(16.0, sc);
        fb.fill_rect(
            lx - si(8.0, sc),
            cy - si(6.0, sc),
            1,
            si(12.0, sc),
            nc::TEXT_DIM,
        );
    }
    let connected = tabs
        .iter()
        .filter(|t| matches!(t.status, TabStatus::Connected))
        .count();
    // 接続数
    nf.draw_icon(fb, icon::WIFI, lx, cy - si(6.0, sc), 10.0 * sc, nc::GREEN);
    nf.draw(
        fb,
        Face::Mono,
        &format!("{} connected", connected),
        lx + si(16.0, sc),
        cy + si(4.0, sc),
        10.0 * sc,
        nc::GREEN,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 情報パネルを開くと中央（端末）がその分だけ狭くなる。
    #[test]
    fn info_panel_takes_width_from_center() {
        let closed = NeoLayout::compute(1600, 900, 1.0, false, false);
        let open = NeoLayout::compute(1600, 900, 1.0, false, true);
        assert!(closed.info.is_none());
        let info = open.info.expect("パネルが開いている");
        assert_eq!(info.w, INFO_W as i32);
        assert_eq!(open.terminal.w, closed.terminal.w - info.w);
        // パネルは中央の右隣、右端まで
        assert_eq!(info.x, open.terminal.x + open.terminal.w);
        assert_eq!(info.x + info.w, 1600);
    }

    /// 中央が最小幅を割るほど狭いウィンドウでは、要求しても畳む。
    #[test]
    fn info_panel_collapses_on_narrow_window() {
        let lay = NeoLayout::compute(600, 800, 1.0, false, true);
        assert!(lay.info.is_none(), "狭い窓でパネルが開いている");
        // 畳んだぶん中央は全幅を使う
        let closed = NeoLayout::compute(600, 800, 1.0, false, false);
        assert_eq!(lay.terminal.w, closed.terminal.w);
    }

    /// 使用率のしきい値（Figma 準拠）。
    #[test]
    fn gauge_color_thresholds() {
        assert_eq!(gauge_color(0.0, nc::CYAN), nc::CYAN);
        assert_eq!(gauge_color(50.0, nc::CYAN), nc::CYAN);
        assert_eq!(gauge_color(50.1, nc::CYAN), nc::AMBER);
        assert_eq!(gauge_color(70.0, nc::CYAN), nc::AMBER);
        assert_eq!(gauge_color(70.1, nc::CYAN), nc::RED);
    }

    #[test]
    fn human_kb_units() {
        assert_eq!(human_kb(512), "512K");
        assert_eq!(human_kb(2048), "2.0M");
        assert_eq!(human_kb(3 * 1024 * 1024), "3.0G");
    }
}
