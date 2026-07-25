//! SFTP 2ペインファイルマネージャ（F3）と Ctrl+Shift+D ダウンロードの App メソッド群。
//! 純ロジックは sftpview.rs、ここは I/O（std::fs / mot_ssh::Sftp）と状態遷移を担う。

use super::*;
use crate::sftpview::{
    other_side, transfer_dir, DrivePicker, Entry as FmEntry, FileManager, FmConfirm, FmInput,
    InputKind, PendingAction, Side,
};
use std::path::{Path, PathBuf};
use winit::event::KeyEvent;
use winit::keyboard::{Key as WKey, NamedKey};

impl App {
    fn active_session(&self) -> Option<Arc<SshSession>> {
        self.tabs
            .get(self.active_tab)
            .and_then(|t| t.session.clone())
    }

    /// F3: ファイルマネージャを開く（Terminal → Sftp）。
    pub(super) fn open_sftp_fm(&mut self) {
        // ローカル初期 cwd: 前回開いたディレクトリ（存在すれば）→ download_dir → $HOME。
        let remembered = mot_core::ui_state::load_ui_state(&self.config_dir)
            .sftp_local_dir
            .map(PathBuf::from)
            .filter(|p| p.is_dir());
        let local_cwd = remembered
            .or_else(|| {
                self.config
                    .download_dir
                    .as_deref()
                    .map(mot_core::model::expand_tilde)
            })
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        // リモート初期 cwd: 端末 OSC7 か SFTP のホーム（canonicalize "."）
        let remote_hint = self
            .tabs
            .get(self.active_tab)
            .and_then(|t| t.panes.get(&t.focus))
            .and_then(|p| p.terminal.screen.remote_cwd.clone());

        let mut fm = FileManager::new(
            local_cwd.to_string_lossy().to_string(),
            remote_hint.clone().unwrap_or_else(|| ".".into()),
        );
        // ローカル読み込み
        let (entries, note) = super::read_local_listing(&local_cwd);
        fm.local.set_listing(entries, !is_fs_root(&local_cwd));
        fm.local.note = note;
        self.fm = Some(fm);
        // SFTP をこのタブの接続に紐づける（切断で自動クローズするため）。
        self.fm_conn = self.tabs.get(self.active_tab).map(|t| t.conn);
        self.mode = Mode::Sftp;
        // リモート読み込み（接続があれば）
        self.fm_reload_remote(remote_hint.is_none());
    }

    pub(super) fn close_sftp_fm(&mut self) {
        self.persist_sftp_local_dir();
        self.fm = None;
        self.fm_conn = None;
        self.mode = Mode::Terminal;
    }

    /// ローカルペインの現在ディレクトリを次回起動用に記憶する（ui_state.json）。
    /// 失敗しても操作の妨げにしない（警告ログのみ）。
    fn persist_sftp_local_dir(&self) {
        let Some(cwd) = self.fm.as_ref().map(|f| f.local.cwd.clone()) else {
            return;
        };
        let mut state = mot_core::ui_state::load_ui_state(&self.config_dir);
        if state.sftp_local_dir.as_deref() == Some(cwd.as_str()) {
            return; // 変化なし＝書き込み不要
        }
        state.sftp_local_dir = Some(cwd);
        if let Err(e) = mot_core::ui_state::save_ui_state(&self.config_dir, &state) {
            log::warn!("SFTP ローカルパスの保存に失敗: {e}");
        }
    }

    /// SFTP がアクティブタブの接続に属しているか（＝いま SFTP を表示・操作してよいか）。
    /// fm_conn=None は MOTERM_OPEN_SFTP のデモ表示（接続なし）で、常に表示可とする。
    pub(super) fn fm_belongs_to_active(&self) -> bool {
        if self.fm.is_none() {
            return false;
        }
        match self.fm_conn {
            None => true,
            Some(_) => self.tabs.get(self.active_tab).map(|t| t.conn) == self.fm_conn,
        }
    }

    /// 紐づく接続が切断（Failed）/消滅したら SFTP を閉じる。毎フレーム呼ぶ。
    pub(super) fn sync_fm_connection(&mut self) {
        if self.fm.is_none() || self.fm_conn.is_none() {
            return; // 未紐づけ（デモ）は対象外
        }
        let alive = self
            .tabs
            .iter()
            .any(|t| Some(t.conn) == self.fm_conn && t.status != TabStatus::Failed);
        if !alive {
            self.close_sftp_fm();
        }
    }

