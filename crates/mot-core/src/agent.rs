//! リモートで動く Claude Code の状況（OSC 7777 で受け取るペイロード）のモデルとパーサ。
//!
//! 送り手はリモート側に置くスクリプト（`scripts/moterm-claude-*.sh`）で、
//! Claude Code の statusLine と hooks が stdin で受け取る JSON をそのまま
//! `/dev/tty` へ OSC で流す。**JSON は加工しない**ので、リモートに jq 等は要らない。
//!
//! ペイロード形式:
//!
//! ```text
//! OSC 7777 ; <tmux-pane> ; <json> BEL
//! ```
//!
//! `<tmux-pane>` は `$TMUX_PANE`（tmux 外なら `-`）。tmux 経由だと複数の
//! Claude Code が同じ PTY ストリームへ混ざって流れてくるため、受け取り側は
//! `session_id` でキー分けして保持する（tmux pane は表示・デバッグ用の補助）。

use serde::Deserialize;

/// OSC 番号（moterm 独自。他端末では未知 OSC として黙殺される）
pub const AGENT_OSC: &str = "7777";

/// tmux の外から送られたことを示す pane プレースホルダ
pub const NO_PANE: &str = "-";

/// エージェントの実行状態（hooks 由来）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentState {
    /// 応答生成中・ツール実行中
    Working,
    /// ユーザの操作待ち（権限プロンプト等）
    Waiting,
    /// 応答が終わって入力待ち
    #[default]
    Idle,
}

/// statusLine 由来のスナップショット。欠けているフィールドは None
/// （Claude Code のバージョン差やモデルによる有無を許容する）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AgentStatus {
    pub session_id: String,
    /// `$TMUX_PANE`（tmux 外なら None）
    pub tmux_pane: Option<String>,
    /// カスタム名 or AI 生成タイトル
    pub session_name: Option<String>,
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub version: Option<String>,
    /// context_window.used_percentage
    pub ctx_pct: Option<f32>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub ctx_size: Option<u64>,
    pub cost_usd: Option<f64>,
    pub duration_ms: Option<u64>,
    pub lines_added: Option<u64>,
    pub lines_removed: Option<u64>,
    /// workspace.git_worktree
    pub worktree: Option<String>,
    /// workspace.repo.name
    pub repo: Option<String>,
    /// effort.level
    pub effort: Option<String>,
    /// 実行中のツール名（hooks 由来。statusLine では更新しない）
    pub state: AgentState,
    pub tool: Option<String>,
}

/// OSC 7777 の1件。
#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    /// statusLine 由来のスナップショット（state/tool は既存値を保つ）
    Status(Box<AgentStatus>),
    /// hooks 由来の状態遷移
    State {
        session_id: String,
        tmux_pane: Option<String>,
        state: AgentState,
        /// Some(None) は「ツール実行が終わったのでクリア」、None は「変更しない」
        tool: Option<Option<String>>,
    },
    /// セッション終了（一覧から消す）
    End { session_id: String },
}

impl AgentEvent {
    pub fn session_id(&self) -> &str {
        match self {
            AgentEvent::Status(s) => &s.session_id,
            AgentEvent::State { session_id, .. } => session_id,
            AgentEvent::End { session_id } => session_id,
        }
    }
}

// ---------------------------------------------------------------------------
// 受信 JSON（statusLine と hooks の両方をこの1つで受ける）
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Raw {
    session_id: Option<String>,
    session_name: Option<String>,
    cwd: Option<String>,
    version: Option<String>,
    model: Option<RawModel>,
    workspace: Option<RawWorkspace>,
    cost: Option<RawCost>,
    context_window: Option<RawCtx>,
    effort: Option<RawEffort>,
    // ここから下は hooks 側にだけ現れる
    hook_event_name: Option<String>,
    tool_name: Option<String>,
    notification_type: Option<String>,
}

#[derive(Deserialize)]
struct RawModel {
    display_name: Option<String>,
    id: Option<String>,
}

#[derive(Deserialize)]
struct RawWorkspace {
    git_worktree: Option<String>,
    repo: Option<RawRepo>,
}

#[derive(Deserialize)]
struct RawRepo {
    name: Option<String>,
}

#[derive(Deserialize)]
struct RawCost {
    total_cost_usd: Option<f64>,
    total_duration_ms: Option<u64>,
    total_lines_added: Option<u64>,
    total_lines_removed: Option<u64>,
}

#[derive(Deserialize)]
struct RawCtx {
    total_input_tokens: Option<u64>,
    total_output_tokens: Option<u64>,
    context_window_size: Option<u64>,
    used_percentage: Option<f32>,
}

#[derive(Deserialize)]
struct RawEffort {
    level: Option<String>,
}

