#!/usr/bin/env bash
# neo マスターパスワードモーダルのスクショ（MOTERM_DEMO_MASTER=1 で開いた状態で起動）。
set -eu
cd "$(dirname "$0")/.."
IMAGE=moterm-dev
echo "=== build ==="; ./docker/run.sh cargo build -p mot-gui 2>&1 | tail -1
mkdir -p artifacts
cat > artifacts/neo-master.lua <<'EOF'
local moterm = require "moterm"
local config = moterm.config()
config.ui = "neo"
config.font_size = 15.0
config.profiles = { { name="demo-host", group="prod", host="10.0.0.1", user="admin" } }
config.groups = { { name="prod", label="Production" } }
return config
EOF
docker run --rm -v "$PWD":/work -v motmot-cargo-registry:/usr/local/cargo/registry -w /work "$IMAGE" bash -eu -c '
  export HOME=/root DISPLAY=:96 MOTERM_DEMO_MASTER=1
  Xvfb :96 -screen 0 1100x700x24 >/tmp/xvfb.log 2>&1 & sleep 2
  target/debug/moterm --screenshot /work/artifacts/neo-master.png /work/artifacts/neo-master.lua
  rc=$?
  if [ $rc -eq 0 ] && [ -s /work/artifacts/neo-master.png ]; then echo "=== neo-master: PASS rc=$rc ==="; else echo "=== neo-master: FAIL rc=$rc ==="; exit 1; fi
'
