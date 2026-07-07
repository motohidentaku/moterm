#!/usr/bin/env bash
# R2 検証 E2E: neo で実接続 → ブロードキャスト赤帯(R2a) と 検索本文ハイライト(R2b) を撮る。
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
StrictModes no
EOF
  /usr/sbin/sshd -f /etc/moterm/sshd_config -E /tmp/sshd.log; sleep 1
  cat > /tmp/moterm.lua <<EOF
local moterm = require "moterm"
local config = moterm.config()
config.font_size = 16.0
config.lang = "ja"
config.profiles = {
  { name="local-demo", group="demo", host="127.0.0.1", port=$PORT, user="$TESTUSER",
    auth={ method="publickey", key="$HOMEDIR/id_ed25519" } },
}
config.groups = { { name="demo", label="デモ環境" } }
return config
EOF
  export HOME=/root DISPLAY=:99; mkdir -p /root/.ssh
  Xvfb :99 -screen 0 1200x800x24 >/tmp/xvfb.log 2>&1 & sleep 2
  RUST_LOG=warn target/debug/moterm /tmp/moterm.lua >/tmp/moterm.log 2>&1 & GUI=$!
  sleep 3
  WID=$(xdotool search --sync --name moterm | head -1); echo "window id: $WID"
  send() { xdotool windowfocus "$WID" 2>/dev/null || true; xdotool "$@"; }
  send windowactivate "$WID" 2>/dev/null || true; sleep 1
  # サイドバーの host 行「local-demo」をクリックして接続。
  send mousemove --window "$WID" 90 182 click 1; sleep 3
  send key --window "$WID" y; sleep 4               # TOFU 承認 → 認証 → シェル
  # 検索対象を含む出力を数行流す。
  send type --window "$WID" --clearmodifiers "echo HELLO_R2 && echo find_HELLO_here && ls /"; send key --window "$WID" Return; sleep 2
  import -window root /work/artifacts/r2-connected.png 2>/dev/null || (xwd -root -silent | convert xwd:- /work/artifacts/r2-connected.png)
  # R2a: ブロードキャスト ON → 赤帯。
  send key --window "$WID" --clearmodifiers ctrl+shift+b; sleep 1
  import -window root /work/artifacts/r2-broadcast.png 2>/dev/null || (xwd -root -silent | convert xwd:- /work/artifacts/r2-broadcast.png)
  send key --window "$WID" --clearmodifiers ctrl+shift+b; sleep 1   # OFF に戻す
  # R2b: 検索 → 本文ハイライト。
  send key --window "$WID" --clearmodifiers ctrl+shift+f; sleep 1
  send type --window "$WID" --clearmodifiers "HELLO"; sleep 2
  import -window root /work/artifacts/r2-search.png 2>/dev/null || (xwd -root -silent | convert xwd:- /work/artifacts/r2-search.png)
  echo "--- moterm.log ---"; tail -8 /tmp/moterm.log || true
  kill $GUI 2>/dev/null || true
  echo "screenshots: r2-connected.png / r2-broadcast.png / r2-search.png"
'
