//! SFTP 2ペインファイルマネージャ（F3）。左=ローカル / 右=リモート。
//! 状態・純ロジック（ソート/スクロール追従/転送方向）と描画をまとめる。
//! 実際の I/O（std::fs / mot_ssh::Sftp）は App 側（app/sftp_handlers.rs）が行う。

use crate::font::FontManager;
use crate::render::Framebuffer;
use crate::theme::Theme;
use mot_core::i18n::{tr, Lang};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Local,
    Remote,
}

/// 一覧の1エントリ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    /// 更新日時（UNIX 秒、不明は 0）
    pub mtime: u64,
    /// 親ディレクトリ（".."）エントリか
    pub parent: bool,
}

impl Entry {
    pub fn file(name: impl Into<String>, size: u64) -> Self {
        Entry {
            name: name.into(),
            is_dir: false,
            size,
            mtime: 0,
            parent: false,
        }
    }
    pub fn dir(name: impl Into<String>) -> Self {
        Entry {
            name: name.into(),
            is_dir: true,
            size: 0,
            mtime: 0,
            parent: false,
        }
    }
    /// 更新日時を設定して返す（ビルダー）。
    pub fn with_mtime(mut self, mtime: u64) -> Self {
        self.mtime = mtime;
        self
    }
}

/// 一覧のソートキー（列に対応）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Date,
}

impl SortKey {
    /// s キーでの巡回: 名前→サイズ→日付→名前。
    pub fn cycle(self) -> SortKey {
        match self {
            SortKey::Name => SortKey::Size,
            SortKey::Size => SortKey::Date,
            SortKey::Date => SortKey::Name,
        }
    }
}

/// 片側ペインの状態。
pub struct PaneFm {
    pub side: Side,
    pub cwd: String,
    pub entries: Vec<Entry>,
    pub sel: usize,
    pub scroll: usize,
    /// 直近のエラー等の短いメッセージ（未接続案内など）
    pub note: Option<String>,
    /// ソートキー（列）。
    pub sort_key: SortKey,
    /// 昇順か（false=降順）。
    pub sort_asc: bool,
    /// 現一覧に「..」を含めるか（再ソート時に維持するため保持）。
    pub has_parent: bool,
    /// 複数選択でマークされたファイル名の集合（ファイルのみ。ディレクトリ/".." は入らない）。
    /// ディレクトリ移動・再読込（set_listing）でクリアされる。
    pub marked: std::collections::HashSet<String>,
}

impl PaneFm {
    pub fn new(side: Side, cwd: String) -> Self {
        PaneFm {
            side,
            cwd,
            entries: Vec::new(),
            sel: 0,
            scroll: 0,
            note: None,
            sort_key: SortKey::Name,
            sort_asc: true,
            has_parent: false,
            marked: std::collections::HashSet::new(),
        }
    }

    /// 生の一覧を現在のソート指定でソートして「..」を先頭に付けてセットする。
    /// 別ディレクトリ/再読込では複数選択マークはクリアする（古い名前を残さない）。
    pub fn set_listing(&mut self, raw: Vec<Entry>, include_parent: bool) {
        self.has_parent = include_parent;
        self.entries = sorted_listing(raw, include_parent, self.sort_key, self.sort_asc);
        self.sel = self.sel.min(self.entries.len().saturating_sub(1));
        self.scroll = 0;
        self.marked.clear();
    }

    /// 列見出しクリック / s キーでソートを変更する。
    /// 同じ列を再指定したら昇順⇔降順をトグル、別の列なら昇順にリセット。
    pub fn set_sort(&mut self, key: SortKey) {
        if self.sort_key == key {
            self.sort_asc = !self.sort_asc;
        } else {
            self.sort_key = key;
            self.sort_asc = true;
        }
        self.re_sort();
    }

    /// 現在の一覧を今のソート指定で並べ直す（選択は名前で追従）。
    pub fn re_sort(&mut self) {
        let keep = self.selected().map(|e| e.name.clone());
        let cur = std::mem::take(&mut self.entries);
        self.entries = sorted_listing(cur, self.has_parent, self.sort_key, self.sort_asc);
        if let Some(name) = keep {
            if let Some(i) = self.entries.iter().position(|e| e.name == name) {
                self.sel = i;
            }
        }
        self.sel = self.sel.min(self.entries.len().saturating_sub(1));
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.entries.get(self.sel)
    }

    pub fn move_sel(&mut self, delta: i32) {
        if self.entries.is_empty() {
            return;
        }
        let n = self.entries.len() as i32;
        self.sel = (self.sel as i32 + delta).clamp(0, n - 1) as usize;
    }

    /// 現在の選択行のマークをトグルして1つ下へ進む。
    /// ディレクトリもマークできる（転送は中身ごと再帰）。".." はマークできない。
    pub fn toggle_mark_sel(&mut self) {
        if let Some(e) = self.entries.get(self.sel) {
            if !e.parent {
                let name = e.name.clone();
                if !self.marked.remove(&name) {
                    self.marked.insert(name);
                }
            }
        }
        self.move_sel(1);
    }

    /// 指定インデックスのマークをトグルする（マウス Ctrl+クリック用。".." は不可）。
    pub fn toggle_mark_at(&mut self, idx: usize) {
        if let Some(e) = self.entries.get(idx) {
            if !e.parent {
                let name = e.name.clone();
                if !self.marked.remove(&name) {
                    self.marked.insert(name);
                }
            }
        }
    }

    /// 一覧中の `name` がディレクトリか（無ければ false）。
    /// 転送/削除で名前しか持たない場面から種別を引くのに使う。
    pub fn is_dir_named(&self, name: &str) -> bool {
        self.entries
            .iter()
            .any(|e| e.name == name && e.is_dir && !e.parent)
    }

