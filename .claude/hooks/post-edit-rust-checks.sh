#!/bin/bash
# PostToolUse(Edit/Write) hook: run fmt, lint after Rust source edits
INPUT=$(cat)
CWD=$(python3 -c "import sys,json; d=json.loads(sys.argv[1]); print(d.get('cwd',''))" "$INPUT" 2>/dev/null)
FILE=$(python3 -c "import sys,json; d=json.loads(sys.argv[1]); print(d.get('tool_input',{}).get('file_path',''))" "$INPUT" 2>/dev/null)
[ -z "$CWD" ] || [ -z "$FILE" ] && exit 0
case "$FILE" in *.rs) ;; *) exit 0 ;; esac
cd "$CWD" || exit 0
echo "--- Rust checks ($(basename "$FILE")) ---"
make fmt 2>&1 && make lint 2>&1
EXIT=$?
[ $EXIT -ne 0 ] && exit 1
exit 0
