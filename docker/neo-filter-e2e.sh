#!/usr/bin/env bash
# neo サイドバー Filter 入力の E2E: フィルタ枠クリック→"prod"入力→一覧が絞られるスクショ。
set -eu
cd "$(dirname "$0")/.."
IMAGE=moterm-dev
echo "=== build ==="; ./docker/run.sh cargo build -p mot-gui 2>&1 | tail -1
mkdir -p artifacts
cat > artifacts/neo-filter.lua <<'EOF'
local moterm = require "moterm"
local config = moterm.config()
config.ui = "neo"
config.font_size = 15.0
config.profiles = {
  { name="web-prod",   group="prod", host="10.0.0.1", user="admin" },
  { name="db-prod",    group="prod", host="10.0.0.2", user="admin" },
  { name="cache-dev",  group="dev",  host="10.0.1.1", user="admin" },
  { name="build-dev",  group="dev",  host="10.0.1.2", user="admin" },
}
config.groups = { { name="prod", label="Production" }, { name="dev", label="Development" } }
return config
EOF
docker run --rm -v "$PWD":/work -v motmot-cargo-registry:/usr/local/cargo/registry -w /work "$IMAGE" bash -eu -c '
  export HOME=/root DISPLAY=:96
  Xvfb :96 -screen 0 1100x700x24 >/tmp/xvfb.log 2>&1 & sleep 2
  target/debug/moterm /work/artifacts/neo-filter.lua >/tmp/moterm.log 2>&1 & GUI=$!
  sleep 3
  WID=$(xdotool search --sync --name moterm | head -1)
  send() { xdotool windowfocus "$WID" 2>/dev/null || true; xdotool "$@"; }
  # フィルタ枠をクリックしてフォーカス
  send mousemove --window "$WID" --sync 150 74 click 1; sleep 1
  # "prod" と入力（prod グループのホストのみ残るはず）
  send type --window "$WID" --clearmodifiers "prod"; sleep 2
  import -window root /work/artifacts/neo-filter.png 2>/dev/null || (xwd -root -silent | convert xwd:- /work/artifacts/neo-filter.png)
  kill $GUI 2>/dev/null || true
  echo "=== neo-filter: done ==="
'
