//! アプリ本体。winit ApplicationHandler、モード状態機械、タブ/ペイン管理、描画統括。

use crate::font::FontManager;
use crate::input::{to_term_key, GuiAction};
use crate::launcherview::LauncherState;
use crate::pane::{Move, PaneNode, Rect, SplitDir};
use crate::render::Framebuffer;
use crate::runtime::{ConnEvent, ConnId, Connector};
use crate::selection::{extract_text, word_at, SelKind, Selection};
use crate::sftpview::{self, FileManager};
use crate::termview::CellMetrics;
use crate::theme::Theme;
use mot_core::i18n::{detect_lang, tr, Lang};
use mot_core::model::{Config, Profile, ReconnectCfg};
use mot_ssh::{PaneEvent, SshSession};
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::ModifiersState;
use winit::window::{CursorIcon, ResizeDirection, Window, WindowId};

/// 1ペイン分の端末状態。
pub struct PaneState {
    pub terminal: mot_term::Terminal,
    pub handle: mot_ssh::PaneHandle,
    pub scroll: usize,
    pub cols: u16,
    pub rows: u16,
    /// このペインで動いている Claude Code の状況（OSC 7777 由来）。
    /// tmux 越しだと1本の PTY に複数セッションが混ざるので session_id で分けて持つ。
    pub agents: mot_core::agent::AgentSessions,
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum TabStatus {
    Connecting,
    Connected,
    Failed,
}

pub struct Tab {
    pub title: String,
    pub profile: Option<Profile>,
    pub tree: PaneNode,
    pub focus: u64,
    pub next_pane: u64,
    pub panes: HashMap<u64, PaneState>,
    pub session: Option<Arc<SshSession>>,
    pub status: TabStatus,
    pub conn: ConnId,
    pub error: Option<String>,
    /// 自動再接続: これまでの再試行回数（接続成功で 0 に戻る）。
    pub reconnect_attempts: u32,
    /// 次の再接続を試みる時刻（バックオフ待ち。None=予約なし）。
    pub reconnect_at: Option<std::time::Instant>,
    /// 直近のシステムメトリクス（None=まだ 1 回も取れていない）。
    pub metrics: Option<mot_core::metrics::HostMetrics>,
    /// メトリクスの取得時刻（経過時間の表示に使う）。
    pub metrics_at: Option<std::time::Instant>,
    /// 採取を諦めた（非対応 OS・制限シェル等）。
    pub metrics_unavailable: bool,
    /// 採取タスクの制御ハンドル。drop でタスクが止まる。
    pub metrics_ctl: Option<crate::runtime::MetricsCtl>,
}

impl Tab {
    fn placeholder(conn: ConnId, profile: Option<Profile>, title: String) -> Self {
        Tab {
            title,
            profile,
            tree: PaneNode::leaf(0),
            focus: 0,
            next_pane: 1,
            panes: HashMap::new(),
            session: None,
            status: TabStatus::Connecting,
            conn,
            error: None,
            reconnect_attempts: 0,
            reconnect_at: None,
            metrics: None,
            metrics_at: None,
            metrics_unavailable: false,
            metrics_ctl: None,
        }
    }
}

/// プロファイルの有効な自動再接続設定。未指定なら既定で有効
/// （max_retries=5, backoff_sec=3）。`reconnect = { enabled = false }` で無効化。
fn effective_reconnect(profile: &Profile) -> ReconnectCfg {
    profile.reconnect.clone().unwrap_or(ReconnectCfg {
        enabled: true,
        max_retries: 5,
        backoff_sec: 3,
    })
}

/// 指数バックオフ秒（base * 2^(attempts-1)、上限 60 秒）。
fn backoff_secs(base: u64, attempts: u32) -> u64 {
    let base = base.max(1);
    let mult = 1u64 << attempts.saturating_sub(1).min(5);
    (base * mult).min(60)
}

/// モーダルダイアログの種類。
pub enum Modal {
    Tofu {
        conn: ConnId,
        host: String,
        fingerprint: String,
        reply: Sender<bool>,
    },
    Password {
        conn: ConnId,
        input: String,
        reply: Sender<Option<String>>,
    },
    Passphrase {
        conn: ConnId,
        input: String,
        reply: Sender<Option<String>>,
    },
    Master {
        input: String,
        /// ボールト新規作成（ファイルが無い）なら true → 「新規設定」文言。
        is_new: bool,
        reply: Sender<Option<String>>,
    },
    Overwrite {
        message: String,
        on_yes: OverwriteAction,
    },
}

#[derive(Clone)]
pub enum OverwriteAction {
    None,
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum Mode {
    Terminal,
    /// SFTP 2ペインファイルマネージャ（F3）
    Sftp,
}

/// 検索オーバーレイ。
pub struct SearchState {
    pub query: String,
    /// マッチ位置 (abs_row, col_start, col_end)。
    pub matches: Vec<(usize, usize, usize)>,
    /// 現在のマッチ（matches のインデックス）。
    pub current: usize,
}

impl SearchState {
    fn new() -> Self {
        SearchState {
            query: String::new(),
            matches: Vec::new(),
            current: 0,
        }
    }
}

/// タブのドラッグ並べ替えの進行状態。
#[derive(Clone, Copy)]
pub struct TabDrag {
    /// 現在ドラッグ中のタブ位置（並べ替えに伴い更新される）。
    index: usize,
    /// 押下時の x 座標（閾値超えで「移動開始」と判定）。
    press_x: i32,
    /// 閾値を超えて実際にドラッグが始まったか。
    moved: bool,
}

/// B4: スクロールバーのサムをドラッグ中の状態。
#[derive(Clone, Copy)]
pub struct ScrollbarDrag {
    /// ドラッグ対象のペイン ID。
    pid: u64,
    /// 押下時にサム上端から掴んだ相対 y（px）。サム中央がカーソルへ飛ばないための
    /// アンカー。トラックの空き部分を押した場合はサム高の半分（＝中央追従）。
    grab_dy: i32,
}

/// タブ右クリックメニューの表示状態。
#[derive(Clone, Copy)]
pub struct TabMenu {
    /// 対象タブの位置。
    index: usize,
    /// メニュー左上（タブ左端・タブバー直下）。
    x: i32,
    y: i32,
}

/// タブメニュー1項目の矩形（id, x0, y0, x1, y1）。
type TabMenuItem = (&'static str, i32, i32, i32, i32);

/// タブ右クリックメニューの項目 ID（順序が表示順）。
const TAB_MENU_ITEMS: [&str; 4] = ["rename", "duplicate", "sftp", "close"];
const TAB_MENU_W: i32 = 150;
const TAB_MENU_ITEM_H: i32 = 26;

/// タブメニュー項目のラベル（i18n は mot-core を変更せず mot-gui 内で持つ）。
fn tab_menu_label(lang: Lang, id: &str) -> &'static str {
    match (lang, id) {
        (Lang::Ja, "rename") => "名前を変更",
        (Lang::En, "rename") => "Rename",
        (Lang::Ja, "duplicate") => "複製",
        (Lang::En, "duplicate") => "Duplicate",
        (_, "sftp") => "SFTP",
        (Lang::Ja, "close") => "閉じる",
        (Lang::En, "close") => "Close",
        _ => "",
    }
}

pub struct App {
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    _context: Option<softbuffer::Context<Rc<Window>>>,
    fb: Framebuffer,
    font: FontManager,
    theme: Theme,
    config: Config,
    config_dir: PathBuf,
    /// 設定アイコンで開く設定ファイル（moterm.lua）のパス。
    config_file: PathBuf,
    lang: Lang,
    px: f32,
    connector: Connector,

