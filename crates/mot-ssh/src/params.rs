//! 接続パラメータ。mot-core の Profile からアプリ側が組み立てて渡す。

use mot_core::model::{AuthMethod, PortForward};

/// proxy_jump 指定（`user@host[:port]`）のパース結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyJump {
    /// 踏み台のログインユーザ。省略時は接続先と同じユーザを使う。
    pub user: Option<String>,
    pub host: String,
    pub port: u16,
}

impl ProxyJump {
    /// `user@host[:port]` をパースする。user 省略可、port 省略時は 22。
    /// host が空、port が数値でない場合は None。
    pub fn parse(s: &str) -> Option<ProxyJump> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        let (user, rest) = match s.rsplit_once('@') {
            Some((u, r)) => (Some(u.to_string()).filter(|u| !u.is_empty()), r),
            None => (None, s),
        };
        let (host, port) = match rest.rsplit_once(':') {
            Some((h, p)) => (h, p.parse().ok()?),
            None => (rest, 22),
        };
        if host.is_empty() {
            return None;
        }
        Some(ProxyJump {
            user,
            host: host.to_string(),
            port,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ConnectParams {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: AuthMethod,
    pub forwards: Vec<PortForward>,
    /// キープアライブ送信間隔（秒）。0 で無効。
    pub keepalive_sec: u64,
    /// 無応答キープアライブの許容回数（russh keepalive_max）。
    pub keepalive_max: usize,
    /// 端末サイズ（最初のペイン）
    pub cols: u16,
    pub rows: u16,
    /// 踏み台（`ssh -J` 相当）。指定時は踏み台経由で direct-tcpip 接続する。
    /// proxy_command と同時指定の場合は proxy_command が優先。
    pub proxy_jump: Option<ProxyJump>,
    /// 外部プロキシコマンド（`ssh -W %h:%p bastion` 等）。%h/%p/%r を展開して
    /// 子プロセスを起動し、その標準入出力を SSH トランスポートに使う。
    pub proxy_command: Option<String>,
}

impl ConnectParams {
    pub fn new(host: impl Into<String>, port: u16, user: impl Into<String>) -> Self {
        ConnectParams {
            host: host.into(),
            port,
            user: user.into(),
            auth: AuthMethod::Agent,
            forwards: Vec::new(),
            keepalive_sec: 0,
            keepalive_max: 3,
            cols: 80,
            rows: 24,
            proxy_jump: None,
            proxy_command: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_jump_parse_full() {
        assert_eq!(
            ProxyJump::parse("admin@bastion.example.com:2222"),
            Some(ProxyJump {
                user: Some("admin".into()),
                host: "bastion.example.com".into(),
                port: 2222,
            })
        );
    }

    #[test]
    fn proxy_jump_parse_defaults() {
        assert_eq!(
            ProxyJump::parse("bastion"),
            Some(ProxyJump {
                user: None,
                host: "bastion".into(),
                port: 22,
            })
        );
        assert_eq!(
            ProxyJump::parse("user@bastion"),
            Some(ProxyJump {
                user: Some("user".into()),
                host: "bastion".into(),
                port: 22,
            })
        );
    }

    #[test]
    fn proxy_jump_parse_invalid() {
        assert_eq!(ProxyJump::parse(""), None);
        assert_eq!(ProxyJump::parse("  "), None);
        assert_eq!(ProxyJump::parse("user@"), None);
        assert_eq!(ProxyJump::parse("host:notaport"), None);
        // user 空でも host があれば通す（@bastion）
        assert_eq!(
            ProxyJump::parse("@bastion"),
            Some(ProxyJump {
                user: None,
                host: "bastion".into(),
                port: 22,
            })
        );
    }
}
