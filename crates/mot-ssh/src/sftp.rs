//! SFTP。サブシステムチャネル上で russh-sftp を駆動し、2ペインFM/ D&D 転送を支える。

use crate::error::SshError;
use russh::client::{Handle, Msg};
use russh::Channel;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// 転送のチャンクサイズ。進捗コールバックの粒度も兼ねる。
const CHUNK: usize = 64 * 1024;

/// 再帰列挙で辿るエントリ数の上限。壊れたツリー/巨大ツリーで無限に膨らむのを防ぐ。
pub const MAX_WALK_ENTRIES: usize = 100_000;

fn sftp_err<E: std::fmt::Display>(e: E) -> SshError {
    SshError::Sftp(e.to_string())
}

/// FM の一覧表示に使うエントリ。
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    /// 更新時刻（UNIX 秒）。取得できなければ 0。
    pub mtime: u64,
}

/// 再帰列挙の結果1件。パスは走査の基準ディレクトリからの相対（区切りは常に "/"）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkEntry {
    pub rel: String,
    pub is_dir: bool,
}

/// リモートパスの連結。`base` 末尾の "/" と `name` の重複を避ける。
fn rjoin(base: &str, name: &str) -> String {
    if base == "/" {
        format!("/{name}")
    } else {
        format!("{}/{}", base.trim_end_matches('/'), name)
    }
}

pub struct Sftp {
    session: SftpSession,
}