    /// リモートペインを SFTP で読み直す。resolve_home=true なら cwd を canonicalize(".") で解決。
    fn fm_reload_remote(&mut self, resolve_home: bool) {
        let Some(session) = self.active_session() else {
            if let Some(fm) = self.fm.as_mut() {
                fm.remote.entries.clear();
                fm.remote.note = Some("(not connected)".into());
            }
            return;
        };
        let cwd = self
            .fm
            .as_ref()
            .map(|f| f.remote.cwd.clone())
            .unwrap_or_else(|| ".".into());
        let result: Option<(String, Vec<FmEntry>)> =
            self.connector.runtime().block_on(async move {
                let sftp = session.open_sftp().await.ok()?;
                let real = if resolve_home || cwd == "." {
                    sftp.canonicalize(".").await.unwrap_or_else(|_| "/".into())
                } else {
                    cwd
                };
                let list = sftp.read_dir(&real).await.ok()?;
                let entries = list
                    .into_iter()
                    .map(|d| FmEntry {
                        name: d.name,
                        is_dir: d.is_dir,
                        size: d.size,
                        mtime: d.mtime,
                        parent: false,
                    })
                    .collect();
                Some((real, entries))
            });
        if let Some(fm) = self.fm.as_mut() {
            match result {
                Some((real, entries)) => {
                    fm.remote.cwd = real.clone();
                    fm.remote.set_listing(entries, real != "/");
                    fm.remote.note = None;
                }
                None => {
                    fm.remote.note = Some(tr(self.lang, "failed").to_string());
                }
            }
        }
    }

    fn fm_reload_local(&mut self) {
        let cwd = self.fm.as_ref().map(|f| f.local.cwd.clone());
        if let Some(cwd) = cwd {
            let p = PathBuf::from(&cwd);
            let (entries, note) = super::read_local_listing(&p);
            if let Some(fm) = self.fm.as_mut() {
                fm.local.set_listing(entries, !is_fs_root(&p));
                fm.local.note = note;
            }
            // ローカルの現在地が変わるたび記憶しておく（突然終了しても最新が残る）。
            self.persist_sftp_local_dir();
        }
    }

    // ---------- キー処理 ----------
    pub(super) fn sftp_key(&mut self, event: &KeyEvent) {
        // 入力プロンプト（mkdir/rename）
        if self.fm.as_ref().map(|f| f.input.is_some()).unwrap_or(false) {
            self.fm_input_key(event);
            return;
        }
        // y/n 確認（上書き/削除）
        if self
            .fm
            .as_ref()
            .map(|f| f.confirm.is_some())
            .unwrap_or(false)
        {
            self.fm_confirm_key(event);
            return;
        }
        // ドライブ選択オーバーレイ（Windows、ローカルペイン）
        if self
            .fm
            .as_ref()
            .map(|f| f.drive_picker.is_some())
            .unwrap_or(false)
        {
            self.fm_drive_key(event);
            return;
        }
        let key = &event.logical_key;
        match key {
            WKey::Named(NamedKey::Escape) | WKey::Named(NamedKey::F3) => self.close_sftp_fm(),
            WKey::Named(NamedKey::Tab) => {
                if let Some(fm) = self.fm.as_mut() {
                    fm.toggle_active();
                }
            }
            WKey::Named(NamedKey::ArrowUp) => {
                if let Some(fm) = self.fm.as_mut() {
                    fm.active_pane_mut().move_sel(-1);
                }
            }
            WKey::Named(NamedKey::ArrowDown) => {
                if let Some(fm) = self.fm.as_mut() {
                    fm.active_pane_mut().move_sel(1);
                }
            }
            WKey::Named(NamedKey::Enter) => self.fm_enter(),
            WKey::Named(NamedKey::Backspace) => self.fm_parent(),
            WKey::Named(NamedKey::F5) => self.fm_reload_active(),
            WKey::Named(NamedKey::F7) => self.fm_begin_input(InputKind::Mkdir),
            WKey::Named(NamedKey::F2) => self.fm_begin_input(InputKind::Rename),
            WKey::Named(NamedKey::F8) => self.fm_begin_delete(),
            // s: アクティブペインのソート列を巡回（名前→サイズ→日付→…）
            WKey::Character(s) if s.as_str() == "s" => {
                if let Some(fm) = self.fm.as_mut() {
                    let next = fm.active_pane().sort_key.cycle();
                    fm.active_pane_mut().sort_key = next;
                    fm.active_pane_mut().sort_asc = true;
                    fm.active_pane_mut().re_sort();
                }
            }
            // Space: アクティブペインの選択ファイルのマークをトグルして下へ（複数選択）
            WKey::Named(NamedKey::Space) => {
                if let Some(fm) = self.fm.as_mut() {
                    fm.active_pane_mut().toggle_mark_sel();
                }
            }
            // m: ミラー同期（アクティブ→反対ペイン。差分ファイルを一括転送）
            WKey::Character(s) if s.as_str() == "m" => self.fm_begin_mirror(),
            // d: ローカルのドライブ切替（Windows のみ。ドライブが無ければ何もしない）
            WKey::Character(s) if s.as_str() == "d" => self.fm_open_drive_picker(),
            _ => {}
        }
    }

