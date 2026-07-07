#!/usr/bin/env bash
# GAP A1/A2/A4/A6/A7 のフルスタック E2E。実処理は docker/gap-inner.sh（コンテナ内 root で実行）。
# ビルドは uid 1001、実行は root コンテナ（実ユーザ sshd 認証と Xvfb/xdotool のため）。
set -eu
cd "$(dirname "$0")/.."
IMAGE=moterm-dev

echo "=== phase1: build moterm (uid $(id -u)) ==="
./docker/run.sh cargo build -p mot-gui 2>&1 | tail -1
mkdir -p artifacts

echo "=== phase2: root container (docker/gap-inner.sh) ==="
docker run --rm \
  -v "$PWD":/work \
  -v motmot-cargo-registry:/usr/local/cargo/registry \
  -w /work \
  "$IMAGE" bash /work/docker/gap-inner.sh
