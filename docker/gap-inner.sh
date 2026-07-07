#!/usr/bin/env bash
# gui-gap-e2e.sh からコンテナ内(root)で実行される本体。
# GAP A1(リサイズ)/A2(マウス転送)/A4(ボールト,exe隣保存)/A6(URL)/A7(検索) を実 sshd で検証。
set -u
cd /work

TESTUSER=moterm; TESTPW=moterm-test-pw; PORT=2222
useradd -m -s /bin/bash "$TESTUSER" 2>/dev/null || true
echo "$TESTUSER:$TESTPW" | chpasswd
HOMEDIR=$(eval echo ~$TESTUSER)
mkdir -p /run/sshd /etc/moterm
ssh-keygen -q -t ed25519 -f /etc/moterm/hostkey -N "" 2>/dev/null || true
su "$TESTUSER" -c "ssh-keygen -q -t ed25519 -f $HOMEDIR/id_ed25519 -N '' 2>/dev/null" || true
mkdir -p "$HOMEDIR/.ssh"
cp "$HOMEDIR/id_ed25519.pub" "$HOMEDIR/.ssh/authorized_keys"
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
AllowTcpForwarding yes
StrictModes no
EOF
/usr/sbin/sshd -f /etc/moterm/sshd_config -E /tmp/sshd.log
sleep 1

# A6: xdg-open スタブ（呼ばれた URL を記録）
cat > /usr/local/bin/xdg-open <<'EOF'
#!/bin/bash
echo "$1" >> /tmp/xdg-open.log
EOF
chmod +x /usr/local/bin/xdg-open

cat > /tmp/moterm.lua <<EOF
local moterm = require "moterm"
local config = moterm.config()
config.ui = "classic"
config.font_size = 16.0
config.lang = "ja"
config.profiles = {
  { name="demo", host="127.0.0.1", port=$PORT, user="$TESTUSER",
    auth={ method="publickey", key="$HOMEDIR/id_ed25519" } },
  { name="pw-demo", host="127.0.0.1", port=$PORT, user="$TESTUSER",
    auth={ method="password", save=true } },
}
return config
EOF

export HOME=/root DISPLAY=:99
Xvfb :99 -screen 0 1100x760x24 >/tmp/xvfb.log 2>&1 &
sleep 2
mkdir -p /root/.ssh
rm -f target/debug/moterm-secrets.enc /tmp/xdg-open.log

PASS=0; FAIL=0
ok() { echo "  [OK]  $1"; PASS=$((PASS+1)); }
ng() { echo "  [NG]  $1"; FAIL=$((FAIL+1)); }

WID=""
start_gui() {
  RUST_LOG=warn target/debug/moterm /tmp/moterm.lua >>/tmp/moterm.log 2>&1 &
  GUI=$!
  sleep 3
  WID=$(xdotool search --sync --name moterm | head -1)
  xdotool windowfocus "$WID" 2>/dev/null || true
}
key() { xdotool key --window "$WID" --clearmodifiers "$@"; }
typestr() { xdotool type --window "$WID" --clearmodifiers "$1"; }

# ===== セッション1: pubkey 接続で A1/A2/A6/A7 =====
start_gui
key Right; sleep 1
key Return; sleep 3
key y; sleep 4

echo "--- A1: 動的リサイズ ---"
typestr "stty size > /tmp/size1.txt"; key Return; sleep 2
xdotool windowsize "$WID" 700 500; sleep 3
typestr "stty size > /tmp/size2.txt"; key Return; sleep 2
S1=$(cat /tmp/size1.txt 2>/dev/null || echo none)
S2=$(cat /tmp/size2.txt 2>/dev/null || echo none)
echo "  size1='$S1' size2='$S2'"
if [ "$S1" != "none" ] && [ "$S2" != "none" ] && [ "$S1" != "$S2" ]; then
  ok "A1 リサイズで stty size 変化 ($S1 -> $S2)"
else
  ng "A1 リサイズ未反映 ($S1 -> $S2)"
fi
xdotool windowsize "$WID" 1100 760; sleep 3

xdotool windowactivate "$WID" 2>/dev/null || true

echo "--- A2: マウスプロトコル転送 ---"
# 本物のマウス対応アプリと同じく PTY を raw にして取得（cooked だと改行なし列が行バッファに滞留するため）
typestr 'printf "\033[?1000h\033[?1006h"; stty raw -echo; timeout 6 head -c 40 > /tmp/mouse.bin; stty sane; printf "\033[?1000l\033[?1006l"'
key Return; sleep 2
for pt in "300 250" "250 200" "400 300"; do
  xdotool mousemove --window "$WID" $pt click 1; sleep 1
