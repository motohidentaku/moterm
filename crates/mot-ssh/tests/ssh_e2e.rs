//! 実 sshd に対する統合テスト。docker/e2e-ssh.sh で起動した使い捨て sshd を使う。
//! 環境変数が無いときは自動スキップ（通常の `cargo test` を汚さない）。
//!
//! 実行:
//!   docker/e2e-ssh.sh で sshd 起動 → MOTERM_E2E=1 等を設定して cargo test。
//!   （docker/e2e-run.sh がこれらをまとめて行う）

use mot_core::model::{AuthMethod, ForwardType, PortForward};
use mot_ssh::{AuthCallbacks, ConnectParams, HostKeyDecision, PaneEvent, SshSession};
use std::sync::Arc;
use std::time::Duration;

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

/// テスト環境が揃っていなければ None（＝スキップ）。
fn e2e_params(auth: AuthMethod) -> Option<ConnectParams> {
    let port: u16 = env("MOTERM_SSH_PORT")?.parse().ok()?;
    let user = env("MOTERM_SSH_USER").or_else(|| env("USER"))?;
    let mut p = ConnectParams::new("127.0.0.1", port, user);
    p.auth = auth;
    Some(p)
}

fn accept_all() -> mot_ssh::HostKeyVerifier {
    Arc::new(|_: &_| HostKeyDecision::AcceptNew)
}

async fn collect_output(pane: &mut mot_ssh::PaneHandle, dur: Duration) -> Vec<u8> {
    let mut buf = Vec::new();
    let deadline = tokio::time::Instant::now() + dur;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, pane.events.recv()).await {
            Ok(Some(PaneEvent::Output(d))) => buf.extend_from_slice(&d),
            Ok(Some(PaneEvent::Exited(_))) | Ok(None) => break,
            Err(_) => break,
        }
    }
    buf
}

#[tokio::test]
async fn pubkey_shell_echo() {
    let Some(mut params) = e2e_params(AuthMethod::Publickey {
        key: env("MOTERM_SSH_KEY").unwrap_or_default(),
    }) else {
        eprintln!("skip: MOTERM_SSH_* 未設定");
        return;
    };
    params.cols = 80;
    params.rows = 24;

    let session = SshSession::connect(params, accept_all(), None, AuthCallbacks::none())
        .await
        .expect("connect");
    let mut pane = session.open_pane(80, 24, None).await.expect("pane");

    // シェルが立ち上がるのを待つ
    let _ = collect_output(&mut pane, Duration::from_millis(800)).await;
    pane.write(b"echo MOTERM_OK_$((6*7))\n".to_vec());
    let out = collect_output(&mut pane, Duration::from_secs(3)).await;
    let text = String::from_utf8_lossy(&out);
    assert!(text.contains("MOTERM_OK_42"), "output was: {text}");
}

#[tokio::test]
async fn password_auth_and_local_forward() {
    let password = match env("MOTERM_SSH_PASSWORD") {
        Some(p) => p,
        None => {
            eprintln!("skip: MOTERM_SSH_PASSWORD 未設定");
            return;
        }
    };
    let Some(mut params) = e2e_params(AuthMethod::Password { save: false }) else {
        return;
    };
    // -L: ローカル 127.0.0.1:0（OS 割当は使えないので固定ポート）→ リモート sshd 自身へ
    let lport = env("MOTERM_LFWD_PORT")
        .and_then(|p| p.parse().ok())
        .unwrap_or(15422u16);
    let rport = env("MOTERM_SSH_PORT").unwrap();
    params.forwards = vec![PortForward {
        kind: ForwardType::Local,
        listen: format!("127.0.0.1:{lport}"),
        target: format!("127.0.0.1:{rport}"),
        required: true,
    }];

    let mut cb = AuthCallbacks::none();
    cb.password = Box::new(move || Some(password.clone()));

    let session = SshSession::connect(params, accept_all(), None, cb)
        .await
        .expect("connect");
    assert_eq!(session.forwards().len(), 1);
    assert!(session.forwards()[0].ok, "forward should be established");

    // 転送先（=sshd）へ TCP 接続すると SSH バナーが返る
    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", lport))
        .await
        .expect("connect via -L");
    use tokio::io::AsyncReadExt;
    let mut banner = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(3), sock.read_exact(&mut banner))
        .await
        .expect("read timeout")
        .expect("read banner");
    assert_eq!(&banner, b"SSH-", "banner via -L forward");
}

