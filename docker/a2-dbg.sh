#!/usr/bin/env bash
set -u
cd /work
TESTUSER=moterm; PORT=2222
useradd -m -s /bin/bash "$TESTUSER" 2>/dev/null || true
HOMEDIR=$(eval echo ~$TESTUSER)
mkdir -p /run/sshd /etc/moterm "$HOMEDIR/.ssh"
ssh-keygen -q -t ed25519 -f /etc/moterm/hostkey -N "" 2>/dev/null || true
su "$TESTUSER" -c "ssh-keygen -q -t ed25519 -f $HOMEDIR/id_ed25519 -N '' 2>/dev/null" || true
cp "$HOMEDIR/id_ed25519.pub" "$HOMEDIR/.ssh/authorized_keys"
chown -R "$TESTUSER:$TESTUSER" "$HOMEDIR/.ssh" "$HOMEDIR/id_ed25519"*
chmod 700 "$HOMEDIR/.ssh"; chmod 600 "$HOMEDIR/.ssh/authorized_keys"
cat > /etc/moterm/sshd_config <<EOF
Port $PORT
ListenAddress 127.0.0.1
HostKey /etc/moterm/hostkey
AuthorizedKeysFile $HOMEDIR/.ssh/authorized_keys
PubkeyAuthentication yes
UsePAM no
StrictModes no
EOF
/usr/sbin/sshd -f /etc/moterm/sshd_config -E /tmp/sshd.log; sleep 1
cat > /tmp/moterm.lua <<EOF
local moterm = require "moterm"
local config = moterm.config()
config.ui = "classic"
config.profiles = { { name="demo", host="127.0.0.1", port=$PORT, user="$TESTUSER",
  auth={ method="publickey", key="$HOMEDIR/id_ed25519" } } }
return config
EOF
export HOME=/root DISPLAY=:99
Xvfb :99 -screen 0 1100x760x24 >/tmp/xvfb.log 2>&1 & sleep 2
mkdir -p /root/.ssh
target/debug/moterm /tmp/moterm.lua >/tmp/moterm.log 2>&1 & GUI=$!
sleep 3
WID=$(xdotool search --sync --name moterm | head -1)
xdotool windowfocus "$WID" 2>/dev/null || true
xdotool key --window "$WID" --clearmodifiers Right; sleep 1
xdotool key --window "$WID" --clearmodifiers Return; sleep 3
xdotool key --window "$WID" --clearmodifiers y; sleep 4
# 接続できたか確認
echo "=== connected? (プロンプト表示) ==="
grep -aq 'Accepted publickey' /tmp/sshd.log && echo "sshd: accepted" || echo "sshd: NOT accepted"
# マウス有効化してクリック
xdotool type --window "$WID" --clearmodifiers 'printf "\033[?1000h\033[?1006h"; timeout 6 cat > /tmp/m.bin'
xdotool key --window "$WID" --clearmodifiers Return; sleep 2
xdotool mousemove --window "$WID" 300 250 click 1; sleep 1
xdotool mousemove --window "$WID" 350 300 click 1; sleep 3
echo "=== A2DBG (クリック時のペイン/mode) ==="
grep 'A2DBG' /tmp/moterm.log | tail -5 || echo "(A2DBG なし)"
echo "=== captured mouse bytes ==="
od -An -tx1 /tmp/m.bin 2>/dev/null | tr -d '\n ' | head -c 80; echo
echo "=== moterm.log tail ==="
tail -5 /tmp/moterm.log
kill $GUI 2>/dev/null || true
