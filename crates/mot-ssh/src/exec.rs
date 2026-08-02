//! 非対話コマンドの実行（exec チャネル）。PTY を使わず 1 コマンドの出力だけを集める。
//!
//! 対話シェルのペインとは別チャネルなので、ユーザの画面・シェル履歴を汚さない。
//! 同一セッション上に開くため再認証も起きない。

use crate::error::SshError;
use crate::handler::ClientHandler;
use russh::client::Handle;
use russh::ChannelMsg;
use std::sync::Arc;
use std::time::Duration;

/// 暴走出力でメモリを食い潰さないための上限（メトリクス用途では数百バイト）。
const MAX_OUTPUT: usize = 64 * 1024;

/// セッション上でコマンドを実行する軽量ハンドル。Clone して
/// バックグラウンドタスクへ渡せる（セッション本体の所有権は GUI 側が持つ）。
#[derive(Clone)]
pub struct ExecProbe {
    handle: Arc<Handle<ClientHandler>>,
}

impl ExecProbe {
    pub(crate) fn new(handle: Arc<Handle<ClientHandler>>) -> ExecProbe {
        ExecProbe { handle }
    }

    /// セッションが切れていれば true（サンプラーの停止判定に使う）。
    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    /// コマンドを実行して stdout を文字列で返す。timeout 超過で打ち切る。
    ///
    /// stderr は捨てる（メトリクス採取では motd や警告が混ざるだけで害があるため）。
    /// 終了ステータスが非ゼロでも stdout は返す（部分的に使える出力を捨てない）。
    pub async fn run(&self, cmd: &str, timeout: Duration) -> Result<String, SshError> {
        match tokio::time::timeout(timeout, self.run_inner(cmd)).await {
            Ok(r) => r,
            Err(_) => Err(SshError::Exec(format!(
                "コマンドが {} 秒で応答しませんでした",
                timeout.as_secs()
            ))),
        }
    }

    async fn run_inner(&self, cmd: &str) -> Result<String, SshError> {
        let mut channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(SshError::Russh)?;
        channel.exec(true, cmd).await.map_err(SshError::Russh)?;

        let mut out: Vec<u8> = Vec::new();
        let mut exit: Option<u32> = None;
        loop {
            match channel.wait().await {
                Some(ChannelMsg::Data { data }) => {
                    // 上限までは貯め、超過分は捨てて読み続ける（チャネルは閉じる）。
                    let room = MAX_OUTPUT.saturating_sub(out.len());
                    if room > 0 {
                        out.extend_from_slice(&data[..data.len().min(room)]);
                    }
                }
                // stderr は破棄。ExitStatus は Eof の前後どちらにも来うるので拾い続ける。
                Some(ChannelMsg::ExtendedData { .. }) | Some(ChannelMsg::Eof) => {}
                Some(ChannelMsg::ExitStatus { exit_status }) => exit = Some(exit_status),
                Some(ChannelMsg::Close) | None => break,
                _ => {}
            }
        }
        if exit.is_some_and(|e| e != 0) {
            log::debug!("exec 非ゼロ終了 ({:?}): {cmd}", exit);
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }
}
