//! PuTTY .ppk（v2/v3、Encryption: none のみ）→ OpenSSH 秘密鍵テキストへの変換。
//!
//! 対応鍵種別: ssh-ed25519 / ssh-rsa。暗号化（パスフレーズ付き）ppk は未対応で、
//! `PpkError::Encrypted`（puttygen での変換を促すメッセージ）を返す。

use base64::Engine as _;
use ssh_key::private::{
    Ed25519Keypair, Ed25519PrivateKey, KeypairData, PrivateKey, RsaKeypair, RsaPrivateKey,
};
use ssh_key::public::{Ed25519PublicKey, RsaPublicKey};
use ssh_key::{LineEnding, Mpint};

/// .ppk 変換のエラー
#[derive(Debug, thiserror::Error)]
pub enum PpkError {
    /// パスフレーズ付き ppk（Encryption: none 以外）
    #[error(
        "暗号化された .ppk は未対応です。puttygen で OpenSSH 形式に変換してください \
         (puttygen key.ppk -O private-openssh)"
    )]
    Encrypted,
    /// PuTTY-User-Key-File-2/3 以外
    #[error("未対応の .ppk バージョンです: {0}")]
    UnsupportedVersion(u32),
    /// ssh-ed25519 / ssh-rsa 以外
    #[error("未対応の鍵種別です: {0}")]
    UnsupportedAlgorithm(String),
    /// ppk テキスト・blob の形式不正
    #[error(".ppk の形式が不正です: {0}")]
    Malformed(String),
    /// OpenSSH 形式への組み立て失敗
    #[error("OpenSSH 形式への変換に失敗しました: {0}")]
    Convert(String),
}

/// .ppk ファイルの内容を OpenSSH 秘密鍵テキスト（PEM 風、LF 改行）へ変換する。
pub fn convert_ppk(content: &str) -> Result<String, PpkError> {
    let ppk = parse_ppk(content)?;
    if !ppk.encryption.eq_ignore_ascii_case("none") {
        return Err(PpkError::Encrypted);
    }

    let key_data = match ppk.algorithm.as_str() {
        "ssh-ed25519" => build_ed25519(&ppk)?,
        "ssh-rsa" => build_rsa(&ppk)?,
        other => return Err(PpkError::UnsupportedAlgorithm(other.to_string())),
    };
    let key =
        PrivateKey::new(key_data, &ppk.comment).map_err(|e| PpkError::Convert(e.to_string()))?;
    let text = key
        .to_openssh(LineEnding::LF)
        .map_err(|e| PpkError::Convert(e.to_string()))?;
    Ok(text.to_string())
}

struct Ppk {
    algorithm: String,
    encryption: String,
    comment: String,
    public_blob: Vec<u8>,
    private_blob: Vec<u8>,
}

fn parse_ppk(content: &str) -> Result<Ppk, PpkError> {
    let mut lines = content.lines().map(str::trim_end);
    let first = lines
        .next()
        .ok_or_else(|| PpkError::Malformed("空のファイルです".into()))?;
    let (header, algorithm) = first
        .split_once(':')
        .ok_or_else(|| PpkError::Malformed("1行目がヘッダではありません".into()))?;
    let version: u32 = header
        .strip_prefix("PuTTY-User-Key-File-")
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| PpkError::Malformed("PuTTY-User-Key-File ヘッダがありません".into()))?;
    if version != 2 && version != 3 {
        return Err(PpkError::UnsupportedVersion(version));
    }

    let mut encryption = String::new();
    let mut comment = String::new();
    let mut public_blob = None;
    let mut private_blob = None;
    while let Some(line) = lines.next() {
        let Some((name, value)) = line.split_once(':') else {
            continue; // 想定外の行は無視（MAC 検証は行わない）
        };
        let value = value.trim();
        match name {
            "Encryption" => encryption = value.to_string(),
            "Comment" => comment = value.to_string(),
            "Public-Lines" | "Private-Lines" => {
                let n: usize = value
                    .parse()
                    .map_err(|_| PpkError::Malformed(format!("{name} が数値ではありません")))?;
                let mut b64 = String::new();
                for _ in 0..n {
                    b64.push_str(lines.next().ok_or_else(|| {
                        PpkError::Malformed(format!("{name} の行数が足りません"))
                    })?);
                }
                let blob = base64::engine::general_purpose::STANDARD
                    .decode(b64.trim())
                    .map_err(|e| PpkError::Malformed(format!("{name} の base64 が不正: {e}")))?;
                if name == "Public-Lines" {
                    public_blob = Some(blob);
                } else {
                    private_blob = Some(blob);
                }
            }
            _ => {} // Private-MAC / Key-Derivation / Argon2-* 等は読み飛ばす
        }
    }
    Ok(Ppk {
        algorithm: algorithm.trim().to_string(),
        encryption,
        comment,
        public_blob: public_blob
            .ok_or_else(|| PpkError::Malformed("Public-Lines がありません".into()))?,
        private_blob: private_blob
            .ok_or_else(|| PpkError::Malformed("Private-Lines がありません".into()))?,
    })
}