/// OSC 7777 のペイロード（`<pane>;<json>`）をパースする。
///
/// 番号を剥がした残りを渡すこと。session_id が取れないものは捨てる
/// （キー分けできないため）。
pub fn parse_agent_osc(rest: &str) -> Option<AgentEvent> {
    // JSON 内にも ';' は現れるので、最初の1つだけで割る。
    let (pane, json) = rest.split_once(';')?;
    let pane = if pane == NO_PANE || pane.is_empty() {
        None
    } else {
        Some(pane.to_string())
    };
    parse_agent_json(pane, json)
}

/// ペイロードの JSON 部分をパースする（pane は分解済み）。
pub fn parse_agent_json(tmux_pane: Option<String>, json: &str) -> Option<AgentEvent> {
    let raw: Raw = serde_json::from_str(json.trim()).ok()?;
    let session_id = raw.session_id.clone()?;

    // hook_event_name があれば hooks 由来 = 状態遷移。
    if let Some(event) = raw.hook_event_name.as_deref() {
        return match event {
            "SessionEnd" => Some(AgentEvent::End { session_id }),
            "UserPromptSubmit" => Some(AgentEvent::State {
                session_id,
                tmux_pane,
                state: AgentState::Working,
                tool: Some(None),
            }),
            "PreToolUse" => Some(AgentEvent::State {
                session_id,
                tmux_pane,
                state: AgentState::Working,
                tool: Some(raw.tool_name.clone()),
            }),
            "PostToolUse" => Some(AgentEvent::State {
                session_id,
                tmux_pane,
                state: AgentState::Working,
                tool: Some(None),
            }),
            "Stop" => Some(AgentEvent::State {
                session_id,
                tmux_pane,
                state: AgentState::Idle,
                tool: Some(None),
            }),
            "SessionStart" => Some(AgentEvent::State {
                session_id,
                tmux_pane,
                state: AgentState::Idle,
                tool: Some(None),
            }),
            // 権限プロンプトだけを「待ち」とみなす。他の通知は状態を変えない。
            "Notification" => {
                if raw.notification_type.as_deref() == Some("permission_prompt") {
                    Some(AgentEvent::State {
                        session_id,
                        tmux_pane,
                        state: AgentState::Waiting,
                        tool: None,
                    })
                } else {
                    None
                }
            }
            _ => None,
        };
    }

    // それ以外は statusLine のスナップショット。
    let ctx = raw.context_window;
    let cost = raw.cost;
    let ws = raw.workspace;
    Some(AgentEvent::Status(Box::new(AgentStatus {
        session_id,
        tmux_pane,
        session_name: raw.session_name,
        model: raw.model.and_then(|m| m.display_name.or(m.id)),
        cwd: raw.cwd,
        version: raw.version,
        ctx_pct: ctx.as_ref().and_then(|c| c.used_percentage),
        input_tokens: ctx.as_ref().and_then(|c| c.total_input_tokens),
        output_tokens: ctx.as_ref().and_then(|c| c.total_output_tokens),
        ctx_size: ctx.as_ref().and_then(|c| c.context_window_size),
        cost_usd: cost.as_ref().and_then(|c| c.total_cost_usd),
        duration_ms: cost.as_ref().and_then(|c| c.total_duration_ms),
        lines_added: cost.as_ref().and_then(|c| c.total_lines_added),
        lines_removed: cost.as_ref().and_then(|c| c.total_lines_removed),
        worktree: ws.as_ref().and_then(|w| w.git_worktree.clone()),
        repo: ws
            .as_ref()
            .and_then(|w| w.repo.as_ref())
            .and_then(|r| r.name.clone()),
        effort: raw.effort.and_then(|e| e.level),
        // state/tool は hooks が持つ。ここでは既定値を入れておき、
        // 受け取り側が既存エントリの値を引き継ぐ。
        state: AgentState::default(),
        tool: None,
    })))
}

/// セッション一覧（1 PTY ストリーム = 1 ペインぶん）。
///
/// tmux 経由だと複数セッションが同じストリームに混ざるため、session_id で
/// キー分けし、更新の新しい順に並べて保持する。
#[derive(Debug, Clone, Default)]
pub struct AgentSessions {
    /// 更新の新しい順
    list: Vec<AgentStatus>,
}

impl AgentSessions {
    /// 保持する最大セッション数（古いものから捨てる）
    pub const MAX: usize = 8;

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
    pub fn len(&self) -> usize {
        self.list.len()
    }
    /// 更新の新しい順
    pub fn iter(&self) -> impl Iterator<Item = &AgentStatus> {
        self.list.iter()
    }
    /// 直近に更新されたセッション
    pub fn latest(&self) -> Option<&AgentStatus> {
        self.list.first()
    }

