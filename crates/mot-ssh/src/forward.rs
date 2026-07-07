//! 静的ポートフォワード（-L / -R）。接続時に確立し、状態を PF パネルへ公開する。
//! Dynamic(-D) や動的な追加削除は仕様どおり対象外。

use crate::error::SshError;
use crate::handler::{ClientEvent, ClientHandler};
use mot_core::model::{ForwardType, PortForward};
use russh::client::{Handle, Msg};
use russh::Channel;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// PF 1件の状態（F2 パネル表示用）。
#[derive(Debug, Clone)]
pub struct ForwardStatus {
    pub kind: ForwardType,
    pub listen: String,
    pub target: String,
    pub ok: bool,
    pub error: Option<String>,
    /// 累積転送バイト数
    pub bytes: Arc<AtomicU64>,
}

/// アドレス "host:port" を分解。
fn split_hostport(s: &str) -> Result<(String, u16), SshError> {
    let (h, p) = s
        .rsplit_once(':')
        .ok_or_else(|| SshError::Forward(format!("不正なアドレス: {s}")))?;
    let port: u16 = p
        .parse()
        .map_err(|_| SshError::Forward(format!("不正なポート: {s}")))?;
    Ok((h.to_string(), port))
}

/// -L ローカルフォワードを確立する。ローカルで listen し、接続ごとに direct-tcpip を開く。
pub async fn setup_local(handle: Arc<Handle<ClientHandler>>, pf: &PortForward) -> ForwardStatus {
    let bytes = Arc::new(AtomicU64::new(0));
    let status = |ok, error| ForwardStatus {
        kind: ForwardType::Local,
        listen: pf.listen.clone(),
        target: pf.target.clone(),
        ok,
        error,
        bytes: bytes.clone(),
    };
    let (lhost, lport) = match split_hostport(&pf.listen) {
        Ok(v) => v,
        Err(e) => return status(false, Some(e.to_string())),
    };
    let (thost, tport) = match split_hostport(&pf.target) {
        Ok(v) => v,
        Err(e) => return status(false, Some(e.to_string())),
    };
    let listener = match TcpListener::bind((lhost.as_str(), lport)).await {
        Ok(l) => l,
        Err(e) => return status(false, Some(format!("bind 失敗: {e}"))),
    };
    let counter = bytes.clone();
    tokio::spawn(async move {
        loop {
            let (mut sock, peer) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => break,
            };
            let handle = handle.clone();
            let thost = thost.clone();
            let counter = counter.clone();
            tokio::spawn(async move {
                let ch = handle
                    .channel_open_direct_tcpip(
                        thost.clone(),
                        tport as u32,
                        peer.ip().to_string(),
                        peer.port() as u32,
                    )
                    .await;
                if let Ok(ch) = ch {
                    let _ = pump_channel_stream(ch, &mut sock, counter).await;
                }
            });
        }
    });
    status(true, None)
}

/// -R リモートフォワードを要求する。戻り接続は ClientEvent 経由で route_forwarded に渡る。
/// tcpip_forward は &mut を要するため、Arc 化前の可変ハンドルで呼ぶ。
pub async fn setup_remote(handle: &mut Handle<ClientHandler>, pf: &PortForward) -> ForwardStatus {
    let bytes = Arc::new(AtomicU64::new(0));
    let status = |ok, error| ForwardStatus {
        kind: ForwardType::Remote,
        listen: pf.listen.clone(),
        target: pf.target.clone(),
        ok,
        error,
        bytes: bytes.clone(),
    };
    let (rhost, rport) = match split_hostport(&pf.listen) {
        Ok(v) => v,
        Err(e) => return status(false, Some(e.to_string())),
    };
    match handle.tcpip_forward(rhost, rport as u32).await {
        Ok(_) => status(true, None),
        Err(e) => status(false, Some(format!("リモート待受要求に失敗: {e}"))),
    }
}

/// forwarded-tcpip（-R の戻り）をローカル target へ中継する。
pub async fn route_forwarded(
    mut events: mpsc::UnboundedReceiver<ClientEvent>,
    targets: Vec<(String, String)>, // (listen "host:port", target "host:port")
) {
    while let Some(ev) = events.recv().await {
        let ClientEvent::ForwardedTcpip {
            channel,
            connected_port,
            ..
        } = ev;
        // listen ポートで target を引く
        let target = targets.iter().find_map(|(l, t)| {
            let lp = l.rsplit_once(':').and_then(|(_, p)| p.parse::<u16>().ok());
            if lp == Some(connected_port as u16) {
                Some(t.clone())
            } else {
                None
            }
        });
        if let Some(t) = target {
            if let Ok((thost, tport)) = split_hostport(&t) {
                tokio::spawn(async move {
                    if let Ok(mut sock) =
                        tokio::net::TcpStream::connect((thost.as_str(), tport)).await
                    {
                        let counter = Arc::new(AtomicU64::new(0));
                        let _ = pump_channel_stream(channel, &mut sock, counter).await;
                    }
                });
            }
        }
    }
}

/// SSH チャネル ⇔ TCP ストリームを双方向中継する。
async fn pump_channel_stream(
    mut channel: Channel<Msg>,
    sock: &mut tokio::net::TcpStream,
    counter: Arc<AtomicU64>,
) -> Result<(), std::io::Error> {
    let mut buf = vec![0u8; 32 * 1024];
    let mut writer = channel.make_writer();
    loop {
        tokio::select! {
            msg = channel.wait() => {
                match msg {
                    Some(russh::ChannelMsg::Data { data }) => {
                        counter.fetch_add(data.len() as u64, Ordering::Relaxed);
                        sock.write_all(&data).await?;
                    }
                    Some(russh::ChannelMsg::Eof) | Some(russh::ChannelMsg::Close) | None => break,
                    _ => {}
                }
            }
            r = sock.read(&mut buf) => {
                match r {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        counter.fetch_add(n as u64, Ordering::Relaxed);
                        writer.write_all(&buf[..n]).await?;
                        writer.flush().await?;
                    }
                }
            }
        }
    }
    Ok(())
}
