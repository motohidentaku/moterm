//! マスターパスワード式の暗号ボールト（design.md §3.3.1 準拠・OS 非依存／ポータブル）。
//!
//! ファイル形式:
//! `magic b"MOTMOTV1"(8) | version u8=1 | m_cost u32le | t_cost u32le | p_cost u32le |
//!  salt(16) | nonce(24) | XChaCha20-Poly1305 暗号文`
//!
//! 鍵導出は Argon2id（パラメータ・salt はヘッダに自己記述）。平文は
//! `{"user@host:port": "password", ...}` の JSON。保存は tmp+アトミック rename。
//! 導出鍵はメモリ上で zeroize され、プロセス終了で破棄される。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;
use zeroize::Zeroizing;

/// ファイル先頭のマジック。
/// 注: アプリ名を moterm に改名後も、この 8 バイトは既存ボールト（moterm-secrets.enc）
/// の互換性を保つため変更しない（値を変えると旧ファイルが復号できなくなる）。
pub const MAGIC: &[u8; 8] = b"MOTMOTV1";
const FORMAT_VERSION: u8 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const HEADER_LEN: usize = 8 + 1 + 4 * 3 + SALT_LEN + NONCE_LEN;

/// ボールト操作のエラー
#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    /// マスターパスワード不一致または暗号文の改ざん（AEAD 認証失敗）
    #[error("マスターパスワードが違います（またはボールトが改ざんされています）")]
    WrongMaster,
    /// ヘッダ形式の不正
    #[error("ボールトファイルの形式が不正です: {0}")]
    Format(String),
    /// 入出力エラー
    #[error("ボールトの入出力に失敗しました: {0}")]
    Io(#[from] std::io::Error),
    /// 鍵導出・暗号化の失敗（パラメータ不正など）
    #[error("暗号処理に失敗しました: {0}")]
    Crypto(String),
}

/// Argon2id の KDF パラメータ（ヘッダに保存され、読み込み時はヘッダの値を使う）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfParams {
    /// メモリコスト（KiB）
    pub m_cost: u32,
    /// 反復回数
    pub t_cost: u32,
    /// 並列度
    pub p_cost: u32,
}

impl Default for KdfParams {
    fn default() -> Self {
        // 64 MiB / 3 iters / 1 lane
        KdfParams {
            m_cost: 65536,
            t_cost: 3,
            p_cost: 1,
        }
    }
}

/// マスターパスワードで暗号化された機密ストア
pub struct Vault {
    path: PathBuf,
    params: KdfParams,
    salt: [u8; SALT_LEN],
    key: Zeroizing<[u8; 32]>,
    entries: BTreeMap<String, String>,
}

impl std::fmt::Debug for Vault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 鍵・平文は出さない
        f.debug_struct("Vault")
            .field("path", &self.path)
            .field("params", &self.params)
            .field("entries", &self.entries.len())
            .finish_non_exhaustive()
    }
}

impl Vault {
    /// ボールトを開く。ファイルが無ければ master を新規マスターパスワードとして
    /// 空ボールトを作成・保存する。復号失敗（誤マスター/改ざん）は `WrongMaster`。
    pub fn open_or_create(path: &Path, master: &str) -> Result<Vault, VaultError> {
        Self::open_or_create_with(path, master, KdfParams::default())
    }