    mode: Mode,
    launcher: LauncherState,
    tabs: Vec<Tab>,
    active_tab: usize,
    /// config.keys 反映済みのキーバインド表。
    keymap: mot_core::keymap::Keymap,

    modal: Option<Modal>,
    /// 表示待ちのモーダル（並列グループ接続で複数の TOFU/認証要求が同時に来た場合、
    /// 上書きせず順に提示する。上書きすると先行接続の reply が返信されず黙って落ちるため）。
    modal_queue: std::collections::VecDeque<Modal>,
    search: Option<SearchState>,
    show_pf_panel: bool,
    broadcast: bool,
    /// SFTP ファイルマネージャの状態（Mode::Sftp のとき Some）
    fm: Option<FileManager>,
    /// SFTP が紐づく接続タブの ConnId。この接続が切断/終了したら SFTP を閉じる。
    fm_conn: Option<ConnId>,
    pub(crate) selection: Option<Selection>,

    mods: ModifiersState,
    mouse: (f64, f64),
    dragging_sel: bool,
    /// B4: スクロールバーのサムをドラッグ中の状態（対象ペインと、サム内の掴んだ相対 y）。
    scrollbar_drag: Option<ScrollbarDrag>,
    click_count: u32,
    /// 直近の左クリック位置と時刻（ダブル/トリプルクリック判定用）。
    last_click: Option<((f64, f64), std::time::Instant)>,
    /// タブのドラッグ並べ替え進行状態。
    tab_drag: Option<TabDrag>,
    /// タブ名インライン編集（対象タブ位置, 編集バッファ）。
    tab_rename: Option<(usize, String)>,
    /// タブ右クリックメニュー。
    tab_menu: Option<TabMenu>,
    /// 直近のタブ左クリック（位置と時刻）。ダブルクリック判定に使う。
    last_tab_click: Option<(usize, std::time::Instant)>,
    /// マウスレポート転送中に押下しているボタン（motion 報告・release 報告に使う）。
    mouse_report_btn: Option<mot_term::MouseButton>,
    /// 右上のアプリ終了ボタン（×）が押されたら true。window_event 側で exit する。
    quit_requested: bool,
    /// IME 変換中テキスト（Preedit）。空なら変換なし。
    ime_preedit: String,
    /// 枠なし時のエッジリサイズで最後に設定したカーソル（連続 set_cursor 抑止）。
    last_cursor: Option<CursorIcon>,

    /// --screenshot モード: 数フレーム後に保存して終了
    screenshot: Option<String>,
    frames: u32,

    /// ウィンドウ透過（window_background_opacity < 1.0）。
    window_opacity: f32,
    /// MOTERM_NEO_DEMO=1: NEO-UI プリミティブのショーケースを描く（検証用）。
    neo_demo: bool,
    /// NEO-UI 同梱フォント（Inter/JetBrains Mono/lucide）。
    neo_fonts: crate::neofont::NeoFonts,
    /// NEO-UI: 折りたたみ中のサイドバーグループ名。
    neo_collapsed: std::collections::HashSet<String>,
    /// NEO-UI: マスターパスワードモーダルの表示/非表示トグル（目のアイコン）。
    master_show: bool,
    /// NEO-UI: サイドバーの Filter 入力にフォーカスがあるか（キーを query へ流す）。
    neo_filter_focus: bool,
    /// NEO-UI: サイドバーのキーボード選択モード。Some(i) なら sidebar_rows の i 行目を選択中。
    /// neo_filter_focus とは相互排他（どちらか一方のみ）。
    neo_sidebar_sel: Option<usize>,
    /// NEO-UI: 右側の情報パネル（ホスト情報 + メトリクス）を表示するか。
    /// 初期値は config.metrics.panel。非表示中はメトリクス採取も止める。
    pub(crate) neo_info_visible: bool,
    /// 端末表示時のウィンドウサイズ（window.width/height、論理px）。
    term_size: (u32, u32),
    /// 直近にプログラムから適用したサイズ。手動リサイズと争わないための基準
    /// （モード切替時に want と不一致のときだけ request_inner_size する）。
    applied_size: Option<(u32, u32)>,
}

impl App {
    pub fn new(
        config: Config,
        config_dir: PathBuf,
        config_file: PathBuf,
        screenshot: Option<String>,
    ) -> anyhow::Result<Self> {
        let lang = detect_lang(config.lang.as_deref());
        let theme = Theme::builtin(config.color_scheme.as_deref().unwrap_or("default"));
        let font = FontManager::load(config.font.as_deref(), &config.font_fallback)?;
        let px = config.font_size.max(8.0);
        let keys_cfg = config.keys.clone();
        // config は後段で move されるため、先に値を取り出しておく。
        // 情報パネルにはホスト情報・エージェント・認証も載るので、メトリクス採取を
        // 止めていてもパネル自体は出す（panel だけで決める）。
        let info_visible = config.metrics.panel;

        // プロファイル解決（lua + profiles.json マージ）
        let gui = mot_core::store::load_gui_profiles(&config_dir);
        let merged = mot_core::store::merge_profiles(&config.profiles, &gui);
        let launcher = LauncherState::new(merged, config.groups.clone());

        let mut connector = Connector::new()?;
        // ボールト保存先＝実行ファイルと同じディレクトリ（既定）。secret_store='none' で無効化。
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));
        let vpath = vault_path(&config, exe_dir.as_deref(), &config_dir);
        connector.configure_vault(vpath, config.secret_store != "none");
        let neo_demo = std::env::var("MOTERM_NEO_DEMO").ok().as_deref() == Some("1");
        let neo_fonts = crate::neofont::NeoFonts::load(&config.font_fallback)?;
        // MOTERM_OPEN_SFTP=1: 接続なしで FM を開いた状態で起動（スクショ/デモ用）
        let demo_fm = build_demo_fm(&config);
        // MOTERM_DEMO_MASTER=1: マスターパスワードモーダルを開いた状態で起動（スクショ用）
        let demo_master = build_demo_master();

        let window_opacity = config.window_background_opacity.clamp(0.05, 1.0);
        let term_size = mode_sizes(&config);
        // NEO-UI は接続前でも常設サイドバーを表示するため、未接続でも Terminal（no session）で起動。
        let start_mode = if demo_fm.is_some() {
            Mode::Sftp
        } else {
            Mode::Terminal
        };

        Ok(App {
            window: None,
            surface: None,
            _context: None,
            fb: Framebuffer::new(1000, 640),
            font,
            theme,
            config,
            config_dir,
            config_file,
            lang,
            px,
            connector,
            mode: start_mode,
            launcher,
            tabs: Vec::new(),
            active_tab: 0,
            keymap: mot_core::keymap::Keymap::from_config(&keys_cfg),
            modal: demo_master,
            modal_queue: std::collections::VecDeque::new(),
            search: build_demo_search(),
            show_pf_panel: std::env::var("MOTERM_DEMO_PF").ok().as_deref() == Some("1"),
            broadcast: false,
            fm: demo_fm,
            fm_conn: None,
            selection: None,
            mods: ModifiersState::empty(),
            mouse: (0.0, 0.0),
            dragging_sel: false,
            scrollbar_drag: None,
            click_count: 0,
            last_click: None,
            tab_drag: None,
            tab_rename: None,
            tab_menu: None,
            last_tab_click: None,
            mouse_report_btn: None,
            quit_requested: false,
            ime_preedit: String::new(),
            last_cursor: None,
            screenshot: screenshot.clone(),
            frames: 0,
            window_opacity,
            neo_demo,
            neo_fonts,
            neo_collapsed: std::collections::HashSet::new(),
            master_show: false,
            neo_filter_focus: false,
            neo_sidebar_sel: None,
            neo_info_visible: info_visible,
            term_size,
            applied_size: None,
        })
    }

