#!/usr/bin/env bash
# タブ操作 E2E。本体は docker/tabops-inner.sh（root コンテナ）。
set -eu
cd "$(dirname "$0")/.."
echo "=== phase1: build (uid $(id -u)) ==="
./docker/run.sh cargo build -p mot-gui 2>&1 | tail -1
mkdir -p artifacts
echo "=== phase2: root container ==="
docker run --rm \
  -v "$PWD":/work \
  -v motmot-cargo-registry:/usr/local/cargo/registry \
  -w /work \
  moterm-dev bash /work/docker/tabops-inner.sh
