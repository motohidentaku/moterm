#!/usr/bin/env bash
# A2 マウス転送の単独診断: cat -v をフォアグラウンドで走らせクリック → 画面に ^[[<...M が出るか
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
PidFile /run/sshd/sshd.pid
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
config.font_size = 16.0
config.profiles = {
  { name="demo", host="127.0.0.1", port=$PORT, user="$TESTUSER",
    auth={ method="publickey", key="$HOMEDIR/id_ed25519" } },
}
return config
EOF
export HOME=/root DISPLAY=:99
Xvfb :99 -screen 0 1100x760x24 >/tmp/xvfb.log 2>&1 & sleep 2
mkdir -p /root/.ssh
RUST_LOG=debug,mot_gui=debug target/debug/moterm /tmp/moterm.lua >/tmp/moterm.log 2>&1 & GUI=$!
sleep 3
WID=$(xdotool search --sync --name moterm | head -1)
xdotool windowactivate "$WID" 2>/dev/null || true
xdotool key --window "$WID" --clearmodifiers Right; sleep 1
xdotool key --window "$WID" --clearmodifiers Return; sleep 3
xdotool key --window "$WID" --clearmodifiers y; sleep 4
# マウス有効化 + cat -v をフォアグラウンドで（画面に受信バイトを可視化）
xdotool type --window "$WID" --clearmodifiers 'printf "\033[?1000h\033[?1006h"; cat -v'
xdotool key --window "$WID" --clearmodifiers Return; sleep 2
xdotool mousemove --window "$WID" 300 250 click 1; sleep 1
xdotool mousemove --window "$WID" 350 300 click 1; sleep 2
import -window root /work/artifacts/a2-probe.png 2>/dev/null || true
echo "=== moterm.log (mouse関連) ==="
grep -iE 'mouse|encode|report' /tmp/moterm.log | head || echo "(no mouse log)"
kill $GUI 2>/dev/null || true
echo "probe done"