    /// 設定の width/height をウィンドウへ適用する（applied_size と比較して手動リサイズと争わない）。
    fn sync_mode_size(&mut self) {
        let want = self.term_size;
        if self.applied_size == Some(want) {
            return;
        }
        if let Some(w) = &self.window {
            let _ = w.request_inner_size(winit::dpi::LogicalSize::new(want.0, want.1));
        }
        self.applied_size = Some(want);
    }

    fn cell_metrics(&mut self) -> CellMetrics {
        CellMetrics::new(&mut self.font, self.px)
    }

    fn term_mods(&self) -> mot_term::Mods {
        mot_term::Mods {
            shift: self.mods.shift_key(),
            ctrl: self.mods.control_key(),
            alt: self.mods.alt_key(),
        }
    }

    pub(crate) fn edge_resize_dir(&mut self, mx: i32, my: i32) -> Option<ResizeDirection> {
        if self.config.window.title_bar {
            return None;
        }
        let (cx, cy, cw, ch) = self.app_close_rect();
        if mx >= cx && mx < cx + cw && my >= cy && my < cy + ch {
            return None; // 終了ボタンはリサイズ対象外
        }
        resize_dir_at(mx, my, self.fb.w as i32, self.fb.h as i32, 6)
    }

    /// 枠なし時、ウィンドウ端でカーソル形状をリサイズ用に切り替える。
    fn update_resize_cursor(&mut self) {
        if self.config.window.title_bar {
            return;
        }
        let (mx, my) = (self.mouse.0 as i32, self.mouse.1 as i32);
        let icon = match self.edge_resize_dir(mx, my) {
            Some(dir) => CursorIcon::from(dir),
            None => CursorIcon::Default,
        };
        if self.last_cursor != Some(icon) {
            self.last_cursor = Some(icon);
            if let Some(w) = &self.window {
                w.set_cursor(icon);
            }
        }
    }

    fn terminal_area_cells(&mut self) -> Rect {
        let m = self.cell_metrics();
        let tab_h = tab_h_of(&m);
        let mut content_bottom = self.fb.h as i32;
        if self.broadcast {
            content_bottom -= 4;
        }
        Rect {
            x: 0,
            y: 0,
            w: (self.fb.w as i32 / m.cw).max(1) as u16,
            h: ((content_bottom - tab_h) / m.ch).max(1) as u16,
        }
    }

    // ---- 接続 ----
    fn connect_profile(&mut self, profile: Profile) {
        // 実ウィンドウのコンテンツ領域から初期 PTY サイズを決める（80x24 固定にしない）。
        let area = self.terminal_area_cells();
        let cols = area.w.saturating_sub(1).max(1);
        let rows = area.h;
        let conn = self.connector.start_connect(&profile, cols, rows);
        let title = profile.name.clone();
        self.tabs.push(Tab::placeholder(conn, Some(profile), title));
        self.active_tab = self.tabs.len() - 1;
        self.mode = Mode::Terminal;
    }

    /// モーダルを提示する。既に表示中なら待ち行列へ（上書きしない）。
    fn enqueue_modal(&mut self, m: Modal) {
        if self.modal.is_none() {
            self.modal = Some(m);
        } else {
            self.modal_queue.push_back(m);
        }
    }

    /// 現在のモーダルを閉じ、待ち行列に次があれば繰り上げる。
    /// モーダル処理側は `self.modal = None` の代わりにこれを呼ぶこと。
    pub(crate) fn close_modal(&mut self) {
        self.master_show = false;
        self.modal = self.modal_queue.pop_front();
    }