    /// イベントを取り込む。更新されたセッションは先頭へ移動する。
    pub fn apply(&mut self, ev: AgentEvent) {
        let id = ev.session_id().to_string();
        let pos = self.list.iter().position(|s| s.session_id == id);
        match ev {
            AgentEvent::End { .. } => {
                if let Some(i) = pos {
                    self.list.remove(i);
                }
                return;
            }
            AgentEvent::Status(mut snap) => {
                // 状態と実行中ツールは hooks 側が持つので既存値を引き継ぐ。
                if let Some(i) = pos {
                    let old = self.list.remove(i);
                    snap.state = old.state;
                    snap.tool = old.tool;
                }
                self.list.insert(0, *snap);
            }
            AgentEvent::State {
                tmux_pane,
                state,
                tool,
                ..
            } => {
                let mut entry = match pos {
                    Some(i) => self.list.remove(i),
                    // hooks が先に来ることもある（statusLine は次の応答まで来ない）。
                    None => AgentStatus {
                        session_id: id,
                        ..Default::default()
                    },
                };
                entry.state = state;
                if let Some(t) = tool {
                    entry.tool = t;
                }
                if entry.tmux_pane.is_none() {
                    entry.tmux_pane = tmux_pane;
                }
                self.list.insert(0, entry);
            }
        }
        self.list.truncate(Self::MAX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// statusLine が渡してくる JSON（docs の全量スキーマから抜粋）
    const STATUS_JSON: &str = r#"{
      "cwd": "/home/dev/app",
      "session_id": "sess-1",
      "session_name": "refactor login",
      "transcript_path": "/x.jsonl",
      "model": { "id": "claude-opus-5", "display_name": "Opus" },
      "workspace": {
        "current_dir": "/home/dev/app",
        "git_worktree": "feature-xyz",
        "repo": { "host": "github.com", "owner": "acme", "name": "app" }
      },
      "version": "2.1.90",
      "cost": {
        "total_cost_usd": 0.5,
        "total_duration_ms": 45000,
        "total_lines_added": 156,
        "total_lines_removed": 23
      },
      "context_window": {
        "total_input_tokens": 15500,
        "total_output_tokens": 1200,
        "context_window_size": 200000,
        "used_percentage": 8
      },
      "exceeds_200k_tokens": false,
      "effort": { "level": "high" }
    }"#;

    #[test]
    fn parses_statusline_snapshot() {
        let ev = parse_agent_osc(&format!("%3;{STATUS_JSON}")).expect("パース成功");
        let AgentEvent::Status(s) = ev else {
            panic!("Status ではない: {ev:?}");
        };
        assert_eq!(s.session_id, "sess-1");
        assert_eq!(s.tmux_pane.as_deref(), Some("%3"));
        assert_eq!(s.session_name.as_deref(), Some("refactor login"));
        assert_eq!(s.model.as_deref(), Some("Opus"));
        assert_eq!(s.ctx_pct, Some(8.0));
        assert_eq!(s.input_tokens, Some(15500));
        assert_eq!(s.output_tokens, Some(1200));
        assert_eq!(s.cost_usd, Some(0.5));
        assert_eq!(s.worktree.as_deref(), Some("feature-xyz"));
        assert_eq!(s.repo.as_deref(), Some("app"));
        assert_eq!(s.effort.as_deref(), Some("high"));
    }

    #[test]
    fn tmux_placeholder_becomes_none() {
        let ev = parse_agent_osc(&format!("-;{STATUS_JSON}")).unwrap();
        let AgentEvent::Status(s) = ev else { panic!() };
        assert_eq!(s.tmux_pane, None);
    }

    #[test]
    fn json_containing_semicolons_survives() {
        // cwd に ';' が入っていても最初の区切りだけで割ること
        let json = r#"{"session_id":"s","cwd":"/tmp/a;b;c"}"#;
        let ev = parse_agent_osc(&format!("-;{json}")).unwrap();
        let AgentEvent::Status(s) = ev else { panic!() };
        assert_eq!(s.cwd.as_deref(), Some("/tmp/a;b;c"));
    }

    #[test]
    fn hook_events_map_to_states() {
        let cases = [
            (
                r#"{"session_id":"s","hook_event_name":"UserPromptSubmit"}"#,
                AgentState::Working,
            ),
            (
                r#"{"session_id":"s","hook_event_name":"Stop"}"#,
                AgentState::Idle,
            ),
            (
                r#"{"session_id":"s","hook_event_name":"SessionStart","source":"startup"}"#,
                AgentState::Idle,
            ),
            (
                r#"{"session_id":"s","hook_event_name":"Notification","notification_type":"permission_prompt"}"#,
                AgentState::Waiting,
            ),
        ];
        for (json, want) in cases {
            let ev = parse_agent_osc(&format!("-;{json}")).unwrap_or_else(|| panic!("{json}"));
            match ev {
                AgentEvent::State { state, .. } => assert_eq!(state, want, "{json}"),
                other => panic!("State ではない: {other:?}"),
            }
        }
    }

    #[test]
    fn pre_tool_use_carries_tool_name() {
        let json = r#"{"session_id":"s","hook_event_name":"PreToolUse","tool_name":"Bash"}"#;
        let ev = parse_agent_osc(&format!("-;{json}")).unwrap();
        match ev {
            AgentEvent::State { tool, .. } => assert_eq!(tool, Some(Some("Bash".into()))),
            other => panic!("{other:?}"),
        }
        // PostToolUse は実行中ツールをクリアする
        let json = r#"{"session_id":"s","hook_event_name":"PostToolUse","tool_name":"Bash"}"#;
        match parse_agent_osc(&format!("-;{json}")).unwrap() {
            AgentEvent::State { tool, .. } => assert_eq!(tool, Some(None)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn other_notifications_are_ignored() {
        let json = r#"{"session_id":"s","hook_event_name":"Notification","notification_type":"auth_success"}"#;
        assert!(parse_agent_osc(&format!("-;{json}")).is_none());
        // 未知のイベントも黙って捨てる
        let json = r#"{"session_id":"s","hook_event_name":"FutureEvent"}"#;
        assert!(parse_agent_osc(&format!("-;{json}")).is_none());
    }

    #[test]
    fn broken_payloads_are_rejected() {
        assert!(parse_agent_osc("").is_none());
        assert!(parse_agent_osc("-;not json").is_none());
        // session_id が無いとキー分けできないので捨てる
        assert!(parse_agent_osc(r#"-;{"cwd":"/tmp"}"#).is_none());
        // 区切りが無い
        assert!(parse_agent_osc(r#"{"session_id":"s"}"#).is_none());
    }

    #[test]
    fn sessions_key_by_id_and_order_by_recency() {
        let mut s = AgentSessions::default();
        for id in ["a", "b", "c"] {
            s.apply(parse_agent_osc(&format!(r#"-;{{"session_id":"{id}"}}"#)).unwrap());
        }
        assert_eq!(s.len(), 3);
        assert_eq!(s.latest().unwrap().session_id, "c");
        // 既存セッションの更新は先頭へ来るだけで、増えない
        s.apply(parse_agent_osc(r#"-;{"session_id":"a"}"#).unwrap());
        assert_eq!(s.len(), 3);
        assert_eq!(s.latest().unwrap().session_id, "a");
    }

    #[test]
    fn statusline_keeps_state_from_hooks() {
        let mut s = AgentSessions::default();
        // hooks が先に届く（statusLine は次の応答まで来ない）
        s.apply(
            parse_agent_osc(
                r#"-;{"session_id":"s","hook_event_name":"PreToolUse","tool_name":"Bash"}"#,
            )
            .unwrap(),
        );
        assert_eq!(s.latest().unwrap().state, AgentState::Working);
        // あとから来た statusLine は state/tool を消さない
        s.apply(parse_agent_osc(&format!("-;{STATUS_JSON}").replace("sess-1", "s")).unwrap());
        let cur = s.latest().unwrap();
        assert_eq!(cur.state, AgentState::Working);
        assert_eq!(cur.tool.as_deref(), Some("Bash"));
        assert_eq!(cur.model.as_deref(), Some("Opus"));
    }

    #[test]
    fn session_end_removes_entry() {
        let mut s = AgentSessions::default();
        s.apply(parse_agent_osc(r#"-;{"session_id":"a"}"#).unwrap());
        s.apply(parse_agent_osc(r#"-;{"session_id":"b"}"#).unwrap());
        s.apply(parse_agent_osc(r#"-;{"session_id":"a","hook_event_name":"SessionEnd"}"#).unwrap());
        assert_eq!(s.len(), 1);
        assert_eq!(s.latest().unwrap().session_id, "b");
    }

    #[test]
    fn keeps_at_most_max_sessions() {
        let mut s = AgentSessions::default();
        for i in 0..(AgentSessions::MAX + 4) {
            s.apply(parse_agent_osc(&format!(r#"-;{{"session_id":"s{i}"}}"#)).unwrap());
        }
        assert_eq!(s.len(), AgentSessions::MAX);
        // 残るのは新しい方
        assert_eq!(
            s.latest().unwrap().session_id,
            format!("s{}", AgentSessions::MAX + 3)
        );
    }
}