    /// KDF パラメータを指定して開く/作成する（params は新規作成時のみ使用。
    /// 既存ファイルはヘッダのパラメータで復号するため互換）。
    pub fn open_or_create_with(
        path: &Path,
        master: &str,
        params: KdfParams,
    ) -> Result<Vault, VaultError> {
        match std::fs::read(path) {
            Ok(data) => Self::open_bytes(path, master, &data),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut salt = [0u8; SALT_LEN];
                rand::rngs::OsRng.fill_bytes(&mut salt);
                let key = derive_key(master, &salt, params)?;
                let mut vault = Vault {
                    path: path.to_path_buf(),
                    params,
                    salt,
                    key,
                    entries: BTreeMap::new(),
                };
                vault.save()?;
                Ok(vault)
            }
            Err(e) => Err(e.into()),
        }
    }

    fn open_bytes(path: &Path, master: &str, data: &[u8]) -> Result<Vault, VaultError> {
        if data.len() < HEADER_LEN {
            return Err(VaultError::Format("ファイルが短すぎます".into()));
        }
        if &data[..8] != MAGIC {
            return Err(VaultError::Format("マジックが一致しません".into()));
        }
        if data[8] != FORMAT_VERSION {
            return Err(VaultError::Format(format!("未対応バージョン: {}", data[8])));
        }
        let u32le = |off: usize| u32::from_le_bytes(data[off..off + 4].try_into().unwrap());
        let params = KdfParams {
            m_cost: u32le(9),
            t_cost: u32le(13),
            p_cost: u32le(17),
        };
        let mut salt = [0u8; SALT_LEN];
        salt.copy_from_slice(&data[21..21 + SALT_LEN]);
        let nonce_off = 21 + SALT_LEN;
        let nonce = XNonce::from_slice(&data[nonce_off..nonce_off + NONCE_LEN]);
        let ciphertext = &data[HEADER_LEN..];

        let key = derive_key(master, &salt, params)?;
        let cipher = XChaCha20Poly1305::new((&*key).into());
        // AEAD 認証失敗 = 誤マスター or 改ざん
        let plaintext = Zeroizing::new(
            cipher
                .decrypt(nonce, ciphertext)
                .map_err(|_| VaultError::WrongMaster)?,
        );
        let entries: BTreeMap<String, String> = serde_json::from_slice(&plaintext)
            .map_err(|e| VaultError::Format(format!("復号結果が JSON ではありません: {e}")))?;
        Ok(Vault {
            path: path.to_path_buf(),
            params,
            salt,
            key,
            entries,
        })
    }

    /// 保存済みパスワードを取得する（キーは "user@host:port"）
    pub fn get(&self, key: &str) -> Option<String> {
        self.entries.get(key).cloned()
    }

    /// パスワードを保存し、即座に暗号化ファイルへ書き出す
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), VaultError> {
        self.entries.insert(key.to_string(), value.to_string());
        self.save()
    }

    /// エントリを削除し保存する。削除した値を返す（無ければ None、保存もしない）
    pub fn remove(&mut self, key: &str) -> Result<Option<String>, VaultError> {
        match self.entries.remove(key) {
            Some(old) => {
                self.save()?;
                Ok(Some(old))
            }
            None => Ok(None),
        }
    }

    /// エントリ数
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// エントリが空か
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// ボールトファイルのパス
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 全体を暗号化して tmp+rename でアトミック保存する（nonce は毎回新規生成）
    fn save(&mut self) -> Result<(), VaultError> {
        let plaintext = Zeroizing::new(
            serde_json::to_vec(&self.entries)
                .map_err(|e| VaultError::Crypto(format!("JSON 化に失敗: {e}")))?,
        );
        let mut nonce = [0u8; NONCE_LEN];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let cipher = XChaCha20Poly1305::new((&*self.key).into());
        let ciphertext = cipher
            .encrypt(XNonce::from_slice(&nonce), plaintext.as_slice())
            .map_err(|e| VaultError::Crypto(format!("暗号化に失敗: {e}")))?;

        let mut buf = Vec::with_capacity(HEADER_LEN + ciphertext.len());
        buf.extend_from_slice(MAGIC);
        buf.push(FORMAT_VERSION);
        buf.extend_from_slice(&self.params.m_cost.to_le_bytes());
        buf.extend_from_slice(&self.params.t_cost.to_le_bytes());
        buf.extend_from_slice(&self.params.p_cost.to_le_bytes());
        buf.extend_from_slice(&self.salt);
        buf.extend_from_slice(&nonce);
        buf.extend_from_slice(&ciphertext);

        if let Some(dir) = self.path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)?;
            }
        }
        let tmp = self
            .path
            .with_extension(format!("tmp{}", std::process::id()));
        std::fs::write(&tmp, &buf)?;
        std::fs::rename(&tmp, &self.path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })?;
        Ok(())
    }
}

