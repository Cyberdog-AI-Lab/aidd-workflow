#!/bin/bash
# PostToolUse(Edit/Write) hook: config.yml 編集後にスキーマ検証警告を出力する
INPUT=$(cat)
CWD=$(python3 -c "import sys,json; d=json.loads(sys.argv[1]); print(d.get('cwd',''))" "$INPUT" 2>/dev/null)
[ -z "$CWD" ] && exit 0
RUNNER="$CWD/target/debug/workflow-runner"
[ ! -x "$RUNNER" ] && exit 0
RESULT=$(echo "$INPUT" | "$RUNNER" --cwd "$CWD" hook post-edit 2>/dev/null)
[ -n "$RESULT" ] && echo "$RESULT"
exit 0