/// proxy_jump: 既存 sshd を踏み台にして、direct-tcpip で同じ sshd 自身へ
/// ループバック接続する（踏み台→接続先の2段認証と connect_stream の検証）。
#[tokio::test]
async fn proxy_jump_shell_echo() {
    let key = env("MOTERM_SSH_KEY").unwrap_or_default();
    let Some(mut params) = e2e_params(AuthMethod::Publickey { key }) else {
        eprintln!("skip: MOTERM_SSH_* 未設定");
        return;
    };
    let user = params.user.clone();
    params.proxy_jump = mot_ssh::ProxyJump::parse(&format!("{user}@127.0.0.1:{}", params.port));
    assert!(params.proxy_jump.is_some());

    let session = SshSession::connect(
        params,
        accept_all(),
        Some(accept_all()),
        AuthCallbacks::none(),
    )
    .await
    .expect("connect via proxy_jump");
    let mut pane = session.open_pane(80, 24, None).await.expect("pane");

    let _ = collect_output(&mut pane, Duration::from_millis(800)).await;
    pane.write(b"echo MOTERM_JUMP_$((6*7))\n".to_vec());
    let out = collect_output(&mut pane, Duration::from_secs(3)).await;
    let text = String::from_utf8_lossy(&out);
    assert!(text.contains("MOTERM_JUMP_42"), "output was: {text}");
}

#[tokio::test]
async fn sftp_roundtrip() {
    let Some(params) = e2e_params(AuthMethod::Publickey {
        key: env("MOTERM_SSH_KEY").unwrap_or_default(),
    }) else {
        return;
    };
    let session = SshSession::connect(params, accept_all(), None, AuthCallbacks::none())
        .await
        .expect("connect");
    let sftp = session.open_sftp().await.expect("sftp");

    let dir = env("MOTERM_E2E_DIR").unwrap_or_else(|| "/tmp".into());
    let remote = format!("{dir}/moterm_sftp_test.txt");
    let local = std::env::temp_dir().join("moterm_sftp_local.txt");
    std::fs::write(&local, b"hello-sftp-42").unwrap();

    sftp.upload(&local, &remote).await.expect("upload");
    assert!(sftp.exists(&remote).await.unwrap());

    let back = std::env::temp_dir().join("moterm_sftp_back.txt");
    sftp.download(&remote, &back).await.expect("download");
    assert_eq!(std::fs::read(&back).unwrap(), b"hello-sftp-42");

    sftp.remove_file(&remote).await.expect("cleanup");
}

#[tokio::test]
async fn exec_captures_stdout() {
    let Some(params) = e2e_params(AuthMethod::Publickey {
        key: env("MOTERM_SSH_KEY").unwrap_or_default(),
    }) else {
        return;
    };
    let session = SshSession::connect(params, accept_all(), None, AuthCallbacks::none())
        .await
        .expect("connect");
    let probe = session.exec_probe();

    // stdout だけを拾い、stderr は混ざらないこと
    let out = probe
        .run(
            "echo MOTERM_EXEC_$((6*7)); echo noise >&2",
            Duration::from_secs(5),
        )
        .await
        .expect("exec");
    assert!(out.contains("MOTERM_EXEC_42"), "output was: {out}");
    assert!(!out.contains("noise"), "stderr が混入: {out}");

    // 非ゼロ終了でも stdout は返る
    let out = probe
        .run("echo before; exit 3", Duration::from_secs(5))
        .await
        .expect("exec with nonzero exit");
    assert!(out.contains("before"), "output was: {out}");

    // タイムアウトはエラーとして返る（ハングしない）
    let err = probe.run("sleep 10", Duration::from_secs(1)).await;
    assert!(err.is_err(), "タイムアウトが検出されていない");

    // 実際のメトリクス採取コマンドが Linux で通ること
    let out = probe
        .run(mot_core::metrics::METRICS_COMMAND, Duration::from_secs(8))
        .await
        .expect("metrics exec");
    assert!(
        mot_core::metrics::parse_metrics(&out).is_some(),
        "メトリクスをパースできない: {out}"
    );
}