    pub(super) fn fm_reload_active(&mut self) {
        let active = self.fm.as_ref().map(|f| f.active);
        match active {
            Some(Side::Local) => self.fm_reload_local(),
            Some(Side::Remote) => self.fm_reload_remote(false),
            None => {}
        }
    }

    pub(super) fn fm_enter(&mut self) {
        let Some(fm) = self.fm.as_ref() else { return };
        let active = fm.active;
        let Some(sel) = fm.active_pane().selected().cloned() else {
            return;
        };
        if sel.parent {
            self.fm_parent();
            return;
        }
        if sel.is_dir {
            // ディレクトリへ移動
            match active {
                Side::Local => {
                    let cwd = fm.local.cwd.clone();
                    let next = PathBuf::from(&cwd).join(&sel.name);
                    if let Some(f) = self.fm.as_mut() {
                        f.local.cwd = next.to_string_lossy().to_string();
                        f.local.sel = 0;
                    }
                    self.fm_reload_local();
                }
                Side::Remote => {
                    let next = crate::sftpview::remote_join(&fm.remote.cwd, &sel.name);
                    if let Some(f) = self.fm.as_mut() {
                        f.remote.cwd = next;
                        f.remote.sel = 0;
                    }
                    self.fm_reload_remote(false);
                }
            }
        } else {
            // ファイル → 反対ペインへ転送（マークがあれば全マーク、無ければこの1件）
            let names = fm.active_pane().action_targets();
            self.fm_request_transfer_many(active, names);
        }
    }

    fn fm_parent(&mut self) {
        let Some(fm) = self.fm.as_ref() else { return };
        match fm.active {
            Side::Local => {
                let cwd = PathBuf::from(&fm.local.cwd);
                if let Some(parent) = cwd.parent() {
                    let p = parent.to_path_buf();
                    if let Some(f) = self.fm.as_mut() {
                        f.local.cwd = p.to_string_lossy().to_string();
                        f.local.sel = 0;
                    }
                    self.fm_reload_local();
                } else {
                    // ドライブルート（"C:\\" 等）で更に上へ→ドライブ選択を開く（Windows）。
                    self.fm_open_drive_picker();
                }
            }
            Side::Remote => {
                let parent = crate::sftpview::remote_parent(&fm.remote.cwd);
                if let Some(f) = self.fm.as_mut() {
                    f.remote.cwd = parent;
                    f.remote.sel = 0;
                }
                self.fm_reload_remote(false);
            }
        }
    }

    /// ローカルのドライブ選択オーバーレイを開く（Windows のみ）。
    /// ドライブが1つも取れない環境（非 Windows 等）では何もしない。
    pub(super) fn fm_open_drive_picker(&mut self) {
        // ドライブ切替はローカルペイン専用。リモートがアクティブなら無視。
        if self.fm.as_ref().map(|f| f.active) != Some(Side::Local) {
            return;
        }
        // 転送中は進捗バーを隠さないよう開かない。
        if self
            .fm
            .as_ref()
            .map(|f| f.progress.is_some())
            .unwrap_or(false)
        {
            self.fm_set_status("transfer in progress");
            return;
        }
        let drives = local_drives();
        if drives.is_empty() {
            return;
        }
        // 現在のローカル cwd のドライブ（"C:\\"）を初期選択にする。
        let cur_root = current_drive_root(self.fm.as_ref().map(|f| f.local.cwd.as_str()));
        if let Some(fm) = self.fm.as_mut() {
            fm.drive_picker = Some(DrivePicker::new(drives, cur_root.as_deref()));
        }
    }

