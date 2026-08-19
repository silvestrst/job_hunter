#!/bin/bash
# Mock LLM agent for tests and offline development.
# Reads the prompt on stdin, optionally records it, and emits a Claude-style
# JSON envelope whose .result contains canned assessment facts.
#
# Env vars:
#   MOCK_FACTS_FILE   JSON to embed, overriding the task-based choice below
#                     (default: facts/default.json, or facts/cv-advice.json
#                     when the prompt points at the cv-advice skill)
#   MOCK_CAPTURE_FILE if set, the received prompt is appended here (for
#                     asserting prompt contents, e.g. the feedback digest)
set -eu
prompt=$(cat)
if [ -n "${MOCK_CAPTURE_FILE:-}" ]; then
  printf '%s\n----\n' "$prompt" >> "$MOCK_CAPTURE_FILE"
fi
dir=$(cd "$(dirname "$0")" && pwd)
# Task-aware: every prompt names its skill, so one mock backend serves both the
# assess and cv-advice calls of a full-pipeline test.
if [ -n "${MOCK_FACTS_FILE:-}" ]; then
  facts_file="$MOCK_FACTS_FILE"
elif printf '%s' "$prompt" | grep -q 'cv-advice` skill'; then
  facts_file="$dir/facts/cv-advice.json"
else
  facts_file="$dir/facts/default.json"
fi
python3 -c '
import json, sys
facts = open(sys.argv[1]).read()
print(json.dumps({"type": "result", "is_error": False, "result": facts}))
' "$facts_file"
