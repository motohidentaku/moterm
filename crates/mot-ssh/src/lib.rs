//! mot-ssh — russh ベースの SSH セッション層。
//! 認証（agent/publickey/password、.ppk 自動変換）・TOFU ホスト鍵検証・PTY ペイン・
//! 静的ポートフォワード(-L/-R)・SFTP・セッションログ・再接続バックオフ。
//! WezTerm 系 crate は使用しない（russh / russh-sftp のみ）。

pub mod auth;
pub mod error;
pub mod exec;
pub mod forward;
pub mod handler;
pub mod log;
pub mod params;
pub mod pty;
pub mod reconnect;
pub mod session;
pub mod sftp;

pub use auth::AuthCallbacks;
pub use error::SshError;
pub use exec::ExecProbe;
pub use forward::ForwardStatus;
pub use handler::{HostKeyDecision, HostKeyVerifier};
pub use params::{ConnectParams, ProxyJump};
pub use pty::{PaneEvent, PaneHandle};
pub use session::SshSession;
pub use sftp::{DirEntry, Sftp};

/// known_hosts を OpenSSH 互換で照合し、TOFU 判断へ橋渡しするヘルパ。
pub mod known_hosts {
    use crate::handler::HostKeyDecision;
    use russh::keys::known_hosts::{check_known_hosts, learn_known_hosts};
    use russh::keys::ssh_key::PublicKey;

    /// known_hosts 照合結果を分類する。未知の場合は Decision を確定せず `Unknown` を返し、
    /// 呼び出し側（GUI）が TOFU ダイアログで AcceptNew/Reject を決める。
    pub enum Lookup {
        Match,
        Unknown,
        Changed,
    }

    pub fn lookup(host: &str, port: u16, key: &PublicKey) -> Lookup {
        match check_known_hosts(host, port, key) {
            Ok(true) => Lookup::Match,
            Ok(false) => Lookup::Unknown,
            Err(russh::keys::Error::KeyChanged { .. }) => Lookup::Changed,
            Err(_) => Lookup::Unknown,
        }
    }

    /// TOFU 承認後に known_hosts へ追記する。
    pub fn learn(host: &str, port: u16, key: &PublicKey) -> std::io::Result<()> {
        learn_known_hosts(host, port, key).map_err(|e| std::io::Error::other(e.to_string()))
    }

    /// Lookup と TOFU コールバックから最終判断を組み立てる。
    pub fn decide(lookup: Lookup, tofu_accept: impl FnOnce() -> bool) -> HostKeyDecision {
        match lookup {
            Lookup::Match => HostKeyDecision::Match,
            Lookup::Changed => HostKeyDecision::Changed,
            Lookup::Unknown => {
                if tofu_accept() {
                    HostKeyDecision::AcceptNew
                } else {
                    HostKeyDecision::Reject
                }
            }
        }
    }
}
