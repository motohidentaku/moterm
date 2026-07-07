#!/usr/bin/env bash
# ランチャーのマウス操作 E2E: GUI を Xvfb で起動し、xdotool の
#   クリック（グループ選択）→ ホスト行ダブルクリック（接続）
# だけで実 sshd へ接続し、シェルで touch したマーカーファイルで合否判定する。
# キーボードは TOFU の y と検証コマンド入力のみに使う（選択・接続はマウスのみ）。
set -eu
cd "$(dirname "$0")/.."
IMAGE=moterm-dev

echo "=== phase1: build moterm (uid $(id -u)) ==="
./docker/run.sh cargo build -p mot-gui 2>&1 | tail -1
mkdir -p artifacts

echo "=== phase2: root container (sshd + Xvfb + xdotool + mouse) ==="
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
PasswordAuthentication yes
PubkeyAuthentication yes
UsePAM no
Subsystem sftp /usr/lib/openssh/sftp-server
AllowTcpForwarding yes
StrictModes no
EOF
    /usr/sbin/sshd -f /etc/moterm/sshd_config -E /tmp/sshd.log
    sleep 1

    cat > /tmp/moterm.lua <<EOF
local moterm = require "moterm"
local config = moterm.config()
config.ui = "classic"
config.font_size = 16.0
config.lang = "ja"
config.profiles = {
  { name="local-demo", group="demo", host="127.0.0.1", port=$PORT, user="$TESTUSER",
    auth={ method="publickey", key="$HOMEDIR/id_ed25519" } },
}
config.groups = { { name="demo", label="デモ環境" } }
return config
EOF

    export HOME=/root
    export DISPLAY=:99
    Xvfb :99 -screen 0 1200x800x24 >/tmp/xvfb.log 2>&1 &
    sleep 2

    mkdir -p /root/.ssh
    RUST_LOG=warn target/debug/moterm /tmp/moterm.lua >/tmp/moterm.log 2>&1 &
    GUI=$!
    sleep 3

    WID=$(xdotool search --sync --name moterm | head -1)
    echo "window id: $WID"
    xdotool windowfocus "$WID" 2>/dev/null || true
    xdotool windowactivate "$WID" 2>/dev/null || true
    sleep 1

    # レイアウト想定（1200x800, font 16px → lh=22..26, pad=8..11）:
    #   グループ行1(デモ環境) ≈ y=80 / ホスト行0 ≈ y=55（どのметрикでも行内に収まる）
    # 1) グループ「デモ環境」をクリックして選択（右ペイン絞り込み）
    xdotool mousemove --window "$WID" --sync 100 80 click 1
    sleep 1
    import -window root /work/artifacts/launcher-mouse-group.png 2>/dev/null || true
    # 2) ホスト行0 をダブルクリック → 接続（Enter 相当）
    xdotool mousemove --window "$WID" --sync 600 55 click --repeat 2 --delay 120 1
    sleep 3
    # TOFU モーダルを y で承認
    xdotool windowfocus "$WID" 2>/dev/null || true
    xdotool key --window "$WID" --clearmodifiers y
    sleep 4
    # シェルが開いたことをマーカーファイルで立証
    xdotool type --window "$WID" --clearmodifiers "touch /tmp/MOUSE_E2E_OK"
    xdotool key --window "$WID" Return
    sleep 2

    import -window root /work/artifacts/launcher-mouse-connected.png 2>/dev/null \
      || (xwd -root -silent | convert xwd:- /work/artifacts/launcher-mouse-connected.png)
    echo "--- moterm.log ---"; tail -10 /tmp/moterm.log || true
    kill $GUI 2>/dev/null || true

    if [ -f /tmp/MOUSE_E2E_OK ]; then
      echo "=== launcher-mouse-e2e: PASS（マウスのみで選択→接続→シェル実行を確認） ==="
    else
      echo "=== launcher-mouse-e2e: FAIL（/tmp/MOUSE_E2E_OK が作成されていない） ==="
      exit 1
    fi
  '
