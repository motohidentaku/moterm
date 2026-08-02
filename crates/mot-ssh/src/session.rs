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

    /// 非対話コマンド実行用のハンドルを得る（メトリクス採取などのバックグラウンド用途）。
    /// Clone 可能でセッション本体とは独立に tokio タスクへ渡せる。
    pub fn exec_probe(&self) -> crate::exec::ExecProbe {
        crate::exec::ExecProbe::new(self.handle.clone())
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

/// OpenSSH ProxyCommand のトークン展開（`%h` ホスト / `%p` ポート / `%r` ユーザ）。
/// 置換後の文字列にトークンが現れても再展開しないよう1パスで処理する。
fn expand_proxy_command(cmd: &str, host: &str, port: u16, user: &str) -> String {
    let mut out = String::with_capacity(cmd.len());
    let mut rest = cmd;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let mut chars = rest[i..].chars();
        chars.next(); // '%'
        match chars.next() {
            Some('h') => out.push_str(host),
            Some('p') => out.push_str(&port.to_string()),
            Some('r') => out.push_str(user),
            Some('%') => out.push('%'),
            // 未知のトークンは OpenSSH 同様そのまま残す
            Some(c) => {
                out.push('%');
                out.push(c);
            }
            // 末尾の孤立した '%'。rest を空にしてから抜ける
            // （進めずに break すると下の push_str(rest) で残りが二重に入る）
            None => {
                out.push('%');
                rest = "";
                break;
            }
        }
        rest = chars.as_str();
    }
    out.push_str(rest);
    out
}

/// proxy_command の stderr から保持する末尾行数（接続エラーに添える分）。
const PROXY_STDERR_TAIL_LINES: usize = 10;
/// トランスポート確立が失敗したあと stderr を読み切るのを待つ上限。
/// 子プロセスは既に死んでいるので EOF は即来るが、読み取りタスクとの
/// レースで行を取りこぼさないよう短く待つ。
const PROXY_STDERR_DRAIN: std::time::Duration = std::time::Duration::from_millis(500);

/// proxy_command: %h/%p/%r を展開した外部コマンドを起動し、その標準入出力を
/// SSH トランスポートとして使う（OpenSSH ProxyCommand 相当）。
///
/// stderr は捨てずに log::warn へ流し、末尾数行を保持して失敗時のエラーに載せる。
/// `aws ssm start-session` 等は失敗理由（SSO 期限切れ / session-manager-plugin
/// 未導入 / IAM 権限不足）を stderr にしか出さないため、これが無いと
/// ユーザには russh の EOF エラーしか見えない。
async fn connect_via_proxy_command(
    config: Arc<client::Config>,
    cmd: &str,
    params: &ConnectParams,
    handler: ClientHandler,
) -> Result<(Handle<ClientHandler>, tokio::process::Child), SshError> {
    let expanded = expand_proxy_command(cmd, &params.host, params.port, &params.user);
    #[cfg(unix)]
    let mut command = {
        let mut c = tokio::process::Command::new("sh");
        c.arg("-c").arg(&expanded);
        c
    };
    #[cfg(windows)]
    let mut command = {
        // release ビルドの moterm.exe は windows_subsystem="windows" でコンソールを
        // 持たないため、cmd /C が自前でコンソールを1枚開いてしまう。proxy_command は
        // 接続している間ずっと生きるので、その黒い窓も出しっぱなしになる。
        // CREATE_NO_WINDOW で抑止する（子孫の aws CLI 等にも引き継がれる）。
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/C").arg(&expanded).creation_flags(CREATE_NO_WINDOW);
        c
    };
    log::info!("proxy_command を起動: {expanded}");
    let mut child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| SshError::ProxyCommand(format!("{expanded}: 起動できません: {e}")))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stdin = child.stdin.take().expect("piped stdin");
    let stderr = child.stderr.take().expect("piped stderr");

    // stderr は接続後も出続ける（SSM の警告など）ので、読み取りタスクは
    // 子プロセスが EOF を返すまで動かし続ける。
    let tail = Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));
    let reader = tokio::spawn(read_proxy_stderr(stderr, tail.clone()));

    match client::connect_stream(config, tokio::io::join(stdout, stdin), handler).await {
        Ok(handle) => Ok((handle, child)),
        Err(e) => {
            let _ = tokio::time::timeout(PROXY_STDERR_DRAIN, reader).await;
            let detail = tail
                .lock()
                .map(|t| t.iter().cloned().collect::<Vec<String>>().join("\n"))
                .unwrap_or_default();
            if detail.is_empty() {
                // stderr が無いなら russh のエラーをそのまま（到達不可等の分類を保つ）
                Err(connect_err(e))
            } else {
                Err(SshError::ProxyCommand(format!("{expanded}\n{detail}")))
            }
        }
    }
}

