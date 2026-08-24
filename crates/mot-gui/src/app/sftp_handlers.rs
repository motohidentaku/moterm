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
    ///
    /// `names` にディレクトリが含まれる場合は中身を再帰的に展開して転送する
    /// （空ディレクトリも宛先に作る）。進捗の分母はファイル・ディレクトリを
    /// 合わせた展開後の件数。
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
        // 0% を即時表示（最初の進捗イベントを待たない）。件数は再帰展開後に補正される。
        if let Some(f) = self.fm.as_mut() {
            f.progress = Some(crate::sftpview::FmProgress {
                name: names[0].clone(),
                done: 0,
                total: 0,
                index: 1,
                count: names.len(),
            });
        }
        let tx = self.connector.sender();
        self.connector.runtime().spawn(async move {
            let sftp = match session.open_sftp().await {
                Ok(s) => s,
                Err(_) => {
                    let _ = tx.send(ConnEvent::TransferDone {
                        from,
                        total: names.len(),
                        failed: names.len(),
                    });
                    return;
                }
            };
            // 1) ディレクトリを再帰展開して平坦なジョブ列にする（親が中身より先）。
            let (jobs, plan_failed) =
                plan_transfer_jobs(&sftp, from, &local_cwd, &remote_cwd, &names).await;
            let count = jobs.len();
            // 2) 先頭から順に転送。ディレクトリは宛先に作るだけ。
            let mut failed = plan_failed;
            for (i, job) in jobs.iter().enumerate() {
                let local_path = local_join(Path::new(&local_cwd), &job.rel);
                let remote_path = crate::sftpview::remote_join(&remote_cwd, &job.rel);
                let txp = tx.clone();
                let pname = job.rel.clone();
                let mut progress = move |done: u64, total: u64| {
                    let _ = txp.send(ConnEvent::TransferProgress {
                        name: pname.clone(),
                        done,
                        total,
                        index: i + 1,
                        count,
                    });
                };
                if job.is_dir {
                    // ディレクトリ自体はバイト数を持たないので 0/0 で1件ぶん進める
                    progress(0, 0);
                    let ok = match from {
                        Side::Local => sftp.mkdir_p(&remote_path).await.is_ok(),
                        Side::Remote => std::fs::create_dir_all(&local_path).is_ok(),
                    };
                    if !ok {
                        failed += 1;
                    }
                    continue;
                }
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
            let _ = tx.send(ConnEvent::TransferDone {
                from,
                total: count + plan_failed,
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
        // マークがあれば一括削除（1件でもマークを優先＝転送と対称）。
        // 無ければ選択1件。どちらもディレクトリは中身ごと消える。
        let marked = fm.active_pane().marked_names();
        if !marked.is_empty() {
            let dirs = marked
                .iter()
                .filter(|n| fm.active_pane().is_dir_named(n))
                .count();
            let msg = if dirs > 0 {
                format!(
                    "{} {} item(s), {} dir(s) recursive?",
                    tr(self.lang, "delete"),
                    marked.len(),
                    dirs
                )
            } else {
                format!("{} {} file(s)?", tr(self.lang, "delete"), marked.len())
            };
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
        // ディレクトリは中身ごと消えるので、確認文でそれを明示する。
        let msg = if is_dir {
            format!("{} '{}' (recursive)", tr(self.lang, "delete"), name)
        } else {
            format!("{} '{}'", tr(self.lang, "delete"), name)
        };
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
                            // ローカル側の remove_dir_all と揃える（確認済み前提）
                            sftp.remove_dir_all(&path).await.ok()
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

    /// マークされた複数エントリを一括削除する。ディレクトリは中身ごと再帰削除する
    /// （種別は一覧から引く）。成否をまとめてステータスに出し、ペインを再読込
    /// （マークもクリアされる）。
    fn fm_do_delete_many(&mut self, side: Side, names: Vec<String>) {
        let Some(fm) = self.fm.as_ref() else { return };
        let total = names.len();
        // (名前, ディレクトリか) に解決してから実行する（一覧を跨いで参照しないため）。
        let targets: Vec<(String, bool)> = names
            .iter()
            .map(|n| (n.clone(), fm.pane(side).is_dir_named(n)))
            .collect();
        let failed = match side {
            Side::Local => {
                let base = PathBuf::from(&fm.local.cwd);
                targets
                    .iter()
                    .filter(|(n, is_dir)| {
                        let p = base.join(n);
                        if *is_dir {
                            std::fs::remove_dir_all(&p).is_err()
                        } else {
                            std::fs::remove_file(&p).is_err()
                        }
                    })
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
                    for (n, is_dir) in &targets {
                        let path = crate::sftpview::remote_join(&cwd, n);
                        let r = if *is_dir {
                            sftp.remove_dir_all(&path).await
                        } else {
                            sftp.remove_file(&path).await
                        };
                        if r.is_err() {
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

/// `base` に "a/b" 形式の相対パスを繋ぐ。区切りを分解して push するので
/// Windows でも正しいパスになる。
fn local_join(base: &Path, rel: &str) -> PathBuf {
    let mut p = base.to_path_buf();
    for c in rel.split('/').filter(|s| !s.is_empty()) {
        p.push(c);
    }
    p
}

/// ローカルの `base` 以下を再帰列挙する（`base` 自身は含まない）。
///
/// 返り値は**先行順**（親ディレクトリが必ずその中身より先）。相対パスの区切りは "/"。
/// シンボリックリンクは `symlink_metadata` で判定して辿らないので、リンクのループで
/// 無限に潜ることはない（リンクはファイル扱いになり、転送時に中身が読まれる）。
/// 読めないディレクトリはその枝を飛ばす。件数が上限を超えたらそこで打ち切る。
fn walk_local(base: &Path) -> Vec<mot_ssh::WalkEntry> {
    let mut out: Vec<mot_ssh::WalkEntry> = Vec::new();
    // 未走査ディレクトリの相対パス。"" は base 自身。
    let mut stack: Vec<String> = vec![String::new()];
    while let Some(rel) = stack.pop() {
        let dir = local_join(base, &rel);
        let Ok(rd) = std::fs::read_dir(&dir) else {
            log::warn!("転送: {} を読めないので飛ばす", dir.display());
            continue;
        };
        for ent in rd.flatten() {
            let name = ent.file_name().to_string_lossy().to_string();
            let child = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            let is_dir = std::fs::symlink_metadata(ent.path())
                .map(|m| m.is_dir())
                .unwrap_or(false);
            if out.len() >= mot_ssh::sftp::MAX_WALK_ENTRIES {
                log::warn!(
                    "転送: {} のエントリ数が上限 {} を超えたので打ち切る",
                    base.display(),
                    mot_ssh::sftp::MAX_WALK_ENTRIES
                );
                return out;
            }
            out.push(mot_ssh::WalkEntry {
                rel: child.clone(),
                is_dir,
            });
            if is_dir {
                stack.push(child);
            }
        }
    }
    out
}

/// ローカル側の転送対象1件をジョブ列へ展開する（自分自身＋ディレクトリなら中身）。
/// `rel` は `local_cwd` からの相対パス。
fn plan_local_jobs(local_cwd: &str, name: &str) -> Vec<mot_ssh::WalkEntry> {
    let root = local_join(Path::new(local_cwd), name);
    let is_dir = std::fs::symlink_metadata(&root)
        .map(|m| m.is_dir())
        .unwrap_or(false);
    let mut jobs = vec![mot_ssh::WalkEntry {
        rel: name.to_string(),
        is_dir,
    }];
    if is_dir {
        jobs.extend(walk_local(&root).into_iter().map(|e| mot_ssh::WalkEntry {
            rel: format!("{name}/{}", e.rel),
            is_dir: e.is_dir,
        }));
    }
    jobs
}

/// 転送対象名の並びを、ディレクトリ再帰込みの平坦なジョブ列へ展開する。
/// `rel` は転送元 cwd からの相対パスで、宛先でも同じ相対位置に置かれる。
///
/// 返り値の2つ目は「列挙に失敗した件数」（リモートの読み取り失敗など）。
/// 失敗した枝は中身を転送できないので、呼び出し側で失敗数に加算する。
async fn plan_transfer_jobs(
    sftp: &mot_ssh::Sftp,
    from: Side,
    local_cwd: &str,
    remote_cwd: &str,
    names: &[String],
) -> (Vec<mot_ssh::WalkEntry>, usize) {
    let mut jobs: Vec<mot_ssh::WalkEntry> = Vec::new();
    let mut failed = 0usize;
    for name in names {
        match from {
            Side::Local => jobs.extend(plan_local_jobs(local_cwd, name)),
            Side::Remote => {
                let root = crate::sftpview::remote_join(remote_cwd, name);
                let is_dir = sftp.is_dir(&root).await;
                jobs.push(mot_ssh::WalkEntry {
                    rel: name.clone(),
                    is_dir,
                });
                if is_dir {
                    match sftp.walk(&root).await {
                        Ok(list) => {
                            jobs.extend(list.into_iter().map(|e| mot_ssh::WalkEntry {
                                rel: format!("{name}/{}", e.rel),
                                is_dir: e.is_dir,
                            }));
                        }
                        Err(e) => {
                            log::warn!("転送: {root} の列挙に失敗: {e}");
                            failed += 1;
                        }
                    }
                }
            }
        }
    }
    (jobs, failed)
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

#[cfg(test)]
mod tests {
    use super::{local_join, plan_local_jobs, walk_local};
    use std::path::{Path, PathBuf};

    /// テスト専用の一時ディレクトリ（外部 crate に頼らない）。Drop で消す。
    struct TmpDir(PathBuf);

    impl TmpDir {
        fn new(tag: &str) -> TmpDir {
            // 同一プロセス内の並行テストでも衝突しないよう連番を混ぜる
            use std::sync::atomic::{AtomicU32, Ordering};
            static N: AtomicU32 = AtomicU32::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let p =
                std::env::temp_dir().join(format!("moterm-walk-{tag}-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            TmpDir(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn local_join_splits_rel_separators() {
        let p = local_join(Path::new("/base"), "sub/deep/f.bin");
        assert_eq!(
            p,
            PathBuf::from("/base")
                .join("sub")
                .join("deep")
                .join("f.bin")
        );
        // 空の相対パスは base そのもの
        assert_eq!(local_join(Path::new("/base"), ""), PathBuf::from("/base"));
    }

    #[test]
    fn walk_local_lists_tree_parent_before_children() {
        let tmp = TmpDir::new("tree");
        let root = tmp.path();
        std::fs::create_dir_all(root.join("sub").join("deep")).unwrap();
        std::fs::create_dir(root.join("empty")).unwrap();
        std::fs::write(root.join("a.txt"), b"a").unwrap();
        std::fs::write(root.join("sub").join("b.txt"), b"b").unwrap();
        std::fs::write(root.join("sub").join("deep").join("c.txt"), b"c").unwrap();

        let out = walk_local(root);
        let rels: Vec<&str> = out.iter().map(|e| e.rel.as_str()).collect();
        // 中身は網羅される（空ディレクトリも1件として出る）
        let mut sorted = rels.clone();
        sorted.sort_unstable();
        assert_eq!(
            sorted,
            vec![
                "a.txt",
                "empty",
                "sub",
                "sub/b.txt",
                "sub/deep",
                "sub/deep/c.txt"
            ]
        );
        // 種別
        let dir_of = |name: &str| out.iter().find(|e| e.rel == name).unwrap().is_dir;
        assert!(dir_of("sub") && dir_of("sub/deep") && dir_of("empty"));
        assert!(!dir_of("a.txt") && !dir_of("sub/b.txt"));
        // 先行順: 親が必ず中身より先（宛先で親を先に作れること）
        let pos = |name: &str| rels.iter().position(|r| *r == name).unwrap();
        assert!(pos("sub") < pos("sub/b.txt"));
        assert!(pos("sub") < pos("sub/deep"));
        assert!(pos("sub/deep") < pos("sub/deep/c.txt"));
    }

    #[test]
    fn plan_local_jobs_expands_dir_and_keeps_file_alone() {
        let tmp = TmpDir::new("plan");
        let cwd = tmp.path();
        std::fs::create_dir_all(cwd.join("d").join("inner")).unwrap();
        std::fs::write(cwd.join("d").join("x.txt"), b"x").unwrap();
        std::fs::write(cwd.join("d").join("inner").join("y.txt"), b"y").unwrap();
        std::fs::write(cwd.join("solo.txt"), b"s").unwrap();
        let cwd_s = cwd.to_string_lossy().to_string();

        // ファイル1件はそのまま1ジョブ
        let solo = plan_local_jobs(&cwd_s, "solo.txt");
        assert_eq!(solo.len(), 1);
        assert_eq!(solo[0].rel, "solo.txt");
        assert!(!solo[0].is_dir);

        // ディレクトリは自分自身＋中身。rel は cwd 起点（宛先で同じ形に置ける）
        let jobs = plan_local_jobs(&cwd_s, "d");
        let mut rels: Vec<&str> = jobs.iter().map(|j| j.rel.as_str()).collect();
        let order = rels.clone();
        rels.sort_unstable();
        assert_eq!(rels, vec!["d", "d/inner", "d/inner/y.txt", "d/x.txt"]);
        // 先頭は必ず対象ディレクトリ自身＝宛先で最初に作られる
        assert_eq!(order[0], "d");
        assert!(jobs[0].is_dir);
        let pos = |n: &str| order.iter().position(|r| *r == n).unwrap();
        assert!(pos("d/inner") < pos("d/inner/y.txt"));
    }

    #[test]
    fn plan_local_jobs_missing_name_is_treated_as_file() {
        let tmp = TmpDir::new("missing");
        let jobs = plan_local_jobs(&tmp.path().to_string_lossy(), "nope.txt");
        // 存在しなければファイル1件として積み、転送段で失敗として数えられる
        assert_eq!(jobs.len(), 1);
        assert!(!jobs[0].is_dir);
    }

    #[test]
    fn walk_local_empty_dir_yields_nothing() {
        let tmp = TmpDir::new("empty");
        assert!(walk_local(tmp.path()).is_empty());
    }

    /// シンボリックリンクは辿らない＝リンクの循環でループしない。
    #[cfg(unix)]
    #[test]
    fn walk_local_does_not_follow_symlink_loop() {
        let tmp = TmpDir::new("link");
        let root = tmp.path();
        std::fs::create_dir(root.join("d")).unwrap();
        std::fs::write(root.join("d").join("x.txt"), b"x").unwrap();
        // d/loop -> ..（自分の親）。辿ると無限に潜る形。
        std::os::unix::fs::symlink("..", root.join("d").join("loop")).unwrap();

        let out = walk_local(root);
        let rels: Vec<&str> = out.iter().map(|e| e.rel.as_str()).collect();
        let mut sorted = rels.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, vec!["d", "d/loop", "d/x.txt"]);
        // リンクは展開されず、ディレクトリ扱いにもならない
        assert!(!out.iter().find(|e| e.rel == "d/loop").unwrap().is_dir);
    }
}
