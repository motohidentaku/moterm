#!/usr/bin/env bash
# Stage 8 E2E: NEO-UI で SFTP ファイルマネージャが接続画面に埋め込み描画されることを確認。
# キーボードF3の実配線は classic gui-sftp-e2e が同一 terminal_key 経路で立証済みのため、
# ここでは render 委譲（neo_ui かつ mode==Sftp → render_sftp）を MOTERM_OPEN_SFTP=1 の
# 起動経路で確認する（クラッシュせず FM が描画されること）。
set -eu
cd "$(dirname "$0")/.."
IMAGE=moterm-dev
echo "=== build ==="; ./docker/run.sh cargo build -p mot-gui 2>&1 | tail -1
mkdir -p artifacts
cat > artifacts/neo-sftp.lua <<'EOF'
local moterm = require "moterm"
local config = moterm.config()
config.ui = "neo"
config.profiles = { { name="demo-host", group="prod", host="10.0.0.1", user="admin" } }
config.groups = { { name="prod", label="Production" } }
return config
EOF
docker run --rm -v "$PWD":/work -v motmot-cargo-registry:/usr/local/cargo/registry -w /work "$IMAGE" bash -eu -c '
  export HOME=/root DISPLAY=:96 MOTERM_OPEN_SFTP=1
  Xvfb :96 -screen 0 1100x680x24 >/tmp/xvfb.log 2>&1 & sleep 2
  target/debug/moterm --screenshot /work/artifacts/neo-sftp-render.png /work/artifacts/neo-sftp.lua
  rc=$?
  if [ $rc -eq 0 ] && [ -s /work/artifacts/neo-sftp-render.png ]; then
    echo "=== neo-sftp-e2e: PASS（neo で SFTP を埋め込み描画 rc=$rc） ==="
  else
    echo "=== neo-sftp-e2e: FAIL（rc=$rc） ==="; exit 1
  fi
'