/// proxy_command の stderr を1行ずつログへ流しつつ、末尾
/// `PROXY_STDERR_TAIL_LINES` 行を `tail` に保持する。
async fn read_proxy_stderr(
    stderr: tokio::process::ChildStderr,
    tail: Arc<std::sync::Mutex<std::collections::VecDeque<String>>>,
) {
    use tokio::io::AsyncBufReadExt;
    let mut lines = tokio::io::BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        log::warn!("proxy_command stderr: {line}");
        if let Ok(mut t) = tail.lock() {
            if t.len() == PROXY_STDERR_TAIL_LINES {
                t.pop_front();
            }
            t.push_back(line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_proxy_command_tokens() {
        assert_eq!(
            expand_proxy_command("ssh -W %h:%p bastion", "target.example.com", 2222, "admin"),
            "ssh -W target.example.com:2222 bastion"
        );
        // AWS SSM の典型形（%h にインスタンス ID が入る）
        assert_eq!(
            expand_proxy_command(
                "aws ssm start-session --target %h --parameters portNumber=%p",
                "i-0123456789abcdef0",
                22,
                "ec2-user"
            ),
            "aws ssm start-session --target i-0123456789abcdef0 --parameters portNumber=22"
        );
        // %r はユーザ名、%% はリテラルの %、未知トークンはそのまま残す
        assert_eq!(
            expand_proxy_command("%r %% %z", "h", 22, "alice"),
            "alice % %z"
        );
        // 置換結果に含まれるトークンは再展開しない
        assert_eq!(expand_proxy_command("%h", "%p", 22, "u"), "%p");
        // 末尾の孤立した % は落とさない
        assert_eq!(expand_proxy_command("cmd %", "h", 22, "u"), "cmd %");
    }

    /// proxy_command が即座に失敗したとき、stderr の内容が接続エラーに載ること。
    /// これが無いと russh の EOF エラーしか見えず、aws ssm の失敗理由
    /// （SSO 期限切れ・plugin 未導入等）が UI に届かない。
    #[cfg(unix)]
    #[tokio::test]
    async fn proxy_command_failure_reports_stderr() {
        let mut params = ConnectParams::new("target", 22, "user");
        params.proxy_command =
            Some("echo 'Token has expired and refresh failed' >&2; exit 1".into());
        // SshSession は Debug を実装しないため expect_err は使えない
        let err = match SshSession::connect(
            params,
            Arc::new(|_| HostKeyDecision::Match),
            None,
            AuthCallbacks::none(),
        )
        .await
        {
            Ok(_) => panic!("proxy_command が失敗するので接続は成功しないはず"),
            Err(e) => e,
        };
        let msg = err.to_string();
        assert!(
            matches!(err, SshError::ProxyCommand(_)),
            "ProxyCommand 由来のエラーになるべき: {msg}"
        );
        assert!(
            msg.contains("Token has expired"),
            "stderr の内容がエラーに含まれるべき: {msg}"
        );
    }

    /// 起動できないコマンドも到達不可（Connect）ではなく ProxyCommand として分類する。
    #[cfg(unix)]
    #[tokio::test]
    async fn proxy_command_not_found_is_classified() {
        let mut params = ConnectParams::new("target", 22, "user");
        params.proxy_command = Some("moterm-no-such-command-xyz".into());
        let err = match SshSession::connect(
            params,
            Arc::new(|_| HostKeyDecision::Match),
            None,
            AuthCallbacks::none(),
        )
        .await
        {
            Ok(_) => panic!("存在しないコマンドなので接続は成功しないはず"),
            Err(e) => e,
        };
        assert!(
            matches!(err, SshError::ProxyCommand(_)),
            "ProxyCommand 由来のエラーになるべき: {err}"
        );
    }
}
