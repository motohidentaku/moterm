//! App の入力ハンドラ（キー/マウス/IME/スクロール/ドロップ）。モード別ディスパッチ。

use super::*;
use winit::event::KeyEvent;
use winit::keyboard::{Key as WKey, NamedKey};

impl App {
    pub(super) fn on_key(&mut self, event: &KeyEvent, _event_loop: &ActiveEventLoop) {
        // モーダルが最優先
        if self.modal.is_some() {
            self.modal_key(event);
            return;
        }
        // タブ名インライン編集中
        if self.tab_rename.is_some() {
            self.tab_rename_key(event);
            return;
        }
        // タブメニュー表示中は Esc で閉じるのみ（他キーは無視）。
        if self.tab_menu.is_some() {
            if matches!(event.logical_key, WKey::Named(NamedKey::Escape)) {
                self.tab_menu = None;
            }
            return;
        }
        // 検索オーバーレイ
        if self.search.is_some() {
            self.search_key(event);
            return;
        }
        // サイドバーの Filter 入力にフォーカスがあればそこへキーを流す。
        if self.neo_filter_focus {
            self.neo_filter_key(event);
            return;
        }
        // サイドバーのキーボード選択モード中は ↑↓/Enter/Esc をそこで処理（端末へ漏らさない）。
        if self.neo_sidebar_sel.is_some() {
            self.neo_sidebar_nav_key(event);
            return;
        }
        // キーは接続タブの端末へ（GUI ショートカットも terminal_key 内で処理）。
        // SFTP は「アクティブタブの接続に紐づく時」だけ sftp_key へ。別タブでは端末へ。
        if self.mode == Mode::Sftp && self.fm_belongs_to_active() {
            self.sftp_key(event);
        } else {
            self.terminal_key(event);
        }
    }

    // ---------- モーダル ----------
    fn modal_key(&mut self, event: &KeyEvent) {
        let key = &event.logical_key;
        // 借用衝突を避けるため、閉じる判断はフラグに集約し、match 後に close_modal を呼ぶ。
        let mut close = false;
        match self.modal.as_mut().unwrap() {
            Modal::Tofu { reply, .. } => {
                if let WKey::Character(s) = key {
                    match s.as_str() {
                        "y" | "Y" => {
                            let _ = reply.send(true);
                            close = true;
                        }
                        "n" | "N" => {
                            let _ = reply.send(false);
                            close = true;
                        }
                        _ => {}
                    }
                } else if matches!(key, WKey::Named(NamedKey::Escape)) {
                    let _ = reply.send(false);
                    close = true;
                }
            }
            Modal::Password { input, reply, .. } | Modal::Passphrase { input, reply, .. } => {
                match key {
                    WKey::Named(NamedKey::Enter) => {
                        let _ = reply.send(Some(input.clone()));
                        close = true;
                    }
                    WKey::Named(NamedKey::Escape) => {
                        let _ = reply.send(None);
                        close = true;
                    }
                    WKey::Named(NamedKey::Backspace) => {
                        input.pop();
                    }
                    _ => {
                        if let Some(t) = &event.text {
                            input.push_str(t);
                        } else if let WKey::Character(s) = key {
                            input.push_str(s);
                        }
                    }
                }
            }
            Modal::Master { input, reply, .. } => match key {
                WKey::Named(NamedKey::Enter) => {
                    let _ = reply.send(Some(input.clone()));
                    close = true;
                }
                WKey::Named(NamedKey::Escape) => {
                    let _ = reply.send(None);
                    close = true;
                }
                WKey::Named(NamedKey::Backspace) => {
                    input.pop();
                }
                _ => {
                    if let Some(t) = &event.text {
                        if !t.chars().any(|c| c.is_control()) {
                            input.push_str(t);
                        }
                    }
                }
            },
            Modal::Overwrite { .. } => {
                close = matches!(key, WKey::Named(NamedKey::Escape))
                    || matches!(key, WKey::Character(s) if matches!(s.as_str(), "y" | "Y" | "n" | "N"));
            }
        }
        if close {
            self.close_modal();
        }
    }

