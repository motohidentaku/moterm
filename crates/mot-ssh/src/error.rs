//! SSH 層のエラー分類。失敗理由を UI で分類表示するため（到達不可 / 認証 / ホスト鍵）。

use std::fmt;

#[derive(Debug)]
pub enum SshError {
    /// ネットワーク到達不可・TCP 接続失敗
    Connect(std::io::Error),
    /// ホスト鍵が known_hosts と不一致（中間者の疑い）
    HostKeyChanged,
    /// ユーザが TOFU を拒否
    HostKeyRejected,
    /// 認証失敗（資格情報が拒否された）
    AuthFailed,
    /// ユーザが認証ダイアログをキャンセル
    AuthCancelled,
    /// SSH エージェントに接続できない/鍵がない
    NoAgent,
    KeyNotFound,
    KeyParse,
    PpkConvert(String),
    /// ポートフォワード確立失敗（required=true 時に接続失敗へ昇格）
    Forward(String),
    Sftp(String),
    Russh(russh::Error),
    Other(String),
}

impl fmt::Display for SshError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SshError::Connect(e) => write!(f, "接続に失敗しました（到達不可）: {e}"),
            SshError::HostKeyChanged => write!(f, "ホスト鍵が変更されています（接続を中断）"),
            SshError::HostKeyRejected => write!(f, "ホスト鍵が拒否されました"),
            SshError::AuthFailed => write!(f, "認証に失敗しました"),
            SshError::AuthCancelled => write!(f, "認証がキャンセルされました"),
            SshError::NoAgent => write!(f, "SSH エージェントに接続できません（鍵未登録）"),
            SshError::KeyNotFound => write!(f, "鍵ファイルが見つかりません"),
            SshError::KeyParse => write!(f, "鍵の読み込みに失敗しました"),
            SshError::PpkConvert(m) => write!(f, ".ppk 変換に失敗しました: {m}"),
            SshError::Forward(m) => write!(f, "ポートフォワード確立に失敗しました: {m}"),
            SshError::Sftp(m) => write!(f, "SFTP エラー: {m}"),
            SshError::Russh(e) => write!(f, "SSH エラー: {e}"),
            SshError::Other(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for SshError {}

impl From<russh::Error> for SshError {
    fn from(e: russh::Error) -> Self {
        SshError::Russh(e)
    }
}
