#!/usr/bin/env bash
# B4 E2E: スクロールバーのサムをマウスでドラッグして履歴移動できることを立証する。
#   1) 実 sshd へ接続 → seq 1 400 で画面より多い出力を流す（スクロールバック生成）
#   2) 接続直後の画面（最下部＝末尾 400 付近）をスクショ
#   3) 右端スクロールバー列を上端へドラッグ（mousedown→mousemove→mouseup）
#   4) 履歴上部（先頭 1 付近）を表示した画面をスクショ
# 2枚のスクショを目視で比較して、ドラッグで履歴が移動したことを確認する。
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
config.window = { width = 900, height = 560 }
config.profiles = {
  { name="local-demo", host="127.0.0.1", port=$PORT, user="$TESTUSER",
    auth={ method="publickey", key="$HOMEDIR/id_ed25519" } },
}
return config
EOF

    export HOME=/root
    export DISPLAY=:99
    Xvfb :99 -screen 0 1100x760x24 >/tmp/xvfb.log 2>&1 &
    sleep 2
    mkdir -p /root/.ssh
    RUST_LOG=warn target/debug/moterm /tmp/moterm.lua >/tmp/moterm.log 2>&1 &
    GUI=$!
    sleep 3

    WID=$(xdotool search --sync --name moterm | head -1)
    echo "window id: $WID"
    xdotool windowfocus "$WID" 2>/dev/null || true
    sleep 1
    # 接続（→ ホストペイン, Enter, TOFU y）
    xdotool key --window "$WID" --clearmodifiers Right; sleep 1
    xdotool key --window "$WID" --clearmodifiers Return; sleep 3
    xdotool key --window "$WID" --clearmodifiers y; sleep 4
    # スクロールバックを生成（画面 ~30 行に対し 400 行）
    xdotool type --window "$WID" --clearmodifiers "seq 1 400"
    xdotool key --window "$WID" Return; sleep 3

    import -window root /work/artifacts/scrollbar-bottom.png 2>/dev/null \
      || (xwd -root -silent | convert xwd:- /work/artifacts/scrollbar-bottom.png)

    # ウィンドウの実ジオメトリを取得し、右端スクロールバー列を上端へドラッグ。
    eval $(xdotool getwindowgeometry --shell "$WID")
    RX=$((X + WIDTH - 5))          # 右端から5px（スクロールバー列）
    Y_BOTTOM=$((Y + HEIGHT - 30))  # トラック下部（現在サムがある付近）
    Y_TOP=$((Y + 40))              # トラック上部（履歴の先頭側）
    echo "drag scrollbar: ($RX,$Y_BOTTOM) -> ($RX,$Y_TOP)  (win ${WIDTH}x${HEIGHT}+${X}+${Y})"
    xdotool mousemove --sync $RX $Y_BOTTOM
    xdotool mousedown 1
    for yy in $(seq $Y_BOTTOM -20 $Y_TOP); do xdotool mousemove --sync $RX $yy; done
    xdotool mousemove --sync $RX $Y_TOP
    xdotool mouseup 1
    sleep 2

    import -window root /work/artifacts/scrollbar-top.png 2>/dev/null \
      || (xwd -root -silent | convert xwd:- /work/artifacts/scrollbar-top.png)

    echo "--- moterm.log ---"; tail -10 /tmp/moterm.log || true
    kill $GUI 2>/dev/null || true
    echo "screenshots: artifacts/scrollbar-bottom.png artifacts/scrollbar-top.png"
  '