    /// ドライブ選択オーバーレイのキー処理（↑↓←→で移動 / Enter 決定 / Esc 取消）。
    fn fm_drive_key(&mut self, event: &KeyEvent) {
        match &event.logical_key {
            WKey::Named(NamedKey::Escape) => {
                if let Some(fm) = self.fm.as_mut() {
                    fm.drive_picker = None;
                }
            }
            WKey::Named(NamedKey::ArrowUp) | WKey::Named(NamedKey::ArrowLeft) => {
                if let Some(fm) = self.fm.as_mut() {
                    if let Some(dp) = fm.drive_picker.as_mut() {
                        dp.move_sel(-1);
                    }
                }
            }
            WKey::Named(NamedKey::ArrowDown) | WKey::Named(NamedKey::ArrowRight) => {
                if let Some(fm) = self.fm.as_mut() {
                    if let Some(dp) = fm.drive_picker.as_mut() {
                        dp.move_sel(1);
                    }
                }
            }
            WKey::Named(NamedKey::Enter) => {
                let chosen = self
                    .fm
                    .as_ref()
                    .and_then(|f| f.drive_picker.as_ref())
                    .and_then(|dp| dp.selected().cloned());
                if let (Some(root), Some(fm)) = (chosen, self.fm.as_mut()) {
                    fm.drive_picker = None;
                    fm.local.cwd = root;
                    fm.local.sel = 0;
                    self.fm_reload_local();
                }
            }
            _ => {}
        }
    }

    /// 転送要求。転送先に同名があれば上書き確認、無ければ即実行。
    pub(super) fn fm_request_transfer(&mut self, from: Side, name: String) {
        let (_from, to) = transfer_dir(from);
        let exists = self.fm_dest_exists(to, &name);
        if exists {
            let msg = format!("{} '{}'", tr(self.lang, "overwrite_confirm"), name);
            if let Some(fm) = self.fm.as_mut() {
                fm.confirm = Some(FmConfirm {
                    message: msg,
                    action: PendingAction::Transfer { from, name },
                });
            }
        } else {
            self.fm_do_transfer(from, name);
        }
    }

    /// 複数ファイルの転送要求。1件なら単発（従来の上書き確認）に委譲、
    /// 複数なら宛先の既存件数を数え、あれば1回だけまとめて上書き確認する。
    pub(super) fn fm_request_transfer_many(&mut self, from: Side, names: Vec<String>) {
        match names.len() {
            0 => {}
            1 => self.fm_request_transfer(from, names.into_iter().next().unwrap()),
            _ => {
                let (_from, to) = transfer_dir(from);
                let existing = self.fm_count_existing(to, &names);
                if existing > 0 {
                    let msg = format!(
                        "{} {} file(s) ({} exist)",
                        tr(self.lang, "overwrite_confirm"),
                        names.len(),
                        existing
                    );
                    if let Some(fm) = self.fm.as_mut() {
                        fm.confirm = Some(FmConfirm {
                            message: msg,
                            action: PendingAction::TransferMany { from, names },
                        });
                    }
                } else {
                    self.fm_spawn_transfer(from, names);
                }
            }
        }
    }

    /// 転送先 side の cwd に names のうち何件が既に存在するか（上書き確認用）。
    /// リモートは SFTP セッションを1回だけ開いてまとめて調べる。
    fn fm_count_existing(&self, to: Side, names: &[String]) -> usize {
        let Some(fm) = self.fm.as_ref() else {
            return 0;
        };
        match to {
            Side::Local => {
                let base = PathBuf::from(&fm.local.cwd);
                names.iter().filter(|n| base.join(n).exists()).count()
            }
            Side::Remote => {
                let cwd = fm.remote.cwd.clone();
                let Some(session) = self.active_session() else {
                    return 0;
                };
                let names: Vec<String> = names.to_vec();
                self.connector
                    .runtime()
                    .block_on(async move {
                        let sftp = session.open_sftp().await.ok()?;
                        let mut count = 0usize;
                        for n in &names {
                            let path = crate::sftpview::remote_join(&cwd, n);
                            if sftp.exists(&path).await.unwrap_or(false) {
                                count += 1;
                            }
                        }
                        Some(count)
                    })
                    .unwrap_or(0)
            }
        }
    }

