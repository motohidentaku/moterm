#!/usr/bin/env bash
# 設定アイコンのクリックで設定ファイルをエディタで開くことの E2E。
# 偽 subl を PATH に置き、設定アイコン/NEW CONNECTION クリックで設定ファイルパスが渡るか検証。
set -eu
cd "$(dirname "$0")/.."
IMAGE=moterm-dev
echo "=== build ==="; ./docker/run.sh cargo build -p mot-gui 2>&1 | tail -1
mkdir -p artifacts
cat > artifacts/neo-settings.lua <<'EOF'
local moterm = require "moterm"
local config = moterm.config()
config.ui = "neo"
config.font_size = 15.0
config.profiles = { { name="demo-host", group="prod", host="10.0.0.1", user="admin" } }
config.groups = { { name="prod", label="Production" } }
return config
EOF
docker run --rm -v "$PWD":/work -v motmot-cargo-registry:/usr/local/cargo/registry -w /work "$IMAGE" bash -eu -c '
  export HOME=/root DISPLAY=:96
  # 偽 subl（優先順の先頭）: 引数(開いたパス)をマーカーへ追記
  printf "#!/bin/sh\necho \"OPENED:\$1\" >> /tmp/opened.txt\n" > /usr/local/bin/subl
  chmod +x /usr/local/bin/subl
  rm -f /tmp/opened.txt
  Xvfb :96 -screen 0 1100x700x24 >/tmp/xvfb.log 2>&1 & sleep 2
  target/debug/moterm /work/artifacts/neo-settings.lua >/tmp/moterm.log 2>&1 & GUI=$!
  sleep 3
  WID=$(xdotool search --sync --name moterm | head -1)
  send() { xdotool windowfocus "$WID" 2>/dev/null || true; xdotool "$@"; }
  # 設定アイコン（歯車）: 設定ゾーン = [w-btn*3-52, w-btn*3]。中心 ~ w-224（btn=63）。
  eval $(xdotool getwindowgeometry --shell "$WID")
  SX=$(( WIDTH - 224 ))
  echo "window WIDTH=$WIDTH, settings click X=$SX"
  send mousemove --window "$WID" --sync $SX 22 click 1; sleep 2
  # NEW CONNECTION ボタン（サイドバー最下部）も同じ挙動になるはず。
  NY=$(( HEIGHT - 65 ))
  echo "NEW CONNECTION click at 150 $NY"
  send mousemove --window "$WID" --sync 150 $NY click 1; sleep 2
  kill $GUI 2>/dev/null || true
  echo "--- opened.txt ---"; cat /tmp/opened.txt 2>/dev/null || echo "(none)"
  N=$(grep -c "neo-settings.lua" /tmp/opened.txt 2>/dev/null || echo 0)
  if [ "$N" -ge 2 ]; then
    echo "=== neo-settings: PASS（設定/NEW CONNECTION の両方で subl が設定ファイルを開いた） ==="
  else
    echo "=== neo-settings: FAIL（opened=$N） ==="; exit 1
  fi
'
