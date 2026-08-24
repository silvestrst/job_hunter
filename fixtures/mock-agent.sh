#!/bin/bash
# Mock LLM agent for tests and offline development.
# Reads the prompt on stdin, optionally records it, and emits a Claude-style
# JSON envelope whose .result contains canned assessment facts.
#
# Env vars:
#   MOCK_FACTS_FILE   JSON to embed (default: facts/default.json)
#   MOCK_CAPTURE_FILE if set, the received prompt is appended here (for
#                     asserting prompt contents, e.g. the feedback digest)
set -eu
prompt=$(cat)
if [ -n "${MOCK_CAPTURE_FILE:-}" ]; then
  printf '%s\n----\n' "$prompt" >> "$MOCK_CAPTURE_FILE"
fi
dir=$(cd "$(dirname "$0")" && pwd)
facts_file="${MOCK_FACTS_FILE:-$dir/facts/default.json}"
python3 -c '
import json, sys
facts = open(sys.argv[1]).read()
print(json.dumps({"type": "result", "is_error": False, "result": facts}))
' "$facts_file"
