//! SFTP。サブシステムチャネル上で russh-sftp を駆動し、2ペインFM/ D&D 転送を支える。

use crate::error::SshError;
use russh::client::{Handle, Msg};
use russh::Channel;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// 転送のチャンクサイズ。進捗コールバックの粒度も兼ねる。
const CHUNK: usize = 64 * 1024;

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

    pub async fn exists(&self, path: &str) -> Result<bool, SshError> {
        self.session
            .try_exists(path)
            .await
            .map_err(|e| SshError::Sftp(e.to_string()))
    }
}

// Handle の型引数を lib 側の ClientHandler に合わせるための別名。
use crate::handler::ClientHandler as ClientHandlerAlias;
