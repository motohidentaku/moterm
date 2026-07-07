#!/usr/bin/env bash
# neo SFTP が接続に紐づくことの E2E: 接続→F3でSFTP→sshd停止で切断→SFTPが閉じて端末へ戻る。
set -eu
cd "$(dirname "$0")/.."
IMAGE=moterm-dev
echo "=== build ==="; ./docker/run.sh cargo build -p mot-gui 2>&1 | tail -1
mkdir -p artifacts
docker run --rm -v "$PWD":/work -v motmot-cargo-registry:/usr/local/cargo/registry -w /work "$IMAGE" bash -eu -c '
  TESTUSER=moterm; TESTPW=moterm-test-pw; PORT=2222
  useradd -m -s /bin/bash "$TESTUSER"; echo "$TESTUSER:$TESTPW" | chpasswd
  HOMEDIR=$(eval echo ~$TESTUSER)
  mkdir -p /run/sshd /etc/moterm
  ssh-keygen -q -t ed25519 -f /etc/moterm/hostkey -N ""
  su "$TESTUSER" -c "ssh-keygen -q -t ed25519 -f $HOMEDIR/id_ed25519 -N \"\""
  mkdir -p "$HOMEDIR/.ssh"; cp "$HOMEDIR/id_ed25519.pub" "$HOMEDIR/.ssh/authorized_keys"
  su "$TESTUSER" -c "echo hello > $HOMEDIR/report.txt && mkdir -p $HOMEDIR/rd"
  chown -R "$TESTUSER:$TESTUSER" "$HOMEDIR/.ssh" "$HOMEDIR/id_ed25519"*
  chmod 700 "$HOMEDIR/.ssh"; chmod 600 "$HOMEDIR/.ssh/authorized_keys"
  cat > /etc/moterm/sshd_config <<EOF
Port $PORT
ListenAddress 127.0.0.1
HostKey /etc/moterm/hostkey
PidFile /run/sshd/sshd.pid
AuthorizedKeysFile $HOMEDIR/.ssh/authorized_keys
PasswordAuthentication yes
PubkeyAuthentication yes
UsePAM no
Subsystem sftp /usr/lib/openssh/sftp-server
StrictModes no
EOF
  /usr/sbin/sshd -f /etc/moterm/sshd_config -E /tmp/sshd.log; sleep 1
  cat > /tmp/moterm.lua <<EOF
local moterm = require "moterm"
local config = moterm.config()
config.ui = "neo"
config.font_size = 15.0
config.download_dir = "$HOMEDIR"
config.profiles = {
  { name="local-demo", group="demo", host="127.0.0.1", port=$PORT, user="$TESTUSER",
    auth={ method="publickey", key="$HOMEDIR/id_ed25519" } },
}
config.groups = { { name="demo", label="Demo" } }
return config
EOF
  export HOME=/root DISPLAY=:97
  Xvfb :97 -screen 0 1100x700x24 >/tmp/xvfb.log 2>&1 & sleep 2
  mkdir -p /root/.ssh
  RUST_LOG=warn target/debug/moterm /tmp/moterm.lua >/tmp/moterm.log 2>&1 & GUI=$!
  sleep 3
  WID=$(xdotool search --sync --name moterm | head -1)
  send() { xdotool windowfocus "$WID" 2>/dev/null || true; xdotool "$@"; }
  # サイドバーの local-demo をクリックして接続
  send mousemove --window "$WID" --sync 90 180 click 1; sleep 2
  # TOFU（未知ホスト鍵）を承認
  send key --window "$WID" --clearmodifiers y; sleep 5
  # F3 で SFTP を開く
  send key --window "$WID" --clearmodifiers F3; sleep 3
  import -window root /work/artifacts/neo-sftp-before.png 2>/dev/null || (xwd -root -silent | convert xwd:- /work/artifacts/neo-sftp-before.png)
  if grep -qi "Accepted" /tmp/sshd.log; then echo "=== connect: OK (before kill) ==="; else echo "=== connect: FAILED (before kill) ==="; fi
  # sshd を停止して接続を切る
  pkill -9 sshd || true; sleep 5
  import -window root /work/artifacts/neo-sftp-after.png 2>/dev/null || (xwd -root -silent | convert xwd:- /work/artifacts/neo-sftp-after.png)
  echo "--- moterm.log tail ---"; tail -8 /tmp/moterm.log || true
  kill $GUI 2>/dev/null || true
  echo "=== neo-sftp-disconnect: done ==="
'