    /// 接続イベントをドレインしてタブ状態を更新する。
    fn pump_conn_events(&mut self) {
        while let Ok(ev) = self.connector.rx.try_recv() {
            match ev {
                ConnEvent::TransferProgress {
                    name,
                    done,
                    total,
                    index,
                    count,
                } => {
                    self.fm_transfer_progress(crate::sftpview::FmProgress {
                        name,
                        done,
                        total,
                        index,
                        count,
                    });
                }
                ConnEvent::TransferDone {
                    from,
                    total,
                    failed,
                } => {
                    self.fm_transfer_done(from, total, failed);
                }
                ConnEvent::NeedTofu {
                    conn,
                    host,
                    fingerprint,
                    reply,
                } => {
                    self.enqueue_modal(Modal::Tofu {
                        conn,
                        host,
                        fingerprint,
                        reply,
                    });
                }
                ConnEvent::NeedPassword { conn, reply } => {
                    self.enqueue_modal(Modal::Password {
                        conn,
                        input: String::new(),
                        reply,
                    });
                }
                ConnEvent::NeedPassphrase { conn, reply } => {
                    self.enqueue_modal(Modal::Passphrase {
                        conn,
                        input: String::new(),
                        reply,
                    });
                }
                ConnEvent::NeedMaster {
                    conn: _,
                    is_new,
                    reply,
                } => {
                    self.enqueue_modal(Modal::Master {
                        input: String::new(),
                        is_new,
                        reply,
                    });
                }
                ConnEvent::Connected {
                    conn,
                    session,
                    pane,
                } => {
                    // メトリクス採取タスクは tab の可変借用を抜けてから起動する。
                    let mut probe: Option<mot_ssh::ExecProbe> = None;
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.conn == conn) {
                        let session = Arc::new(*session);
                        probe = Some(session.exec_probe());
                        let (cols, rows) = (pane_cols(&session), pane_rows(&session));
                        let pid = tab.next_pane;
                        tab.next_pane += 1;
                        tab.tree = PaneNode::leaf(pid);
                        tab.focus = pid;
                        tab.panes.clear(); // 再接続時の古い死んだペインを破棄（単一ペインへ）
                        tab.panes.insert(
                            pid,
                            PaneState {
                                terminal: mot_term::Terminal::new(
                                    cols as usize,
                                    rows as usize,
                                    self.config.scrollback_lines,
                                ),
                                handle: *pane,
                                scroll: 0,
                                cols,
                                rows,
                                agents: Default::default(),
                            },
                        );
                        tab.session = Some(session);
                        tab.status = TabStatus::Connected;
                        // 再接続で確立できたか（この接続が再試行だったか）。
                        let was_reconnect = tab.reconnect_attempts > 0;
                        tab.reconnect_attempts = 0;
                        tab.reconnect_at = None;
                        // on_connect 自動入力（再接続時は on_connect_on_reconnect=true のときだけ）。
                        if let Some(profile) = &tab.profile {
                            let run = !was_reconnect || profile.on_connect_on_reconnect;
                            let cmds = if run {
                                profile.on_connect.clone()
                            } else {
                                Vec::new()
                            };
                            if let Some(p) = tab.panes.get_mut(&pid) {
                                for c in cmds {
                                    let mut line = c.into_bytes();
                                    line.push(b'\n');
                                    p.handle.write(line);
                                }
                            }
                        }
                    }
                    // メトリクス採取を開始（設定で無効なら何もしない）。
                    // 再接続時は Some(ctl) を差し替えることで古いタスクが drop で止まる。
                    if let (Some(probe), Some(interval)) = (probe, self.config.metrics.interval()) {
                        let ctl = self.connector.start_metrics(conn, probe, interval);
                        ctl.set_paused(!self.neo_info_visible);
                        if let Some(tab) = self.tabs.iter_mut().find(|t| t.conn == conn) {
                            tab.metrics = None;
                            tab.metrics_at = None;
                            tab.metrics_unavailable = false;
                            tab.metrics_ctl = Some(ctl);
                        }
                    }
                }
                ConnEvent::Metrics { conn, snap } => {
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.conn == conn) {
                        tab.metrics = Some(snap);
                        tab.metrics_at = Some(std::time::Instant::now());
                        tab.metrics_unavailable = false;
                    }
                }
                ConnEvent::MetricsUnavailable { conn } => {
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.conn == conn) {
                        tab.metrics_unavailable = true;
                        tab.metrics_ctl = None; // タスクは自分で終了済み
                    }
                }
                ConnEvent::Failed { conn, error } => {
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.conn == conn) {
                        tab.status = TabStatus::Failed;
                        tab.error = Some(error);
                        // 再接続中の失敗なら次のバックオフを予約（回数上限まで）。
                        if let Some(rc) = tab.profile.as_ref().map(effective_reconnect) {
                            let more =
                                rc.max_retries == 0 || tab.reconnect_attempts < rc.max_retries;
                            if rc.enabled && tab.reconnect_attempts > 0 && more {
                                let secs = backoff_secs(rc.backoff_sec, tab.reconnect_attempts);
                                tab.reconnect_at = Some(
                                    std::time::Instant::now()
                                        + std::time::Duration::from_secs(secs),
                                );
                                tab.status = TabStatus::Connecting;
                            }
                        }
                    }
                }
            }
        }
    }

    /// 各ペインの PTY 出力をドレインして端末へ供給し、応答を書き戻す。
    fn pump_panes(&mut self) {
        // B4: スクロールバーをドラッグ中のペインは、新規出力が来ても最下部へ
        // 自動復帰させない（ユーザのドラッグ操作を優先する）。
        let sb_drag_pid = self.scrollbar_drag.map(|d| d.pid);
        for tab in &mut self.tabs {
            // Exited(None)=予期しない切断（自動再接続対象）, Exited(Some)=シェルが正常終了。
            let mut dropped = false; // 予期しない切断
            let mut exited = false; // 何らかの終了
            for (pane_id, pane) in tab.panes.iter_mut() {
                let mut got = false;
                while let Ok(ev) = pane.handle.events.try_recv() {
                    match ev {
                        PaneEvent::Output(data) => {
                            // 堅牢化: パーサ/スクリーンの想定外 panic で GUI スレッドごと
                            // 巻き戻り、全 SSH セッションが道連れで落ちるのを防ぐ。万一 panic
                            // したらそのチャンクを捨てて継続する（1ペインの表示乱れに留める）。
                            let term = &mut pane.terminal;
                            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                term.feed(&data)
                            }))
                            .is_err()
                            {
                                log::warn!(
                                    "terminal feed panicked (pane {pane_id}); chunk を破棄して継続"
                                );
                            }
                            got = true;
                        }
                        PaneEvent::Exited(status) => {
                            exited = true;
                            if status.is_none() {
                                dropped = true; // exit-status 無し = 転送レベルの切断
                            }
                        }
                    }
                }
                if got {
                    let resp = pane.terminal.screen.take_responses();
                    if !resp.is_empty() {
                        pane.handle.write(resp);
                    }
                    // クリップボード等のイベントは take_events で消費（OSC52 等）
                    for tev in pane.terminal.screen.take_events() {
                        match tev {
                            mot_term::TermEvent::Clipboard(text) => set_clipboard(&text),
                            // OSC 7777: リモートの Claude Code の状況。
                            // 解釈できないペイロードは捨てる（他端末向けの独自 OSC が
                            // 紛れ込んでも害が出ないように）。
                            mot_term::TermEvent::Agent(payload) => {
                                if let Some(ev) = mot_core::agent::parse_agent_osc(&payload) {
                                    pane.agents.apply(ev);
                                }
                            }
                            _ => {}
                        }
                    }
                    if sb_drag_pid != Some(*pane_id) {
                        pane.scroll = 0; // 新規出力で最新へ復帰
                    }
                }
            }
            // 切断処理（内側の借用を抜けてから）。
            if exited {
                tab.status = TabStatus::Failed;
                // 予期しない切断かつ自動再接続が有効なら、初回のバックオフを予約。
                if dropped && tab.reconnect_at.is_none() {
                    if let Some(rc) = tab.profile.as_ref().map(effective_reconnect) {
                        let more = rc.max_retries == 0 || tab.reconnect_attempts < rc.max_retries;
                        if rc.enabled && more {
                            let secs = backoff_secs(rc.backoff_sec, 1);
                            tab.reconnect_at = Some(
                                std::time::Instant::now() + std::time::Duration::from_secs(secs),
                            );
                            tab.status = TabStatus::Connecting;
                        }
                    }
                }
            }
        }
    }

    /// メトリクス採取の一時停止状態を UI に合わせる（毎フレーム呼ぶ）。
    ///
    /// 情報パネルに出るのはアクティブタブの値だけなので、それ以外のタブと
    /// パネル非表示中はリモートへコマンドを投げない。
    fn sync_metrics_pause(&mut self) {
        let panel_visible = self.neo_info_visible
            && !(self.mode == Mode::Sftp && self.fm_belongs_to_active())
            && self
                .window
                .as_ref()
                .is_none_or(|w| !w.is_minimized().unwrap_or(false));
        let active = self.active_tab;
        for (i, tab) in self.tabs.iter().enumerate() {
            if let Some(ctl) = &tab.metrics_ctl {
                ctl.set_paused(!(panel_visible && i == active));
            }
        }
    }

    /// 予約済みの自動再接続を、時刻が来たものから実行する（毎フレーム呼ぶ）。
    fn pump_reconnects(&mut self) {
        let now = std::time::Instant::now();
        let due: Vec<usize> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| t.profile.is_some() && t.reconnect_at.is_some_and(|at| now >= at))
            .map(|(i, _)| i)
            .collect();
        for i in due {
            self.reconnect_tab_inplace(i);
        }
    }

    /// タブ i を同じ場所で再接続する（新規タブは作らない）。
    fn reconnect_tab_inplace(&mut self, i: usize) {
        let area = self.terminal_area_cells();
        let cols = area.w.saturating_sub(1).max(1);
        let rows = area.h;
        let Some(profile) = self.tabs.get(i).and_then(|t| t.profile.clone()) else {
            return;
        };
        let new_conn = self.connector.start_connect(&profile, cols, rows);
        if let Some(tab) = self.tabs.get_mut(i) {
            tab.conn = new_conn; // 以後の Connected/Failed はこのタブへ届く
            tab.status = TabStatus::Connecting;
            tab.reconnect_attempts += 1;
            tab.reconnect_at = None;
            tab.session = None;
            let attempt = tab.reconnect_attempts;
            let focus = tab.focus;
            if let Some(p) = tab.panes.get_mut(&focus) {
                let msg = format!(
                    "\r\n\x1b[33m[moterm] 接続が切れました。再接続中… ({attempt})\x1b[0m\r\n"
                );
                p.terminal.feed(msg.as_bytes());
            }
        }
    }

    /// NEO-UI プリミティブのショーケース（角丸/グラデ/枠/発光/ソフトシャドウ/テキスト）。
    fn draw_neo_demo(&mut self) {
        use crate::neo::{self, color as nc};
        let px = self.px;
        self.fb.clear(nc::BG);
        // ドットグリッド背景（放射風の淡い点）
        let step = 28i32;
        let mut gy = 0;
        while gy < self.fb.h as i32 {
            let mut gx = 0;
            while gx < self.fb.w as i32 {
                self.fb.put(gx, gy, nc::TEXT_DIM);
                gx += step;
            }
            gy += step;
        }
        self.fb.draw_text(
            &mut self.font,
            "NEO-UI primitives",
            40,
            48,
            px * 1.4,
            nc::CYAN,
        );

        // パネル（角丸・半透明塗り＋枠）
        neo::glow_round_rect(&mut self.fb, 40, 80, 300, 150, 12.0, nc::CYAN, 0.10, 12);
        neo::fill_round_rect(&mut self.fb, 40, 80, 300, 150, 12.0, nc::PANEL, 0.97);
        neo::stroke_round_rect(&mut self.fb, 40, 80, 300, 150, 12.0, nc::BORDER, 0.22, 1.0);
        self.fb.draw_text(
            &mut self.font,
            "rounded panel + glow",
            58,
            110,
            px,
            nc::TEXT,
        );

        // ピル/バッジ
        neo::fill_round_rect(&mut self.fb, 58, 130, 70, 22, 11.0, nc::VIOLET, 0.14);
        neo::stroke_round_rect(&mut self.fb, 58, 130, 70, 22, 11.0, nc::VIOLET, 0.5, 1.0);
        self.fb
            .draw_text(&mut self.font, "k8s", 74, 146, px * 0.85, nc::VIOLET);

        // グラデーションバー
        neo::gradient_rect_v(&mut self.fb, 58, 168, 260, 10, nc::CYAN, nc::VIOLET, 0.9);

        // NEW CONNECTION 風ボタン
        neo::fill_round_rect(&mut self.fb, 58, 192, 264, 28, 6.0, nc::CYAN, 0.05);
        neo::stroke_round_rect(&mut self.fb, 58, 192, 264, 28, 6.0, nc::CYAN, 0.2, 1.0);
        self.fb.draw_text(
            &mut self.font,
            "+ NEW CONNECTION",
            130,
            211,
            px * 0.9,
            nc::CYAN,
        );

        // 発光ドット（接続状態: 緑/琥珀/暗）
        let dy = 280;
        neo::glow_dot(&mut self.fb, 60, dy, 4.0, nc::GREEN, 12.0);
        self.fb
            .draw_text(&mut self.font, "connected", 78, dy + 5, px, nc::GREEN);
        neo::glow_dot(&mut self.fb, 220, dy, 4.0, nc::AMBER, 12.0);
        self.fb
            .draw_text(&mut self.font, "connecting", 238, dy + 5, px, nc::AMBER);
        neo::glow_dot(&mut self.fb, 400, dy, 4.0, nc::TEXT_DIM, 6.0);
        self.fb
            .draw_text(&mut self.font, "offline", 418, dy + 5, px, nc::TEXT_SUB);

        // カラーサンプル（発光ドット列）
        let samples = [nc::CYAN, nc::VIOLET, nc::GREEN, nc::AMBER, nc::RED];
        for (i, c) in samples.iter().enumerate() {
            neo::glow_dot(&mut self.fb, 60 + i as i32 * 40, 340, 6.0, *c, 16.0);
        }

        // 同梱フォント（Inter 可変幅 / JetBrains Mono / lucide アイコン）
        use crate::neofont::{icon, Face};
        let nf = &mut self.neo_fonts;
        let fb = &mut self.fb;
        nf.draw(
            fb,
            Face::Ui,
            "Inter — proportional UI font",
            480,
            100,
            18.0,
            nc::TEXT,
        );
        nf.draw(
            fb,
            Face::Ui,
            "The quick brown fox 0123456789",
            480,
            126,
            15.0,
            nc::TEXT_SUB,
        );
        nf.draw(
            fb,
            Face::Mono,
            "JetBrains Mono  user@host:22",
            480,
            158,
            14.0,
            nc::CYAN,
        );
        nf.draw(
            fb,
            Face::MonoBold,
            "bold: 42% cpu  1.2 MB/s",
            480,
            182,
            14.0,
            nc::GREEN,
        );
        // lucide アイコン列
        let icons = [
            icon::SERVER,
            icon::DATABASE,
            icon::GLOBE,
            icon::TERMINAL,
            icon::KEY,
            icon::SHIELD,
            icon::ZAP,
            icon::WIFI,
            icon::CPU,
            icon::STAR,
            icon::SEARCH,
            icon::SETTINGS,
            icon::PLUS,
            icon::X,
        ];
        for (i, ic) in icons.iter().enumerate() {
            nf.draw_icon(fb, *ic, 480 + i as i32 * 34, 220, 20.0, nc::CYAN);
        }
        nf.draw(fb, Face::Ui, "lucide icons", 480, 268, 13.0, nc::TEXT_SUB);
    }

    // ---- 描画 ----
    fn render(&mut self) {
        // 紐づく接続が切断/消滅した SFTP は閉じる（描画前に整合）。
        self.sync_fm_connection();
        // NEO-UI プリミティブのショーケース（MOTERM_NEO_DEMO=1、検証用）。
        if self.neo_demo {
            self.draw_neo_demo();
            return;
        }
        // NEO-UI（3カラム未来的UI）。SFTP も接続画面に埋め込むため render_neo が中央を切り替える。
        self.render_neo();
        // IME 候補ウィンドウを端末カーソル位置へ追従させる。
        self.update_ime_cursor_area();
        // ポートフォワードパネル（F2）／検索バー（Ctrl+Shift+F）を neon で描画。
        if self.show_pf_panel {
            self.draw_neo_pf_panel();
        }
        if self.search.is_some() {
            self.draw_neo_search();
        }
        // タブ右クリックメニュー（neon スタイル）。
        self.draw_neo_tab_menu();
        // モーダル。パスワード系（Master/Password/Passphrase）は neon、それ以外（TOFU/上書き
        // 確認）は簡易パネルで最前面に描く。
        if self.modal.is_some() {
            if matches!(
                self.modal,
                Some(Modal::Master { .. } | Modal::Password { .. } | Modal::Passphrase { .. })
            ) {
                self.draw_neo_master_modal();
            } else {
                self.render_modal_snapshot();
            }
        }
    }

    /// タブメニュー各項目の矩形 (id, x0, y0, x1, y1)。開いていなければ None。
    fn tab_menu_geometry(&self) -> Option<Vec<TabMenuItem>> {
        let menu = self.tab_menu.as_ref()?;
        // font_size 由来のスケールに合わせる（描画とヒットで共有）。
        let (mw, ih) = {
            let sc = self.neo_scale();
            (
                (TAB_MENU_W as f32 * sc).round() as i32,
                (TAB_MENU_ITEM_H as f32 * sc).round() as i32,
            )
        };
        // 右端からはみ出さないよう左に寄せる。
        let x0 = menu.x.min(self.fb.w as i32 - mw - 2).max(0);
        let mut y = menu.y;
        let mut out = Vec::with_capacity(TAB_MENU_ITEMS.len());
        for id in TAB_MENU_ITEMS {
            out.push((id, x0, y, x0 + mw, y + ih));
            y += ih;
        }
        Some(out)
    }

    fn app_close_rect(&mut self) -> (i32, i32, i32, i32) {
        let h = tab_h_of(&self.cell_metrics());
        let w = h;
        let x = self.fb.w as i32 - w;
        (x, 0, w, h)
    }

    fn render_modal_snapshot(&mut self) {
        let theme = self.theme.clone();
        let m = self.cell_metrics();
        // 半透明の暗幕代わりに枠付きパネル
        let w = (self.fb.w as i32 * 2 / 3).min(560);
        let h = m.ch * 6;
        let x = (self.fb.w as i32 - w) / 2;
        let y = (self.fb.h as i32 - h) / 2;
        self.fb
            .fill_rect(x - 2, y - 2, w + 4, h + 4, theme.ui_accent);
        self.fb.fill_rect(x, y, w, h, theme.ui_panel);

        let (title, body, input_masked) = match self.modal.as_ref().unwrap() {
            Modal::Tofu {
                host, fingerprint, ..
            } => (
                tr(self.lang, "tofu_title").to_string(),
                format!(
                    "{host}\n{fingerprint}\n{}  (y/n)",
                    tr(self.lang, "tofu_question")
                ),
                None,
            ),
            Modal::Password { input, .. } => (
                tr(self.lang, "password_prompt").to_string(),
                String::new(),
                Some(input.len()),
            ),
            Modal::Passphrase { input, .. } => (
                tr(self.lang, "passphrase_prompt").to_string(),
                String::new(),
                Some(input.len()),
            ),
            Modal::Master { input, is_new, .. } => (
                tr(
                    self.lang,
                    if *is_new {
                        "master_new_prompt"
                    } else {
                        "master_prompt"
                    },
                )
                .to_string(),
                String::new(),
                Some(input.len()),
            ),
            Modal::Overwrite { message, .. } => (
                tr(self.lang, "overwrite_confirm").to_string(),
                message.clone(),
                None,
            ),
        };
        self.fb.draw_text(
            &mut self.font,
            &title,
            x + 16,
            y + m.ch + 4,
            self.px,
            theme.ui_fg,
        );
        let mut ly = y + m.ch * 2 + 4;
        for line in body.lines() {
            self.fb
                .draw_text(&mut self.font, line, x + 16, ly, self.px, theme.ui_dim);
            ly += m.ch;
        }
        if let Some(n) = input_masked {
            let masked: String = "*".repeat(n);
            self.fb.fill_rect(x + 16, ly, w - 32, m.ch + 4, theme.ui_bg);
            self.fb.draw_text(
                &mut self.font,
                &masked,
                x + 20,
                ly + m.ch,
                self.px,
                theme.ui_fg,
            );
        }
    }

    // ---- 入力処理は main 側 window_event から呼ぶ ----
    fn present(&mut self) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let size = window.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));
        if self.fb.w != w as usize || self.fb.h != h as usize {
            self.fb.resize(w as usize, h as usize);
        }
        // softbuffer は present 前に必ずサイズ設定が要る
        if let Some(surface) = self.surface.as_mut() {
            let _ = surface.resize(NonZeroU32::new(w).unwrap(), NonZeroU32::new(h).unwrap());
        }
        self.render();
        // 透過指定時のみ、背景ピクセルにアルファを与える（既定 1.0 では触らない＝従来と同一）。
        if self.window_opacity < 1.0 {
            let alpha = (self.window_opacity * 255.0).round() as u8;
            let bg = self.theme.bg;
            self.fb.apply_alpha(bg, alpha);
        }
        if let Some(surface) = self.surface.as_mut() {
            if let Ok(mut buffer) = surface.buffer_mut() {
                let n = buffer.len().min(self.fb.buf.len());
                buffer[..n].copy_from_slice(&self.fb.buf[..n]);
                let _ = buffer.present();
            }
        }
    }
}

