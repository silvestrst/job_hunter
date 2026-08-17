#!/bin/bash
# Mock agent that always fails - exercises the error path.
cat > /dev/null
echo "quota exceeded" >&2
exit 1
