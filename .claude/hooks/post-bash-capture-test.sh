#!/bin/bash
# PostToolUse(Bash) hook: テスト実行を検出して state.json と checklist.md に記録する
INPUT=$(cat)
CWD=$(python3 -c "import sys,json; d=json.loads(sys.argv[1]); print(d.get('cwd',''))" "$INPUT" 2>/dev/null)
[ -z "$CWD" ] && exit 0
RUNNER="$CWD/target/debug/workflow-runner"
[ ! -x "$RUNNER" ] && exit 0
echo "$INPUT" | "$RUNNER" --cwd "$CWD" hook post-bash 2>/dev/null
exit 0
