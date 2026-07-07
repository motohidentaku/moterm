//! russh クライアント Handler。ホスト鍵検証（TOFU）と、リモート発の
//! forwarded-tcpip チャネル（-R の戻り）をアプリ側へ橋渡しする。

use russh::client::Msg;
use russh::client::{Handler, Session};
use russh::keys::ssh_key::PublicKey;
use russh::{Channel, ChannelId};
use std::sync::Arc;
use tokio::sync::mpsc;

/// ホスト鍵の検証判断。TOFU ダイアログの結果をここへ返す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKeyDecision {
    /// known_hosts と一致（無条件で許可）
    Match,
    /// 未知のホスト → TOFU 承認された（許可し、known_hosts へ追記）
    AcceptNew,
    /// 拒否（接続中断）
    Reject,
    /// 鍵が変更されている（接続中断・警告）
    Changed,
}

/// Handler が接続確立時にアプリへ渡すイベント。
pub enum ClientEvent {
    /// -R フォワードでリモートから戻ってきた接続
    ForwardedTcpip {
        channel: Channel<Msg>,
        connected_address: String,
        connected_port: u32,
    },
}

/// ホスト鍵検証コールバック。同期関数（呼び出し側で known_hosts 照合＋TOFU を解決）。
pub type HostKeyVerifier = Arc<dyn Fn(&PublicKey) -> HostKeyDecision + Send + Sync>;

pub struct ClientHandler {
    pub verifier: HostKeyVerifier,
    pub events: mpsc::UnboundedSender<ClientEvent>,
}

impl Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        match (self.verifier)(server_public_key) {
            HostKeyDecision::Match | HostKeyDecision::AcceptNew => Ok(true),
            HostKeyDecision::Reject | HostKeyDecision::Changed => Ok(false),
        }
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: Channel<Msg>,
        connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let _ = self.events.send(ClientEvent::ForwardedTcpip {
            channel,
            connected_address: connected_address.to_string(),
            connected_port,
        });
        Ok(())
    }

    async fn channel_close(
        &mut self,
        _channel: ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
}
