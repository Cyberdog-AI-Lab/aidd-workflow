#!/bin/bash
# PostToolUse(Bash) hook: detect test execution and record it in state.json / checklist.md
INPUT=$(cat)
CWD=$(echo "$INPUT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('cwd',''))" 2>/dev/null)
[ -z "$CWD" ] && exit 0

RUNNER=""
for candidate in "$CWD/target/debug/workflow-runner" "$CWD/target/release/workflow-runner"; do
  [ -x "$candidate" ] && RUNNER="$candidate" && break
done
[ -z "$RUNNER" ] && exit 0

echo "$INPUT" | "$RUNNER" --cwd "$CWD" hook post-bash 2>/dev/null
exit 0
