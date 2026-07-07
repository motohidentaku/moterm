#!/usr/bin/env bash
# OSC 133 シェル統合を最小エミュレートしてプロンプト境界を発行する。
# moterm の「前/次のプロンプトへジャンプ」E2E 用。
p() { printf '\033]133;A\007%s\033]133;B\007' "$1"; }   # プロンプト開始+入力開始
c() { printf '\033]133;C\007'; }                          # 出力開始
d() { printf '\033]133;D;%s\007' "$1"; }                  # 終了(exit)
for i in 1 2 3 4 5; do
  p "prompt$i\$ "
  c
  printf 'output-line-%s\n' "$i"
  seq 1 8 | sed "s/^/  filler-$i-/"
  d 0
done
printf '\033]133;A\007done\$ '