impl Sftp {
    /// セッション上に SFTP サブシステムを開く。
    pub async fn open(handle: &Handle<ClientHandlerAlias>) -> Result<Sftp, SshError> {
        let channel: Channel<Msg> = handle
            .channel_open_session()
            .await
            .map_err(SshError::Russh)?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(SshError::Russh)?;
        let session = SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| SshError::Sftp(e.to_string()))?;
        Ok(Sftp { session })
    }

    pub async fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>, SshError> {
        let rd = self
            .session
            .read_dir(path)
            .await
            .map_err(|e| SshError::Sftp(e.to_string()))?;
        let mut out = Vec::new();
        for entry in rd {
            let meta = entry.metadata();
            out.push(DirEntry {
                name: entry.file_name(),
                is_dir: meta.is_dir(),
                size: meta.size.unwrap_or(0),
                mtime: meta.mtime.unwrap_or(0) as u64,
            });
        }
        Ok(out)
    }

    pub async fn canonicalize(&self, path: &str) -> Result<String, SshError> {
        self.session
            .canonicalize(path)
            .await
            .map_err(|e| SshError::Sftp(e.to_string()))
    }

    pub async fn upload(&self, local: &std::path::Path, remote: &str) -> Result<(), SshError> {
        self.upload_with_progress(local, remote, &mut |_, _| {})
            .await
    }

    /// チャンク転送でアップロードし、チャンクごとに progress(転送済, 総バイト) を呼ぶ。
    pub async fn upload_with_progress(
        &self,
        local: &std::path::Path,
        remote: &str,
        progress: &mut (dyn FnMut(u64, u64) + Send),
    ) -> Result<(), SshError> {
        let mut src = tokio::fs::File::open(local).await.map_err(sftp_err)?;
        let total = src.metadata().await.map_err(sftp_err)?.len();
        // russh-sftp の write() は CREATE を付けないため、明示的に作成フラグで開く
        let mut file = self
            .session
            .open_with_flags(
                remote,
                OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
            )
            .await
            .map_err(sftp_err)?;
        progress(0, total);
        let mut done = 0u64;
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = src.read(&mut buf).await.map_err(sftp_err)?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).await.map_err(sftp_err)?;
            done += n as u64;
            progress(done, total);
        }
        file.flush().await.map_err(sftp_err)?;
        file.shutdown().await.map_err(sftp_err)?;
        Ok(())
    }

    pub async fn download(&self, remote: &str, local: &std::path::Path) -> Result<(), SshError> {
        self.download_with_progress(remote, local, &mut |_, _| {})
            .await
    }

    /// チャンク転送でダウンロードし、チャンクごとに progress(転送済, 総バイト) を呼ぶ。
    pub async fn download_with_progress(
        &self,
        remote: &str,
        local: &std::path::Path,
        progress: &mut (dyn FnMut(u64, u64) + Send),
    ) -> Result<(), SshError> {
        let total = self
            .session
            .metadata(remote)
            .await
            .map_err(sftp_err)?
            .size
            .unwrap_or(0);
        let mut src = self.session.open(remote).await.map_err(sftp_err)?;
        let mut dst = tokio::fs::File::create(local).await.map_err(sftp_err)?;
        progress(0, total);
        let mut done = 0u64;
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = src.read(&mut buf).await.map_err(sftp_err)?;
            if n == 0 {
                break;
            }
            dst.write_all(&buf[..n]).await.map_err(sftp_err)?;
            done += n as u64;
            progress(done, total);
        }
        dst.flush().await.map_err(sftp_err)?;
        Ok(())
    }

    pub async fn mkdir(&self, path: &str) -> Result<(), SshError> {
        self.session
            .create_dir(path)
            .await
            .map_err(|e| SshError::Sftp(e.to_string()))
    }

    /// 既に存在すれば成功として扱う mkdir（再帰転送で宛先ツリーを作るのに使う）。
    /// 親が無い場合は作らない（呼び出し側が先行順で親から作る前提）。
    pub async fn mkdir_p(&self, path: &str) -> Result<(), SshError> {
        if self.exists(path).await.unwrap_or(false) {
            return Ok(());
        }
        match self.mkdir(path).await {
            Ok(()) => Ok(()),
            // 競合で先に作られていた場合も成功扱い
            Err(e) => {
                if self.exists(path).await.unwrap_or(false) {
                    Ok(())
                } else {
                    Err(e)
                }
            }
        }
    }

    /// path がディレクトリか。metadata が取れなければ false。
    pub async fn is_dir(&self, path: &str) -> bool {
        match self.session.metadata(path).await {
            Ok(m) => m.is_dir(),
            Err(_) => false,
        }
    }

    /// `dir` 以下を再帰列挙する（`dir` 自身は含まない）。
    ///
    /// 返り値は**先行順**（親ディレクトリが必ずその中身より先）なので、
    /// 順に処理すれば宛先側で親を先に作れる。シンボリックリンクは辿らない
    /// （サーバは readdir で lstat 相当を返すため、リンクは is_dir=false になる）。
    /// エントリ数が [`MAX_WALK_ENTRIES`] を超えたらエラーで打ち切る。
    pub async fn walk(&self, dir: &str) -> Result<Vec<WalkEntry>, SshError> {
        let mut out: Vec<WalkEntry> = Vec::new();
        // 未走査ディレクトリの相対パス。"" は dir 自身。
        let mut stack: Vec<String> = vec![String::new()];
        while let Some(rel) = stack.pop() {
            let abs = if rel.is_empty() {
                dir.to_string()
            } else {
                rjoin(dir, &rel)
            };
            for e in self.read_dir(&abs).await? {
                if e.name == "." || e.name == ".." {
                    continue;
                }
                let child = if rel.is_empty() {
                    e.name.clone()
                } else {
                    format!("{rel}/{}", e.name)
                };
                if out.len() >= MAX_WALK_ENTRIES {
                    return Err(SshError::Sftp(format!(
                        "{dir}: エントリ数が上限 {MAX_WALK_ENTRIES} を超えた"
                    )));
                }
                out.push(WalkEntry {
                    rel: child.clone(),
                    is_dir: e.is_dir,
                });
                if e.is_dir {
                    stack.push(child);
                }
            }
        }
        Ok(out)
    }

    pub async fn rename(&self, from: &str, to: &str) -> Result<(), SshError> {
        self.session
            .rename(from, to)
            .await
            .map_err(|e| SshError::Sftp(e.to_string()))
    }

    pub async fn remove_file(&self, path: &str) -> Result<(), SshError> {
        self.session
            .remove_file(path)
            .await
            .map_err(|e| SshError::Sftp(e.to_string()))
    }

    pub async fn remove_dir(&self, path: &str) -> Result<(), SshError> {
        self.session
            .remove_dir(path)
            .await
            .map_err(|e| SshError::Sftp(e.to_string()))
    }

    /// `dir` を中身ごと再帰削除する。[`walk`](Self::walk) は先行順なので、
    /// 逆順に処理すれば子から先に消える。
    pub async fn remove_dir_all(&self, dir: &str) -> Result<(), SshError> {
        let entries = self.walk(dir).await?;
        for e in entries.iter().rev() {
            let path = rjoin(dir, &e.rel);
            if e.is_dir {
                self.remove_dir(&path).await?;
            } else {
                self.remove_file(&path).await?;
            }
        }
        self.remove_dir(dir).await
    }

    pub async fn exists(&self, path: &str) -> Result<bool, SshError> {
        self.session
            .try_exists(path)
            .await
            .map_err(|e| SshError::Sftp(e.to_string()))
    }
}

// Handle の型引数を lib 側の ClientHandler に合わせるための別名。
use crate::handler::ClientHandler as ClientHandlerAlias;

#[cfg(test)]
mod tests {
    use super::rjoin;

    #[test]
    fn rjoin_avoids_double_slash() {
        assert_eq!(rjoin("/home/u", "a.txt"), "/home/u/a.txt");
        assert_eq!(rjoin("/home/u/", "a.txt"), "/home/u/a.txt");
        assert_eq!(rjoin("/", "a.txt"), "/a.txt");
        // 相対パスの入れ子（walk が作る rel をそのまま渡すケース）
        assert_eq!(rjoin("/base", "sub/deep/f.bin"), "/base/sub/deep/f.bin");
    }
}
