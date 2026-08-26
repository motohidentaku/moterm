//! 認証。agent / publickey(OpenSSH/PEM/.ppk 自動変換) / password。
//! keyboard-interactive は非対応（仕様どおり）。

use crate::error::SshError;
use mot_core::model::AuthMethod;
use russh::client::{AuthResult, Handle};
use russh::keys::key::PrivateKeyWithHashAlg;
use russh::keys::{decode_secret_key, PrivateKey};
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
        // 0.59 で request_identities() は AgentIdentity を返す。署名対象の公開鍵を取り出す。
        let key = id.public_key().into_owned();
        let hash = if key.algorithm().is_rsa() { best } else { None };
        match handle
            .authenticate_publickey_with(user, key, hash, &mut agent)
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
///
/// パスフレーズを訊くのは**暗号化されていると判定できたときだけ**。以前は読み込みに
/// 失敗したら理由を問わず要求していたため、パスが違う・読めない・壊れているといった
/// ケースでも「パスフレーズ無しの鍵なのにパスワードを訊かれる」状態になっていた。
fn load_key_with_conversion(
    path: &std::path::Path,
    cb: &mut AuthCallbacks,
) -> Result<PrivateKey, SshError> {
    let text = read_key_file(path)?;
    let is_ppk = path
        .extension()
        .map(|e| e.eq_ignore_ascii_case("ppk"))
        .unwrap_or(false);
    if is_ppk {
        let openssh =
            mot_core::ppk::convert_ppk(&text).map_err(|e| SshError::PpkConvert(e.to_string()))?;
        return PrivateKey::from_openssh(&openssh).map_err(|_| SshError::KeyParse);
    }
    match decode_secret_key(&text, None) {
        Ok(k) => Ok(k),
        Err(e) if is_encrypted(&e, &text) => {
            let phrase = (cb.passphrase)().ok_or(SshError::AuthCancelled)?;
            decode_secret_key(&text, Some(&phrase)).map_err(|_| SshError::KeyParse)
        }
        Err(e) => {
            log::warn!("鍵を解析できません: {} ({e})", path.display());
            Err(SshError::KeyParse)
        }
    }
}

/// 鍵ファイルを文字列として読む。失敗理由をそのまま UI に出せる形に落とす
/// （ここを握り潰すと「鍵が読めない」が「パスフレーズを訊かれる」に化ける）。
fn read_key_file(path: &std::path::Path) -> Result<String, SshError> {
    std::fs::read_to_string(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => SshError::KeyNotFound,
        _ => SshError::Other(format!("鍵ファイルを読めません: {} ({e})", path.display())),
    })
}

/// パスフレーズを要求すべきエラーか。
///
/// OpenSSH 形式と PKCS#5(PEM) は russh が `KeyIsEncrypted` を返すが、
/// PKCS#8 の暗号化鍵（`BEGIN ENCRYPTED PRIVATE KEY`）はパスワード無しだと
/// ただの ASN.1 パースエラーになるため、本文のヘッダからも判定する。
fn is_encrypted(err: &russh::keys::Error, text: &str) -> bool {
    matches!(err, russh::keys::Error::KeyIsEncrypted)
        || text.contains("-----BEGIN ENCRYPTED PRIVATE KEY-----")
        || text.contains("Proc-Type: 4,ENCRYPTED")
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::keys::ssh_key::rand_core::OsRng;
    use russh::keys::ssh_key::{Algorithm, LineEnding};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// パスフレーズ要求の回数を数えるコールバック。
    fn counting_cb(answer: Option<&str>) -> (AuthCallbacks, Arc<AtomicUsize>) {
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = n.clone();
        let ans = answer.map(str::to_string);
        let mut cb = AuthCallbacks::none();
        cb.passphrase = Box::new(move || {
            n2.fetch_add(1, Ordering::Relaxed);
            ans.clone()
        });
        (cb, n)
    }

    struct TmpKey(std::path::PathBuf);

    impl TmpKey {
        /// 使い捨ての鍵を生成して書き出す（pass=Some で暗号化）。
        fn new(tag: &str, pass: Option<&str>) -> TmpKey {
            static N: AtomicUsize = AtomicUsize::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("moterm-key-{tag}-{}-{n}", std::process::id()));
            let key = russh::keys::PrivateKey::random(&mut OsRng, Algorithm::Ed25519).unwrap();
            let pem = match pass {
                Some(p) => key
                    .encrypt(&mut OsRng, p)
                    .unwrap()
                    .to_openssh(LineEnding::LF),
                None => key.to_openssh(LineEnding::LF),
            }
            .unwrap();
            std::fs::write(&path, pem.as_bytes()).unwrap();
            TmpKey(path)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TmpKey {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// 本題: パスフレーズ無しの鍵ではプロンプトを一切出さない。
    #[test]
    fn unencrypted_key_never_asks_for_passphrase() {
        let key = TmpKey::new("plain", None);
        let (mut cb, calls) = counting_cb(Some("should-not-be-used"));
        let loaded = load_key_with_conversion(key.path(), &mut cb).expect("load");
        assert_eq!(loaded.algorithm(), Algorithm::Ed25519);
        assert_eq!(
            calls.load(Ordering::Relaxed),
            0,
            "パスフレーズを訊いてはいけない"
        );
    }

    /// 暗号化鍵では従来どおり1回だけ訊いて、答えで復号できる。
    #[test]
    fn encrypted_key_asks_once_and_decrypts() {
        let key = TmpKey::new("enc", Some("secretpw"));
        let (mut cb, calls) = counting_cb(Some("secretpw"));
        load_key_with_conversion(key.path(), &mut cb).expect("load");
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    /// 鍵が無いときはパスフレーズを訊かず、そのまま KeyNotFound を返す。
    #[test]
    fn missing_key_reports_not_found_without_prompt() {
        let path = std::env::temp_dir().join("moterm-key-does-not-exist");
        let _ = std::fs::remove_file(&path);
        let (mut cb, calls) = counting_cb(Some("pw"));
        let err = load_key_with_conversion(&path, &mut cb).unwrap_err();
        assert!(matches!(err, SshError::KeyNotFound), "got {err:?}");
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }

    /// 壊れた鍵もパスフレーズ要求に化けさせず KeyParse にする。
    #[test]
    fn corrupt_key_reports_parse_error_without_prompt() {
        let path = std::env::temp_dir().join(format!("moterm-key-corrupt-{}", std::process::id()));
        std::fs::write(&path, b"-----BEGIN OPENSSH PRIVATE KEY-----\nnot-base64\n").unwrap();
        let (mut cb, calls) = counting_cb(Some("pw"));
        let err = load_key_with_conversion(&path, &mut cb).unwrap_err();
        assert!(matches!(err, SshError::KeyParse), "got {err:?}");
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        let _ = std::fs::remove_file(&path);
    }

    /// PKCS#8 の暗号化鍵は russh が KeyIsEncrypted を返さないので、
    /// ヘッダで判定してパスフレーズを訊けること。
    #[test]
    fn pkcs8_encrypted_header_triggers_prompt() {
        let err = russh::keys::Error::KeyIsCorrupt;
        assert!(is_encrypted(
            &err,
            "-----BEGIN ENCRYPTED PRIVATE KEY-----\nxxx\n"
        ));
        assert!(is_encrypted(
            &err,
            "Proc-Type: 4,ENCRYPTED\nDEK-Info: ...\n"
        ));
        assert!(!is_encrypted(&err, "-----BEGIN PRIVATE KEY-----\nxxx\n"));
    }
}
