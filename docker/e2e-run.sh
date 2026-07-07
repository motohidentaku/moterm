#!/usr/bin/env bash
# Docker 内で sshd を起動し、mot-ssh の実接続 E2E テストと GUI(Xvfb) スモークを実行する。
# ホストから: ./docker/run.sh bash docker/e2e-run.sh
set -eu
cd /work

PORT=2222
export MOTERM_SSH_PORT=$PORT
export MOTERM_SSH_USER=$(id -un)
export MOTERM_SSH_KEY=/tmp/moterm-e2e/home/id_ed25519
export MOTERM_SSH_PASSWORD=moterm-test-pw
export MOTERM_LFWD_PORT=15422
export MOTERM_E2E_DIR=/tmp/moterm-e2e/home

# テストユーザにパスワードを設定できない（非 root）ため、パスワード認証テストは
# sshd 側で対応する場合のみ。ここでは pubkey/sftp/forward を主に検証する。
bash docker/e2e-ssh.sh

echo "=== mot-ssh 実接続 E2E ==="
cargo test -p mot-ssh --test ssh_e2e -- --nocapture --test-threads=1

# 後片付け
if [ -f /tmp/moterm-e2e/run/sshd.pid ]; then
  kill "$(cat /tmp/moterm-e2e/run/sshd.pid)" 2>/dev/null || true
fi
echo "=== E2E 完了 ==="
