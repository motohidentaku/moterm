#!/usr/bin/env bash
# フルスタック E2E: GUI(moterm) を Xvfb で起動し、xdotool でランチャー操作 →
# コンテナ内 sshd へ実接続 → シェルにコマンド送信 → 端末画面をスクリーンショット。
# GUI → mot-ssh → sshd → PTY → mot-term → 描画 の全経路を1枚の画像で立証する。
# ビルドは uid 1001、実行は root コンテナ（実ユーザ sshd 認証のため）。
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
PasswordAuthentication yes
PubkeyAuthentication yes
UsePAM no
Subsystem sftp /usr/lib/openssh/sftp-server
AllowTcpForwarding yes
StrictModes no
EOF
    /usr/sbin/sshd -f /etc/moterm/sshd_config -E /tmp/sshd.log
    sleep 1

    # 実接続用のデモ設定（1プロファイル: local-demo → 127.0.0.1:2222 pubkey）
    cat > /tmp/moterm.lua <<EOF
local moterm = require "moterm"
local config = moterm.config()
config.ui = "classic"
config.font_size = 16.0
config.lang = "ja"
config.profiles = {
  { name="local-demo", group="demo", host="127.0.0.1", port=$PORT, user="$TESTUSER",
    auth={ method="publickey", key="$HOMEDIR/id_ed25519" },
    on_connect={ "echo GUI_E2E_CONNECTED_OK" } },
}
config.groups = { { name="demo", label="デモ環境" } }
return config
EOF

    export HOME=/root
    export DISPLAY=:99
    Xvfb :99 -screen 0 1200x800x24 >/tmp/xvfb.log 2>&1 &
    sleep 2

    # GUI 起動（スクショなしの通常モード。known_hosts は $HOME/.ssh）
    mkdir -p /root/.ssh
    RUST_LOG=warn target/debug/moterm /tmp/moterm.lua >/tmp/moterm.log 2>&1 &
    GUI=$!
    sleep 3

    # Xvfb には WM が無く入力フォーカスが定まらないため、ウィンドウを明示 focus する。
    WID=$(xdotool search --sync --name moterm | head -1)
    echo "window id: $WID"
    xdotool windowfocus "$WID" 2>/dev/null || true
    xdotool windowactivate "$WID" 2>/dev/null || true
    sleep 1

    send() { xdotool windowfocus "$WID" 2>/dev/null || true; xdotool "$@"; }
    # 起動時フォーカスは左(グループ)ペイン。→ でホストペインへ移り、Enter で local-demo に接続。
    send key --window "$WID" --clearmodifiers Right; sleep 1
    send key --window "$WID" --clearmodifiers Return; sleep 3   # 接続開始 → TOFU モーダル
    send key --window "$WID" --clearmodifiers y; sleep 4        # TOFU 承認 → 認証 → シェル
    # シェルへコマンド送信
    send type --window "$WID" --clearmodifiers "echo GUI_E2E_SHELL_42"
    send key --window "$WID" Return; sleep 2

    import -window root /work/artifacts/gui-connected.png 2>/dev/null \
      || (xwd -root -silent | convert xwd:- /work/artifacts/gui-connected.png)
    echo "--- moterm.log ---"; tail -20 /tmp/moterm.log || true
    echo "--- sshd.log ---"; tail -5 /tmp/sshd.log || true
    kill $GUI 2>/dev/null || true
    echo "screenshot: artifacts/gui-connected.png"
  '