    // ---------- 検索 ----------
    fn search_key(&mut self, event: &KeyEvent) {
        let key = &event.logical_key;
        let shift = self.mods.shift_key();
        let Some(s) = self.search.as_mut() else {
            return;
        };
        match key {
            WKey::Named(NamedKey::Escape) => {
                self.search = None;
            }
            WKey::Named(NamedKey::Backspace) => {
                s.query.pop();
                self.recompute_search();
            }
            WKey::Named(NamedKey::Enter) => {
                // Enter=次 / Shift+Enter=前
                self.step_search(if shift { -1 } else { 1 });
            }
            _ => {
                if let Some(t) = &event.text {
                    // 制御文字は無視
                    if !t.chars().any(|c| c.is_control()) {
                        s.query.push_str(t);
                        self.recompute_search();
                    }
                }
            }
        }
    }

    /// クエリ変更時にフォーカスペイン全行を走査してマッチを再計算し、先頭マッチへジャンプ。
    fn recompute_search(&mut self) {
        let query = match &self.search {
            Some(s) => s.query.to_lowercase(),
            None => return,
        };
        let mut found = Vec::new();
        if !query.is_empty() {
            if let Some(tab) = self.tabs.get(self.active_tab) {
                if let Some(p) = tab.panes.get(&tab.focus) {
                    let sb = p.terminal.screen.scrollback_len();
                    let rows = p.terminal.screen.rows();
                    let total = sb + rows;
                    for a in 0..total {
                        let line = p.terminal.screen.view_line(a, sb);
                        for (c0, c1) in crate::app::search::matches_in_line(line, &query) {
                            found.push((a, c0, c1));
                        }
                    }
                }
            }
        }
        if let Some(s) = self.search.as_mut() {
            s.matches = found;
            s.current = 0;
        }
        self.jump_to_current_match();
    }

    /// 現在マッチを delta 方向へ移動してジャンプ。
    fn step_search(&mut self, delta: i32) {
        if let Some(s) = self.search.as_mut() {
            let n = s.matches.len();
            if n == 0 {
                return;
            }
            s.current = ((s.current as i32 + delta).rem_euclid(n as i32)) as usize;
        }
        self.jump_to_current_match();
    }

