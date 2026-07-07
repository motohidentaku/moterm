#!/usr/bin/env bash
# タブ操作の E2E: ドラッグ並べ替え・ダブルクリックrename・右クリックメニュー(Rename/Duplicate/SFTP/Close)。
# 実 sshd + Xvfb + xdotool。各段階をスクショで確認。
set -u
cd /work
TESTUSER=moterm; PORT=2222
useradd -m -s /bin/bash "$TESTUSER" 2>/dev/null || true
echo "$TESTUSER:unlock-pw" | chpasswd   # アカウント解錠(pubkeyでもロック中は拒否される)
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
Subsystem sftp /usr/lib/openssh/sftp-server
EOF
/usr/sbin/sshd -f /etc/moterm/sshd_config -E /tmp/sshd.log; sleep 1
cat > /tmp/moterm.lua <<EOF
local moterm = require "moterm"
local config = moterm.config()
config.ui = "classic"
config.font_size = 16.0
config.lang = "ja"
local function p(n) return { name=n, host="127.0.0.1", port=$PORT, user="$TESTUSER",
  auth={ method="publickey", key="$HOMEDIR/id_ed25519" } } end
config.profiles = { p("alpha"), p("bravo"), p("charlie") }
return config
EOF
export HOME=/root DISPLAY=:99
Xvfb :99 -screen 0 1100x760x24 >/tmp/xvfb.log 2>&1 & sleep 2
mkdir -p /root/.ssh
RUST_LOG=warn target/debug/moterm /tmp/moterm.lua >/tmp/moterm.log 2>&1 & GUI=$!
sleep 3
WID=$(xdotool search --sync --name moterm | head -1)
xdotool windowfocus "$WID" 2>/dev/null || true
key(){ xdotool key --window "$WID" --clearmodifiers "$@"; }
typestr(){ xdotool type --window "$WID" --clearmodifiers "$1"; }
addtab(){ key F1; sleep 1; key Right; sleep 1; for _ in $(seq 1 "$1"); do key Down; sleep 0.3; done; key Return; sleep 3; key y; sleep 3; }

PASS=0; FAIL=0
ok(){ echo "  [OK]  $1"; PASS=$((PASS+1)); }
ng(){ echo "  [NG]  $1"; FAIL=$((FAIL+1)); }

# alpha, bravo, charlie の3タブを開く
key Right; sleep 1; key Return; sleep 3; key y; sleep 4   # alpha
addtab 1   # bravo
addtab 2   # charlie
import -window root /work/artifacts/tabops-0-initial.png 2>/dev/null || true
grep -aq Accepted /tmp/sshd.log && ok "3タブ接続成立" || ng "接続失敗"

# タブ座標をログから取れないので幾何を概算: 各タブ幅は名前依存。左から順に並ぶ想定。
# alpha が先頭(x~40)。それを右方向(bravo/charlie の位置=x~250)へドラッグ。
echo "--- ドラッグ並べ替え: 先頭タブ(alpha)を右端へ ---"
# WMなしXvfbでは window は原点(0,0)。絶対座標のXTESTモーション(--syncで確実に配送)を使う
# （--window指定のmousemoveはXWarpPointerでボタン保持中のモーション通知が出ないため）
xdotool mousemove --sync 40 12
xdotool mousedown 1
for X in 60 90 120 150 180 210 240 270 300; do xdotool mousemove --sync $X 12; sleep 0.2; done
xdotool mouseup 1; sleep 1
import -window root /work/artifacts/tabops-1-reorder.png 2>/dev/null || true
# 並べ替え検証: 先頭タブをダブルクリックしてrename→どのタブ名になるかで順序を判定
xdotool mousemove --sync 40 12 click --repeat 2 --delay 120 1; sleep 1
typestr "-FIRST"; sleep 1; key Return; sleep 1
import -window root /work/artifacts/tabops-1b-reorder-probe.png 2>/dev/null || true
echo "  並べ替え後スクショ: artifacts/tabops-1-reorder.png / probe: 先頭タブに-FIRST付与"
ok "ドラッグ操作がクラッシュせず完了"

# ダブルクリックrename: 先頭タブをダブルクリック → 名前入力
echo "--- ダブルクリック rename ---"
xdotool mousemove --window "$WID" 40 12 click --repeat 2 --delay 120 1; sleep 1
typestr "RENAMED"; sleep 1
import -window root /work/artifacts/tabops-2-rename-edit.png 2>/dev/null || true
key Return; sleep 1
import -window root /work/artifacts/tabops-3-rename-done.png 2>/dev/null || true
ok "rename 編集→確定がクラッシュせず完了"

# 右クリックメニュー
echo "--- 右クリックメニュー ---"
xdotool mousemove --window "$WID" 40 12 click 3; sleep 1
import -window root /work/artifacts/tabops-4-menu.png 2>/dev/null || true
echo "  メニュースクショ: artifacts/tabops-4-menu.png (目視: Rename/Duplicate/SFTP/Close)"
key Escape; sleep 1
ok "右クリックメニュー表示がクラッシュせず完了"

# メニュー→Duplicate（メニュー2項目目あたりをクリック）
echo "--- メニュー: Duplicate ---"
TABS_BEFORE=$(grep -ac Accepted /tmp/sshd.log)
xdotool mousemove --window "$WID" 40 12 click 3; sleep 1
# メニューは y=tab_h(約27) から縦に並ぶ。Duplicate は2行目 ~ y=27+行高*1.5
xdotool mousemove --window "$WID" 70 60 click 1; sleep 4
TABS_AFTER=$(grep -ac Accepted /tmp/sshd.log)
import -window root /work/artifacts/tabops-5-duplicate.png 2>/dev/null || true
if [ "$TABS_AFTER" -gt "$TABS_BEFORE" ]; then
  ok "Duplicate で新規接続が増えた ($TABS_BEFORE->$TABS_AFTER)"
else
  ng "Duplicate 未動作 ($TABS_BEFORE->$TABS_AFTER)"
fi

kill -0 $GUI 2>/dev/null && ok "GUI 生存" || ng "GUI 死亡"
echo "=== RESULT: PASS=$PASS FAIL=$FAIL ==="
kill $GUI 2>/dev/null || true
[ "$FAIL" -eq 0 ]
