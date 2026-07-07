#!/usr/bin/env bash
# SFTP フルスタック E2E: GUI で実 sshd に接続 → F3 で2ペインファイルマネージャを開き、
# リモート/ローカルのファイル一覧が描画されるところをスクリーンショットで立証する。
# ビルドは uid 1001、実行は root コンテナ（実ユーザ sshd 認証のため）。
set -eu
cd "$(dirname "$0")/.."
IMAGE=moterm-dev

echo "=== phase1: build moterm (uid $(id -u)) ==="
./docker/run.sh cargo build -p mot-gui 2>&1 | tail -1
mkdir -p artifacts

echo "=== phase2: root container (sshd + Xvfb + xdotool + F3) ==="
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
    # リモート側に一覧表示用のテスト内容を用意
    su "$TESTUSER" -c "mkdir -p $HOMEDIR/remote_dir && echo hello > $HOMEDIR/report.txt && echo x > $HOMEDIR/data.bin && echo y > $HOMEDIR/remote_dir/inner.txt"
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
config.download_dir = "$HOMEDIR"
config.profiles = {
  { name="local-demo", group="demo", host="127.0.0.1", port=$PORT, user="$TESTUSER",
    auth={ method="publickey", key="$HOMEDIR/id_ed25519" } },
}
config.groups = { { name="demo", label="デモ環境" } }
return config
EOF

    export HOME=/root DISPLAY=:99
    Xvfb :99 -screen 0 1000x640x24 >/tmp/xvfb.log 2>&1 &
    sleep 2
    mkdir -p /root/.ssh
    RUST_LOG=warn target/debug/moterm /tmp/moterm.lua >/tmp/moterm.log 2>&1 &
    GUI=$!
    sleep 3
    WID=$(xdotool search --sync --name moterm | head -1)
    send() { xdotool windowfocus "$WID" 2>/dev/null || true; xdotool "$@"; }
    # 接続（→ でホストペイン、Enter で接続、y で TOFU）
    send key --window "$WID" --clearmodifiers Right; sleep 1
    send key --window "$WID" --clearmodifiers Return; sleep 3
    send key --window "$WID" --clearmodifiers y; sleep 4
    # F3 で SFTP ファイルマネージャを開く
    send key --window "$WID" --clearmodifiers F3; sleep 3

    import -window root /work/artifacts/gui-sftp.png 2>/dev/null \
      || (xwd -root -silent | convert xwd:- /work/artifacts/gui-sftp.png)
    echo "--- moterm.log ---"; tail -15 /tmp/moterm.log || true
    kill $GUI 2>/dev/null || true
    echo "screenshot: artifacts/gui-sftp.png"
  '
