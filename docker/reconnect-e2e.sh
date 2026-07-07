#!/usr/bin/env bash
# 自動再接続の E2E: 接続→sshd停止で切断→sshd再起動→バックオフ後に自動再接続することを
# sshd ログの "Accepted" 2回で立証する。
set -eu
cd "$(dirname "$0")/.."
IMAGE=moterm-dev
echo "=== build ==="; ./docker/run.sh cargo build -p mot-gui 2>&1 | tail -1
mkdir -p artifacts
docker run --rm -v "$PWD":/work -v motmot-cargo-registry:/usr/local/cargo/registry -w /work "$IMAGE" bash -eu -c '
  TESTUSER=moterm; PORT=2222
  useradd -m -s /bin/bash "$TESTUSER"; echo "$TESTUSER:pw" | chpasswd
  HOMEDIR=$(eval echo ~$TESTUSER)
  mkdir -p /run/sshd /etc/moterm
  ssh-keygen -q -t ed25519 -f /etc/moterm/hostkey -N ""        # 永続ホスト鍵（再起動で同じ）
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
  start_sshd() { /usr/sbin/sshd -f /etc/moterm/sshd_config -E /tmp/sshd.log; }
  start_sshd; sleep 1
  cat > /tmp/moterm.lua <<EOF
local moterm = require "moterm"
local config = moterm.config()
config.ui = "neo"; config.font_size = 15.0
config.profiles = {
  { name="local-demo", group="demo", host="127.0.0.1", port=$PORT, user="$TESTUSER",
    auth={ method="publickey", key="$HOMEDIR/id_ed25519" },
    reconnect = { enabled = true, max_retries = 5, backoff_sec = 2 } },
}
config.groups = { { name="demo", label="Demo" } }
return config
EOF
  export HOME=/root DISPLAY=:98
  Xvfb :98 -screen 0 1100x700x24 >/tmp/xvfb.log 2>&1 & sleep 2
  mkdir -p /root/.ssh
  RUST_LOG=warn target/debug/moterm /tmp/moterm.lua >/tmp/moterm.log 2>&1 & GUI=$!
  sleep 3
  WID=$(xdotool search --sync --name moterm | head -1)
  send() { xdotool windowfocus "$WID" 2>/dev/null || true; xdotool "$@"; }
  send mousemove --window "$WID" --sync 90 180 click 1; sleep 2   # 接続
  send key --window "$WID" --clearmodifiers y; sleep 4            # TOFU 承認
  A1=$(grep -c "Accepted" /tmp/sshd.log || echo 0); echo "after connect: Accepted=$A1"
  # 切断: sshd を停止 → すぐ再起動（バックオフ2秒の間に復帰させる）
  pkill -9 sshd || true; sleep 1; start_sshd; echo "sshd restarted"
  sleep 8                                                          # バックオフ+再接続待ち
  import -window root /work/artifacts/reconnect.png 2>/dev/null || (xwd -root -silent | convert xwd:- /work/artifacts/reconnect.png)
  A2=$(grep -c "Accepted" /tmp/sshd.log || echo 0); echo "after reconnect: Accepted=$A2"
  echo "--- moterm.log ---"; grep -i "reconnect\|再接続\|connect" /tmp/moterm.log | tail -5 || true
  kill $GUI 2>/dev/null || true
  if [ "$A2" -ge 2 ]; then echo "=== reconnect: PASS（Accepted $A2 回=自動再接続成功） ==="; else echo "=== reconnect: FAIL（Accepted=$A2） ==="; exit 1; fi
'
