//! async SSH と sync GUI の橋渡し。tokio ランタイムを1本持ち、接続タスクを spawn する。
//! TOFU/パスワード等のブロッキングコールバックは、GUI スレッドへ要求を送って
//! モーダルの結果を std チャネルで受け取ることで同期する。

use mot_core::model::{AuthMethod, Profile};
use mot_core::vault::{Vault, VaultError};
use mot_ssh::{known_hosts, AuthCallbacks, ConnectParams, HostKeyDecision, PaneHandle, SshSession};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 解錠済みボールトの共有ハンドル（プロセス内でセッション横断に保持）。
pub type SharedVault = Arc<Mutex<Option<Vault>>>;

/// 接続識別子（タブと対応）。
pub type ConnId = u64;

/// 接続タスク → GUI へのイベント。
pub enum ConnEvent {
    /// 未知ホスト鍵の TOFU 確認要求。GUI は reply に判断を返す。
    NeedTofu {
        conn: ConnId,
        host: String,
        fingerprint: String,
        reply: Sender<bool>,
    },
    /// パスワード入力要求（マスク）。
    NeedPassword {
        conn: ConnId,
        reply: Sender<Option<String>>,
    },
    /// パスフレーズ入力要求。
    NeedPassphrase {
        conn: ConnId,
        reply: Sender<Option<String>>,
    },
    /// マスターパスワード入力要求（ボールト解錠/新規作成）。
    NeedMaster {
        conn: ConnId,
        is_new: bool,
        reply: Sender<Option<String>>,
    },
    /// 接続成功。セッションと最初のペインを渡す。
    Connected {
        conn: ConnId,
        session: Box<SshSession>,
        pane: Box<PaneHandle>,
    },
    /// 接続失敗（分類済みメッセージ）。
    Failed { conn: ConnId, error: String },
    /// SFTP 転送の進捗（FM 下部帯のバー表示用）。
    TransferProgress {
        name: String,
        done: u64,
        total: u64,
        /// 何個目/全体（1始まり）
        index: usize,
        count: usize,
    },
    /// SFTP 一括転送の完了。from は転送元（再読込するのは反対側）。
    TransferDone {
        from: crate::sftpview::Side,
        total: usize,
        failed: usize,
    },
    /// リモートのシステムメトリクスを1回分採取した。
    Metrics {
        conn: ConnId,
        snap: mot_core::metrics::HostMetrics,
    },
    /// メトリクス採取を諦めた（非対応OS・制限シェル等）。以後このタブでは更新されない。
    MetricsUnavailable { conn: ConnId },
}

/// メトリクス採取タスクの外部制御。GUI 側がタブと同寿命で保持する。
///
/// Clone しない: drop でタスクを止めるため、所有者はタブ 1 箇所に限る。
/// タブを閉じても再接続で差し替えても、これが落ちれば採取タスクも止まる。
pub struct MetricsCtl {
    /// true でタスクを終了させる（タブを閉じた・切断した）
    stop: Arc<AtomicBool>,
    /// true の間は採取をスキップする（情報パネル非表示・最小化中）
    paused: Arc<AtomicBool>,
}

impl MetricsCtl {
    fn new() -> MetricsCtl {
        MetricsCtl {
            stop: Arc::new(AtomicBool::new(false)),
            paused: Arc::new(AtomicBool::new(false)),
        }
    }
    /// 採取の一時停止/再開。リモートへの無駄なコマンド実行を止める。
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, AtomicOrdering::Relaxed);
    }
}

impl Drop for MetricsCtl {
    fn drop(&mut self) {
        self.stop.store(true, AtomicOrdering::Relaxed);
    }
}

/// メトリクス採取が続けて失敗したときに諦める回数。
/// 非対応 OS・制限シェルで延々とコマンドを投げ続けないための上限。
const METRICS_MAX_FAILS: u32 = 3;
/// 1 回の採取に許す時間。コマンド自身が `sleep 1` を含むぶん長めに取る。
const METRICS_TIMEOUT: Duration = Duration::from_secs(8);