    /// マークをすべて解除する。
    pub fn clear_marks(&mut self) {
        self.marked.clear();
    }

    /// マークされたファイル名を現在の表示順で返す（一覧に無いマークは無視）。
    pub fn marked_names(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|e| self.marked.contains(&e.name))
            .map(|e| e.name.clone())
            .collect()
    }

    /// 転送/削除の対象名リスト。マークがあればそれ（表示順）、無ければ現在の選択1件
    /// （".." は除外）。ディレクトリを含むことがあり、その場合は中身ごと再帰的に扱う。
    pub fn action_targets(&self) -> Vec<String> {
        let marked = self.marked_names();
        if !marked.is_empty() {
            return marked;
        }
        match self.selected() {
            Some(e) if !e.parent => vec![e.name.clone()],
            _ => Vec::new(),
        }
    }
}

/// 入力プロンプト（新規フォルダ名 / リネーム）。
pub struct FmInput {
    pub kind: InputKind,
    pub buffer: String,
    pub side: Side,
    /// リネーム対象の元名（Rename のみ）
    pub target: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Mkdir,
    Rename,
}

/// y/n 確認（上書き / 削除）。
pub struct FmConfirm {
    pub message: String,
    pub action: PendingAction,
}

/// 確認後に実行する操作。
#[derive(Debug, Clone)]
pub enum PendingAction {
    /// from 側の name を to 側 cwd へ転送
    Transfer { from: Side, name: String },
    /// side の name を削除（is_dir で remove_dir/remove_file を選ぶ）
    Delete {
        side: Side,
        name: String,
        is_dir: bool,
    },
    /// from 側の複数ファイル names を反対側へ一括転送（上書き確認後、サイレント上書き）
    TransferMany { from: Side, names: Vec<String> },
    /// side の複数ファイル names を一括削除（マーク削除。ファイルのみ）
    DeleteMany { side: Side, names: Vec<String> },
    /// from 側（ソース）から反対側（宛先）へ names を一括ミラー転送（サイレント上書き）
    Mirror { from: Side, names: Vec<String> },
}

/// 進行中の SFTP 転送の進捗（転送ワーカーからのイベントで更新）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FmProgress {
    /// 現在転送中のファイル名
    pub name: String,
    pub done: u64,
    pub total: u64,
    /// 何個目か（1始まり。複数転送・ミラー用）
    pub index: usize,
    pub count: usize,
}

/// 進捗の表示文字列。"name  42%  1.2M/3.0M  [2/5]"（単発は [i/n] を省略）。
pub fn fmt_progress(p: &FmProgress) -> String {
    let pct = (p.done * 100).checked_div(p.total).unwrap_or(0).min(100);
    let mut s = format!(
        "{}  {}%  {}/{}",
        p.name,
        pct,
        fmt_size(p.done),
        fmt_size(p.total)
    );
    if p.count > 1 {
        s.push_str(&format!("  [{}/{}]", p.index, p.count));
    }
    s
}

/// ローカルペインのドライブ選択オーバーレイ（Windows）。
/// ドライブのルート一覧（"C:\\" 形式）から1つを選んで移動する。
pub struct DrivePicker {
    /// 選択候補（"C:\\", "D:\\" …）。少なくとも1件ある前提で開く。
    pub drives: Vec<String>,
    pub sel: usize,
}

impl DrivePicker {
    /// preselect（現在のドライブルート）があればそこを初期選択にする。
    pub fn new(drives: Vec<String>, preselect: Option<&str>) -> Self {
        let sel = preselect
            .and_then(|p| drives.iter().position(|d| d.eq_ignore_ascii_case(p)))
            .unwrap_or(0);
        DrivePicker { drives, sel }
    }

    pub fn move_sel(&mut self, delta: i32) {
        if self.drives.is_empty() {
            return;
        }
        let n = self.drives.len() as i32;
        self.sel = (self.sel as i32 + delta).clamp(0, n - 1) as usize;
    }

    pub fn selected(&self) -> Option<&String> {
        self.drives.get(self.sel)
    }
}

/// Windows の GetLogicalDrives ビットマスク（bit0=A, bit1=B, …）を
/// ドライブルート文字列一覧（"C:\\" 形式）へ変換する純関数。
/// 実利用は Windows のみだが、テストは全環境で回すため常にコンパイルする。
#[cfg_attr(not(windows), allow(dead_code))]
pub fn drives_from_bitmask(mask: u32) -> Vec<String> {
    (0u32..26)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| format!("{}:\\", (b'A' + i as u8) as char))
        .collect()
}

pub struct FileManager {
    pub local: PaneFm,
    pub remote: PaneFm,
    pub active: Side,
    pub input: Option<FmInput>,
    pub confirm: Option<FmConfirm>,
    pub status: Option<String>,
    /// Some の間は転送中（下部帯にバー表示、新規転送はブロック）。
    pub progress: Option<FmProgress>,
    /// Some の間はドライブ選択中（ローカルペイン、Windows）。
    pub drive_picker: Option<DrivePicker>,
}

impl FileManager {
    pub fn new(local_cwd: String, remote_cwd: String) -> Self {
        FileManager {
            local: PaneFm::new(Side::Local, local_cwd),
            remote: PaneFm::new(Side::Remote, remote_cwd),
            active: Side::Local,
            input: None,
            confirm: None,
            status: None,
            progress: None,
            drive_picker: None,
        }
    }

