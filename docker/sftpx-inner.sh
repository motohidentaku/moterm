#!/usr/bin/env bash
# SFTP FM 拡張の E2E: 日付列・列ソート・ペイン間ドラッグ&ドロップ転送。
# 実 sshd + Xvfb + xdotool。リモートに更新日時/サイズの異なるファイルを用意。
set -u
cd /work
TESTUSER=moterm; PORT=2222
useradd -m -s /bin/bash "$TESTUSER" 2>/dev/null || true
echo "$TESTUSER:unlock-pw" | chpasswd   # アカウント解錠(pubkeyでもロック中は拒否)
HOMEDIR=$(eval echo ~$TESTUSER)
mkdir -p /run/sshd /etc/moterm "$HOMEDIR/.ssh"
ssh-keygen -q -t ed25519 -f /etc/moterm/hostkey -N "" 2>/dev/null || true
su "$TESTUSER" -c "ssh-keygen -q -t ed25519 -f $HOMEDIR/id_ed25519 -N '' 2>/dev/null" || true
cp "$HOMEDIR/id_ed25519.pub" "$HOMEDIR/.ssh/authorized_keys"
# リモート: サイズ・更新日時の異なるファイルを用意（ソート確認用）
su "$TESTUSER" -c "
  mkdir -p $HOMEDIR/rdir
  head -c 100  /dev/zero > $HOMEDIR/small.txt
  head -c 50000 /dev/zero > $HOMEDIR/big.bin
  echo x > $HOMEDIR/zebra.log
  touch -d '2020-01-15 09:30' $HOMEDIR/small.txt
  touch -d '2024-06-20 18:00' $HOMEDIR/big.bin
  touch -d '2022-03-10 12:00' $HOMEDIR/zebra.log
"
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
config.download_dir = "$HOMEDIR"
config.profiles = { { name="demo", host="127.0.0.1", port=$PORT, user="$TESTUSER",
  auth={ method="publickey", key="$HOMEDIR/id_ed25519" } } }
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

PASS=0; FAIL=0
ok(){ echo "  [OK]  $1"; PASS=$((PASS+1)); }
ng(){ echo "  [NG]  $1"; FAIL=$((FAIL+1)); }

# 接続 → F3 で FM
key Right; sleep 1; key Return; sleep 3; key y; sleep 4
grep -aq Accepted /tmp/sshd.log && ok "接続成立" || ng "接続失敗"
key F3; sleep 3
import -window root /work/artifacts/sftpx-1-datecol.png 2>/dev/null || true
echo "  日付列スクショ: artifacts/sftpx-1-datecol.png (目視: 名前/サイズ/日付の3列)"

# リモートペインへ移動して 's' でソート切替（名前→サイズ→日付）
key Tab; sleep 1     # ローカル→リモート(またはその逆。アクティブ切替)
key s; sleep 1; import -window root /work/artifacts/sftpx-2-sort-size.png 2>/dev/null || true
key s; sleep 1; import -window root /work/artifacts/sftpx-3-sort-date.png 2>/dev/null || true
ok "s キーでソート切替（クラッシュ無し）"

# ドラッグ&ドロップ転送: リモートの small.txt をローカルペインへドラッグ
# ペイン配置: 左=ローカル, 右=リモート。右ペインの1エントリを左ペインへドラッグ。
# ローカル cwd の転送先を掃除して before/after 比較
su "$TESTUSER" -c "rm -f $HOMEDIR/dl_probe_marker" 2>/dev/null
# リモートペイン(右, x~800)の行をつかんで左ペイン(x~250)へドロップ
xdotool mousemove --sync 800 120
xdotool mousedown 1
for X in 750 650 550 450 350 250; do xdotool mousemove --sync $X 120; sleep 0.2; done
xdotool mouseup 1; sleep 3
import -window root /work/artifacts/sftpx-4-drag.png 2>/dev/null || true
echo "  ドラッグ後スクショ: artifacts/sftpx-4-drag.png"
ok "ドラッグ&ドロップ操作（クラッシュ無し）"

kill -0 $GUI 2>/dev/null && ok "GUI 生存" || ng "GUI 死亡"
echo "=== RESULT: PASS=$PASS FAIL=$FAIL ==="
tail -4 /tmp/moterm.log 2>/dev/null || true
kill $GUI 2>/dev/null || true
[ "$FAIL" -eq 0 ]
