#!/usr/bin/env bash
# 情報パネル（CPU/メモリ/ディスク）の実測確認: 接続 → 採取を待つ → スクショ。
# 実 sshd の /proc・df から採った値がパネルに描かれることを見る。
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
  chown -R "$TESTUSER:$TESTUSER" "$HOMEDIR/.ssh" "$HOMEDIR/id_ed25519"*
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

  # 参考値: 同じコマンドを ssh 越しでなく直に叩いた結果（描画値との突き合わせ用）
  echo "--- 期待値（コンテナ実測） ---"
  grep -E "^Mem(Total|Available):" /proc/meminfo
  df -Pk /

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
config.metrics = { enabled = true, interval_min = 1, panel = true }
return config
EOF
  export HOME=/root DISPLAY=:99
  Xvfb :99 -screen 0 1600x900x24 >/tmp/xvfb.log 2>&1 & sleep 2
  mkdir -p /root/.ssh
  RUST_LOG=warn,mot_gui=debug target/debug/moterm /tmp/moterm.lua >/tmp/moterm.log 2>&1 & GUI=$!
  sleep 3
  WID=$(xdotool search --sync --name moterm | head -1); echo "window: $WID"
  xdotool windowfocus "$WID" 2>/dev/null || true; sleep 1
  # サイドバーのホスト行をクリックして接続 → TOFU を y で承認
  xdotool mousemove --window "$WID" --sync 90 180 click 1
  sleep 3
  xdotool windowfocus "$WID" 2>/dev/null || true
  xdotool key --window "$WID" --clearmodifiers y
  sleep 4
  # 初回採取（コマンド内 sleep 1 + 往復）が届くのを待つ
  sleep 5
  import -window root /work/artifacts/neo-metrics.png 2>/dev/null \
    || (xwd -root -silent | convert xwd:- /work/artifacts/neo-metrics.png)
  # F6 で情報パネルを畳んだ状態も記録（端末が広がること）
  xdotool key --window "$WID" --clearmodifiers F6
  sleep 2
  import -window root /work/artifacts/neo-metrics-off.png 2>/dev/null \
    || (xwd -root -silent | convert xwd:- /work/artifacts/neo-metrics-off.png)
  echo "--- moterm.log ---"; tail -12 /tmp/moterm.log || true
  if grep -q "Accepted publickey" /tmp/sshd.log 2>/dev/null; then
    echo "=== neo-metrics-e2e: 接続 OK（artifacts/neo-metrics.png を目視確認） ==="
  else
    echo "=== neo-metrics-e2e: FAIL（sshd に Accepted が無い） ==="; tail -20 /tmp/sshd.log || true; exit 1
  fi
  kill $GUI 2>/dev/null || true
'
