#!/bin/bash
# PostToolUse(Edit/Write) hook: validate config.yml schema after edit
INPUT=$(cat)
CWD=$(echo "$INPUT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('cwd',''))" 2>/dev/null)
[ -z "$CWD" ] && exit 0

RUNNER=""
for candidate in "$CWD/target/debug/workflow-runner" "$CWD/target/release/workflow-runner"; do
  [ -x "$candidate" ] && RUNNER="$candidate" && break
done
[ -z "$RUNNER" ] && exit 0

RESULT=$(echo "$INPUT" | "$RUNNER" --cwd "$CWD" hook post-edit 2>/dev/null)
[ -n "$RESULT" ] && echo "$RESULT"
exit 0
