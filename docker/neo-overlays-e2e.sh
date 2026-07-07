#!/usr/bin/env bash
# neon オーバーレイ3種のスクショ: パスワードモーダル / 検索バー / PFパネル。
set -eu
cd "$(dirname "$0")/.."
IMAGE=moterm-dev
mkdir -p artifacts
cat > artifacts/neo-ov.lua <<'EOF'
local moterm = require "moterm"
local config = moterm.config()
config.ui = "neo"
config.font_size = 15.0
config.profiles = { { name="prod-01", group="prod", host="10.0.1.11", port=22, user="deploy" } }
config.groups = { { name="prod", label="Production" } }
return config
EOF
docker run --rm -v "$PWD":/work -v motmot-cargo-registry:/usr/local/cargo/registry -w /work "$IMAGE" bash -eu -c '
  export HOME=/root DISPLAY=:96
  Xvfb :96 -screen 0 1100x700x24 >/tmp/xvfb.log 2>&1 & sleep 2
  MOTERM_DEMO_MODAL=password target/debug/moterm --screenshot /work/artifacts/neo-ov-password.png /work/artifacts/neo-ov.lua
  MOTERM_DEMO_SEARCH=1      target/debug/moterm --screenshot /work/artifacts/neo-ov-search.png   /work/artifacts/neo-ov.lua
  MOTERM_DEMO_PF=1          target/debug/moterm --screenshot /work/artifacts/neo-ov-pf.png       /work/artifacts/neo-ov.lua
  for f in password search pf; do [ -s /work/artifacts/neo-ov-$f.png ] && echo "ok: $f" || { echo "FAIL: $f"; exit 1; }; done
  echo "=== neo-overlays: PASS ==="
'
