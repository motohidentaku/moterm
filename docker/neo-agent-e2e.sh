#!/usr/bin/env bash
# 情報パネルの AGENT セクション E2E。
#
# claude 本体はコンテナに無いので、Claude Code が statusLine / hooks へ渡すのと
# 同じ形の JSON を scripts/moterm-claude.sh に食わせ、実 SSH セッションの PTY へ
# OSC 7777 を流す。経路（PTY → mot-term の OSC → mot-core のパーサ → 情報パネル）
# は本番と同じものを通る。
set -eu
cd "$(dirname "$0")/.."
IMAGE=moterm-dev
# shellcheck source=docker/lib.sh
. "$(dirname "$0")/lib.sh"
echo "=== phase1: build ==="; ./docker/run.sh cargo build -p mot-gui 2>&1 | tail -1
detect_secopt "$IMAGE"
mkdir -p artifacts
echo "=== phase2 ==="
docker run --rm "${SECOPT[@]+"${SECOPT[@]}"}" \
  -v "$PWD":/work -v motmot-cargo-registry:/usr/local/cargo/registry -w /work "$IMAGE" bash -eu -c '
  TESTUSER=moterm; PORT=2222
  id -u sshd >/dev/null 2>&1 || useradd -r -M -d /run/sshd -s /usr/sbin/nologin sshd
  useradd -m -s /bin/bash "$TESTUSER"; echo "$TESTUSER:unlockpw" | chpasswd
  HOMEDIR=$(eval echo ~$TESTUSER)
  mkdir -p /run/sshd /etc/moterm
  ssh-keygen -q -t ed25519 -f /etc/moterm/hostkey -N ""
  su "$TESTUSER" -c "ssh-keygen -q -t ed25519 -f $HOMEDIR/id_ed25519 -N \"\""
  mkdir -p "$HOMEDIR/.ssh"; cp "$HOMEDIR/id_ed25519.pub" "$HOMEDIR/.ssh/authorized_keys"
  chmod 700 "$HOMEDIR/.ssh"; chmod 600 "$HOMEDIR/.ssh/authorized_keys"
  cat > /etc/moterm/sshd_config <<EOF
Port $PORT
ListenAddress 127.0.0.1
HostKey /etc/moterm/hostkey
PidFile /run/sshd/sshd.pid
AuthorizedKeysFile $HOMEDIR/.ssh/authorized_keys
PubkeyAuthentication yes
UsePAM no
StrictModes no
EOF
  /usr/sbin/sshd -f /etc/moterm/sshd_config -E /tmp/sshd.log; sleep 1
  # tmux 経由の検証用（ベースイメージには無い）。openssh-server の half-configured で
  # apt は非ゼロ終了するが tmux 自体は入る。
  apt-get update -qq >/dev/null 2>&1
  apt-get install -y -qq --no-install-recommends tmux >/dev/null 2>&1 || true
  command -v tmux >/dev/null && echo "tmux: $(tmux -V)" || echo "tmux: 導入できず（tmux 検証はスキップ）"

  # --- リモート側の配置（実運用で README に従ってユーザが置くもの）---
  cp /work/scripts/moterm-claude.sh "$HOMEDIR/moterm-claude.sh"
  chmod +x "$HOMEDIR/moterm-claude.sh"

  # statusLine が渡してくる JSON（2 セッションぶん = tmux で混ざる状況の模擬）
  cat > "$HOMEDIR/s1.json" <<EOF
{"cwd":"/srv/app","session_id":"sess-aaa","session_name":"refactor login flow",
 "model":{"id":"claude-opus-5","display_name":"Opus"},
 "workspace":{"current_dir":"/srv/app","git_worktree":"feature-login","repo":{"name":"app"}},
 "version":"2.1.90",
 "cost":{"total_cost_usd":1.27,"total_duration_ms":845000,"total_lines_added":156,"total_lines_removed":23},
 "context_window":{"total_input_tokens":427000,"total_output_tokens":1800,
                   "context_window_size":1000000,"used_percentage":43},
 "effort":{"level":"high"}}
EOF
  cat > "$HOMEDIR/s2.json" <<EOF
{"cwd":"/srv/api","session_id":"sess-bbb","session_name":"fix flaky test",
 "model":{"id":"claude-sonnet-5","display_name":"Sonnet"},
 "cost":{"total_cost_usd":0.08},
 "context_window":{"total_input_tokens":15500,"total_output_tokens":1200,
                   "context_window_size":200000,"used_percentage":78}}
