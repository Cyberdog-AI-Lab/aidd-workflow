#!/bin/bash
# PreToolUse(TaskUpdate) hook: in_progress ステップに gate 未実行があればブロックする
INPUT=$(cat)
CWD=$(python3 -c "import sys,json; d=json.loads(sys.argv[1]); print(d.get('cwd',''))" "$INPUT" 2>/dev/null)
[ -z "$CWD" ] && exit 0
RUNNER="$CWD/target/debug/workflow-runner"
[ ! -x "$RUNNER" ] && exit 0
RESULT=$(echo "$INPUT" | "$RUNNER" --cwd "$CWD" hook pre-taskupdate 2>/dev/null)
[ -n "$RESULT" ] && echo "$RESULT"
exit 0