    pub fn pane(&self, side: Side) -> &PaneFm {
        match side {
            Side::Local => &self.local,
            Side::Remote => &self.remote,
        }
    }
    pub fn pane_mut(&mut self, side: Side) -> &mut PaneFm {
        match side {
            Side::Local => &mut self.local,
            Side::Remote => &mut self.remote,
        }
    }
    pub fn active_pane(&self) -> &PaneFm {
        self.pane(self.active)
    }
    pub fn active_pane_mut(&mut self) -> &mut PaneFm {
        let s = self.active;
        self.pane_mut(s)
    }
    pub fn toggle_active(&mut self) {
        self.active = other_side(self.active);
    }
}

/// 転送方向: アクティブ側 → 反対側。
pub fn transfer_dir(active: Side) -> (Side, Side) {
    (active, other_side(active))
}

pub fn other_side(s: Side) -> Side {
    match s {
        Side::Local => Side::Remote,
        Side::Remote => Side::Local,
    }
}

/// ディレクトリ先頭を維持しつつ、指定キー・方向でソートし、
/// include_parent なら先頭へ「..」を足す。
/// ".." は常に最上段、ディレクトリはファイルより常に先（dirs-first は方向に依らず固定）、
/// 同区分内だけを key と asc で並べる。
pub fn sorted_listing(
    mut entries: Vec<Entry>,
    include_parent: bool,
    key: SortKey,
    asc: bool,
) -> Vec<Entry> {
    use std::cmp::Ordering;
    // 「.」と既存の「..」は取り除く（重複防止）
    entries.retain(|e| e.name != "." && e.name != "..");
    entries.sort_by(|a, b| {
        // dirs-first は固定。
        match b.is_dir.cmp(&a.is_dir) {
            Ordering::Equal => {}
            other => return other,
        }
        // ディレクトリ同士は常に名前昇順（方向に依らず入れ替えない）。
        if a.is_dir && b.is_dir {
            return a.name.to_lowercase().cmp(&b.name.to_lowercase());
        }
        // ファイル同士はキーで比較。
        let ord = match key {
            SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortKey::Size => a.size.cmp(&b.size),
            SortKey::Date => a.mtime.cmp(&b.mtime),
        };
        // 同値は名前昇順で安定化。
        let ord = ord.then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        if asc {
            ord
        } else {
            ord.reverse()
        }
    });
    if include_parent {
        let mut out = Vec::with_capacity(entries.len() + 1);
        out.push(Entry {
            name: "..".into(),
            is_dir: true,
            size: 0,
            mtime: 0,
            parent: true,
        });
        out.extend(entries);
        out
    } else {
        entries
    }
}

/// 片方向ミラー同期の転送対象を算出する。
/// src（ソース）の各ファイルについて、dst（宛先）と比較し、次のいずれかなら転送対象:
///   - dst に同名が無い
///   - dst 側の size が違う
///   - src の mtime が dst の mtime より新しい
///
/// dst に同名があり size 一致かつ mtime が dst >= src ならスキップ。
/// ディレクトリ・".." （parent）・ドット始まりの名前は常に除外（ファイルのみ・再帰なし）。
/// 返すのは転送すべきファイル名のリスト（src の順）。
pub fn mirror_plan(src: &[Entry], dst: &[Entry]) -> Vec<String> {
    src.iter()
        .filter(|e| !e.is_dir && !e.parent && !e.name.starts_with('.'))
        .filter(|s| match dst.iter().find(|d| d.name == s.name) {
            None => true,                                     // 宛先に無い
            Some(d) => s.size != d.size || s.mtime > d.mtime, // 差分あり
        })
        .map(|e| e.name.clone())
        .collect()
}

/// UNIX 秒を UTC の "YYYY-MM-DD HH:MM" へ整形する（0 は "-"）。
/// chrono を使わず Howard Hinnant の civil-from-days アルゴリズムで計算する。
pub fn fmt_mtime(secs: u64) -> String {
    if secs == 0 {
        return "-".to_string();
    }
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (hh, mm) = (rem / 3600, (rem % 3600) / 60);
    // civil_from_days: 1970-01-01 を 0 とする通日 → (y, m, d)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    format!("{year:04}-{m:02}-{d:02} {hh:02}:{mm:02}")
}

/// 選択位置が可視範囲に入るようスクロール量を調整して返す。
pub fn scroll_follow(sel: usize, scroll: usize, visible: usize, total: usize) -> usize {
    if visible == 0 || total <= visible {
        return 0;
    }
    let max_scroll = total - visible;
    let mut s = scroll.min(max_scroll);
    if sel < s {
        s = sel;
    } else if sel >= s + visible {
        s = sel + 1 - visible;
    }
    s.min(max_scroll)
}

/// リモート(POSIX)のパス結合。
pub fn remote_join(cwd: &str, name: &str) -> String {
    if cwd.ends_with('/') {
        format!("{cwd}{name}")
    } else {
        format!("{cwd}/{name}")
    }
}

/// リモート(POSIX)の親ディレクトリ。
pub fn remote_parent(cwd: &str) -> String {
    let trimmed = cwd.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(i) => trimmed[..i].to_string(),
    }
}

/// バイト数を短い可読表記へ。
pub fn fmt_size(n: u64) -> String {
    const U: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n}{}", U[0])
    } else {
        format!("{v:.1}{}", U[i])
    }
}

// ----------------------------------------------------------------------
// レイアウト & ヒットテスト（描画とマウス判定で共有＝ずれ防止）
// ----------------------------------------------------------------------

/// 片側ペインの画面矩形。
#[derive(Debug, Clone, Copy)]
pub struct PaneRect {
    pub side: Side,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub bottom: i32,
}

/// FM 全体のレイアウト（両ペイン矩形）。
#[derive(Debug, Clone, Copy)]
pub struct FmLayout {
    pub local: PaneRect,
    pub remote: PaneRect,
}

