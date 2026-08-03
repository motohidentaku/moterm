#!/bin/sh
# moterm の情報パネルへ Claude Code の状況を送る（接続先のホストに置くスクリプト）。
#
# Claude Code の statusLine と hooks は stdin で JSON を渡してくる。それを**加工せず**
# OSC 7777 に包んで /dev/tty へ直接書く。moterm はその PTY ストリームから拾う。
# JSON をそのまま流すので、リモート側に jq などは要らない。
#
# 置き場所（例）: ~/.claude/moterm-claude.sh  （chmod +x を忘れずに）
#
# ~/.claude/settings.json:
#
#   {
#     "statusLine": {
#       "type": "command",
#       "command": "~/.claude/moterm-claude.sh"
#     },
#     "hooks": {
#       "SessionStart":      [{ "hooks": [{ "type": "command", "command": "~/.claude/moterm-claude.sh" }] }],
#       "UserPromptSubmit":  [{ "hooks": [{ "type": "command", "command": "~/.claude/moterm-claude.sh" }] }],
#       "PreToolUse":        [{ "hooks": [{ "type": "command", "command": "~/.claude/moterm-claude.sh" }] }],
#       "PostToolUse":       [{ "hooks": [{ "type": "command", "command": "~/.claude/moterm-claude.sh" }] }],
#       "Notification":      [{ "hooks": [{ "type": "command", "command": "~/.claude/moterm-claude.sh" }] }],
#       "Stop":              [{ "hooks": [{ "type": "command", "command": "~/.claude/moterm-claude.sh" }] }],
#       "SessionEnd":        [{ "hooks": [{ "type": "command", "command": "~/.claude/moterm-claude.sh" }] }]
#     }
#   }
#
# すでに statusLine を使っている場合は、既存のコマンドを**引数として渡す**こと。
# 同じ JSON をそのまま流し込むので、Claude Code 側の表示は変わらない:
#
#   "command": "~/.claude/moterm-claude.sh ~/.claude/statusline.sh"
#
# tmux 越しに使う場合は ~/.tmux.conf に次が必要（tmux は未知の OSC を捨てるため）:
#
#   set -g allow-passthrough on
#
# なお tmux では複数の Claude Code が 1 本の PTY に混ざって流れる。moterm は
# session_id で分けて保持するが、どの pane を見ているかまでは判別できない。

json=$(cat)

# 切り分け用: MOTERM_CLAUDE_DEBUG にファイルパスを入れておくと、受け取った JSON と
# /dev/tty への書き込み結果を追記する。Claude Code がこのスクリプトを呼んでいるか
# （＝設定が効いているか）、呼ばれた上で端末まで届いたかが分かる。
#   例: ~/.claude/settings.json の command を
#       "env MOTERM_CLAUDE_DEBUG=/tmp/moterm-claude.log ~/.claude/moterm-claude.sh"
debug_log() {
  [ -n "${MOTERM_CLAUDE_DEBUG:-}" ] || return 0
  printf '%s\t%s\n' "$(date '+%F %T')" "$1" >>"$MOTERM_CLAUDE_DEBUG" 2>/dev/null || true
}

debug_log "$json"

# 制御文字を変数に持つ（printf の書式解釈で JSON を壊さないため、出力は %s で行う）
ESC=$(printf '\033')
BEL=$(printf '\007')

pane=${TMUX_PANE:-}
[ -n "$pane" ] || pane='-'

payload="${ESC}]7777;${pane};${json}${BEL}"

if [ -n "${TMUX:-}" ]; then
  # tmux の DCS passthrough。包む対象の ESC を二重にするのが規則。
  payload="${ESC}Ptmux;${ESC}${payload}${ESC}\\"
  in_tmux=yes
else
  in_tmux=no
fi

# /dev/tty が無い実行形態（claude -p、CI 等）では黙って諦める。
# 書けたかどうかは切り分けの分岐点なので、デバッグ時だけ結果を残す。
# リダイレクトは左から適用されるので 2>/dev/null を先に置く
# （/dev/tty が開けないときのエラーが素通しで stderr へ出るのを防ぐ）。
if printf '%s' "$payload" 2>/dev/null >/dev/tty; then
  debug_log "-> /dev/tty へ書けた（${#payload} 文字 tmux=${in_tmux} pane=$pane）"
else
  debug_log "-> /dev/tty へ書けなかった（tty が無い / 権限）"
fi

# 引数があれば既存の statusLine コマンドへ委譲する（stdin は同じ JSON）。
# hooks から呼ぶときは引数なし = stdout に何も出さない
# （SessionStart / UserPromptSubmit の stdout は Claude のコンテキストに入るため）。
if [ "$#" -gt 0 ]; then
  printf '%s' "$json" | "$@"
fi