fn build_ed25519(ppk: &Ppk) -> Result<KeypairData, PpkError> {
    // public blob = string "ssh-ed25519" + string pub(32B)
    let mut r = WireReader::new(&ppk.public_blob);
    let alg = r.string_utf8()?;
    if alg != "ssh-ed25519" {
        return Err(PpkError::Malformed(format!(
            "公開鍵 blob の種別不一致: {alg}"
        )));
    }
    let public: [u8; 32] = r
        .string()?
        .try_into()
        .map_err(|_| PpkError::Malformed("ed25519 公開鍵が 32 バイトではありません".into()))?;
    // private blob = string priv(32B)
    let mut r = WireReader::new(&ppk.private_blob);
    let private: [u8; 32] = r
        .string()?
        .try_into()
        .map_err(|_| PpkError::Malformed("ed25519 秘密鍵が 32 バイトではありません".into()))?;
    Ok(KeypairData::Ed25519(Ed25519Keypair {
        public: Ed25519PublicKey(public),
        private: Ed25519PrivateKey::from_bytes(&private),
    }))
}

fn build_rsa(ppk: &Ppk) -> Result<KeypairData, PpkError> {
    let mpint = |bytes: &[u8]| {
        Mpint::from_bytes(bytes).map_err(|e| PpkError::Malformed(format!("mpint が不正: {e}")))
    };
    // public blob = string "ssh-rsa" + mpint e + mpint n
    let mut r = WireReader::new(&ppk.public_blob);
    let alg = r.string_utf8()?;
    if alg != "ssh-rsa" {
        return Err(PpkError::Malformed(format!(
            "公開鍵 blob の種別不一致: {alg}"
        )));
    }
    let e = mpint(r.string()?)?;
    let n = mpint(r.string()?)?;
    // private blob = mpint d + mpint p + mpint q + mpint iqmp
    let mut r = WireReader::new(&ppk.private_blob);
    let d = mpint(r.string()?)?;
    let p = mpint(r.string()?)?;
    let q = mpint(r.string()?)?;
    let iqmp = mpint(r.string()?)?;
    Ok(KeypairData::Rsa(RsaKeypair {
        public: RsaPublicKey { e, n },
        private: RsaPrivateKey { d, iqmp, p, q },
    }))
}

