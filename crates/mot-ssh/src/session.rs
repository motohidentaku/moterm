//! セッション確立フロー: TCP 接続 → ホスト鍵検証 → 認証 → PTY ペイン。
//! ペイン分割は同一セッション上に追加チャネルを開く（再認証なし）。

use crate::auth::{authenticate, AuthCallbacks};
use crate::error::SshError;
use crate::forward::{self, ForwardStatus};
use crate::handler::{ClientEvent, ClientHandler, HostKeyDecision, HostKeyVerifier};
use crate::params::{ConnectParams, ProxyJump};
use crate::pty::{spawn_pane, PaneHandle};
use mot_core::model::{AuthMethod, ForwardType};
use russh::client::{self, Handle};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

/// 確立済みセッション。GUI が保持し、ペイン追加・PF 参照・SFTP を行う。
pub struct SshSession {
    handle: Arc<Handle<ClientHandler>>,
    params: ConnectParams,
    pane_counter: AtomicU64,
    forwards: Vec<ForwardStatus>,
    /// password 認証で新規入力され、保存対象となったパスワード
    pub saved_password: Option<String>,
    /// proxy_jump 時の踏み台セッション。drop すると direct-tcpip ごと切れるため
    /// 本セッションと同寿命で保持する。
    bastion: Option<Handle<ClientHandler>>,
    /// proxy_command の子プロセス（kill_on_drop で本セッション終了時に回収）。
    _proxy_child: Option<tokio::process::Child>,
}

impl SshSession {
    /// 接続して最初のペインは開かず、セッションだけ返す。
    /// bastion_verifier は proxy_jump 指定時の踏み台ホスト鍵検証。
    /// None のまま proxy_jump を使うと踏み台は常に拒否される（安全側）。
    pub async fn connect(
        params: ConnectParams,
        verifier: HostKeyVerifier,
        bastion_verifier: Option<HostKeyVerifier>,
        mut auth_cb: AuthCallbacks,
    ) -> Result<SshSession, SshError> {
        let config = Arc::new(client::Config {
            keepalive_interval: if params.keepalive_sec > 0 {
                Some(std::time::Duration::from_secs(params.keepalive_sec))
            } else {
                None
            },
            keepalive_max: params.keepalive_max,
            ..Default::default()
        });

        let (event_tx, event_rx) = mpsc::unbounded_channel::<ClientEvent>();
        let handler = ClientHandler {
            verifier,
            events: event_tx,
        };

        // トランスポート確立: proxy_command > proxy_jump > 直接続 の優先順。
        let (mut handle, bastion, proxy_child) = if let Some(cmd) = params.proxy_command.clone() {
            let (h, child) = connect_via_proxy_command(config, &cmd, &params, handler).await?;
            (h, None, Some(child))
        } else if let Some(pj) = params.proxy_jump.clone() {
            let bverifier =
                bastion_verifier.unwrap_or_else(|| Arc::new(|_: &_| HostKeyDecision::Reject));
            let (h, bh) =
                connect_via_jump(config, &pj, &params, handler, bverifier, &mut auth_cb).await?;
            (h, Some(bh), None)
        } else {
            let h = client::connect(config, (params.host.as_str(), params.port), handler)
                .await
                .map_err(connect_err)?;
            (h, None, None)
        };

        // 認証
        let saved_password =
            authenticate(&mut handle, &params.user, &params.auth, &mut auth_cb).await?;

        let mut forwards: Vec<ForwardStatus> = Vec::new();
        let mut remote_targets = Vec::new();

        // -R は tcpip_forward が &mut を要するため Arc 化前に確立する
        for pf in params
            .forwards
            .iter()
            .filter(|p| p.kind == ForwardType::Remote)
        {
            let status = forward::setup_remote(&mut handle, pf).await;
            if !status.ok && pf.required {
                return Err(SshError::Forward(format!(
                    "{} (required) の確立に失敗しました",
                    pf.listen
                )));
            }
            remote_targets.push((pf.listen.clone(), pf.target.clone()));
            forwards.push(status);
        }

        let handle = Arc::new(handle);

        // -L は listen 後にバックグラウンドで direct-tcpip を開く
        for pf in params
            .forwards
            .iter()
            .filter(|p| p.kind == ForwardType::Local)
        {
            let status = forward::setup_local(handle.clone(), pf).await;
            if !status.ok && pf.required {
                return Err(SshError::Forward(format!(
                    "{} (required) の確立に失敗しました",
                    pf.listen
                )));
            }
            forwards.push(status);
        }

        // -R 戻り接続のルーティングタスク
        if !remote_targets.is_empty() {
            tokio::spawn(forward::route_forwarded(event_rx, remote_targets));
        } else {
            // events を破棄しないよう保持だけする（drop で受信側が閉じるが害はない）
            drop(event_rx);
        }

        Ok(SshSession {
            handle,
            params,
            pane_counter: AtomicU64::new(0),
            forwards,
            saved_password,
            bastion,
            _proxy_child: proxy_child,
        })
    }