/// フレームバッファ寸法と行高からレイアウトを決める（描画・ヒットテスト共通）。
pub fn fm_layout(fb_w: i32, fb_h: i32, lh: i32) -> FmLayout {
    let top = 4;
    let bottom = fb_h - lh - 8;
    let mid = fb_w / 2;
    FmLayout {
        local: PaneRect {
            side: Side::Local,
            x: 0,
            y: top,
            w: mid - 2,
            bottom,
        },
        remote: PaneRect {
            side: Side::Remote,
            x: mid + 2,
            y: top,
            w: fb_w - mid - 2,
            bottom,
        },
    }
}

/// 一覧の先頭行 y。ヘッダ2行（側ラベル＋列見出し）の下。
pub fn fm_list_top(y: i32, lh: i32) -> i32 {
    y + 2 * lh + 6
}

/// 列見出し行の上端 y。
pub fn fm_header_row_y(y: i32, lh: i32) -> i32 {
    y + lh + 4
}

/// 列の境界 x を返す: (サイズ列左, 日付列左, 右端)。
pub fn col_bounds(x: i32, w: i32, cw: i32) -> (i32, i32, i32) {
    let right = x + w - 10;
    let date_w = 16 * cw; // "YYYY-MM-DD HH:MM"
    let size_w = 8 * cw;
    let date_left = right - date_w;
    let size_left = date_left - size_w;
    (size_left, date_left, right)
}

/// マウス位置がペインのどこを指すか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FmHit {
    /// 列見出し（クリックでソート）
    Header(SortKey),
    /// 一覧の行（entries のインデックス）
    Row(usize),
    None,
}

/// ペイン内のヒットテスト。scroll/total は表示中ペインの値。
pub fn fm_hit(
    rect: &PaneRect,
    lh: i32,
    cw: i32,
    scroll: usize,
    total: usize,
    mx: i32,
    my: i32,
) -> FmHit {
    if mx < rect.x || mx >= rect.x + rect.w {
        return FmHit::None;
    }
    let hdr_y = fm_header_row_y(rect.y, lh);
    if my >= hdr_y && my < hdr_y + lh {
        let (size_left, date_left, _right) = col_bounds(rect.x, rect.w, cw);
        let key = if mx >= date_left {
            SortKey::Date
        } else if mx >= size_left {
            SortKey::Size
        } else {
            SortKey::Name
        };
        return FmHit::Header(key);
    }
    let list_top = fm_list_top(rect.y, lh);
    if my >= list_top && my < rect.bottom {
        let row = ((my - list_top) / lh) as usize;
        let idx = scroll + row;
        if idx < total {
            return FmHit::Row(idx);
        }
    }
    FmHit::None
}

/// x 座標がどちらのペイン領域か。
pub fn fm_pane_at(layout: &FmLayout, mx: i32) -> Option<Side> {
    if mx >= layout.local.x && mx < layout.local.x + layout.local.w {
        Some(Side::Local)
    } else if mx >= layout.remote.x && mx < layout.remote.x + layout.remote.w {
        Some(Side::Remote)
    } else {
        None
    }
}

// ----------------------------------------------------------------------
// 描画
// ----------------------------------------------------------------------