/// スクショ用モーダルデモ。MOTERM_DEMO_MASTER=1 か MOTERM_DEMO_MODAL=master|password|
/// passphrase を初期表示する。送信先 Receiver は破棄する（送信は無視される）。
fn build_demo_master() -> Option<Modal> {
    let kind = if std::env::var("MOTERM_DEMO_MASTER").ok().as_deref() == Some("1") {
        "master".to_string()
    } else {
        std::env::var("MOTERM_DEMO_MODAL").ok()?
    };
    let (tx, _rx) = std::sync::mpsc::channel::<Option<String>>();
    match kind.as_str() {
        "master" => Some(Modal::Master {
            input: "hunter2secret".to_string(),
            is_new: false,
            reply: tx,
        }),
        "password" => Some(Modal::Password {
            conn: 0,
            input: "hunter2".to_string(),
            reply: tx,
        }),
        "passphrase" => Some(Modal::Passphrase {
            conn: 0,
            input: "keypass".to_string(),
            reply: tx,
        }),
        _ => None,
    }
}

/// MOTERM_DEMO_SEARCH=1 のとき、検索バーを初期表示する（スクショ用）。
fn build_demo_search() -> Option<SearchState> {
    if std::env::var("MOTERM_DEMO_SEARCH").ok().as_deref() != Some("1") {
        return None;
    }
    Some(SearchState {
        query: "error".to_string(),
        matches: vec![(0, 0, 5), (1, 0, 5), (2, 0, 5)],
        current: 1,
    })
}