    /// 新しい PTY ペインを開く（分割 = 追加シェル）。
    pub async fn open_pane(
        &self,
        cols: u16,
        rows: u16,
        log: Option<crate::log::LogTap>,
    ) -> Result<PaneHandle, SshError> {
        let channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(SshError::Russh)?;
        let id = self.pane_counter.fetch_add(1, Ordering::Relaxed);
        spawn_pane(channel, id, cols, rows, log)
            .await
            .map_err(SshError::Russh)
    }

    /// SFTP セッションを開く。
    pub async fn open_sftp(&self) -> Result<crate::sftp::Sftp, SshError> {
        crate::sftp::Sftp::open(&self.handle).await
    }

    pub fn forwards(&self) -> &[ForwardStatus] {
        &self.forwards
    }

    pub fn params(&self) -> &ConnectParams {
        &self.params
    }

    /// セッションが生存しているか（切断検知の簡易判定）。
    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    pub async fn disconnect(&self) {
        let _ = self
            .handle
            .disconnect(russh::Disconnect::ByApplication, "", "en")
            .await;
        if let Some(b) = &self.bastion {
            let _ = b
                .disconnect(russh::Disconnect::ByApplication, "", "en")
                .await;
        }
    }
}

fn connect_err(e: russh::Error) -> SshError {
    match e {
        russh::Error::IO(io) => SshError::Connect(io),
        other => SshError::Russh(other),
    }
}

/// proxy_jump: 踏み台へ接続・認証し、direct-tcpip チャネル上で目的ホストへ
/// SSH セッションを張る（`ssh -J` 相当、外部コマンド不要）。
/// 踏み台の認証は Agent/Publickey ならプロファイルと同じ方式、Password
/// プロファイルでは Agent を試す（目的ホストのパスワードを踏み台へ使い回さない）。
async fn connect_via_jump(
    config: Arc<client::Config>,
    pj: &ProxyJump,
    params: &ConnectParams,
    handler: ClientHandler,
    bastion_verifier: HostKeyVerifier,
    auth_cb: &mut AuthCallbacks,
) -> Result<(Handle<ClientHandler>, Handle<ClientHandler>), SshError> {
    // 踏み台側イベント（-R は張らないので受信は捨てる）
    let (btx, _brx) = mpsc::unbounded_channel::<ClientEvent>();
    let bhandler = ClientHandler {
        verifier: bastion_verifier,
        events: btx,
    };
    let mut bh = client::connect(config.clone(), (pj.host.as_str(), pj.port), bhandler)
        .await
        .map_err(connect_err)?;

    let buser = pj.user.clone().unwrap_or_else(|| params.user.clone());
    let bauth = match &params.auth {
        AuthMethod::Password { .. } => AuthMethod::Agent,
        other => other.clone(),
    };
    authenticate(&mut bh, &buser, &bauth, auth_cb).await?;

    let channel = bh
        .channel_open_direct_tcpip(params.host.clone(), params.port as u32, "127.0.0.1", 0)
        .await
        .map_err(SshError::Russh)?;
    let handle = client::connect_stream(config, channel.into_stream(), handler)
        .await
        .map_err(connect_err)?;
    Ok((handle, bh))
}

/// proxy_command: %h/%p/%r を展開した外部コマンドを起動し、その標準入出力を
/// SSH トランスポートとして使う（OpenSSH ProxyCommand 相当）。
async fn connect_via_proxy_command(
    config: Arc<client::Config>,
    cmd: &str,
    params: &ConnectParams,
    handler: ClientHandler,
) -> Result<(Handle<ClientHandler>, tokio::process::Child), SshError> {
    let expanded = cmd
        .replace("%h", &params.host)
        .replace("%p", &params.port.to_string())
        .replace("%r", &params.user);
    #[cfg(unix)]
    let mut command = {
        let mut c = tokio::process::Command::new("sh");
        c.arg("-c").arg(&expanded);
        c
    };
    #[cfg(windows)]
    let mut command = {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/C").arg(&expanded);
        c
    };
    let mut child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(SshError::Connect)?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stdin = child.stdin.take().expect("piped stdin");
    let handle = client::connect_stream(config, tokio::io::join(stdout, stdin), handler)
        .await
        .map_err(connect_err)?;
    Ok((handle, child))
}
