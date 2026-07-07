#!/usr/bin/env bash
# コンテナ内で使い捨ての sshd を起動し、mot-ssh の統合テストを実行する。
# ローカルループバックのみ・使い捨てホスト鍵・専用ポートで、外部に一切開かない。
set -eu

WORK=/tmp/moterm-e2e
rm -rf "$WORK"
mkdir -p "$WORK"/{etc,run,home}

# 使い捨てホスト鍵
ssh-keygen -q -t ed25519 -f "$WORK/etc/ssh_host_ed25519_key" -N ''
ssh-keygen -q -t rsa -b 2048 -f "$WORK/etc/ssh_host_rsa_key" -N ''

# テストユーザ鍵（publickey 認証用）
ssh-keygen -q -t ed25519 -f "$WORK/home/id_ed25519" -N ''
mkdir -p "$WORK/home/.ssh"
cp "$WORK/home/id_ed25519.pub" "$WORK/home/.ssh/authorized_keys"
chmod 700 "$WORK/home/.ssh"
chmod 600 "$WORK/home/.ssh/authorized_keys"

USER_NAME=$(id -un)
PORT=${MOTERM_SSH_PORT:-2222}

cat > "$WORK/etc/sshd_config" <<EOF
Port $PORT
ListenAddress 127.0.0.1
HostKey $WORK/etc/ssh_host_ed25519_key
HostKey $WORK/etc/ssh_host_rsa_key
PidFile $WORK/run/sshd.pid
AuthorizedKeysFile $WORK/home/.ssh/authorized_keys
PasswordAuthentication yes
PubkeyAuthentication yes
UsePAM no
Subsystem sftp /usr/lib/openssh/sftp-server
AllowTcpForwarding yes
StrictModes no
EOF

/usr/sbin/sshd -f "$WORK/etc/sshd_config" -E "$WORK/run/sshd.log"
echo "sshd started on 127.0.0.1:$PORT (user=$USER_NAME, key=$WORK/home/id_ed25519)"
echo "$PORT" > "$WORK/port"