pub struct Connector {
    rt: tokio::runtime::Runtime,
    tx: Sender<ConnEvent>,
    pub rx: Receiver<ConnEvent>,
    next_id: ConnId,
    /// 解錠済みボールト（保存済みパスワードの取得・保存に使う）。
    vault: SharedVault,
    vault_path: PathBuf,
    /// config.secret_store != "none" のとき true。
    store_enabled: bool,
}

impl Connector {
    pub fn new() -> anyhow::Result<Connector> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()?;
        let (tx, rx) = std::sync::mpsc::channel();
        Ok(Connector {
            rt,
            tx,
            rx,
            next_id: 1,
            vault: Arc::new(Mutex::new(None)),
            vault_path: PathBuf::from("moterm-secrets.enc"),
            store_enabled: true,
        })
    }

    /// ボールトの保存先と有効/無効を設定する（App::new で config から決定）。
    pub fn configure_vault(&mut self, path: PathBuf, store_enabled: bool) {
        self.vault_path = path;
        self.store_enabled = store_enabled;
    }

    pub fn runtime(&self) -> &tokio::runtime::Runtime {
        &self.rt
    }

    /// バックグラウンドタスクから GUI へイベントを送るための送信端。
    pub fn sender(&self) -> Sender<ConnEvent> {
        self.tx.clone()
    }

    /// プロファイルへの接続を開始する。ConnId を即座に返し、結果は rx に届く。
    pub fn start_connect(&mut self, profile: &Profile, cols: u16, rows: u16) -> ConnId {
        let conn = self.next_id;
        self.next_id += 1;

        let params = build_params(profile, cols, rows);
        let tx = self.tx.clone();
        let profile_log = profile.log.clone();
        let profile_name = profile.name.clone();
        // ボールト連携情報
        let save_pw = matches!(profile.auth, AuthMethod::Password { save: true });
        let use_vault = save_pw && self.store_enabled;
        let secret_key = profile.secret_key();
        let vault = self.vault.clone();
        let vault_path = self.vault_path.clone();

        self.rt.spawn(async move {
            let verifier = make_verifier(conn, params.host.clone(), params.port, tx.clone());
            // 踏み台のホスト鍵は踏み台の host:port で known_hosts 照合・TOFU する
            let bastion_verifier = params
                .proxy_jump
                .as_ref()
                .map(|pj| make_verifier(conn, pj.host.clone(), pj.port, tx.clone()));
            let auth_cb = make_auth_callbacks(
                conn,
                &params.auth,
                tx.clone(),
                vault.clone(),
                vault_path.clone(),
                secret_key.clone(),
                use_vault,
            );

            match SshSession::connect(params.clone(), verifier, bastion_verifier, auth_cb).await {
                Ok(session) => {
                    // 認証成功時のみ、新規入力されたパスワードをボールトへ保存
                    if use_vault {
                        if let Some(pw) = &session.saved_password {
                            if let Ok(mut guard) = vault.lock() {
                                if let Some(v) = guard.as_mut() {
                                    let _ = v.set(&secret_key, pw);
                                }
                            }
                        }
                    }
                    // セッションログ
                    let log = build_log(&profile_log, &profile_name);
                    match session.open_pane(params.cols, params.rows, log).await {
                        Ok(pane) => {
                            let _ = tx.send(ConnEvent::Connected {
                                conn,
                                session: Box::new(session),
                                pane: Box::new(pane),
                            });
                        }
                        Err(e) => {
                            let _ = tx.send(ConnEvent::Failed {
                                conn,
                                error: e.to_string(),
                            });
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(ConnEvent::Failed {
                        conn,
                        error: e.to_string(),
                    });
                }
            }
        });
        conn
    }

    /// メトリクス採取タスクを起動する。interval ごとに 1 回 exec し、結果を
    /// `ConnEvent::Metrics` で GUI へ返す。戻り値の Ctl でタブ側から停止・一時停止する。
    ///
    /// 接続直後は待たずに 1 回採取する（パネルが空のまま数分放置されるのを避ける）。
    pub fn start_metrics(
        &self,
        conn: ConnId,
        probe: mot_ssh::ExecProbe,
        interval: Duration,
    ) -> MetricsCtl {
        let ctl = MetricsCtl::new();
        let tx = self.tx.clone();
        let stop = ctl.stop.clone();
        let paused = ctl.paused.clone();

        self.rt.spawn(async move {
            let mut fails: u32 = 0;
            loop {
                if stop.load(AtomicOrdering::Relaxed) {
                    break;
                }
                // セッションが切れていれば黙って終わる（切断は別経路で通知済み）
                if probe.is_closed() {
                    break;
                }
                if !paused.load(AtomicOrdering::Relaxed) {
                    let result = probe
                        .run(mot_core::metrics::METRICS_COMMAND, METRICS_TIMEOUT)
                        .await;
                    match result {
                        Ok(out) => match mot_core::metrics::parse_metrics(&out) {
                            Some(snap) => {
                                fails = 0;
                                if tx.send(ConnEvent::Metrics { conn, snap }).is_err() {
                                    break; // GUI 側が畳まれた
                                }
                            }
                            None => {
                                fails += 1;
                                log::debug!(
                                    "conn {conn}: メトリクスをパースできません（{fails}回目）: {out:?}"
                                );
                            }
                        },
                        Err(e) => {
                            fails += 1;
                            log::debug!("conn {conn}: メトリクス採取に失敗（{fails}回目）: {e}");
                        }
                    }
                    if fails >= METRICS_MAX_FAILS {
                        log::info!(
                            "conn {conn}: メトリクス採取を停止します（{METRICS_MAX_FAILS}回連続失敗）"
                        );
                        let _ = tx.send(ConnEvent::MetricsUnavailable { conn });
                        break;
                    }
                }
                tokio::time::sleep(interval).await;
            }
        });
        ctl
    }

    /// 既存セッション上に追加ペインを開く（分割）。ブロッキングで即取得。
    pub fn open_pane(
        &self,
        session: &SshSession,
        cols: u16,
        rows: u16,
    ) -> anyhow::Result<PaneHandle> {
        self.rt
            .block_on(session.open_pane(cols, rows, None))
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }
}

fn build_params(profile: &Profile, cols: u16, rows: u16) -> ConnectParams {
    let mut p = ConnectParams::new(profile.host.clone(), profile.port, profile.effective_user());
    p.auth = profile.auth.clone();
    p.forwards = profile.port_forwards.clone();
    // キープアライブは既定で有効（60秒間隔）。NAT/FW/サーバのアイドルタイムアウトに
    // よる無通信セッションの切断を防ぐ。プロファイルで interval_sec=0 にすれば無効化できる。
    p.keepalive_sec = profile
        .keepalive
        .as_ref()
        .map(|k| k.interval_sec)
        .unwrap_or(60);
    p.keepalive_max = profile.keepalive.as_ref().map(|k| k.max).unwrap_or(3);
    p.cols = cols;
    p.rows = rows;
    p.proxy_jump = profile.proxy_jump.as_deref().and_then(|s| {
        let parsed = mot_ssh::ProxyJump::parse(s);
        if parsed.is_none() {
            log::warn!("proxy_jump のパースに失敗（無視）: {s:?}");
        }
        parsed
    });
    p.proxy_command = profile
        .proxy_command
        .clone()
        .filter(|s| !s.trim().is_empty());
    p
}

fn build_log(cfg: &Option<mot_core::model::LogCfg>, name: &str) -> Option<mot_ssh::log::LogTap> {
    let cfg = cfg.as_ref()?;
    if !cfg.enabled {
        return None;
    }
    let dir = cfg.dir.as_ref().map(|d| mot_core::model::expand_tilde(d))?;
    // タイムスタンプは決定性不要（ログ用）。プロセス時刻を秒で。
    let stamp = format!("{}", std::process::id());
    mot_ssh::log::LogTap::create(&dir, name, &stamp, cfg.plaintext).ok()
}

/// known_hosts 照合 + 未知なら GUI へ TOFU 要求を送るホスト鍵検証器。
fn make_verifier(
    conn: ConnId,
    host: String,
    port: u16,
    tx: Sender<ConnEvent>,
) -> mot_ssh::HostKeyVerifier {
    Arc::new(move |key: &russh::keys::ssh_key::PublicKey| {
        let lookup = known_hosts::lookup(&host, port, key);
        let fingerprint = key
            .fingerprint(russh::keys::ssh_key::HashAlg::Sha256)
            .to_string();
        let host2 = host.clone();
        let tx2 = tx.clone();
        let decision = known_hosts::decide(lookup, || {
            // GUI に TOFU を尋ね、返事を待つ（ブロッキング）
            let (rtx, rrx) = std::sync::mpsc::channel();
            let _ = tx2.send(ConnEvent::NeedTofu {
                conn,
                host: host2,
                fingerprint,
                reply: rtx,
            });
            rrx.recv().unwrap_or(false)
        });
        // 承認されたら known_hosts へ追記
        if decision == HostKeyDecision::AcceptNew {
            let _ = known_hosts::learn(&host, port, key);
        }
        decision
    })
}

#[allow(clippy::too_many_arguments)]
fn make_auth_callbacks(
    conn: ConnId,
    auth: &AuthMethod,
    tx: Sender<ConnEvent>,
    vault: SharedVault,
    vault_path: PathBuf,
    secret_key: String,
    use_vault: bool,
) -> AuthCallbacks {
    let mut cb = AuthCallbacks::none();
    if matches!(auth, AuthMethod::Password { .. }) {
        let tx_pw = tx.clone();
        cb.password = Box::new(move || {
            // 1. ボールト連携: 解錠 → 保存済みパスワードがあればプロンプト無しで返す
            if use_vault {
                if let Ok(mut guard) = vault.lock() {
                    if guard.is_none() {
                        let is_new = !vault_path.exists();
                        // 誤マスターは最大3回まで再入力を許す
                        for _ in 0..3 {
                            let (rtx, rrx) = std::sync::mpsc::channel();
                            let _ = tx_pw.send(ConnEvent::NeedMaster {
                                conn,
                                is_new,
                                reply: rtx,
                            });
                            let master = match rrx.recv().unwrap_or(None) {
                                Some(m) if !m.is_empty() => m,
                                _ => break, // キャンセル → 通常入力へフォールバック
                            };
                            match Vault::open_or_create(&vault_path, &master) {
                                Ok(v) => {
                                    *guard = Some(v);
                                    break;
                                }
                                Err(VaultError::WrongMaster) => continue,
                                Err(_) => break,
                            }
                        }
                    }
                    if let Some(v) = guard.as_ref() {
                        if let Some(pw) = v.get(&secret_key) {
                            return Some(pw);
                        }
                    }
                }
            }
            // 2. 通常のパスワード入力
            let (rtx, rrx) = std::sync::mpsc::channel();
            let _ = tx_pw.send(ConnEvent::NeedPassword { conn, reply: rtx });
            rrx.recv().unwrap_or(None)
        });
    }
    if matches!(auth, AuthMethod::Publickey { .. }) {
        let tx_pp = tx.clone();
        cb.passphrase = Box::new(move || {
            let (rtx, rrx) = std::sync::mpsc::channel();
            let _ = tx_pp.send(ConnEvent::NeedPassphrase { conn, reply: rtx });
            rrx.recv().unwrap_or(None)
        });
    }
    cb
}