/// SSH wire 形式（u32be 長さ + データ）の小さなリーダ
struct WireReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> WireReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        WireReader { data, pos: 0 }
    }

    fn string(&mut self) -> Result<&'a [u8], PpkError> {
        let end_len = self.pos + 4;
        if end_len > self.data.len() {
            return Err(PpkError::Malformed("blob が途中で終わっています".into()));
        }
        let len = u32::from_be_bytes(self.data[self.pos..end_len].try_into().unwrap()) as usize;
        let end = end_len + len;
        if end > self.data.len() {
            return Err(PpkError::Malformed(
                "blob の長さフィールドが範囲外です".into(),
            ));
        }
        let out = &self.data[end_len..end];
        self.pos = end;
        Ok(out)
    }

    fn string_utf8(&mut self) -> Result<&'a str, PpkError> {
        std::str::from_utf8(self.string()?)
            .map_err(|_| PpkError::Malformed("blob 内の文字列が UTF-8 ではありません".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ssh-keygen で生成した既知の ed25519 鍵（テストベクタ）
    const ED25519_OPENSSH: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACBoycJ3M/NNDMOZBfTjxMu+d7ViLFxeFk9YxD7Gh5JLJQAAAJhq7jkhau45
IQAAAAtzc2gtZWQyNTUxOQAAACBoycJ3M/NNDMOZBfTjxMu+d7ViLFxeFk9YxD7Gh5JLJQ
AAAECEGVrb7KnzkpvoyODW5A4quRCJ9xF3RFoIcbNpLXK88GjJwncz800Mw5kF9OPEy753
tWIsXF4WT1jEPsaHkkslAAAAEHBway10ZXN0LWVkMjU1MTkBAgMEBQ==
-----END OPENSSH PRIVATE KEY-----
";

    /// ssh-keygen で生成した既知の RSA 2048bit 鍵（テストベクタ）
    const RSA_OPENSSH: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAABFwAAAAdzc2gtcn
NhAAAAAwEAAQAAAQEAyCNOERLRCWFIGsBRHi2bDCCDxgroGjfI8ba0cNRrTjij7cbNuGXG
4QjwvQOeO2uzOGzBlY03f3wPnTypk7xc2eDUrJXqPMkTrzQBEAJsU+jGKOt7FGpHMbTDs1
9bkBJwBJ/dCerKjBlSk5Tv9iShiJkbhxlO47yilaF56mxycUywwqUKDk34rGHv9prsa8BK
NNOzA2GU6rkAtVTTN4kpjqhrVShxFF2vLcGMbyE7yA0RBxtA9aeFtVcplHUSVmiwB18m96
m40pWquWnVLRQdCOvFHIrxquNJtFmqMi9xoF/77dIbX1rchw0MjW6d+fobiBE5K7zJ+IKS
Mv7B2YiFuwAAA8jNxwN/zccDfwAAAAdzc2gtcnNhAAABAQDII04REtEJYUgawFEeLZsMII
PGCugaN8jxtrRw1GtOOKPtxs24ZcbhCPC9A547a7M4bMGVjTd/fA+dPKmTvFzZ4NSsleo8
yROvNAEQAmxT6MYo63sUakcxtMOzX1uQEnAEn90J6sqMGVKTlO/2JKGImRuHGU7jvKKVoX
nqbHJxTLDCpQoOTfisYe/2muxrwEo007MDYZTquQC1VNM3iSmOqGtVKHEUXa8twYxvITvI
DREHG0D1p4W1VymUdRJWaLAHXyb3qbjSlaq5adUtFB0I68UcivGq40m0WaoyL3GgX/vt0h
tfWtyHDQyNbp35+huIETkrvMn4gpIy/sHZiIW7AAAAAwEAAQAAAQBBLK4ZhUUphtKSU5qW
90cMlfITpi2bjBsWC+eK7sHbATrxDdKkgBBZ7C1pgCohM5tzfoc0Cn7ONzpmfADFKYwbL8
pSQae8D8cnQQovinp4gM83OCgmp81zdGhem2kX68kq2FyFBD7djMmFYfUa9Sbdcu6x+h3k
r+NKUwF+w74pArr6GlkbCXLBI2V/Ia/EaqoWkMz6Z430XL+M5W7SxAILl9zkfri0D18hYZ
qGtA3r/ltvtVkFVWGjN12IG4wUC+5sH3FnW6I3tiBCB6IlqPA65uCfyzEx16LiHVBnlN3s
GrF5khHHHF9UIn1hslxJ2Y7qmPkeG7F+uoXeSW/lBDGhAAAAgHApBS15mnfDzmVhqbPOLS
A9pK4XdSEz8yZGttTBxxir+nkleB7yMnII7KoNwIUNrDutGokboqwUhxvQUBf6KGY9lV2+
BL9soy670L2YGL72CBofUufpCwpsWAG0QMiODfuWvMNgdo2x4lOEGr9aCaK6htFNoaFX9K
wdXvhDosn0AAAAgQDotlCI8SxTPPiqYSJ97ExaRwbCT+D6KH3YdeviiiLwNF4u+MDTMdrQ
qzubQ6xyjqQO1JAHXGonEnN7qck+xS8wo9/jj27f4TWbAG+XLaJZKajGJKVQc2740lUOpq
tKaa1lMS0zoPXE3/EzZKVfqMN1/eJyd4D9A9fUdR9H6/Pl2QAAAIEA3Cp+cuUbh/37xc4x
dSp5ZqRU7CcOmTluIzbJHtULlEGtVW0ttsDaG4IO/99BlVUwA7tmGnN5Z9dkWqcgLUuzYu
+BgAVahQhm2dzR/AJVjczMxxZFVslUQZqlBjHlujz/6hmQWnmWy4BqKIRP4H0kNoLRrGXS
3zsytfWJ5p9Y57MAAAAMcHBrLXRlc3QtcnNhAQIDBAUGBw==
-----END OPENSSH PRIVATE KEY-----
";

    // ---- テスト側の小さな wire ライタ / ppk エンコーダ ----

    fn put_string(buf: &mut Vec<u8>, data: &[u8]) {
        buf.extend_from_slice(&(data.len() as u32).to_be_bytes());
        buf.extend_from_slice(data);
    }

    fn encode_ppk(
        version: u32,
        algorithm: &str,
        encryption: &str,
        comment: &str,
        public_blob: &[u8],
        private_blob: &[u8],
    ) -> String {
        let b64 = |blob: &[u8]| {
            let s = base64::engine::general_purpose::STANDARD.encode(blob);
            let lines: Vec<&str> = s
                .as_bytes()
                .chunks(64)
                .map(|c| std::str::from_utf8(c).unwrap())
                .collect();
            (lines.len(), lines.join("\n"))
        };
        let (pub_n, pub_lines) = b64(public_blob);
        let (priv_n, priv_lines) = b64(private_blob);
        format!(
            "PuTTY-User-Key-File-{version}: {algorithm}\n\
             Encryption: {encryption}\n\
             Comment: {comment}\n\
             Public-Lines: {pub_n}\n{pub_lines}\n\
             Private-Lines: {priv_n}\n{priv_lines}\n\
             Private-MAC: 0000000000000000000000000000000000000000\n"
        )
    }

    /// 既知の OpenSSH 鍵から ppk を組み立てる（ed25519）
    fn ed25519_ppk(version: u32) -> (PrivateKey, String) {
        let key = PrivateKey::from_openssh(ED25519_OPENSSH).unwrap();
        let kp = key.key_data().ed25519().unwrap().clone();
        let mut public_blob = Vec::new();
        put_string(&mut public_blob, b"ssh-ed25519");
        put_string(&mut public_blob, kp.public.as_ref());
        let mut private_blob = Vec::new();
        put_string(&mut private_blob, kp.private.as_ref());
        let ppk = encode_ppk(
            version,
            "ssh-ed25519",
            "none",
            key.comment(),
            &public_blob,
            &private_blob,
        );
        (key, ppk)
    }

    /// 既知の OpenSSH 鍵から ppk を組み立てる（rsa）
    fn rsa_ppk() -> (PrivateKey, String) {
        let key = PrivateKey::from_openssh(RSA_OPENSSH).unwrap();
        let kp = key.key_data().rsa().unwrap().clone();
        let mut public_blob = Vec::new();
        put_string(&mut public_blob, b"ssh-rsa");
        put_string(&mut public_blob, kp.public.e.as_bytes());
        put_string(&mut public_blob, kp.public.n.as_bytes());
        let mut private_blob = Vec::new();
        put_string(&mut private_blob, kp.private.d.as_bytes());
        put_string(&mut private_blob, kp.private.p.as_bytes());
        put_string(&mut private_blob, kp.private.q.as_bytes());
        put_string(&mut private_blob, kp.private.iqmp.as_bytes());
        let ppk = encode_ppk(
            2,
            "ssh-rsa",
            "none",
            key.comment(),
            &public_blob,
            &private_blob,
        );
        (key, ppk)
    }

    #[test]
    fn ed25519_v2_roundtrip() {
        let (expected, ppk) = ed25519_ppk(2);
        let openssh = convert_ppk(&ppk).unwrap();
        let converted = PrivateKey::from_openssh(&openssh).unwrap();
        assert_eq!(converted.key_data(), expected.key_data());
        assert_eq!(converted.comment(), expected.comment());
    }

    #[test]
    fn ed25519_v3_roundtrip() {
        let (expected, ppk) = ed25519_ppk(3);
        let openssh = convert_ppk(&ppk).unwrap();
        let converted = PrivateKey::from_openssh(&openssh).unwrap();
        assert_eq!(converted.key_data(), expected.key_data());
    }

    #[test]
    fn ed25519_crlf_lines_are_accepted() {
        let (expected, ppk) = ed25519_ppk(2);
        let crlf = ppk.replace('\n', "\r\n");
        let openssh = convert_ppk(&crlf).unwrap();
        let converted = PrivateKey::from_openssh(&openssh).unwrap();
        assert_eq!(converted.key_data(), expected.key_data());
    }

    #[test]
    fn rsa_v2_roundtrip() {
        let (expected, ppk) = rsa_ppk();
        let openssh = convert_ppk(&ppk).unwrap();
        let converted = PrivateKey::from_openssh(&openssh).unwrap();
        assert_eq!(converted.key_data(), expected.key_data());
        assert_eq!(converted.comment(), expected.comment());
    }

    #[test]
    fn encrypted_ppk_is_rejected_with_guidance() {
        let (_, ppk) = ed25519_ppk(2);
        let encrypted = ppk.replace("Encryption: none", "Encryption: aes256-cbc");
        let err = convert_ppk(&encrypted).unwrap_err();
        assert!(matches!(err, PpkError::Encrypted));
        assert!(err.to_string().contains("puttygen"));
    }

    #[test]
    fn unsupported_version_and_algorithm() {
        let (_, ppk) = ed25519_ppk(2);
        let v1 = ppk.replace("PuTTY-User-Key-File-2", "PuTTY-User-Key-File-1");
        assert!(matches!(
            convert_ppk(&v1).unwrap_err(),
            PpkError::UnsupportedVersion(1)
        ));
        let ecdsa = ppk.replace(
            "PuTTY-User-Key-File-2: ssh-ed25519",
            "PuTTY-User-Key-File-2: ecdsa-sha2-nistp256",
        );
        assert!(matches!(
            convert_ppk(&ecdsa).unwrap_err(),
            PpkError::UnsupportedAlgorithm(_)
        ));
    }

    #[test]
    fn malformed_inputs_are_errors() {
        assert!(matches!(
            convert_ppk("").unwrap_err(),
            PpkError::Malformed(_)
        ));
        assert!(matches!(
            convert_ppk("garbage\n").unwrap_err(),
            PpkError::Malformed(_)
        ));
        // Public-Lines の行数不足
        let truncated =
            "PuTTY-User-Key-File-2: ssh-ed25519\nEncryption: none\nComment: c\nPublic-Lines: 5\nAAAA\n";
        assert!(matches!(
            convert_ppk(truncated).unwrap_err(),
            PpkError::Malformed(_)
        ));
    }
}