done
sleep 4
echo "  captured bytes: $(od -An -tx1 /tmp/mouse.bin 2>/dev/null | tr -d '\n ' | head -c 60)"
# SGR マウス = ESC(1b) [(5b) <(3c)
if od -An -tx1 /tmp/mouse.bin 2>/dev/null | tr -d '\n ' | grep -q '1b5b3c'; then
  ok "A2 SGR マウスシーケンス受信"
else
  ng "A2 マウスシーケンス未受信"
fi

echo "--- A6: Ctrl+クリック URL ---"
# clear 後の row0 に URL 単体を出す（列0起点で当てやすくする）
rm -f /tmp/xdg-open.log
FOUND=""
# ヘッドレス XTEST は取りこぼしがあるため、URL 再表示＋密な掃引を数回繰り返す
for attempt in 1 2 3; do
  typestr "clear; printf 'https://example.com/page42\\n'"; key Return; sleep 2
  xdotool mousemove --window "$WID" 500 400; sleep 1
  for YY in 34 38 42 46 50; do
    for XX in 30 50 70 90 110 130 150 170 190; do
      xdotool keydown --window "$WID" ctrl
      xdotool mousemove --window "$WID" $XX $YY click 1
      xdotool keyup --window "$WID" ctrl
      sleep 0.25
      [ -s /tmp/xdg-open.log ] && { FOUND=yes; break 3; }
    done
  done
done
if [ -n "$FOUND" ] && grep -q "example.com/page42" /tmp/xdg-open.log; then
  ok "A6 xdg-open に URL 受渡し [$(head -1 /tmp/xdg-open.log)]"
else
  ng "A6 URL オープン未動作 [log: $(cat /tmp/xdg-open.log 2>/dev/null | head -c 60)]"
fi

echo "--- A7: スクロールバック検索 ---"
typestr 'clear; for i in 1 2 3; do echo needle-line-$i extra; echo filler-row; done'; key Return; sleep 2
xdotool key --window "$WID" ctrl+shift+f; sleep 1
typestr "needle"; sleep 2
import -window root /work/artifacts/gap-search.png 2>/dev/null || true
key Escape; sleep 1
key F4; sleep 2

# ===== A4: ボールト初回（マスター新規+パスワード→保存） =====
# ホスト鍵は直前の pubkey 接続で学習済み＝pw-demo では TOFU は出ない（y を送らない）。
echo "--- A4: ボールト初回 ---"
typestr "pw-demo"; sleep 1; key Return; sleep 5   # 接続開始 → マスター新規設定モーダル
typestr "master-secret-1"; sleep 1; key Return; sleep 3   # 新規マスター設定
typestr "$TESTPW"; sleep 1; key Return; sleep 4           # パスワード入力 → 認証成功で保存
import -window root /work/artifacts/gap-vault1.png 2>/dev/null || true
if [ -f target/debug/moterm-secrets.enc ]; then
  ok "A4 ボールトが exe と同じ場所に生成 (target/debug/moterm-secrets.enc)"
else
  ng "A4 ボールト未生成"
fi
grep -aq "Accepted password for $TESTUSER" /tmp/sshd.log && ok "A4 初回パスワード認証成立" || ng "A4 初回パスワード認証不明"
kill $GUI 2>/dev/null || true; sleep 2

# ===== A4: 2回目（マスターのみで保存パスワード自動使用） =====
echo "--- A4: ボールト2回目 ---"
: > /tmp/moterm.log
start_gui
typestr "pw-demo"; sleep 1; key Return; sleep 5   # 接続開始 → マスターモーダル出現待ち（延長）
typestr "master-secret-1"; sleep 1; key Return; sleep 6   # マスター入力のみで接続されるはず
typestr "echo VAULT_AUTO_OK"; key Return; sleep 2
import -window root /work/artifacts/gap-vault2.png 2>/dev/null || true
PW_COUNT=$(grep -ac "Accepted password for $TESTUSER" /tmp/sshd.log)
if [ "${PW_COUNT:-0}" -ge 2 ]; then
  ok "A4 2回目もパスワード認証成立（保存自動使用 accepted=$PW_COUNT）"
else
  ng "A4 2回目接続不明 (accepted=${PW_COUNT:-0})"
fi
kill $GUI 2>/dev/null || true

echo
echo "=== RESULT: PASS=$PASS FAIL=$FAIL ==="
tail -6 /tmp/moterm.log 2>/dev/null || true
[ "$FAIL" -eq 0 ]