    /// 転送先 side の cwd に name が存在するか。
    fn fm_dest_exists(&self, to: Side, name: &str) -> bool {
        let Some(fm) = self.fm.as_ref() else {
            return false;
        };
        match to {
            Side::Local => {
                let dest = PathBuf::from(&fm.local.cwd).join(name);
                dest.exists()
            }
            Side::Remote => {
                let dest = crate::sftpview::remote_join(&fm.remote.cwd, name);
                let Some(session) = self.active_session() else {
                    return false;
                };
                self.connector
                    .runtime()
                    .block_on(async move {
                        let sftp = session.open_sftp().await.ok()?;
                        sftp.exists(&dest).await.ok()
                    })
                    .unwrap_or(false)
            }
        }
    }

    fn fm_do_transfer(&mut self, from: Side, name: String) {
        self.fm_spawn_transfer(from, vec![name]);
    }

    /// 転送をワーカータスクへ投げ、進捗/完了を ConnEvent で受ける（UI 非ブロック）。
    /// 単発転送・ミラーの共通経路。転送中の再入はブロックする。
    fn fm_spawn_transfer(&mut self, from: Side, names: Vec<String>) {
        let Some(fm) = self.fm.as_ref() else { return };
        if names.is_empty() {
            return;
        }
        if fm.progress.is_some() {
            self.fm_set_status("transfer in progress");
            return;
        }
        let local_cwd = fm.local.cwd.clone();
        let remote_cwd = fm.remote.cwd.clone();
        let Some(session) = self.active_session() else {
            self.fm_set_status("(not connected)");
            return;
        };
        let count = names.len();
        // 0% を即時表示（最初の進捗イベントを待たない）
        if let Some(f) = self.fm.as_mut() {
            f.progress = Some(crate::sftpview::FmProgress {
                name: names[0].clone(),
                done: 0,
                total: 0,
                index: 1,
                count,
            });
        }
        let tx = self.connector.sender();
        self.connector.runtime().spawn(async move {
            let mut failed = 0usize;
            match session.open_sftp().await {
                Err(_) => failed = count,
                Ok(sftp) => {
                    for (i, name) in names.iter().enumerate() {
                        let local_path = PathBuf::from(&local_cwd).join(name);
                        let remote_path = crate::sftpview::remote_join(&remote_cwd, name);
                        let txp = tx.clone();
                        let pname = name.clone();
                        let mut progress = move |done: u64, total: u64| {
                            let _ = txp.send(ConnEvent::TransferProgress {
                                name: pname.clone(),
                                done,
                                total,
                                index: i + 1,
                                count,
                            });
                        };
                        let ok = match from {
                            Side::Local => sftp
                                .upload_with_progress(&local_path, &remote_path, &mut progress)
                                .await
                                .is_ok(),
                            Side::Remote => sftp
                                .download_with_progress(&remote_path, &local_path, &mut progress)
                                .await
                                .is_ok(),
                        };
                        if !ok {
                            failed += 1;
                        }
                    }
                }
            }
            let _ = tx.send(ConnEvent::TransferDone {
                from,
                total: count,
                failed,
            });
        });
    }

    /// ConnEvent::TransferProgress の反映（app.rs の pump から呼ぶ）。
    pub(super) fn fm_transfer_progress(&mut self, p: crate::sftpview::FmProgress) {
        if let Some(fm) = self.fm.as_mut() {
            fm.progress = Some(p);
        }
    }

    /// ConnEvent::TransferDone の反映: バー消去→宛先ペイン再読込→ステータス。
    pub(super) fn fm_transfer_done(&mut self, from: Side, total: usize, failed: usize) {
        if let Some(fm) = self.fm.as_mut() {
            fm.progress = None;
        }
        if self.fm.is_none() {
            return;
        }
        // 転送元のマークは用済み（宛先は下で再読込＝set_listing でクリアされる）。
        if let Some(fm) = self.fm.as_mut() {
            fm.pane_mut(from).clear_marks();
        }
        match other_side(from) {
            Side::Local => self.fm_reload_local(),
            Side::Remote => self.fm_reload_remote(false),
        }
        let msg = if failed == 0 && total == 1 {
            if from == Side::Local {
                tr(self.lang, "upload").to_string()
            } else {
                tr(self.lang, "download").to_string()
            }
        } else if failed == 0 {
            format!("{total}/{total} transferred")
        } else {
            format!("{}/{total} transferred ({failed} failed)", total - failed)
        };
        self.fm_set_status(&msg);
    }

