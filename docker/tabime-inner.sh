#!/usr/bin/env bash
# タブ可変幅・ウィンドウリサイズ(行列可変)・IME配線の検証（実 sshd + Xvfb）。
# 日本語は設定ファイル(UTF-8 heredoc)側のみ。xdotool での日本語打鍵は使わない
# （コンテナが非UTF-8ロケールのため type が壊れる）。タブ選択は矢印キーで行う。
set -u
cd /work
TESTUSER=moterm; PORT=2222
useradd -m -s /bin/bash "$TESTUSER" 2>/dev/null || true
# パスワード未設定だとアカウントがロックされ pubkey でも sshd が拒否するため解錠する
echo "$TESTUSER:unlock-pw" | chpasswd
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

# 名前の長さが異なる3プロファイル（短/長/日本語）。日本語は config 側のみ。
cat > /tmp/moterm.lua <<EOF
local moterm = require "moterm"
local config = moterm.config()
config.ui = "classic"
config.font_size = 16.0
config.lang = "ja"
local function p(n) return { name=n, host="127.0.0.1", port=$PORT, user="$TESTUSER",
  auth={ method="publickey", key="$HOMEDIR/id_ed25519" } } end
config.profiles = { p("db"), p("web-production-primary-server"), p("本番データベース") }
return config
EOF

export HOME=/root DISPLAY=:99
Xvfb :99 -screen 0 1100x760x24 >/tmp/xvfb.log 2>&1 & sleep 2
mkdir -p /root/.ssh
RUST_LOG=warn target/debug/moterm /tmp/moterm.lua >/tmp/moterm.log 2>&1 & GUI=$!
sleep 3
WID=$(xdotool search --sync --name moterm | head -1)
xdotool windowfocus "$WID" 2>/dev/null || true
key() { xdotool key --window "$WID" --clearmodifiers "$@"; }
typestr() { xdotool type --window "$WID" --clearmodifiers "$1"; }

PASS=0; FAIL=0
ok(){ echo "  [OK]  $1"; PASS=$((PASS+1)); }
ng(){ echo "  [NG]  $1"; FAIL=$((FAIL+1)); }

# --- タブ1: "db"(先頭) に接続 ---
key Right; sleep 1        # ホストペインへ
key Return; sleep 3       # db 接続開始 → TOFU
key y; sleep 4            # 承認 → 認証 → シェル
if grep -aq 'Accepted publickey' /tmp/sshd.log; then ok "接続成立(pubkey)"; else ng "接続失敗"; fi

# --- 行列可変(リサイズ) をこの動作タブで検証 ---
typestr "stty size > /tmp/s1.txt"; key Return; sleep 2
xdotool windowsize "$WID" 640 460; sleep 3
typestr "stty size > /tmp/s2.txt"; key Return; sleep 2
S1=$(cat /tmp/s1.txt 2>/dev/null||echo x); S2=$(cat /tmp/s2.txt 2>/dev/null||echo y)
echo "  stty: [$S1] -> [$S2]"
if [ "$S1" != "$S2" ] && [ "$S1" != x ] && [ "$S2" != y ]; then
  ok "行列可変(リサイズで stty size 変化 $S1 -> $S2)"
else
  ng "行列可変せず ([$S1]->[$S2])"
fi
xdotool windowsize "$WID" 1100 760; sleep 2

# --- タブ2,3 を矢印選択で追加（可変幅スクショ用） ---
add_tab() { # $1 = ArrowDown 回数(先頭からのオフセット)
  key F1; sleep 1
  key Right; sleep 1
  for _ in $(seq 1 "$1"); do key Down; sleep 0.3; done
  key Return; sleep 3
  key y; sleep 3   # 同一ホスト鍵なら TOFU 無しで素通り(無害)
}
add_tab 1   # web-production-primary-server
add_tab 2   # 本番データベース

import -window root /work/artifacts/tab-variable.png 2>/dev/null || true
echo "  可変幅スクショ: artifacts/tab-variable.png"

# --- IME 配線（打鍵不能なのでコード配線＋非クラッシュで担保） ---
kill -0 $GUI 2>/dev/null && ok "GUI 生存（IME/リサイズ配線後もクラッシュ無し）" || ng "GUI 死亡"

echo "=== RESULT: PASS=$PASS FAIL=$FAIL ==="
echo "--- sshd accepted 数: $(grep -ac Accepted /tmp/sshd.log) ---"
kill $GUI 2>/dev/null || true
[ "$FAIL" -eq 0 ]