/// Argon2id で 256bit 鍵を導出する
fn derive_key(
    master: &str,
    salt: &[u8; SALT_LEN],
    params: KdfParams,
) -> Result<Zeroizing<[u8; 32]>, VaultError> {
    let a2_params = argon2::Params::new(params.m_cost, params.t_cost, params.p_cost, Some(32))
        .map_err(|e| VaultError::Crypto(format!("KDF パラメータが不正: {e}")))?;
    let argon = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        a2_params,
    );
    let mut key = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(master.as_bytes(), salt, key.as_mut())
        .map_err(|e| VaultError::Crypto(format!("鍵導出に失敗: {e}")))?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト用の低コスト KDF（パラメータはヘッダから読むので互換）
    const FAST: KdfParams = KdfParams {
        m_cost: 8,
        t_cost: 1,
        p_cost: 1,
    };

    fn vault_path(tag: &str) -> PathBuf {
        crate::test_util::temp_dir(&format!("vault_{tag}")).join("secrets.enc")
    }

    #[test]
    fn roundtrip_and_persistence() {
        let path = vault_path("roundtrip");
        {
            let mut v = Vault::open_or_create_with(&path, "master", FAST).unwrap();
            assert!(v.is_empty());
            v.set("deploy@10.0.0.10:22", "s3cret").unwrap();
            v.set("admin@10.0.0.20:22", "パスワード").unwrap();
            assert_eq!(v.get("deploy@10.0.0.10:22").as_deref(), Some("s3cret"));
        }
        // 再オープン（ヘッダのパラメータで復号）
        let mut v = Vault::open_or_create_with(&path, "master", KdfParams::default()).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v.get("admin@10.0.0.20:22").as_deref(), Some("パスワード"));
        // remove も永続化
        assert_eq!(
            v.remove("admin@10.0.0.20:22").unwrap().as_deref(),
            Some("パスワード")
        );
        assert_eq!(v.remove("nope").unwrap(), None);
        let v = Vault::open_or_create_with(&path, "master", FAST).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v.get("admin@10.0.0.20:22"), None);
    }

    #[test]
    fn wrong_master_is_rejected() {
        let path = vault_path("wrong_master");
        let mut v = Vault::open_or_create_with(&path, "correct", FAST).unwrap();
        v.set("k", "v").unwrap();
        let err = Vault::open_or_create_with(&path, "incorrect", FAST).unwrap_err();
        assert!(matches!(err, VaultError::WrongMaster), "got: {err:?}");
    }

    #[test]
    fn tamper_is_detected() {
        let path = vault_path("tamper");
        let mut v = Vault::open_or_create_with(&path, "master", FAST).unwrap();
        v.set("k", "v").unwrap();
        // 暗号文の1バイトを反転
        let mut data = std::fs::read(&path).unwrap();
        let last = data.len() - 1;
        data[last] ^= 0x01;
        std::fs::write(&path, &data).unwrap();
        let err = Vault::open_or_create_with(&path, "master", FAST).unwrap_err();
        assert!(matches!(err, VaultError::WrongMaster), "got: {err:?}");
    }

    #[test]
    fn bad_magic_and_truncated_are_format_errors() {
        let path = vault_path("format");
        std::fs::write(
            &path,
            b"NOTVAULT........................................................",
        )
        .unwrap();
        assert!(matches!(
            Vault::open_or_create_with(&path, "m", FAST).unwrap_err(),
            VaultError::Format(_)
        ));
        std::fs::write(&path, b"short").unwrap();
        assert!(matches!(
            Vault::open_or_create_with(&path, "m", FAST).unwrap_err(),
            VaultError::Format(_)
        ));
    }

    #[test]
    fn header_layout_matches_spec() {
        let path = vault_path("header");
        Vault::open_or_create_with(&path, "m", FAST).unwrap();
        let data = std::fs::read(&path).unwrap();
        assert_eq!(&data[..8], MAGIC);
        assert_eq!(data[8], 1); // version
        assert_eq!(u32::from_le_bytes(data[9..13].try_into().unwrap()), 8); // m_cost
        assert_eq!(u32::from_le_bytes(data[13..17].try_into().unwrap()), 1); // t_cost
        assert_eq!(u32::from_le_bytes(data[17..21].try_into().unwrap()), 1); // p_cost
                                                                             // salt(16) + nonce(24) + 暗号文（空 JSON "{}" 2B + タグ 16B）
        assert_eq!(data.len(), HEADER_LEN + 2 + 16);
    }
}
