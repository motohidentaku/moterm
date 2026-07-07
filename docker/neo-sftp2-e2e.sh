#!/usr/bin/env bash
# neo 埋め込みSFTP確認: 接続→タブ右クリック→SFTP選択→2ペイン表示→スクショ。
set -eu
cd "$(dirname "$0")/.."
IMAGE=moterm-dev
echo "=== phase1: build ==="; ./docker/run.sh cargo build -p mot-gui 2>&1 | tail -1
mkdir -p artifacts
echo "=== phase2 ==="
docker run --rm -v "$PWD":/work -v motmot-cargo-registry:/usr/local/cargo/registry -w /work "$IMAGE" bash -eu -c '
  TESTUSER=moterm; PORT=2222
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
  cat > /tmp/moterm.lua <<EOF
local moterm = require "moterm"
local config = moterm.config()
config.ui = "neo"
config.font_size = 15.0
config.profiles = {
  { name="local-demo", group="demo", host="127.0.0.1", port=$PORT, user="$TESTUSER",
    auth={ method="publickey", key="$HOMEDIR/id_ed25519" } },
}
config.groups = { { name="demo", label="Demo" } }
return config
EOF
  export HOME=/root DISPLAY=:99
  Xvfb :99 -screen 0 1100x700x24 >/tmp/xvfb.log 2>&1 & sleep 2
  mkdir -p /root/.ssh
  RUST_LOG=warn target/debug/moterm /tmp/moterm.lua >/tmp/moterm.log 2>&1 & GUI=$!
  sleep 3
  WID=$(xdotool search --sync --name moterm | head -1); echo "window: $WID"
  xdotool windowfocus "$WID" 2>/dev/null || true; sleep 1
  # サイドバー: グループ見出し(y~80) の下、ホスト行 local-demo (y~95..)。行クリックで接続。
  import -window root /work/artifacts/neo-sidebar-before.png 2>/dev/null || true
  xdotool mousemove --window "$WID" --sync 90 180 click 1
  sleep 3
  # TOFU モーダルを y で承認
  xdotool windowfocus "$WID" 2>/dev/null || true
  xdotool key --window "$WID" --clearmodifiers y
  sleep 4
  # タブを右クリック → メニューの SFTP（3項目目）を選択
  xdotool mousemove --sync 380 65 click 3
  sleep 1
  # メニュー: Rename/Duplicate/SFTP/Close。SFTP は3行目あたり (y~178)
  xdotool mousemove --sync 360 178 click 1
  sleep 3
  import -window root /work/artifacts/neo-sftp2.png 2>/dev/null \
    || (xwd -root -silent | convert xwd:- /work/artifacts/neo-sftp2.png)
  echo "--- moterm.log ---"; tail -8 /tmp/moterm.log || true
  # 接続できたか: サーバ側に接続ログ（Accepted）が出ているか
  if grep -q "Accepted publickey" /tmp/sshd.log 2>/dev/null; then
    echo "=== neo-sidebar-e2e: PASS（サイドバークリックで実接続 Accepted） ==="
  else
    echo "=== neo-sidebar-e2e: FAIL（sshd に Accepted が無い） ==="; tail -20 /tmp/sshd.log || true; exit 1
  fi
  kill $GUI 2>/dev/null || true
'
