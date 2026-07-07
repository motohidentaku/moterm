#!/usr/bin/env bash
# B7 E2E: window.menu_width/menu_height のモード別ウィンドウサイズ切替を実測する。
#   1) 起動直後（ランチャー）= menu 600x460
#   2) 接続して端末表示     = width/height 1000x640
# xdotool getwindowgeometry の実寸で判定する。
set -eu
cd "$(dirname "$0")/.."
IMAGE=moterm-dev

echo "=== phase1: build moterm (uid $(id -u)) ==="
./docker/run.sh cargo build -p mot-gui 2>&1 | tail -1
mkdir -p artifacts

echo "=== phase2: root container (sshd + Xvfb + xdotool) ==="
docker run --rm \
  -v "$PWD":/work \
  -v motmot-cargo-registry:/usr/local/cargo/registry \
  -w /work \
  "$IMAGE" bash -eu -c '
    TESTUSER=moterm; TESTPW=moterm-test-pw; PORT=2222
    useradd -m -s /bin/bash "$TESTUSER"
    echo "$TESTUSER:$TESTPW" | chpasswd
    HOMEDIR=$(eval echo ~$TESTUSER)
    mkdir -p /run/sshd /etc/moterm
    ssh-keygen -q -t ed25519 -f /etc/moterm/hostkey -N ""
    su "$TESTUSER" -c "ssh-keygen -q -t ed25519 -f $HOMEDIR/id_ed25519 -N \"\""
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
PubkeyAuthentication yes
UsePAM no
StrictModes no
EOF
    /usr/sbin/sshd -f /etc/moterm/sshd_config -E /tmp/sshd.log
    sleep 1

    cat > /tmp/moterm.lua <<EOF
local moterm = require "moterm"
local config = moterm.config()
config.ui = "classic"
config.font_size = 16.0
config.window = { width = 1000, height = 640, menu_width = 600, menu_height = 460 }
config.profiles = {
  { name="local-demo", host="127.0.0.1", port=$PORT, user="$TESTUSER",
    auth={ method="publickey", key="$HOMEDIR/id_ed25519" } },
}
return config
EOF

    export HOME=/root
    export DISPLAY=:99
    Xvfb :99 -screen 0 1400x900x24 >/tmp/xvfb.log 2>&1 &
    sleep 2

    mkdir -p /root/.ssh
    RUST_LOG=warn target/debug/moterm /tmp/moterm.lua >/tmp/moterm.log 2>&1 &
    GUI=$!
    sleep 3

    WID=$(xdotool search --sync --name moterm | head -1)
    xdotool windowfocus "$WID" 2>/dev/null || true
    sleep 1

    geom() { xdotool getwindowgeometry --shell "$WID" | grep -E "^(WIDTH|HEIGHT)=" | tr "\n" " "; }
    G1=$(geom); echo "launcher geometry: $G1"

    # 接続（→ でホストペイン、Enter、TOFU y）→ 端末表示
    xdotool key --window "$WID" --clearmodifiers Right; sleep 1
    xdotool key --window "$WID" --clearmodifiers Return; sleep 3
    xdotool key --window "$WID" --clearmodifiers y; sleep 4
    G2=$(geom); echo "terminal geometry: $G2"

    kill $GUI 2>/dev/null || true

    PASS=1
    echo "$G1" | grep -q "WIDTH=600 "  || { echo "NG: launcher width != 600";  PASS=0; }
    echo "$G1" | grep -q "HEIGHT=460 " || { echo "NG: launcher height != 460"; PASS=0; }
    echo "$G2" | grep -q "WIDTH=1000 " || { echo "NG: terminal width != 1000"; PASS=0; }
    echo "$G2" | grep -q "HEIGHT=640 " || { echo "NG: terminal height != 640"; PASS=0; }
    if [ "$PASS" = 1 ]; then
      echo "=== menusize-e2e: PASS（menu 600x460 → term 1000x640 の切替を実測） ==="
    else
      echo "=== menusize-e2e: FAIL ==="
      tail -20 /tmp/moterm.log || true
      exit 1
    fi
  '