EOF
  # hooks 由来: sess-aaa が Bash を実行中
  cat > "$HOMEDIR/hook.json" <<EOF
{"session_id":"sess-aaa","hook_event_name":"PreToolUse","tool_name":"Bash","cwd":"/srv/app"}
EOF
  # tmux 越しに流す 3 つめのセッション
  cat > "$HOMEDIR/s3.json" <<EOF
{"cwd":"/srv/worker","session_id":"sess-ccc","session_name":"via tmux",
 "model":{"id":"claude-haiku-4-5","display_name":"Haiku"},
 "cost":{"total_cost_usd":0.02},
 "context_window":{"total_input_tokens":3200,"total_output_tokens":410,
                   "context_window_size":200000,"used_percentage":12}}
EOF
  # tmux は未知の OSC を捨てるので passthrough を有効にする
  echo "set -g allow-passthrough on" > "$HOMEDIR/.tmux.conf"
  cat > "$HOMEDIR/tmuxdemo.sh" <<EOF
cd \$HOME
tmux kill-server 2>/dev/null
tmux new-session -s t "./moterm-claude.sh < s3.json; sleep 300"
EOF
  cat > "$HOMEDIR/demo.sh" <<EOF
cd \$HOME
./moterm-claude.sh < s2.json
./moterm-claude.sh < s1.json
./moterm-claude.sh < hook.json
EOF
  chown -R "$TESTUSER:$TESTUSER" "$HOMEDIR"

  cat > /tmp/moterm.lua <<EOF
local moterm = require "moterm"
local config = moterm.config()
config.ui = "neo"
config.font_size = 15.0
config.window = { width = 1500, height = 850 }
config.profiles = {
  { name="local-demo", group="demo", host="127.0.0.1", port=$PORT, user="$TESTUSER",
    auth={ method="publickey", key="$HOMEDIR/id_ed25519" } },
}
config.groups = { { name="demo", label="Demo" } }
-- メトリクス採取は今回の対象外なので止めておく（パネルは開いたまま）
config.metrics = { enabled = false, panel = true }
return config
EOF
  export HOME=/root DISPLAY=:99
  Xvfb :99 -screen 0 1600x900x24 >/tmp/xvfb.log 2>&1 & sleep 2
  mkdir -p /root/.ssh
  RUST_LOG=warn target/debug/moterm /tmp/moterm.lua >/tmp/moterm.log 2>&1 & GUI=$!
  sleep 3
  WID=$(xdotool search --sync --name moterm | head -1); echo "window: $WID"
  xdotool windowfocus "$WID" 2>/dev/null || true; sleep 1
  xdotool mousemove --window "$WID" --sync 90 180 click 1
  sleep 3
  xdotool windowfocus "$WID" 2>/dev/null || true
  xdotool key --window "$WID" --clearmodifiers y
  sleep 4
  # 2 セッション分の statusLine と、実行中ツールの hook を流す
  xdotool type --window "$WID" --clearmodifiers "sh demo.sh"
  xdotool key --window "$WID" Return
  sleep 3
  import -window root /work/artifacts/neo-agent.png 2>/dev/null \
    || (xwd -root -silent | convert xwd:- /work/artifacts/neo-agent.png)

  # --- tmux 越し（allow-passthrough on）で 3 つめのセッションを流す ---
  if command -v tmux >/dev/null; then
    xdotool type --window "$WID" --clearmodifiers "sh tmuxdemo.sh"
    xdotool key --window "$WID" Return
    sleep 5
    import -window root /work/artifacts/neo-agent-tmux.png 2>/dev/null \
      || (xwd -root -silent | convert xwd:- /work/artifacts/neo-agent-tmux.png)
  fi

  echo "--- moterm.log ---"; tail -12 /tmp/moterm.log || true
  if grep -q "Accepted publickey" /tmp/sshd.log 2>/dev/null; then
    echo "=== neo-agent-e2e: 接続 OK（artifacts/neo-agent.png を目視確認） ==="
  else
    echo "=== neo-agent-e2e: FAIL（sshd に Accepted が無い） ==="; tail -20 /tmp/sshd.log || true; exit 1
  fi
  kill $GUI 2>/dev/null || true
'
