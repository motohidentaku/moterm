#!/usr/bin/env bash
# docker 実行スクリプト共通のヘルパ。`source docker/lib.sh` して使う。

# AppArmor が有効でも dockerd が docker-default プロファイルをロードできない環境
# （ネストした仮想化ホスト・QNAP NAS 等。/sys/kernel/security/apparmor/profiles が
# root でも読めない）では、既定プロファイルの適用に失敗してコンテナが起動できない。
# その場合だけ unconfined で起動する（適用できる環境では既定のまま保護を効かせる）。
#
# 結果を配列 SECOPT に入れる。使う側は "${SECOPT[@]+"${SECOPT[@]}"}" と展開する
# （空配列 + set -u でも壊れない形）。
# コンテナ内で sshd を起動する前に呼ぶ（コンテナ内で実行されるスニペットを出力する）。
#
# xattr 非対応の FS 上でイメージをビルドすると openssh-server の postinst が落ち、
# パッケージが half-configured のまま残る。バイナリはあるが特権分離ユーザ `sshd` が
# 作られていないため sshd が "Privilege separation user sshd does not exist" で
# 起動できない。イメージを作り直せない環境でも e2e を回せるよう、起動前に補う。
ENSURE_SSHD_USER='id -u sshd >/dev/null 2>&1 || useradd -r -M -d /run/sshd -s /usr/sbin/nologin sshd'

detect_secopt() {
  local image="$1"
  SECOPT=()
  if ! docker run --rm "$image" true >/dev/null 2>&1 \
    && docker run --rm --security-opt apparmor=unconfined "$image" true >/dev/null 2>&1; then
    SECOPT=(--security-opt apparmor=unconfined)
  fi
}