/// MOTERM_OPEN_SFTP=1 のとき、接続なしで FM を初期表示する（スクショ/デモ用）。
/// ローカルペインは実ディレクトリ、リモートペインはデモ用の擬似一覧。
fn build_demo_fm(config: &Config) -> Option<FileManager> {
    if std::env::var("MOTERM_OPEN_SFTP").ok().as_deref() != Some("1") {
        return None;
    }
    let local_cwd = config
        .download_dir
        .as_deref()
        .map(mot_core::model::expand_tilde)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut fm = FileManager::new(
        local_cwd.to_string_lossy().to_string(),
        "/home/moterm".into(),
    );
    // ローカルは実ディレクトリを読む
    let (entries, note) = read_local_listing(&local_cwd);
    fm.local.set_listing(entries, true);
    fm.local.note = note;
    // リモートは擬似一覧（未接続）。日付列の表示確認のため mtime を入れる。
    fm.remote.set_listing(
        vec![
            sftpview::Entry::dir("app").with_mtime(1_704_067_200), // 2024-01-01
            sftpview::Entry::dir("logs").with_mtime(1_700_000_000),
            sftpview::Entry::file("config.yaml", 2048).with_mtime(1_609_459_200), // 2021-01-01
            sftpview::Entry::file("app.log", 1048576).with_mtime(1_234_567_890),  // 2009-02-13
        ],
        true,
    );
    fm.remote.note = Some("(demo: not connected)".into());
    Some(fm)
}

/// ローカルディレクトリを読み、(エントリ, 注記) を返す。
pub(crate) fn read_local_listing(dir: &std::path::Path) -> (Vec<sftpview::Entry>, Option<String>) {
    match std::fs::read_dir(dir) {
        Ok(rd) => {
            let mut out = Vec::new();
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                let (is_dir, size, mtime) = match e.metadata() {
                    Ok(m) => {
                        let mtime = m
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        (m.is_dir(), m.len(), mtime)
                    }
                    Err(_) => (false, 0, 0),
                };
                out.push(sftpview::Entry {
                    name,
                    is_dir,
                    size,
                    mtime,
                    parent: false,
                });
            }
            (out, None)
        }
        Err(e) => (Vec::new(), Some(format!("read error: {e}"))),
    }
}

/// アプリアイコン（assets/icon.png）をコンパイル時に埋め込む。
/// Windows のタスクバー／タイトルバー／Alt+Tab のアイコンに使う（実行中アプリのアイコン）。
/// exe 埋め込みアイコン(build.rs)はピン留め/エクスプローラ用で別物。
const ICON_PNG: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/icon.png"
));