    /// カレントマッチが見えるようフォーカスペインの scroll を調整する。
    fn jump_to_current_match(&mut self) {
        let target = self
            .search
            .as_ref()
            .and_then(|s| s.matches.get(s.current).map(|m| m.0));
        let Some(abs_row) = target else {
            return;
        };
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            if let Some(p) = tab.panes.get_mut(&tab.focus) {
                let sb = p.terminal.screen.scrollback_len();
                let rows = p.terminal.screen.rows();
                p.scroll = crate::app::search::scroll_to_row(abs_row, sb, rows);
            }
        }
    }

    // ---------- 端末入力 ----------
    fn terminal_key(&mut self, event: &KeyEvent) {
        let Some(tk) = to_term_key(&event.logical_key, event.text.as_deref()) else {
            return;
        };
        let mods = self.term_mods();
        // GUI ショートカット判定: config.keys 反映済み keymap → 固定分割/フォーカス等
        if let Some((k, mm)) = crate::input::to_keymap_key(&tk, mods) {
            if let Some(action) = self.keymap.lookup(k, mm) {
                self.handle_gui_action(crate::input::action_to_gui(action));
                return;
            }
        }
        if let Some(action) = crate::input::fixed_shortcut(&tk, mods) {
            self.handle_gui_action(action);
            return;
        }
        // それ以外は PTY へ（ブロードキャスト対応）
        let app_cursor = self
            .tabs
            .get(self.active_tab)
            .and_then(|t| t.panes.get(&t.focus))
            .map(|p| p.terminal.screen.app_cursor_keys)
            .unwrap_or(false);
        if let Some(bytes) = mot_term::encode_key(tk, mods, app_cursor) {
            self.send_input(&bytes);
        }
    }

    fn send_input(&mut self, bytes: &[u8]) {
        if self.broadcast {
            for tab in &mut self.tabs {
                for pane in tab.panes.values_mut() {
                    pane.handle.write(bytes.to_vec());
                }
            }
        } else if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            if let Some(p) = tab.panes.get_mut(&tab.focus) {
                p.handle.write(bytes.to_vec());
                p.scroll = 0;
            }
        }
    }

    fn handle_gui_action(&mut self, action: GuiAction) {
        match action {
            // NEO-UI: ランチャー/新規タブは常設サイドバーの Filter へフォーカス（ホスト検索）。
            // Filter とサイドバー選択は相互排他。
            GuiAction::Launcher | GuiAction::NewTab => {
                self.neo_filter_focus = true;
                self.neo_sidebar_sel = None;
            }
            // Ctrl+T: サイドバーのキーボード選択モードに入る（先頭行を選択）。
            GuiAction::SidebarFocus => {
                self.neo_sidebar_sel = Some(0);
                self.neo_filter_focus = false;
            }
            GuiAction::CloseTab => self.close_active_tab(),
            GuiAction::NextTab => self.switch_tab(1),
            GuiAction::PrevTab => self.switch_tab(-1),
            GuiAction::TabIndex(n) => {
                let idx = if n == 9 {
                    self.tabs.len().saturating_sub(1)
                } else {
                    (n as usize).saturating_sub(1)
                };
                if idx < self.tabs.len() {
                    self.active_tab = idx;
                }
            }
            GuiAction::SplitH => self.split_focus(SplitDir::Horizontal),
            GuiAction::SplitV => self.split_focus(SplitDir::Vertical),
            GuiAction::ClosePane => self.close_focus_pane(),
            GuiAction::FocusLeft => self.move_focus(Move::Left),
            GuiAction::FocusRight => self.move_focus(Move::Right),
            GuiAction::FocusUp => self.move_focus(Move::Up),
            GuiAction::FocusDown => self.move_focus(Move::Down),
            GuiAction::Broadcast => self.broadcast = !self.broadcast,
            GuiAction::PfPanel => self.show_pf_panel = !self.show_pf_panel,
            GuiAction::InfoPanel => self.neo_info_visible = !self.neo_info_visible,
            GuiAction::Sftp => self.open_sftp_fm(),
            GuiAction::Search => self.search = Some(SearchState::new()),
            GuiAction::Copy => self.copy_selection(),
            GuiAction::Paste => self.paste_clipboard(),
            GuiAction::Reconnect => self.reconnect_active(),
            GuiAction::ScrollPageUp => self.scroll_focus(10),
            GuiAction::ScrollPageDown => self.scroll_focus(-10),
            GuiAction::PromptPrev => self.jump_prompt(true),
            GuiAction::PromptNext => self.jump_prompt(false),
            GuiAction::Reload => self.reload_config(),
            GuiAction::Download => self.download_selection(),
        }
    }

    fn split_focus(&mut self, dir: SplitDir) {
        let (session, cols, rows, new_id, focus) = {
            let Some(tab) = self.tabs.get_mut(self.active_tab) else {
                return;
            };
            let Some(session) = tab.session.clone() else {
                return;
            };
            let new_id = tab.next_pane;
            (session, 80u16, 24u16, new_id, tab.focus)
        };
        // 追加ペインを同一セッションで開く
        if let Ok(handle) = self.connector.open_pane(&session, cols, rows) {
            let Some(tab) = self.tabs.get_mut(self.active_tab) else {
                return;
            };
            if tab.tree.split(focus, dir, new_id) {
                tab.next_pane += 1;
                tab.panes.insert(
                    new_id,
                    PaneState {
                        terminal: mot_term::Terminal::new(
                            cols as usize,
                            rows as usize,
                            self.config.scrollback_lines,
                        ),
                        handle,
                        scroll: 0,
                        cols,
                        rows,
                        agents: Default::default(),
                    },
                );
                tab.focus = new_id;
            }
        }
    }

    fn close_focus_pane(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        let focus = tab.focus;
        let tree = std::mem::replace(&mut tab.tree, PaneNode::leaf(0));
        match tree.close(focus) {
            Some(new_tree) => {
                tab.tree = new_tree;
                tab.panes.remove(&focus);
                tab.focus = tab.tree.leaves().first().copied().unwrap_or(0);
            }
            None => self.close_active_tab(),
        }
    }

    fn move_focus(&mut self, mv: Move) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        let area = Rect {
            x: 0,
            y: 0,
            w: 200,
            h: 60,
        };
        if let Some(id) = tab.tree.focus_move(area, tab.focus, mv) {
            tab.focus = id;
        }
    }

    fn switch_tab(&mut self, delta: i32) {
        if self.tabs.is_empty() {
            return;
        }
        let n = self.tabs.len() as i32;
        self.active_tab = ((self.active_tab as i32 + delta) % n + n) as usize % self.tabs.len();
    }

    pub(super) fn close_active_tab(&mut self) {
        if self.active_tab < self.tabs.len() {
            let tab = self.tabs.remove(self.active_tab);
            if let Some(s) = tab.session {
                self.connector
                    .runtime()
                    .block_on(async { s.disconnect().await });
            }
        }
        if self.tabs.is_empty() {
            // NEO-UI にランチャーモードは無い。端末（no session）表示へ戻す。
            self.mode = Mode::Terminal;
            self.active_tab = 0;
        } else {
            self.active_tab = self.active_tab.min(self.tabs.len() - 1);
        }
    }

    fn scroll_focus(&mut self, lines: i32) {
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            if let Some(p) = tab.panes.get_mut(&tab.focus) {
                let max = p.terminal.screen.scrollback_len();
                let new = (p.scroll as i32 + lines).clamp(0, max as i32);
                p.scroll = new as usize;
            }
        }
    }

    /// OSC 133 のプロンプトマークへスクロールジャンプする（prev=前 / false=次）。
    /// マークが無ければ何もしない。絶対行 T を最上段に置く scroll = pushed_total - T。
    fn jump_prompt(&mut self, prev: bool) {
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            if let Some(p) = tab.panes.get_mut(&tab.focus) {
                let screen = &p.terminal.screen;
                let scrolled = screen.pushed_total();
                let sb = screen.scrollback_len();
                let from = screen.top_visible_abs(p.scroll);
                let target = if prev {
                    screen.prev_prompt_abs(from)
                } else {
                    screen.next_prompt_abs(from)
                };
                if let Some(t) = target {
                    let s = scrolled.saturating_sub(t) as usize;
                    p.scroll = s.min(sb);
                }
            }
        }
    }

    fn copy_selection(&mut self) {
        // 選択中テキストを抽出（フォーカスペイン）
        if let Some(text) = self.current_selection_text() {
            if !text.is_empty() {
                set_clipboard(&text);
            }
        }
    }

    pub(super) fn paste_clipboard(&mut self) {
        if let Some(text) = get_clipboard() {
            let bracketed = self
                .tabs
                .get(self.active_tab)
                .and_then(|t| t.panes.get(&t.focus))
                .map(|p| p.terminal.screen.bracketed_paste)
                .unwrap_or(false);
            let bytes = mot_term::encode_paste(&text, bracketed);
            self.send_input(&bytes);
        }
    }

    fn reconnect_active(&mut self) {
        // アクティブタブにプロファイルがあれば同じタブで再接続（手動なので再試行回数はリセット）。
        let i = self.active_tab;
        if self.tabs.get(i).and_then(|t| t.profile.as_ref()).is_some() {
            if let Some(tab) = self.tabs.get_mut(i) {
                tab.reconnect_attempts = 0;
                tab.reconnect_at = None;
            }
            self.reconnect_tab_inplace(i);
        }
    }

    fn reload_config(&mut self) {
        if let Ok(cfg) = mot_core::config_lua::load_config(None) {
            self.config = cfg;
            self.theme = Theme::builtin(self.config.color_scheme.as_deref().unwrap_or("default"));
            self.keymap = mot_core::keymap::Keymap::from_config(&self.config.keys);
            // ウィンドウサイズも更新（次の sync_mode_size で反映）
            self.term_size = crate::app::mode_sizes(&self.config);
            self.rebuild_launcher();
        }
    }

    // ---------- マウス ----------
    pub(super) fn on_mouse(&mut self, state: ElementState, button: MouseButton) {
        // パスワード系モーダルはクリック（目のトグル / 送信ボタン）に対応。
        if matches!(
            self.modal,
            Some(Modal::Master { .. } | Modal::Password { .. } | Modal::Passphrase { .. })
        ) {
            if state == ElementState::Pressed && button == MouseButton::Left {
                self.neo_master_modal_mouse();
            }
            return;
        }
        // モーダルが無いとき専用のマウス処理へ（自前タイトルバー/サイドバー等）。
        // SFTP は接続画面に埋め込むので neo_sftp_mouse、それ以外は neo_mouse。
        if self.modal.is_none() {
            if self.mode == Mode::Sftp && self.fm_belongs_to_active() {
                self.neo_sftp_mouse(state, button);
            } else {
                self.neo_mouse(state, button);
            }
        }
    }

    /// B4: (mx,my) がどれかのペインのスクロールバー列（右端の判定帯）内で、かつ
    /// サムが存在する（スクロールバックがある）なら、そのペインのトラック幾何を返す。
    /// 返り値: (pid, track_top_y, track_h, thumb_top, thumb_h)（描画と同一の式）。
    pub(super) fn on_scrollbar_drag(&mut self) {
        self.neo_on_scrollbar_drag();
    }

    /// マウス座標が指すペインとペイン内セル座標＋そのペインの mouse_mode を返す。
    pub(super) fn send_mouse(
        &mut self,
        pid: u64,
        button: mot_term::MouseButton,
        pressed: bool,
        motion: bool,
        col: usize,
        row: usize,
    ) {
        let (shift, ctrl) = (self.mods.shift_key(), self.mods.control_key());
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            if let Some(p) = tab.panes.get_mut(&pid) {
                if let Some(bytes) = p
                    .terminal
                    .screen
                    .encode_mouse(button, pressed, motion, col, row, shift, ctrl)
                {
                    p.handle.write(bytes);
                }
            }
        }
    }

    /// マウス転送中のドラッグ motion 報告（CursorMoved から呼ぶ）。
    pub(super) fn on_mouse_motion(&mut self) {
        if self.mods.shift_key() {
            return;
        }
        let Some(mb) = self.mouse_report_btn else {
            return;
        };
        let (mx, my) = (self.mouse.0 as i32, self.mouse.1 as i32);
        if let Some((pid, col, row, mode)) = self.neo_pane_cell_at(mx, my) {
            if matches!(mode, mot_term::MouseMode::Button | mot_term::MouseMode::Any) {
                self.send_mouse(pid, mb, true, true, col, row);
            }
        }
    }

    /// マウス座標が指すセルの URL（OSC 8 リンク優先、無ければ行テキストから http(s) 検出）。
    pub(super) fn on_tab_drag(&mut self) {
        let Some(mut drag) = self.tab_drag else {
            return;
        };
        let mx = self.mouse.0 as i32;
        if !drag.moved && (mx - drag.press_x).abs() > TAB_DRAG_THRESHOLD {
            drag.moved = true;
        }
        if drag.moved {
            let geom = self.neo_tab_geometry().0;
            let target = geom
                .iter()
                .find(|(_, x0, x1)| mx >= *x0 && mx < *x1)
                .map(|(idx, _, _)| *idx);
            if let Some(to) = target {
                if to != drag.index {
                    move_tab(&mut self.tabs, drag.index, to);
                    self.active_tab = to;
                    drag.index = to;
                }
            }
        }
        self.tab_drag = Some(drag);
    }

    /// タブメニュークリック（項目実行 or 閉じる）。
    pub(super) fn handle_tab_menu_click(&mut self, button: MouseButton) {
        let (mx, my) = (self.mouse.0 as i32, self.mouse.1 as i32);
        let mut action: Option<&'static str> = None;
        if button == MouseButton::Left {
            if let Some(items) = self.tab_menu_geometry() {
                for (id, x0, y0, x1, y1) in items {
                    if mx >= x0 && mx < x1 && my >= y0 && my < y1 {
                        action = Some(id);
                        break;
                    }
                }
            }
        }
        let idx = self.tab_menu.map(|m| m.index);
        self.tab_menu = None;
        if let (Some(id), Some(idx)) = (action, idx) {
            self.exec_tab_menu(id, idx);
        }
    }

    /// タブメニュー項目を実行。
    fn exec_tab_menu(&mut self, id: &str, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        match id {
            "rename" => {
                self.tab_rename = Some((idx, self.tabs[idx].title.clone()));
            }
            "duplicate" => {
                if let Some(p) = self.tabs[idx].profile.clone() {
                    self.connect_profile(p);
                }
            }
            "sftp" => {
                self.active_tab = idx;
                self.open_sftp_fm();
            }
            "close" => {
                self.active_tab = idx;
                self.close_active_tab();
            }
            _ => {}
        }
    }

    /// タブ名インライン編集のキー処理。
    fn tab_rename_key(&mut self, event: &KeyEvent) {
        match &event.logical_key {
            WKey::Named(NamedKey::Enter) => {
                if let Some((i, buf)) = self.tab_rename.take() {
                    if let Some(t) = self.tabs.get_mut(i) {
                        t.title = rename_result(&t.title, &buf);
                    }
                }
            }
            WKey::Named(NamedKey::Escape) => {
                self.tab_rename = None;
            }
            WKey::Named(NamedKey::Backspace) => {
                if let Some((_, buf)) = self.tab_rename.as_mut() {
                    buf.pop();
                }
            }
            _ => {
                if let Some(t) = &event.text {
                    if !t.chars().any(|c| c.is_control()) {
                        if let Some((_, buf)) = self.tab_rename.as_mut() {
                            buf.push_str(t);
                        }
                    }
                }
            }
        }
    }

    pub(super) fn bump_click_count(&mut self) {
        let now = std::time::Instant::now();
        let pos = self.mouse;
        let recent = self.last_click.is_some_and(|(p, t)| {
            now.duration_since(t).as_millis() < DOUBLE_CLICK_MS
                && (p.0 - pos.0).abs() < 4.0
                && (p.1 - pos.1).abs() < 4.0
        });
        self.click_count = if recent {
            if self.click_count >= 3 {
                1
            } else {
                self.click_count + 1
            }
        } else {
            1
        };
        self.last_click = Some((pos, now));
    }

    /// 選択の生成本体（ビュー座標 col / view_row を与える）。classic/neo で共有。
    /// click_count に応じて単語/行/範囲を作り、ダブル・トリプルは即コピーする。
    pub(super) fn start_selection_at(&mut self, col: usize, view_row: usize) {
        let row = view_row;
        let alt = self.mods.alt_key();
        let kind = if alt { SelKind::Block } else { SelKind::Linear };
        let abs = self.view_to_abs(row);
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            if let Some(p) = tab.panes.get_mut(&tab.focus) {
                let sel = match self.click_count {
                    2 => {
                        // 単語選択
                        if let Some((s, e)) = word_at(&p.terminal.screen, abs, col) {
                            let mut sel = Selection::new(SelKind::Linear, (abs, s));
                            sel.head = (abs, e);
                            sel
                        } else {
                            Selection::new(kind, (abs, col))
                        }
                    }
                    3 => {
                        // 行選択
                        let mut sel = Selection::new(SelKind::Linear, (abs, 0));
                        sel.head = (abs, p.cols as usize);
                        sel
                    }
                    _ => Selection::new(kind, (abs, col)),
                };
                self.selection = Some(sel);
                self.dragging_sel = self.click_count == 1;
                // 即コピー（選択→プライマリ）
                if self.click_count >= 2 {
                    if let Some(t) = self.current_selection_text() {
                        set_clipboard(&t);
                    }
                }
            }
        }
    }

    pub(super) fn on_drag(&mut self) {
        // neo 用のセル変換で選択 head を更新。
        if let Some((col, view_row)) = self.neo_term_cell(self.mouse.0 as i32, self.mouse.1 as i32)
        {
            let abs = self.view_to_abs(view_row);
            if let Some(sel) = &mut self.selection {
                sel.head = (abs, col);
            }
        }
    }

    pub(super) fn on_scroll(&mut self, lines: i32) {
        if self.mode == Mode::Sftp && self.fm_belongs_to_active() {
            // ホイールでアクティブペインの選択を移動（上=前, 下=次）。
            if let Some(fm) = self.fm.as_mut() {
                fm.active_pane_mut()
                    .move_sel(if lines > 0 { -3 } else { 3 });
            }
            return;
        }
        // マウス対応 TUI へホイールを転送（Shift 非押下 & mouse_mode 有効時）
        if !self.mods.shift_key() {
            let (mx, my) = (self.mouse.0 as i32, self.mouse.1 as i32);
            if let Some((pid, col, row, mode)) = self.neo_pane_cell_at(mx, my) {
                if mode != mot_term::MouseMode::Off {
                    let mb = if lines > 0 {
                        mot_term::MouseButton::WheelUp
                    } else {
                        mot_term::MouseButton::WheelDown
                    };
                    for _ in 0..lines.unsigned_abs().min(5) {
                        self.send_mouse(pid, mb, true, false, col, row);
                    }
                    return;
                }
            }
        }
        self.scroll_focus(lines * 3);
    }

    pub(super) fn on_ime_commit(&mut self, text: &str) {
        if self.mode == Mode::Terminal && self.modal.is_none() && self.config.use_ime {
            let bytes = text.as_bytes().to_vec();
            self.send_input(&bytes);
        }
    }

    pub(super) fn on_drop_file(&mut self, path: PathBuf) {
        // SFTP アップロード（フォーカスタブのセッション、リモート cwd or home へ）
        let session = self
            .tabs
            .get(self.active_tab)
            .and_then(|t| t.session.clone());
        let remote_cwd = self
            .tabs
            .get(self.active_tab)
            .and_then(|t| t.panes.get(&t.focus))
            .and_then(|p| p.terminal.screen.remote_cwd.clone());
        if let Some(session) = session {
            let fname = path
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default();
            let remote = match remote_cwd {
                Some(dir) => format!("{}/{}", dir.trim_end_matches('/'), fname),
                None => fname.clone(),
            };
            let rt = self.connector.runtime();
            let _ = rt.block_on(async move {
                let sftp = session.open_sftp().await.ok()?;
                sftp.upload(&path, &remote).await.ok()
            });
        }
    }

    // ---------- ヘルパ ----------
    fn view_to_abs(&self, view_row: usize) -> usize {
        if let Some(tab) = self.tabs.get(self.active_tab) {
            if let Some(p) = tab.panes.get(&tab.focus) {
                let sb = p.terminal.screen.scrollback_len();
                let rows = p.terminal.screen.rows();
                let top_abs = (sb + rows).saturating_sub(rows + p.scroll);
                return top_abs + view_row;
            }
        }
        view_row
    }

    pub(super) fn current_selection_text(&self) -> Option<String> {
        let sel = self.selection.as_ref()?;
        let tab = self.tabs.get(self.active_tab)?;
        let p = tab.panes.get(&tab.focus)?;
        Some(extract_text(&p.terminal.screen, sel))
    }

    pub(super) fn merged_profiles(&self) -> Vec<Profile> {
        let gui = mot_core::store::load_gui_profiles(&self.config_dir);
        mot_core::store::merge_profiles(&self.config.profiles, &gui)
    }

    fn rebuild_launcher(&mut self) {
        let merged = self.merged_profiles();
        self.launcher.rebuild(merged, self.config.groups.clone());
    }
}
