#!/usr/bin/env bash
# SSH 実接続 E2E。sshd は実ユーザ認証を要するため root コンテナで実行する。
# ビルドは通常ユーザ(uid 1001)のキャッシュを使い、実行のみ root コンテナで行う。
#   フェーズ1: テストバイナリをビルド（共有キャッシュ利用）
#   フェーズ2: root コンテナで test ユーザ作成 → sshd 起動 → バイナリ実行
set -eu
cd "$(dirname "$0")/.."
IMAGE=moterm-dev

echo "=== phase1: build test binary (uid $(id -u)) ==="
./docker/run.sh cargo test -p mot-ssh --test ssh_e2e --no-run 2>&1 | tail -3

# 最新のテストバイナリを特定
BIN=$(ls -t target/debug/deps/ssh_e2e-* 2>/dev/null | grep -v '\.d$' | head -1)
if [ -z "${BIN:-}" ]; then echo "test binary not found"; exit 1; fi
echo "test binary: $BIN"

# AppArmor 適用不能な環境（QNAP NAS 等）と、sshd 特権分離ユーザ欠落への対処。
# 詳細は docker/lib.sh。
# shellcheck source=docker/lib.sh
. "$(dirname "$0")/lib.sh"
detect_secopt "$IMAGE"

echo "=== phase2: root container (create user + sshd + run) ==="
docker run --rm \
  "${SECOPT[@]+"${SECOPT[@]}"}" \
  -v "$PWD":/work \
  -v motmot-cargo-registry:/usr/local/cargo/registry \
  -w /work \
  "$IMAGE" bash -eu -c '
    '"$ENSURE_SSHD_USER"'
    TESTUSER=moterm
    TESTPW=moterm-test-pw
    PORT=2222
    useradd -m -s /bin/bash "$TESTUSER"
    echo "$TESTUSER:$TESTPW" | chpasswd
    HOMEDIR=$(eval echo ~$TESTUSER)

    # 使い捨てホスト鍵と test ユーザ鍵
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

    export MOTERM_SSH_PORT=$PORT
    export MOTERM_SSH_USER=$TESTUSER
    export MOTERM_SSH_KEY=$HOMEDIR/id_ed25519
    export MOTERM_SSH_PASSWORD=$TESTPW
    export MOTERM_LFWD_PORT=15422
    export MOTERM_E2E_DIR=$HOMEDIR
    export HOME=/root

    echo "--- sshd log ---"; cat /tmp/sshd.log || true
    echo "--- running e2e ---"
    '"$BIN"' --nocapture --test-threads=1
  '