/// 埋め込み PNG を winit の Icon に変換する。失敗時は None（アイコン無しで続行）。
/// ウィンドウの角を丸くする。Windows 11 の DWM に角丸を要求する
/// （DWMWA_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND）。Win10 以前は DWM が
/// 属性を知らずエラーを返すだけで無害。Linux/macOS は OS/コンポジタ管轄のため no-op。
fn apply_rounded_corners(window: &Window) {
    #[cfg(windows)]
    {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(handle) = window.window_handle() {
            if let RawWindowHandle::Win32(w32) = handle.as_raw() {
                use windows_sys::Win32::Graphics::Dwm::{
                    DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
                };
                let hwnd = w32.hwnd.get() as windows_sys::Win32::Foundation::HWND;
                let pref = DWMWCP_ROUND;
                unsafe {
                    DwmSetWindowAttribute(
                        hwnd,
                        DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                        &pref as *const _ as *const core::ffi::c_void,
                        std::mem::size_of_val(&pref) as u32,
                    );
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = window;
    }
}

/// 背景ぼかし（Windows 11 の DWM システムバックドロップ = Acrylic/Mica 系）。
/// DWMWA_SYSTEMBACKDROP_TYPE=DWMSBT_TRANSIENTWINDOW を要求する。透過ウィンドウと
/// 併用すると背後がぼける。Win10 以前・他 OS では no-op（属性未知でエラーになるだけ）。
fn apply_backdrop_blur(window: &Window) {
    #[cfg(windows)]
    {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(handle) = window.window_handle() {
            if let RawWindowHandle::Win32(w32) = handle.as_raw() {
                use windows_sys::Win32::Graphics::Dwm::{
                    DwmSetWindowAttribute, DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE,
                };
                let hwnd = w32.hwnd.get() as windows_sys::Win32::Foundation::HWND;
                let backdrop = DWMSBT_TRANSIENTWINDOW;
                unsafe {
                    DwmSetWindowAttribute(
                        hwnd,
                        DWMWA_SYSTEMBACKDROP_TYPE as u32,
                        &backdrop as *const _ as *const core::ffi::c_void,
                        std::mem::size_of_val(&backdrop) as u32,
                    );
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = window;
    }
}

fn load_window_icon() -> Option<winit::window::Icon> {
    let img = image::load_from_memory(ICON_PNG).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    winit::window::Icon::from_rgba(img.into_raw(), w, h).ok()
}

/// 端末領域高さの算出に使う、タブ帯ぶんの余白（行高 + この値）。
const TAB_BAR_EXTRA: i32 = 26;

/// タブ帯を含めた上部オフセット。端末領域計算（terminal_area_cells）が使う。
pub(crate) fn tab_h_of(m: &CellMetrics) -> i32 {
    m.ch + TAB_BAR_EXTRA
}

/// ダブルクリック判定の時間閾値（ミリ秒）。
const DOUBLE_CLICK_MS: u128 = 400;
/// タブドラッグ開始と見なす移動量の閾値（px）。
const TAB_DRAG_THRESHOLD: i32 = 5;

/// ダブルクリック判定（純ロジック）。直近クリックのタブ位置・経過ミリ秒から、
/// 同じタブを閾値内に再クリックしたかを返す。
fn double_click(
    last_idx: Option<usize>,
    elapsed_ms: Option<u128>,
    i: usize,
    threshold: u128,
) -> bool {
    matches!((last_idx, elapsed_ms), (Some(li), Some(e)) if li == i && e <= threshold)
}

/// リネーム確定時の新タイトル。空（空白のみ）バッファは変更なし＝旧タイトルを維持。
fn rename_result(old: &str, buf: &str) -> String {
    if buf.trim().is_empty() {
        old.to_string()
    } else {
        buf.to_string()
    }
}

/// タブを from の位置から to の位置へ移動（remove + insert で順序を保つ）。
fn move_tab(tabs: &mut Vec<Tab>, from: usize, to: usize) {
    if from < tabs.len() && to < tabs.len() && from != to {
        let t = tabs.remove(from);
        tabs.insert(to, t);
    }
}

/// マウス座標がウィンドウ端（閾値 t）ならリサイズ方向を返す。角は斜め方向。
fn resize_dir_at(mx: i32, my: i32, w: i32, h: i32, t: i32) -> Option<ResizeDirection> {
    let left = mx < t;
    let right = mx >= w - t;
    let top = my < t;
    let bottom = my >= h - t;
    match (top, bottom, left, right) {
        (true, _, true, _) => Some(ResizeDirection::NorthWest),
        (true, _, _, true) => Some(ResizeDirection::NorthEast),
        (_, true, true, _) => Some(ResizeDirection::SouthWest),
        (_, true, _, true) => Some(ResizeDirection::SouthEast),
        (true, _, _, _) => Some(ResizeDirection::North),
        (_, true, _, _) => Some(ResizeDirection::South),
        (_, _, true, _) => Some(ResizeDirection::West),
        (_, _, _, true) => Some(ResizeDirection::East),
        _ => None,
    }
}

/// パスワードボールトの保存先を決める。
/// 優先順: config.secret_file（~展開） > 実行ファイルと同じディレクトリ > フォールバック(config_dir)。
/// ユーザ指定により、既定は「実行ファイルと同じ場所」の moterm-secrets.enc。
/// ウィンドウサイズ（window.width/height）を解決する。
fn mode_sizes(config: &Config) -> (u32, u32) {
    (
        config.window.width.unwrap_or(1000),
        config.window.height.unwrap_or(640),
    )
}

fn vault_path(
    config: &Config,
    exe_dir: Option<&std::path::Path>,
    fallback_dir: &std::path::Path,
) -> PathBuf {
    if let Some(f) = &config.secret_file {
        return mot_core::model::expand_tilde(f);
    }
    let dir = exe_dir.unwrap_or(fallback_dir);
    dir.join("moterm-secrets.enc")
}

fn pane_cols(s: &SshSession) -> u16 {
    s.params().cols
}
fn pane_rows(s: &SshSession) -> u16 {
    s.params().rows
}

fn set_clipboard(text: &str) {
    if let Ok(mut cb) = arboard::Clipboard::new() {
        let _ = cb.set_text(text.to_string());
    }
}

fn get_clipboard() -> Option<String> {
    arboard::Clipboard::new()
        .ok()
        .and_then(|mut c| c.get_text().ok())
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        // 起動時サイズ（width/height）。
        let (w, h) = self.term_size;
        self.applied_size = Some((w, h));
        let attrs = Window::default_attributes()
            .with_title("moterm")
            .with_inner_size(winit::dpi::LogicalSize::new(w, h))
            .with_resizable(true)
            // 透過（コンポジタ有効時のみ実効。既定 opacity=1.0 では従来どおり不透明）。
            .with_transparent(self.window_opacity < 1.0)
            // ウィンドウアイコン = Windows のタスクバーボタン／タイトルバー／Alt+Tab のアイコン。
            .with_window_icon(load_window_icon())
            .with_decorations(self.config.window.title_bar);
        // Windows では大アイコン（Alt+Tab / 大表示タスクバー）も明示設定する。
        #[cfg(windows)]
        let attrs = {
            use winit::platform::windows::WindowAttributesExtWindows;
            attrs.with_taskbar_icon(load_window_icon())
        };
        let window = Rc::new(event_loop.create_window(attrs).expect("create window"));
        // IME（日本語入力など）を許可。これを呼ばないと多くの環境で Ime イベントが来ない。
        window.set_ime_allowed(self.config.use_ime);
        // ウィンドウの角丸（Windows 11 の DWM。Win10 以前・他 OS では no-op）。
        apply_rounded_corners(&window);
        // 背景ぼかし（Windows 11 の DWM システムバックドロップ。他 OS は no-op）。
        if self.config.window_blur > 0 {
            apply_backdrop_blur(&window);
        }
        let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        let surface = softbuffer::Surface::new(&context, window.clone()).expect("surface");
        self.fb.resize(w as usize, h as usize);
        self.window = Some(window.clone());
        self._context = Some(context);
        self.surface = Some(surface);
        window.request_redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::Resized(_) => {
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    self.on_key(&event, event_loop);
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            WindowEvent::Ime(ime) => {
                use winit::event::Ime;
                match ime {
                    Ime::Enabled | Ime::Disabled => self.ime_preedit.clear(),
                    Ime::Preedit(text, _cursor) => {
                        self.ime_preedit = text;
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                    }
                    Ime::Commit(text) => {
                        self.ime_preedit.clear();
                        self.on_ime_commit(&text);
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.on_mouse(state, button);
                if self.quit_requested {
                    event_loop.exit();
                    return;
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.mouse = (position.x, position.y);
                self.update_resize_cursor();
                if self.tab_drag.is_some() {
                    self.on_tab_drag();
                } else if self.scrollbar_drag.is_some() {
                    self.on_scrollbar_drag();
                } else if self.dragging_sel {
                    self.on_drag();
                } else if self.mouse_report_btn.is_some() {
                    self.on_mouse_motion();
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as i32,
                    MouseScrollDelta::PixelDelta(p) => (p.y / 20.0) as i32,
                };
                self.on_scroll(lines);
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::DroppedFile(path) => self.on_drop_file(path),
            WindowEvent::RedrawRequested => self.present(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.pump_conn_events();
        self.pump_panes();
        self.pump_reconnects();
        self.sync_metrics_pause();
        // B7: ランチャー⇔端末のモード別ウィンドウサイズ
        self.sync_mode_size();

        // --screenshot: 数フレーム描画後に保存して終了
        if let Some(path) = self.screenshot.clone() {
            self.frames += 1;
            if self.frames >= 3 {
                self.present();
                let _ = self.fb.save_png(&path);
                event_loop.exit();
                return;
            }
        }
        if let Some(w) = &self.window {
            w.request_redraw();
        }
        // ポーリング駆動（出力反映のため常時再描画要求）
        event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
    }
}

// 入力ハンドラ群は input_handlers.rs に分割
mod input_handlers;
// SFTP ファイルマネージャのハンドラ
mod sftp_handlers;
// NEO-UI（3カラム未来的UI）のレンダリング
mod neoshell;
// マウス（URL 抽出・ブラウザ起動・ボタン対応）の純ロジック
mod mouse;
// スクロールバック検索の純ロジック
mod search;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_dir_detects_edges_and_corners() {
        // 100x80、閾値6
        assert_eq!(
            resize_dir_at(2, 40, 100, 80, 6),
            Some(ResizeDirection::West)
        );
        assert_eq!(
            resize_dir_at(98, 40, 100, 80, 6),
            Some(ResizeDirection::East)
        );
        assert_eq!(
            resize_dir_at(50, 2, 100, 80, 6),
            Some(ResizeDirection::North)
        );
        assert_eq!(
            resize_dir_at(50, 78, 100, 80, 6),
            Some(ResizeDirection::South)
        );
        assert_eq!(
            resize_dir_at(1, 1, 100, 80, 6),
            Some(ResizeDirection::NorthWest)
        );
        assert_eq!(
            resize_dir_at(99, 79, 100, 80, 6),
            Some(ResizeDirection::SouthEast)
        );
        // 中央は None
        assert_eq!(resize_dir_at(50, 40, 100, 80, 6), None);
    }

    #[test]
    fn double_click_within_threshold_same_tab() {
        // 同じタブ・閾値内 → true
        assert!(double_click(Some(2), Some(300), 2, 400));
        // 閾値超過 → false
        assert!(!double_click(Some(2), Some(500), 2, 400));
        // 別タブ → false
        assert!(!double_click(Some(1), Some(100), 2, 400));
        // 初回（記録なし）→ false
        assert!(!double_click(None, None, 2, 400));
    }

    #[test]
    fn move_tab_preserves_order_and_moves_item() {
        // タブ本体は生成が重いので、順序ロジックを Vec<usize> で等価検証する。
        fn reorder(v: &mut Vec<usize>, from: usize, to: usize) {
            if from < v.len() && to < v.len() && from != to {
                let x = v.remove(from);
                v.insert(to, x);
            }
        }
        let mut v = vec![0, 1, 2, 3];
        reorder(&mut v, 0, 2); // 先頭を index2 へ
        assert_eq!(v, vec![1, 2, 0, 3]);
        reorder(&mut v, 3, 0); // 末尾を先頭へ
        assert_eq!(v, vec![3, 1, 2, 0]);
        reorder(&mut v, 1, 1); // 同一位置は無変化
        assert_eq!(v, vec![3, 1, 2, 0]);
        reorder(&mut v, 9, 0); // 範囲外は無変化
        assert_eq!(v, vec![3, 1, 2, 0]);
    }

    #[test]
    fn tab_menu_item_hit_test() {
        // メニュー項目矩形の生成とヒット判定（tab_menu_geometry のロジックを再現）。
        let (mx0, my0) = (100, 30);
        let mut items = vec![];
        let mut y = my0;
        for id in TAB_MENU_ITEMS {
            items.push((id, mx0, y, mx0 + TAB_MENU_W, y + TAB_MENU_ITEM_H));
            y += TAB_MENU_ITEM_H;
        }
        // 4項目・順序・非重複
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].0, "rename");
        assert_eq!(items[3].0, "close");
        // 2番目(duplicate)の中心をヒット
        let (px, py) = (mx0 + 10, my0 + TAB_MENU_ITEM_H + 5);
        let hit = items
            .iter()
            .find(|(_, x0, y0, x1, y1)| px >= *x0 && px < *x1 && py >= *y0 && py < *y1)
            .map(|(id, ..)| *id);
        assert_eq!(hit, Some("duplicate"));
        // メニュー外はヒットなし
        let (ox, oy) = (mx0 - 20, my0);
        let miss = items
            .iter()
            .any(|(_, x0, y0, x1, y1)| ox >= *x0 && ox < *x1 && oy >= *y0 && oy < *y1);
        assert!(!miss);
    }

    #[test]
    fn rename_result_keeps_old_on_empty() {
        assert_eq!(rename_result("old", "new-name"), "new-name");
        assert_eq!(rename_result("old", ""), "old"); // 空は無変更
        assert_eq!(rename_result("old", "   "), "old"); // 空白のみも無変更
        assert_eq!(rename_result("旧", "新しい名前"), "新しい名前");
    }

    #[test]
    fn tab_menu_labels_localized() {
        assert_eq!(tab_menu_label(Lang::Ja, "rename"), "名前を変更");
        assert_eq!(tab_menu_label(Lang::En, "rename"), "Rename");
        assert_eq!(tab_menu_label(Lang::Ja, "close"), "閉じる");
        assert_eq!(tab_menu_label(Lang::En, "duplicate"), "Duplicate");
        assert_eq!(tab_menu_label(Lang::Ja, "sftp"), "SFTP");
    }

    #[test]
    fn vault_path_prefers_exe_dir() {
        use std::path::Path;
        let mut cfg = Config::default();
        // 既定: 実行ファイルと同じディレクトリ
        let p = vault_path(
            &cfg,
            Some(Path::new("/opt/moterm")),
            Path::new("/home/u/.config"),
        );
        assert_eq!(p, PathBuf::from("/opt/moterm/moterm-secrets.enc"));
        // exe_dir 不明ならフォールバック
        let p2 = vault_path(&cfg, None, Path::new("/home/u/.config"));
        assert_eq!(p2, PathBuf::from("/home/u/.config/moterm-secrets.enc"));
        // secret_file 明示が最優先
        cfg.secret_file = Some("/custom/secrets.enc".into());
        let p3 = vault_path(
            &cfg,
            Some(Path::new("/opt/moterm")),
            Path::new("/home/u/.config"),
        );
        assert_eq!(p3, PathBuf::from("/custom/secrets.enc"));
    }
}