    pub(super) fn fm_begin_input(&mut self, kind: InputKind) {
        let Some(fm) = self.fm.as_ref() else { return };
        let side = fm.active;
        // Rename は選択が必要（".." は不可）
        let target = if kind == InputKind::Rename {
            match fm.active_pane().selected() {
                Some(e) if !e.parent => Some(e.name.clone()),
                _ => return,
            }
        } else {
            None
        };
        let buffer = target.clone().unwrap_or_default();
        if let Some(f) = self.fm.as_mut() {
            f.input = Some(FmInput {
                kind,
                buffer,
                side,
                target,
            });
        }
    }

    pub(super) fn fm_begin_delete(&mut self) {
        let Some(fm) = self.fm.as_ref() else { return };
        let side = fm.active;
        // マークがあれば一括削除（ファイルのみ。1件でもマークを優先＝転送と対称）。
        // 無ければ選択1件（ディレクトリ可）。
        let marked = fm.active_pane().marked_names();
        if !marked.is_empty() {
            let msg = format!("{} {} file(s)?", tr(self.lang, "delete"), marked.len());
            if let Some(f) = self.fm.as_mut() {
                f.confirm = Some(FmConfirm {
                    message: msg,
                    action: PendingAction::DeleteMany {
                        side,
                        names: marked,
                    },
                });
            }
            return;
        }
        let Some(sel) = fm.active_pane().selected() else {
            return;
        };
        if sel.parent {
            return;
        }
        let (name, is_dir) = (sel.name.clone(), sel.is_dir);
        let msg = format!("{} '{}'", tr(self.lang, "delete"), name);
        if let Some(f) = self.fm.as_mut() {
            f.confirm = Some(FmConfirm {
                message: msg,
                action: PendingAction::Delete { side, name, is_dir },
            });
        }
    }

    fn fm_input_key(&mut self, event: &KeyEvent) {
        let key = &event.logical_key;
        let mut commit: Option<(InputKind, Side, String, Option<String>)> = None;
        if let Some(fm) = self.fm.as_mut() {
            if let Some(inp) = fm.input.as_mut() {
                match key {
                    WKey::Named(NamedKey::Escape) => {
                        fm.input = None;
                        return;
                    }
                    WKey::Named(NamedKey::Enter) => {
                        if !inp.buffer.is_empty() {
                            commit =
                                Some((inp.kind, inp.side, inp.buffer.clone(), inp.target.clone()));
                        }
                        fm.input = None;
                    }
                    WKey::Named(NamedKey::Backspace) => {
                        inp.buffer.pop();
                    }
                    _ => {
                        if let Some(t) = &event.text {
                            inp.buffer.push_str(t);
                        }
                    }
                }
            }
        }
        if let Some((kind, side, name, target)) = commit {
            match kind {
                InputKind::Mkdir => self.fm_do_mkdir(side, name),
                InputKind::Rename => {
                    if let Some(old) = target {
                        self.fm_do_rename(side, old, name);
                    }
                }
            }
        }
    }

    fn fm_confirm_key(&mut self, event: &KeyEvent) {
        let key = &event.logical_key;
        let mut yes = false;
        let mut dismiss = false;
        match key {
            WKey::Character(s) if matches!(s.as_str(), "y" | "Y") => {
                yes = true;
                dismiss = true;
            }
            WKey::Character(s) if matches!(s.as_str(), "n" | "N") => dismiss = true,
            WKey::Named(NamedKey::Escape) => dismiss = true,
            _ => {}
        }
        if !dismiss {
            return;
        }
        let action = self
            .fm
            .as_mut()
            .and_then(|f| f.confirm.take())
            .map(|c| c.action);
        if yes {
            match action {
                Some(PendingAction::Transfer { from, name }) => self.fm_do_transfer(from, name),
                Some(PendingAction::Delete { side, name, is_dir }) => {
                    self.fm_do_delete(side, name, is_dir)
                }
                Some(PendingAction::TransferMany { from, names }) => {
                    self.fm_spawn_transfer(from, names)
                }
                Some(PendingAction::DeleteMany { side, names }) => {
                    self.fm_do_delete_many(side, names)
                }
                Some(PendingAction::Mirror { from, names }) => self.fm_do_mirror(from, names),
                None => {}
            }
        }
    }

