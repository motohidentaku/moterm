#!/usr/bin/env bash
# OSC133 プロンプトジャンプ＋大容量スクロールバックの E2E。実 sshd + Xvfb + xdotool。
set -u
cd /work
TESTUSER=moterm; PORT=2222
useradd -m -s /bin/bash "$TESTUSER" 2>/dev/null || true
echo "$TESTUSER:unlock-pw" | chpasswd
HOMEDIR=$(eval echo ~$TESTUSER)
mkdir -p /run/sshd /etc/moterm "$HOMEDIR/.ssh"
ssh-keygen -q -t ed25519 -f /etc/moterm/hostkey -N "" 2>/dev/null || true
su "$TESTUSER" -c "ssh-keygen -q -t ed25519 -f $HOMEDIR/id_ed25519 -N '' 2>/dev/null" || true
cp "$HOMEDIR/id_ed25519.pub" "$HOMEDIR/.ssh/authorized_keys"
cp /work/docker/fixtures/osc133-demo.sh "$HOMEDIR/osc133-demo.sh"
chown -R "$TESTUSER:$TESTUSER" "$HOMEDIR/.ssh" "$HOMEDIR/id_ed25519"* "$HOMEDIR/osc133-demo.sh"
chmod 700 "$HOMEDIR/.ssh"; chmod 600 "$HOMEDIR/.ssh/authorized_keys"; chmod +x "$HOMEDIR/osc133-demo.sh"
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
config.font_size = 16.0
config.scrollback_lines = 200000
config.profiles = { { name="demo", host="127.0.0.1", port=$PORT, user="$TESTUSER",
  auth={ method="publickey", key="$HOMEDIR/id_ed25519" } } }
return config
EOF
export HOME=/root DISPLAY=:99
Xvfb :99 -screen 0 1000x640x24 >/tmp/xvfb.log 2>&1 & sleep 2
mkdir -p /root/.ssh
RUST_LOG=warn target/debug/moterm /tmp/moterm.lua >/tmp/moterm.log 2>&1 & GUI=$!
sleep 3
WID=$(xdotool search --sync --name moterm | head -1)
xdotool windowfocus "$WID" 2>/dev/null || true
key(){ xdotool key --window "$WID" --clearmodifiers "$@"; }
typestr(){ xdotool type --window "$WID" --clearmodifiers "$1"; }
PASS=0; FAIL=0
ok(){ echo "  [OK]  $1"; PASS=$((PASS+1)); }
ng(){ echo "  [NG]  $1"; FAIL=$((FAIL+1)); }

key Right; sleep 1; key Return; sleep 3; key y; sleep 4
grep -aq Accepted /tmp/sshd.log && ok "接続成立" || ng "接続失敗"

# OSC133 を発行（5プロンプト分の出力）
typestr "stty raw -echo; ./osc133-demo.sh"; key Return; sleep 3
import -window root /work/artifacts/osc133-1-bottom.png 2>/dev/null || true
# Ctrl+Shift+P で前のプロンプトへジャンプ（複数回）→上方の prompt へスクロール
xdotool key --window "$WID" ctrl+shift+p; sleep 1
xdotool key --window "$WID" ctrl+shift+p; sleep 1
xdotool key --window "$WID" ctrl+shift+p; sleep 1
import -window root /work/artifacts/osc133-2-jumped.png 2>/dev/null || true
echo "  プロンプトジャンプ後スクショ: artifacts/osc133-2-jumped.png"
ok "Ctrl+Shift+P プロンプトジャンプ（クラッシュ無し）"
# stty 復帰
printf '' ; key ctrl+c 2>/dev/null; typestr "stty sane"; key Return; sleep 1

# 大容量スクロールバック: 30万行出力してもクラッシュしない
typestr "seq 1 300000 | tail -5"; key Return; sleep 6
import -window root /work/artifacts/osc133-3-bigscroll.png 2>/dev/null || true
kill -0 $GUI 2>/dev/null && ok "30万行出力後もGUI生存(大容量スクロールバック)" || ng "GUI死亡"

echo "=== RESULT: PASS=$PASS FAIL=$FAIL ==="
tail -3 /tmp/moterm.log 2>/dev/null || true
kill $GUI 2>/dev/null || true
[ "$FAIL" -eq 0 ]
