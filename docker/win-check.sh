#!/usr/bin/env bash
# Windows ターゲットの型チェック（./check win から呼ばれる）。
#
# `#[cfg(windows)]` のコードは Linux 上の lint/test では一切コンパイルされないため、
# クロスコンパイルの cargo check で最低限の担保を取る。実行時の挙動までは見られない
# （コンソール窓の有無などは実機か GitHub Actions の windows-latest ビルド頼み）。
set -eu
cd "$(dirname "$0")/.."

BASE=moterm-dev
IMAGE=moterm-win
TARGET=x86_64-pc-windows-gnu
# shellcheck source=docker/lib.sh
. "$(dirname "$0")/lib.sh"

docker image inspect "$BASE" >/dev/null 2>&1 \
  || docker build -t "$BASE" -f docker/dev.Dockerfile docker

# moterm-dev にクロスビルド用ツールを足したイメージを1度だけ作る。
#   - mingw-w64: mlua が vendored Lua を C でビルドするのに要る
#   - nasm: aws-lc-sys（russh の依存）が Windows ターゲットでアセンブラを要求する
# docker build ではなく run + commit で作るのは、AppArmor プロファイルを適用できない
# 環境（notes/NOTES.md 参照）で build にだけ回避フラグを渡せないため。手順はこの1本に
# 寄せてある（どの環境でも同じ経路で作られる）。
if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
  echo "== $IMAGE を作成中（初回のみ、数分かかります） =="
  detect_secopt "$BASE"
  cid=$(docker run -d "${SECOPT[@]+"${SECOPT[@]}"}" -u 0:0 "$BASE" bash -c '
    apt-get update -qq
    # ベースイメージの openssh-server が half-configured のまま残っている環境では
    # apt が非ゼロで終わるが、ここで入れるパッケージ自体は導入できる。
    apt-get install -y -qq --no-install-recommends gcc-mingw-w64-x86-64 nasm || true
    command -v x86_64-w64-mingw32-gcc >/dev/null
    command -v nasm >/dev/null
    rustup target add '"$TARGET"'
    rm -rf /var/lib/apt/lists/*
  ')
  rc=$(docker wait "$cid")
  if [ "$rc" -ne 0 ]; then
    echo "== $IMAGE の作成に失敗 ==" >&2
    docker logs "$cid" 2>&1 | tail -20 >&2
    docker rm -f "$cid" >/dev/null 2>&1 || true
    exit 1
  fi
  docker commit --change 'WORKDIR /work' "$cid" "$IMAGE" >/dev/null
  docker rm "$cid" >/dev/null
fi

detect_secopt "$IMAGE"

# root で走らせる（cargo が mingw を呼ぶだけなので書き込み先は target/ のみ）。
# 生成物がホストから消せなくならないよう、最後に所有権を戻す。
UID_GID="$(id -u):$(id -g)"
exec docker run --rm \
  "${SECOPT[@]+"${SECOPT[@]}"}" \
  -v "$PWD":/work \
  -v motmot-cargo-registry:/usr/local/cargo/registry \
  -e CARGO_TERM_COLOR=never \
  -e CARGO_TARGET_DIR=/work/target/win-gnu \
  -w /work \
  "$IMAGE" bash -c "
    cargo check --workspace --all-targets --target $TARGET
    rc=\$?
    chown -R $UID_GID /work/target/win-gnu 2>/dev/null || true
    exit \$rc
  "
