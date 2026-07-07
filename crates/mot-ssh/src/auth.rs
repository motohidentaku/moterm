//! 認証。agent / publickey(OpenSSH/PEM/.ppk 自動変換) / password。
//! keyboard-interactive は非対応（仕様どおり）。

use crate::error::SshError;
use mot_core::model::AuthMethod;
use russh::client::{AuthResult, Handle};
use russh::keys::key::PrivateKeyWithHashAlg;
use russh::keys::{load_secret_key, PrivateKey};
use std::sync::Arc;

use crate::handler::ClientHandler;

/// 認証中に必要な対話入力を供給するコールバック群。
pub struct AuthCallbacks {
    /// パスワード認証のパスワード取得（保存済みがあれば Some を先に渡す設計）
    pub password: Box<dyn FnMut() -> Option<String> + Send>,
    /// 鍵パスフレーズ取得
    pub passphrase: Box<dyn FnMut() -> Option<String> + Send>,
}

impl AuthCallbacks {
    pub fn none() -> Self {
        AuthCallbacks {
            password: Box::new(|| None),
            passphrase: Box::new(|| None),
        }
    }
}

/// 認証を実行する。成功時、password 認証で新規入力されたパスワードがあれば返す
/// （呼び出し側が save=true のときボールトへ保存する）。
pub async fn authenticate(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    auth: &AuthMethod,
    cb: &mut AuthCallbacks,
) -> Result<Option<String>, SshError> {
    match auth {
        AuthMethod::Agent => {
            auth_agent(handle, user).await?;
            Ok(None)
        }
        AuthMethod::Publickey { key } => {
            auth_publickey(handle, user, key, cb).await?;
            Ok(None)
        }
        AuthMethod::Password { .. } => {
            let pw = (cb.password)().ok_or(SshError::AuthCancelled)?;
            let res = handle
                .authenticate_password(user, pw.clone())
                .await
                .map_err(SshError::Russh)?;
            if res.success() {
                Ok(Some(pw))
            } else {
                Err(SshError::AuthFailed)
            }
        }
    }
}

async fn auth_agent(handle: &mut Handle<ClientHandler>, user: &str) -> Result<(), SshError> {
    // SSH エージェントへの接続はプラットフォームで異なる。
    //   unix    : $SSH_AUTH_SOCK（connect_env）
    //   windows : OpenSSH エージェントの名前付きパイプ（Pageant は現状未対応）
    // cfg で排他分岐するため、agent の具体型は各プラットフォームで1つに定まる。
    #[cfg(unix)]
    let mut agent = russh::keys::agent::client::AgentClient::connect_env()
        .await
        .map_err(|_| SshError::NoAgent)?;
    #[cfg(windows)]
    let mut agent =
        russh::keys::agent::client::AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent")
            .await
            .map_err(|_| SshError::NoAgent)?;

    let identities = agent
        .request_identities()
        .await
        .map_err(|_| SshError::NoAgent)?;
    if identities.is_empty() {
        return Err(SshError::AuthFailed);
    }
    let best = handle
        .best_supported_rsa_hash()
        .await
        .ok()
        .flatten()
        .flatten();
    for id in identities {
        let hash = if id.algorithm().is_rsa() { best } else { None };
        match handle
            .authenticate_publickey_with(user, id, hash, &mut agent)
            .await
        {
            Ok(AuthResult::Success) => return Ok(()),
            _ => continue,
        }
    }
    Err(SshError::AuthFailed)
}

async fn auth_publickey(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    key_path: &str,
    cb: &mut AuthCallbacks,
) -> Result<(), SshError> {
    let path = mot_core::model::expand_tilde(key_path);
    let key = load_key_with_conversion(&path, cb)?;

    let best = handle
        .best_supported_rsa_hash()
        .await
        .ok()
        .flatten()
        .flatten();
    let hash = if key.algorithm().is_rsa() { best } else { None };
    let key_with = PrivateKeyWithHashAlg::new(Arc::new(key), hash);
    let res = handle
        .authenticate_publickey(user, key_with)
        .await
        .map_err(SshError::Russh)?;
    if res.success() {
        Ok(())
    } else {
        Err(SshError::AuthFailed)
    }
}

/// 鍵ファイルを読み込む。.ppk なら OpenSSH へ変換し、暗号化鍵はパスフレーズを要求する。
fn load_key_with_conversion(
    path: &std::path::Path,
    cb: &mut AuthCallbacks,
) -> Result<PrivateKey, SshError> {
    let is_ppk = path
        .extension()
        .map(|e| e.eq_ignore_ascii_case("ppk"))
        .unwrap_or(false);
    if is_ppk {
        let content = std::fs::read_to_string(path).map_err(|_| SshError::KeyNotFound)?;
        let openssh = mot_core::ppk::convert_ppk(&content)
            .map_err(|e| SshError::PpkConvert(e.to_string()))?;
        return PrivateKey::from_openssh(&openssh).map_err(|_| SshError::KeyParse);
    }
    // まずパスフレーズ無しで試し、失敗したら要求する
    match load_secret_key(path, None) {
        Ok(k) => Ok(k),
        Err(_) => {
            let phrase = (cb.passphrase)().ok_or(SshError::AuthCancelled)?;
            load_secret_key(path, Some(&phrase)).map_err(|_| SshError::KeyParse)
        }
    }
}