/// FM 全体を描画する。scroll は選択追従で内部調整する。
pub fn draw(
    fb: &mut Framebuffer,
    font: &mut FontManager,
    theme: &Theme,
    fm: &mut FileManager,
    px: f32,
    lang: Lang,
) {
    fb.clear(theme.bg);
    let lh = font.cell_height(px);
    let w = fb.w as i32;
    let h = fb.h as i32;
    let layout = fm_layout(w, h, lh);
    let bottom = layout.local.bottom;

    let active = fm.active;
    let lr = layout.local;
    draw_pane(
        fb,
        font,
        theme,
        &mut fm.local,
        px,
        lang,
        lr,
        active == Side::Local,
    );
    let rr = layout.remote;
    draw_pane(
        fb,
        font,
        theme,
        &mut fm.remote,
        px,
        lang,
        rr,
        active == Side::Remote,
    );

    // 下部: 入力/確認/ヒント
    let by = h - lh - 2;
    fb.fill_rect(0, bottom + 2, w, h - bottom - 2, theme.ui_bg);
    if let Some(inp) = &fm.input {
        let label = match inp.kind {
            InputKind::Mkdir => "New folder",
            InputKind::Rename => "Rename",
        };
        let s = format!("{label}: {}_", inp.buffer);
        fb.draw_text(font, &s, 10, by + lh - 4, px, theme.ui_fg);
    } else if let Some(cf) = &fm.confirm {
        let s = format!("{}  (y/n)", cf.message);
        fb.draw_text(font, &s, 10, by + lh - 4, px, theme.warn);
    } else if let Some(p) = &fm.progress {
        // 転送中: 帯全体を進捗バーとして塗り、上にテキストを重ねる
        let ratio = if p.total > 0 {
            (p.done as f64 / p.total as f64).min(1.0)
        } else {
            0.0
        };
        let bar_w = ((w - 4) as f64 * ratio) as i32;
        if bar_w > 0 {
            fb.fill_rect(2, by, bar_w, lh, theme.ui_accent);
        }
        fb.draw_text(font, &fmt_progress(p), 10, by + lh - 4, px, theme.ui_fg);
    } else {
        let hint = "Tab: switch  Enter: open/transfer  drag: transfer  s/click header: sort  m: mirror  Backspace: up  F5: reload  F7: mkdir  F2: rename  F8: delete  Esc/F3: close";
        fb.draw_text(font, hint, 10, by + lh - 4, px, theme.ui_dim);
        if let Some(st) = &fm.status {
            // ステータスは上部帯にも出す
            fb.draw_text(font, st, w / 2, by + lh - 4, px, theme.ui_accent);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_pane(
    fb: &mut Framebuffer,
    font: &mut FontManager,
    theme: &Theme,
    pane: &mut PaneFm,
    px: f32,
    lang: Lang,
    rect: PaneRect,
    is_active: bool,
) {
    let (x, y, w, bottom) = (rect.x, rect.y, rect.w, rect.bottom);
    let lh = font.cell_height(px);
    let cw = font.cell_width(px);
    // 背景・枠
    let panel = if is_active {
        theme.ui_panel
    } else {
        theme.ui_bg
    };
    fb.fill_rect(x, y, w, bottom - y, panel);
    let border = if is_active {
        theme.ui_accent
    } else {
        theme.ui_dim
    };
    fb.stroke_rect(x, y, w, bottom - y, border);

    // ヘッダ1: 側ラベル + cwd + [n/total]
    let side_label = match pane.side {
        Side::Local => tr(lang, "local"),
        Side::Remote => tr(lang, "remote"),
    };
    let total = pane.entries.len();
    let header = format!(
        "{}  {}  [{}/{}]",
        side_label,
        elide(&pane.cwd, 40),
        if total == 0 { 0 } else { pane.sel + 1 },
        total
    );
    let hfg = if is_active { theme.ui_fg } else { theme.ui_dim };
    fb.draw_text(font, &header, x + 8, y + lh, px, hfg);

    // ヘッダ2: 列見出し（名前 / サイズ / 日付）＋ソート矢印。クリック可能。
    let (size_left, date_left, right) = col_bounds(x, w, cw);
    let hdr_y = fm_header_row_y(y, lh);
    let hdr_base = hdr_y + lh - 4;
    let arrow = |k: SortKey| -> &'static str {
        if pane.sort_key == k {
            if pane.sort_asc {
                " ▲"
            } else {
                " ▼"
            }
        } else {
            ""
        }
    };
    let col_fg = |k: SortKey| {
        if pane.sort_key == k {
            theme.ui_accent
        } else {
            theme.ui_dim
        }
    };
    let name_lbl = format!("{}{}", col_label(lang, SortKey::Name), arrow(SortKey::Name));
    let size_lbl = format!("{}{}", col_label(lang, SortKey::Size), arrow(SortKey::Size));
    let date_lbl = format!("{}{}", col_label(lang, SortKey::Date), arrow(SortKey::Date));
    fb.draw_text(font, &name_lbl, x + 10, hdr_base, px, col_fg(SortKey::Name));
    fb.draw_text(
        font,
        &size_lbl,
        size_left,
        hdr_base,
        px,
        col_fg(SortKey::Size),
    );
    fb.draw_text(
        font,
        &date_lbl,
        date_left,
        hdr_base,
        px,
        col_fg(SortKey::Date),
    );
    // 見出し下の区切り線
    fb.fill_rect(x + 2, hdr_y + lh, w - 4, 1, theme.ui_dim);

    let list_top = fm_list_top(y, lh);
    let visible = ((bottom - list_top) / lh).max(1) as usize;
    // 選択追従
    pane.scroll = scroll_follow(pane.sel, pane.scroll, visible, total);

    // 注記は一覧が空のときだけ表示（エントリと重ならないように）
    if pane.entries.is_empty() {
        if let Some(note) = &pane.note {
            fb.draw_text(font, note, x + 8, list_top + lh, px, theme.ui_dim);
        }
    }

    let dir_color = theme.ansi[12]; // 明るい青
    let name_max_cols = ((size_left - (x + 10)) / cw).max(1) as usize;
    for row in 0..visible {
        let idx = pane.scroll + row;
        if idx >= total {
            break;
        }
        let e = &pane.entries[idx];
        let ry = list_top + row as i32 * lh;
        let selected = idx == pane.sel;
        if selected {
            let selbg = if is_active {
                theme.selection
            } else {
                theme.ui_panel
            };
            fb.fill_rect(x + 2, ry, w - 4, lh, selbg);
        }
        let raw_name = if e.is_dir {
            format!("{}/", e.name)
        } else {
            e.name.clone()
        };
        let name = elide_tail(&raw_name, name_max_cols);
        let fg = if e.is_dir {
            dir_color
        } else if is_active {
            theme.ui_fg
        } else {
            theme.ui_dim
        };
        fb.draw_text(font, &name, x + 10, ry + lh - 4, px, fg);
        // サイズ（ファイルは可読表記、ディレクトリは "-"、右寄せでサイズ列内）
        let s = if e.is_dir {
            "-".to_string()
        } else {
            fmt_size(e.size)
        };
        let sw = s.chars().count() as i32 * cw;
        fb.draw_text(font, &s, date_left - sw - 6, ry + lh - 4, px, theme.ui_dim);
        // 日付（右寄せで右端）
        let d = fmt_mtime(e.mtime);
        let dw = d.chars().count() as i32 * cw;
        fb.draw_text(font, &d, right - dw, ry + lh - 4, px, theme.ui_dim);
    }

    // スクロールバー
    if total > visible {
        let track_h = (bottom - list_top).max(1);
        let thumb_h = ((visible as f32 / total as f32) * track_h as f32).max(8.0) as i32;
        let max_scroll = (total - visible) as f32;
        let t = if max_scroll > 0.0 {
            pane.scroll as f32 / max_scroll
        } else {
            0.0
        };
        let thumb_y = list_top + ((track_h - thumb_h) as f32 * t) as i32;
        fb.fill_rect(x + w - 4, thumb_y, 3, thumb_h, theme.ui_dim);
    }
}

/// 列見出しラベル（日英）。mot-core の i18n には size/date が無いためここで持つ。
pub fn col_label(lang: Lang, key: SortKey) -> &'static str {
    match (lang, key) {
        (Lang::Ja, SortKey::Name) => "名前",
        (Lang::En, SortKey::Name) => "Name",
        (Lang::Ja, SortKey::Size) => "サイズ",
        (Lang::En, SortKey::Size) => "Size",
        (Lang::Ja, SortKey::Date) => "更新日時",
        (Lang::En, SortKey::Date) => "Date",
    }
}

/// 表示幅（文字数）に収まるよう末尾を「…」で切り詰める。
fn elide_tail(s: &str, max_cols: usize) -> String {
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

/// 長いパスを末尾優先で省略する。
fn elide(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    let tail: String = chars[chars.len() - (max - 1)..].iter().collect();
    format!("…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_dirs_first_alpha_and_parent() {
        let raw = vec![
            Entry::file("zeta.txt", 10),
            Entry::dir("beta"),
            Entry::file("alpha.txt", 5),
            Entry::dir("Alpha"),
        ];
        let out = sorted_listing(raw, true, SortKey::Name, true);
        assert_eq!(out[0].name, ".."); // 親が先頭
        assert!(out[0].parent);
        // ディレクトリが先（Alpha, beta）、その後ファイル（alpha.txt, zeta.txt）
        assert_eq!(out[1].name, "Alpha");
        assert_eq!(out[2].name, "beta");
        assert_eq!(out[3].name, "alpha.txt");
        assert_eq!(out[4].name, "zeta.txt");
    }

    #[test]
    fn drives_from_bitmask_maps_bits_to_letters() {
        // bit0=A, bit2=C, bit3=D → A:\ C:\ D:\
        let m = (1 << 0) | (1 << 2) | (1 << 3);
        assert_eq!(drives_from_bitmask(m), vec!["A:\\", "C:\\", "D:\\"]);
        assert!(drives_from_bitmask(0).is_empty());
        // bit25=Z が最大。26 以上のビットは無視される。
        assert_eq!(drives_from_bitmask(1 << 25), vec!["Z:\\"]);
    }

    #[test]
    fn drive_picker_preselects_current_case_insensitive() {
        let drives = vec!["C:\\".to_string(), "D:\\".to_string(), "E:\\".to_string()];
        let dp = DrivePicker::new(drives.clone(), Some("d:\\"));
        assert_eq!(dp.sel, 1);
        assert_eq!(dp.selected().map(String::as_str), Some("D:\\"));
        // 見つからない/None は先頭。
        assert_eq!(DrivePicker::new(drives.clone(), Some("Z:\\")).sel, 0);
        assert_eq!(DrivePicker::new(drives, None).sel, 0);
    }

    #[test]
    fn drive_picker_move_sel_clamps() {
        let drives = vec!["C:\\".to_string(), "D:\\".to_string()];
        let mut dp = DrivePicker::new(drives, None);
        dp.move_sel(-1); // 先頭で上→据え置き
        assert_eq!(dp.sel, 0);
        dp.move_sel(1);
        assert_eq!(dp.sel, 1);
        dp.move_sel(1); // 末尾で下→据え置き
        assert_eq!(dp.sel, 1);
    }

    #[test]
    fn sort_removes_dot_entries_and_can_skip_parent() {
        let raw = vec![Entry::dir("."), Entry::dir(".."), Entry::file("a", 1)];
        let out = sorted_listing(raw, false, SortKey::Name, true);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "a");
    }

    #[test]
    fn mirror_plan_selects_new_changed_and_newer() {
        let src = vec![
            Entry::file("new.txt", 10).with_mtime(100), // (a) dst に無い
            Entry::file("resized.bin", 50).with_mtime(100), // (b) size 違い
            Entry::file("newer.log", 20).with_mtime(300), // (c) src が新しい
            Entry::file("same.dat", 30).with_mtime(100), // (d) 同一 → 除外
            Entry::file("older.dat", 30).with_mtime(50), // (d') src が古い → 除外
            Entry::dir("subdir"),                       // (e) dir → 除外
            Entry {
                name: "..".into(),
                is_dir: true,
                size: 0,
                mtime: 0,
                parent: true,
            }, // (e) ".." → 除外
            Entry::file(".hidden", 5).with_mtime(999),  // (e) ドット → 除外
        ];
        let dst = vec![
            Entry::file("resized.bin", 40).with_mtime(100), // size 違い
            Entry::file("newer.log", 20).with_mtime(200),   // 古い
            Entry::file("same.dat", 30).with_mtime(100),    // 同一
            Entry::file("older.dat", 30).with_mtime(100),   // dst の方が新しい
        ];
        let plan = mirror_plan(&src, &dst);
        assert_eq!(plan, vec!["new.txt", "resized.bin", "newer.log"]);
    }

    #[test]
    fn fmt_progress_single_and_batch() {
        let p = FmProgress {
            name: "big.iso".into(),
            done: 512 * 1024,
            total: 1024 * 1024,
            index: 1,
            count: 1,
        };
        assert_eq!(fmt_progress(&p), "big.iso  50%  512.0K/1.0M");
        let p2 = FmProgress {
            name: "a.txt".into(),
            done: 0,
            total: 0,
            index: 2,
            count: 5,
        };
        // total 不明(0)は 0% 扱い、複数転送は [i/n] を付ける
        assert_eq!(fmt_progress(&p2), "a.txt  0%  0B/0B  [2/5]");
    }

    #[test]
    fn mirror_plan_empty_when_in_sync() {
        let src = vec![
            Entry::file("a", 1).with_mtime(10),
            Entry::file("b", 2).with_mtime(20),
        ];
        let dst = vec![
            Entry::file("a", 1).with_mtime(10),
            Entry::file("b", 2).with_mtime(30), // dst が新しくても size 一致なら転送不要
        ];
        assert!(mirror_plan(&src, &dst).is_empty());
    }

    #[test]
    fn sort_by_size_and_date_keeps_dirs_first_and_parent() {
        let raw = vec![
            Entry::file("big.bin", 900).with_mtime(100),
            Entry::dir("dirB"),
            Entry::file("small.txt", 10).with_mtime(300),
            Entry::dir("dirA"),
            Entry::file("mid.dat", 400).with_mtime(200),
        ];
        // サイズ昇順: dirs 先頭（名前昇順）→ small(10), mid(400), big(900)
        let out = sorted_listing(raw.clone(), true, SortKey::Size, true);
        assert_eq!(out[0].name, ".."); // 常に先頭
        assert_eq!(out[1].name, "dirA");
        assert_eq!(out[2].name, "dirB");
        assert_eq!(out[3].name, "small.txt");
        assert_eq!(out[4].name, "mid.dat");
        assert_eq!(out[5].name, "big.bin");
        // サイズ降順: dirs はやはり先頭・名前昇順のまま、ファイルだけ反転
        let out = sorted_listing(raw.clone(), true, SortKey::Size, false);
        assert_eq!(out[1].name, "dirA");
        assert_eq!(out[2].name, "dirB");
        assert_eq!(out[3].name, "big.bin");
        assert_eq!(out[5].name, "small.txt");
        // 日付昇順: mtime 100,200,300 → big, mid, small
        let out = sorted_listing(raw, true, SortKey::Date, true);
        assert_eq!(out[3].name, "big.bin");
        assert_eq!(out[4].name, "mid.dat");
        assert_eq!(out[5].name, "small.txt");
    }

    #[test]
    fn set_sort_toggles_direction() {
        let mut p = PaneFm::new(Side::Local, "/".into());
        p.set_listing(
            vec![
                Entry::file("b", 2).with_mtime(1),
                Entry::file("a", 1).with_mtime(2),
            ],
            false,
        );
        assert_eq!((p.sort_key, p.sort_asc), (SortKey::Name, true));
        // 同じ列 → 降順トグル
        p.set_sort(SortKey::Name);
        assert!(!p.sort_asc);
        assert_eq!(p.entries[0].name, "b");
        // 別の列 → 昇順にリセット
        p.set_sort(SortKey::Size);
        assert_eq!((p.sort_key, p.sort_asc), (SortKey::Size, true));
        assert_eq!(p.entries[0].name, "a"); // size 1 が先
    }

    #[test]
    fn fmt_mtime_utc() {
        assert_eq!(fmt_mtime(0), "-");
        assert_eq!(fmt_mtime(1), "1970-01-01 00:00");
        // 2021-01-01 00:00:00 UTC = 1609459200
        assert_eq!(fmt_mtime(1_609_459_200), "2021-01-01 00:00");
        // 2009-02-13 23:31:30 UTC = 1234567890
        assert_eq!(fmt_mtime(1_234_567_890), "2009-02-13 23:31");
    }

    #[test]
    fn fm_hit_header_and_row() {
        let lh = 20;
        let cw = 8;
        let layout = fm_layout(1000, 700, lh);
        let rect = layout.local;
        // 列見出し行の各列
        let hdr_y = fm_header_row_y(rect.y, lh);
        let (size_left, date_left, _r) = col_bounds(rect.x, rect.w, cw);
        assert_eq!(
            fm_hit(&rect, lh, cw, 0, 5, rect.x + 20, hdr_y + 2),
            FmHit::Header(SortKey::Name)
        );
        assert_eq!(
            fm_hit(&rect, lh, cw, 0, 5, size_left + 2, hdr_y + 2),
            FmHit::Header(SortKey::Size)
        );
        assert_eq!(
            fm_hit(&rect, lh, cw, 0, 5, date_left + 2, hdr_y + 2),
            FmHit::Header(SortKey::Date)
        );
        // 一覧の行
        let list_top = fm_list_top(rect.y, lh);
        assert_eq!(
            fm_hit(&rect, lh, cw, 0, 5, rect.x + 20, list_top + 2),
            FmHit::Row(0)
        );
        assert_eq!(
            fm_hit(&rect, lh, cw, 0, 5, rect.x + 20, list_top + lh + 2),
            FmHit::Row(1)
        );
        // スクロール考慮
        assert_eq!(
            fm_hit(&rect, lh, cw, 3, 10, rect.x + 20, list_top + 2),
            FmHit::Row(3)
        );
        // 範囲外（total 超え）
        assert_eq!(
            fm_hit(&rect, lh, cw, 0, 1, rect.x + 20, list_top + lh + 2),
            FmHit::None
        );
        // 別ペイン領域
        assert_eq!(fm_pane_at(&layout, rect.x + 20), Some(Side::Local));
        assert_eq!(
            fm_pane_at(&layout, layout.remote.x + 20),
            Some(Side::Remote)
        );
    }

    #[test]
    fn scroll_follow_keeps_selection_visible() {
        // 20 件、可視 5 行
        assert_eq!(scroll_follow(0, 0, 5, 20), 0);
        assert_eq!(scroll_follow(4, 0, 5, 20), 0); // まだ見える
        assert_eq!(scroll_follow(5, 0, 5, 20), 1); // 1つ下へ
        assert_eq!(scroll_follow(19, 0, 5, 20), 15); // 末尾
                                                     // 上に戻る
        assert_eq!(scroll_follow(2, 10, 5, 20), 2);
        // 全件が可視に収まるならスクロールなし
        assert_eq!(scroll_follow(3, 2, 10, 5), 0);
    }

    #[test]
    fn transfer_direction() {
        assert_eq!(transfer_dir(Side::Local), (Side::Local, Side::Remote));
        assert_eq!(transfer_dir(Side::Remote), (Side::Remote, Side::Local));
        assert_eq!(other_side(Side::Local), Side::Remote);
    }

    #[test]
    fn remote_path_join_and_parent() {
        assert_eq!(remote_join("/home/user", "a.txt"), "/home/user/a.txt");
        assert_eq!(remote_join("/", "a.txt"), "/a.txt");
        assert_eq!(remote_parent("/home/user"), "/home");
        assert_eq!(remote_parent("/home"), "/");
        assert_eq!(remote_parent("/"), "/");
        assert_eq!(remote_parent("/a/b/c/"), "/a/b");
    }

    #[test]
    fn size_formatting() {
        assert_eq!(fmt_size(0), "0B");
        assert_eq!(fmt_size(512), "512B");
        assert_eq!(fmt_size(1024), "1.0K");
        assert_eq!(fmt_size(1536), "1.5K");
        assert_eq!(fmt_size(1048576), "1.0M");
    }

    #[test]
    fn toggle_mark_marks_files_and_dirs_but_not_parent() {
        let mut p = PaneFm::new(Side::Local, "/".into());
        p.set_listing(
            vec![
                Entry::dir("sub"),
                Entry::file("a.txt", 1),
                Entry::file("b.txt", 2),
            ],
            true, // ".." 先頭
        );
        // 並びは: [".." , "sub", "a.txt", "b.txt"]
        assert_eq!(p.sel, 0);
        // ".." はマーク不可、そのまま下へ
        p.toggle_mark_sel();
        assert!(p.marked.is_empty());
        assert_eq!(p.sel, 1);
        // "sub"（ディレクトリ）はマーク可（転送は中身ごと再帰）
        p.toggle_mark_sel();
        assert_eq!(p.marked_names(), vec!["sub"]);
        assert_eq!(p.sel, 2);
        // "a.txt" をマーク→下へ
        p.toggle_mark_sel();
        assert_eq!(p.marked_names(), vec!["sub", "a.txt"]);
        assert_eq!(p.sel, 3);
        // "b.txt" をマーク→末尾でクランプ
        p.toggle_mark_sel();
        assert_eq!(p.marked_names(), vec!["sub", "a.txt", "b.txt"]);
        assert_eq!(p.sel, 3);
        // 再トグルで解除
        p.sel = 2;
        p.toggle_mark_sel();
        assert_eq!(p.marked_names(), vec!["sub", "b.txt"]);
    }

    #[test]
    fn is_dir_named_resolves_kind_from_listing() {
        let mut p = PaneFm::new(Side::Local, "/".into());
        p.set_listing(vec![Entry::dir("sub"), Entry::file("a.txt", 1)], true);
        assert!(p.is_dir_named("sub"));
        assert!(!p.is_dir_named("a.txt"));
        assert!(!p.is_dir_named("nope"));
        // ".." はディレクトリだが対象外（削除・転送に混ぜない）
        assert!(!p.is_dir_named(".."));
    }

    #[test]
    fn action_targets_uses_marks_or_falls_back_to_selection() {
        let mut p = PaneFm::new(Side::Local, "/".into());
        p.set_listing(
            vec![Entry::file("a.txt", 1), Entry::file("b.txt", 2)],
            false,
        );
        // マーク無し→選択1件
        p.sel = 1;
        assert_eq!(p.action_targets(), vec!["b.txt"]);
        // マークあり→表示順のマーク集合（選択は無視）
        p.toggle_mark_at(0);
        p.toggle_mark_at(1);
        assert_eq!(p.action_targets(), vec!["a.txt", "b.txt"]);
        // ".." 選択で未マークなら空
        let mut q = PaneFm::new(Side::Local, "/sub".into());
        q.set_listing(vec![Entry::file("x", 1)], true);
        q.sel = 0; // ".."
        assert!(q.action_targets().is_empty());
    }

    #[test]
    fn set_listing_clears_marks() {
        let mut p = PaneFm::new(Side::Local, "/".into());
        p.set_listing(vec![Entry::file("a.txt", 1)], false);
        p.toggle_mark_at(0);
        assert_eq!(p.marked_names(), vec!["a.txt"]);
        // 別ディレクトリの読み込みでマークは消える
        p.set_listing(
            vec![Entry::file("a.txt", 1), Entry::file("c.txt", 3)],
            false,
        );
        assert!(p.marked.is_empty());
        assert!(p.action_targets().is_empty() || p.action_targets() == vec!["a.txt"]);
    }

    #[test]
    fn re_sort_keeps_marks() {
        let mut p = PaneFm::new(Side::Local, "/".into());
        p.set_listing(
            vec![Entry::file("a.txt", 30), Entry::file("b.txt", 10)],
            false,
        );
        p.toggle_mark_at(0); // "a.txt"
        p.set_sort(SortKey::Size); // 再ソート（set_listing は呼ばれない）
        assert_eq!(p.marked_names(), vec!["a.txt"]);
    }

    #[test]
    fn toggle_and_selection() {
        let mut fm = FileManager::new("/local".into(), "/remote".into());
        assert_eq!(fm.active, Side::Local);
        fm.toggle_active();
        assert_eq!(fm.active, Side::Remote);
        fm.local
            .set_listing(vec![Entry::file("a", 1), Entry::file("b", 2)], false);
        fm.local.move_sel(1);
        assert_eq!(fm.local.selected().unwrap().name, "b");
        fm.local.move_sel(5); // クランプ
        assert_eq!(fm.local.sel, 1);
    }
}
