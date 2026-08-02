#!/usr/bin/env bash
# Docker 内でコマンドを実行する薄いラッパ。
#   docker/run.sh cargo test --workspace
# cargo レジストリは named volume で永続化、target/ はプロジェクト直下（gitignore 済み）。
set -eu
cd "$(dirname "$0")/.."

IMAGE=moterm-dev
# イメージが無ければ自動ビルド
docker image inspect "$IMAGE" >/dev/null 2>&1 || docker build -t "$IMAGE" -f docker/dev.Dockerfile docker

# AppArmor 適用不能な環境の判定（詳細は docker/lib.sh）。
# shellcheck source=docker/lib.sh
. "$(dirname "$0")/lib.sh"
detect_secopt "$IMAGE"

# cargo レジストリの named volume は初回作成時 root 所有になり、下の -u $(id -u) では
# 書けず cargo が Permission denied で落ちる。作成が必要なときだけ作って所有権を
# ホスト uid に合わせる（既存ボリュームには触らない＝毎回のオーバーヘッド無し）。
VOL=motmot-cargo-registry
if ! docker volume inspect "$VOL" >/dev/null 2>&1; then
  docker volume create "$VOL" >/dev/null
  docker run --rm "${SECOPT[@]+"${SECOPT[@]}"}" -u 0:0 -v "$VOL":/reg "$IMAGE" chown "$(id -u)":"$(id -g)" /reg
fi

exec docker run --rm \
  "${SECOPT[@]+"${SECOPT[@]}"}" \
  -v "$PWD":/work \
  -v motmot-cargo-registry:/usr/local/cargo/registry \
  -e CARGO_TERM_COLOR=never \
  -u "$(id -u)":"$(id -g)" \
  -e CARGO_HOME=/usr/local/cargo \
  -e HOME=/tmp \
  "$IMAGE" "$@"
