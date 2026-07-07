//! PTY ペイン。1 SSH セッション上の 1 チャネル = 1 シェル。
//! 出力バイトは output チャネルへ流し、GUI は try_recv でポーリングする。

use crate::log::LogTap;
use russh::client::Msg;
use russh::{Channel, ChannelMsg};
use tokio::sync::mpsc;

/// ペインから GUI への通知。
#[derive(Debug)]
pub enum PaneEvent {
    /// リモートからの出力バイト（端末エミュレータへ供給する）
    Output(Vec<u8>),
    /// シェルが終了した（exit_status 付き）。タブ/ペインを畳む判断に使う。
    Exited(Option<u32>),
}

/// GUI が保持するペインハンドル。入力送信・リサイズ・出力受信。
pub struct PaneHandle {
    input_tx: mpsc::UnboundedSender<PaneInput>,
    pub events: mpsc::UnboundedReceiver<PaneEvent>,
    pub id: u64,
}

enum PaneInput {
    Data(Vec<u8>),
    Resize(u16, u16),
}

impl PaneHandle {
    /// キー入力等をシェルへ送る。
    pub fn write(&self, bytes: Vec<u8>) {
        let _ = self.input_tx.send(PaneInput::Data(bytes));
    }
    /// ウィンドウ/ペインのリサイズを SIGWINCH としてリモートへ伝える。
    pub fn resize(&self, cols: u16, rows: u16) {
        let _ = self.input_tx.send(PaneInput::Resize(cols, rows));
    }
    /// 非ブロッキングでイベントを1件取り出す。
    pub fn try_event(&mut self) -> Option<PaneEvent> {
        self.events.try_recv().ok()
    }
}

/// チャネルを PTY シェルとして起動し、ポンプタスクを spawn して PaneHandle を返す。
pub async fn spawn_pane(
    mut channel: Channel<Msg>,
    id: u64,
    cols: u16,
    rows: u16,
    mut log: Option<LogTap>,
) -> Result<PaneHandle, russh::Error> {
    channel
        .request_pty(false, "xterm-256color", cols as u32, rows as u32, 0, 0, &[])
        .await?;
    channel.request_shell(false).await?;

    let (input_tx, mut input_rx) = mpsc::unbounded_channel::<PaneInput>();
    let (event_tx, event_rx) = mpsc::unbounded_channel::<PaneEvent>();

    tokio::spawn(async move {
        let mut exit_status: Option<u32> = None;
        loop {
            tokio::select! {
                msg = channel.wait() => {
                    match msg {
                        Some(ChannelMsg::Data { data }) => {
                            if let Some(t) = log.as_mut() {
                                t.write_raw(&data);
                            }
                            if event_tx.send(PaneEvent::Output(data.to_vec())).is_err() {
                                break;
                            }
                        }
                        Some(ChannelMsg::ExtendedData { data, .. }) => {
                            // stderr も端末へ合流させる
                            if let Some(t) = log.as_mut() {
                                t.write_raw(&data);
                            }
                            let _ = event_tx.send(PaneEvent::Output(data.to_vec()));
                        }
                        Some(ChannelMsg::ExitStatus { exit_status: es }) => {
                            exit_status = Some(es);
                        }
                        Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) => {
                            log::info!("pane {id}: リモートがチャネルを閉じた (Eof/Close)");
                            break;
                        }
                        None => {
                            // channel.wait()=None はチャネル/セッションが予期せず尽きた合図。
                            // 転送層の切断・サーバ側 disconnect・キープアライブ timeout などで起きる。
                            // 切断の切り分けができるよう既定レベル(warn)で残す。russh 自身の理由は
                            // RUST_LOG=russh=debug で併せて確認できる。
                            log::warn!(
                                "pane {id}: SSH セッションが予期せず終了 (channel.wait()=None)"
                            );
                            break;
                        }
                        _ => {}
                    }
                }
                inp = input_rx.recv() => {
                    match inp {
                        Some(PaneInput::Data(bytes)) => {
                            if let Err(e) = channel.data(&bytes[..]).await {
                                log::warn!("pane {id}: 入力送信に失敗しチャネル終了: {e}");
                                break;
                            }
                        }
                        Some(PaneInput::Resize(c, r)) => {
                            let _ = channel.window_change(c as u32, r as u32, 0, 0).await;
                        }
                        None => break,
                    }
                }
            }
        }
        let _ = event_tx.send(PaneEvent::Exited(exit_status));
    });

    Ok(PaneHandle {
        input_tx,
        events: event_rx,
        id,
    })
}