    /// m: アクティブ側（ソース）から反対側（宛先）への片方向ミラー同期。
    /// 差分（宛先に無い/サイズ違い/ソースが新しい）のファイルだけを一括転送する。
    /// サブディレクトリの再帰同期は対象外（ファイルのみ）。
    fn fm_begin_mirror(&mut self) {
        let Some(fm) = self.fm.as_ref() else { return };
        let from = fm.active;
        let (src, dst) = crate::sftpview::transfer_dir(from);
        let names = crate::sftpview::mirror_plan(&fm.pane(src).entries, &fm.pane(dst).entries);
        if names.is_empty() {
            self.fm_set_status("Mirror: already in sync");
            return;
        }
        let (src_label, dst_label) = match from {
            Side::Local => (tr(self.lang, "local"), tr(self.lang, "remote")),
            Side::Remote => (tr(self.lang, "remote"), tr(self.lang, "local")),
        };
        let msg = format!(
            "Mirror {} file(s) {}→{}?",
            names.len(),
            src_label,
            dst_label
        );
        if let Some(fm) = self.fm.as_mut() {
            fm.confirm = Some(FmConfirm {
                message: msg,
                action: PendingAction::Mirror { from, names },
            });
        }
    }

    /// ミラー確定後の一括転送。単発転送と同じワーカー経路で順次転送する
    /// （ミラーの意味に沿ってサイレント上書き。進捗はバー表示）。
    fn fm_do_mirror(&mut self, from: Side, names: Vec<String>) {
        self.fm_spawn_transfer(from, names);
    }

    fn fm_do_mkdir(&mut self, side: Side, name: String) {
        let Some(fm) = self.fm.as_ref() else { return };
        match side {
            Side::Local => {
                let path = PathBuf::from(&fm.local.cwd).join(&name);
                if std::fs::create_dir(&path).is_ok() {
                    self.fm_reload_local();
                } else {
                    self.fm_set_status(tr(self.lang, "failed"));
                }
            }
            Side::Remote => {
                let path = crate::sftpview::remote_join(&fm.remote.cwd, &name);
                let Some(session) = self.active_session() else {
                    return;
                };
                let ok = self
                    .connector
                    .runtime()
                    .block_on(async move {
                        let sftp = session.open_sftp().await.ok()?;
                        sftp.mkdir(&path).await.ok()
                    })
                    .is_some();
                if ok {
                    self.fm_reload_remote(false);
                } else {
                    self.fm_set_status(tr(self.lang, "failed"));
                }
            }
        }
    }

    fn fm_do_rename(&mut self, side: Side, old: String, new: String) {
        let Some(fm) = self.fm.as_ref() else { return };
        match side {
            Side::Local => {
                let base = PathBuf::from(&fm.local.cwd);
                if std::fs::rename(base.join(&old), base.join(&new)).is_ok() {
                    self.fm_reload_local();
                } else {
                    self.fm_set_status(tr(self.lang, "failed"));
                }
            }
            Side::Remote => {
                let from = crate::sftpview::remote_join(&fm.remote.cwd, &old);
                let to = crate::sftpview::remote_join(&fm.remote.cwd, &new);
                let Some(session) = self.active_session() else {
                    return;
                };
                let ok = self
                    .connector
                    .runtime()
                    .block_on(async move {
                        let sftp = session.open_sftp().await.ok()?;
                        sftp.rename(&from, &to).await.ok()
                    })
                    .is_some();
                if ok {
                    self.fm_reload_remote(false);
                } else {
                    self.fm_set_status(tr(self.lang, "failed"));
                }
            }
        }
    }

    fn fm_do_delete(&mut self, side: Side, name: String, is_dir: bool) {
        let Some(fm) = self.fm.as_ref() else { return };
        match side {
            Side::Local => {
                let path = PathBuf::from(&fm.local.cwd).join(&name);
                let r = if is_dir {
                    std::fs::remove_dir_all(&path)
                } else {
                    std::fs::remove_file(&path)
                };
                if r.is_ok() {
                    self.fm_reload_local();
                } else {
                    self.fm_set_status(tr(self.lang, "failed"));
                }
            }
            Side::Remote => {
                let path = crate::sftpview::remote_join(&fm.remote.cwd, &name);
                let Some(session) = self.active_session() else {
                    return;
                };
                let ok = self
                    .connector
                    .runtime()
                    .block_on(async move {
                        let sftp = session.open_sftp().await.ok()?;
                        if is_dir {
                            sftp.remove_dir(&path).await.ok()
                        } else {
                            sftp.remove_file(&path).await.ok()
                        }
                    })
                    .is_some();
                if ok {
                    self.fm_reload_remote(false);
                } else {
                    self.fm_set_status(tr(self.lang, "failed"));
                }
            }
        }
    }

