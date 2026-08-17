#!/bin/bash
# Mock agent that fails once then succeeds: first call returns prose with no
# JSON, later calls behave like mock-agent.sh. Exercises the retry loop.
# Requires MOCK_FLAKY_STATE to point at a writable counter file.
set -eu
cat > /dev/null
state="${MOCK_FLAKY_STATE:?set MOCK_FLAKY_STATE to a writable file path}"
n=$(cat "$state" 2>/dev/null || echo 0)
echo $((n + 1)) > "$state"
if [ "$n" -eq 0 ]; then
  echo "I could not produce structured output for this posting."
  exit 0
fi
dir=$(cd "$(dirname "$0")" && pwd)
facts_file="${MOCK_FACTS_FILE:-$dir/facts/default.json}"
python3 -c '
import json, sys
facts = open(sys.argv[1]).read()
print(json.dumps({"type": "result", "is_error": False, "result": facts}))
' "$facts_file"