    /// マークされた複数ファイルを一括削除する（ファイルのみ）。
    /// 成否をまとめてステータスに出し、宛先ペインを再読込（マークもクリアされる）。
    fn fm_do_delete_many(&mut self, side: Side, names: Vec<String>) {
        let Some(fm) = self.fm.as_ref() else { return };
        let total = names.len();
        let failed = match side {
            Side::Local => {
                let base = PathBuf::from(&fm.local.cwd);
                names
                    .iter()
                    .filter(|n| std::fs::remove_file(base.join(n)).is_err())
                    .count()
            }
            Side::Remote => {
                let cwd = fm.remote.cwd.clone();
                let Some(session) = self.active_session() else {
                    return;
                };
                self.connector.runtime().block_on(async move {
                    let sftp = match session.open_sftp().await {
                        Ok(s) => s,
                        Err(_) => return total, // 全失敗扱い
                    };
                    let mut failed = 0usize;
                    for n in &names {
                        let path = crate::sftpview::remote_join(&cwd, n);
                        if sftp.remove_file(&path).await.is_err() {
                            failed += 1;
                        }
                    }
                    failed
                })
            }
        };
        match side {
            Side::Local => self.fm_reload_local(),
            Side::Remote => self.fm_reload_remote(false),
        }
        let msg = if failed == 0 {
            format!("{total} deleted")
        } else {
            format!("{}/{total} deleted ({failed} failed)", total - failed)
        };
        self.fm_set_status(&msg);
    }

    fn fm_set_status(&mut self, s: &str) {
        if let Some(fm) = self.fm.as_mut() {
            fm.status = Some(s.to_string());
        }
    }

    // ---------- マウス（列見出しクリックでソート / 行選択 / ペイン間 D&D 転送） ----------
    pub(super) fn download_selection(&mut self) {
        let Some(text) = self.current_selection_text() else {
            log::info!("download: 選択テキストがありません");
            return;
        };
        let remote = text.trim().to_string();
        if remote.is_empty() {
            log::info!("download: 選択が空です");
            return;
        }
        let Some(session) = self.active_session() else {
            log::info!("download: 未接続です");
            return;
        };
        let dl_dir = self
            .config
            .download_dir
            .as_deref()
            .map(mot_core::model::expand_tilde)
            .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
            .unwrap_or_else(|| PathBuf::from("."));
        let _ = std::fs::create_dir_all(&dl_dir);
        let base = Path::new(&remote)
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| "download".into());
        let local = dl_dir.join(base);
        let ok = self.connector.runtime().block_on(async move {
            let sftp = session.open_sftp().await.ok()?;
            sftp.download(&remote, &local).await.ok()
        });
        if ok.is_some() {
            log::info!("download: 完了");
        } else {
            log::warn!("download: 失敗");
        }
    }
}

/// ファイルシステムのルート（"/" や "C:\"）か。ルートでは ".." を出さない。
fn is_fs_root(p: &Path) -> bool {
    p.parent().is_none()
}

/// 利用可能なローカルドライブのルート一覧（"C:\\" 形式）。
/// Windows は GetLogicalDrives のビットマスクから生成。それ以外は空（機能無効）。
#[cfg(windows)]
fn local_drives() -> Vec<String> {
    // GetLogicalDrives: 現在利用可能なドライブのビットマスク（bit0=A …）。0 は失敗。
    let mask = unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() };
    crate::sftpview::drives_from_bitmask(mask)
}

#[cfg(not(windows))]
fn local_drives() -> Vec<String> {
    Vec::new()
}

/// パス文字列からドライブルート（"C:\\"）を取り出す。Windows パス想定。
/// ドライブレター（英字1 + ':'）で始まらなければ None。
fn current_drive_root(cwd: Option<&str>) -> Option<String> {
    let cwd = cwd?;
    let mut chars = cwd.chars();
    let letter = chars.next()?;
    if letter.is_ascii_alphabetic() && chars.next() == Some(':') {
        Some(format!("{}:\\", letter.to_ascii_uppercase()))
    } else {
        None
    }
}
