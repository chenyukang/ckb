#!/usr/bin/env bash
set -euo pipefail

METHOD="${1:?usage: tally-rpc.sh <method> [params-json]}"
if [[ $# -ge 2 ]]; then
  PARAMS="$2"
else
  PARAMS="{}"
fi
TALLY_RPC_URL="${TALLY_RPC_URL:-http://127.0.0.1:8124}"

python3 - "$TALLY_RPC_URL" "$METHOD" "$PARAMS" <<'PY'
import json
import sys
import urllib.request

url, method, params_text = sys.argv[1:4]
payload = {
    "jsonrpc": "2.0",
    "id": 1,
    "method": method,
    "params": json.loads(params_text),
}
req = urllib.request.Request(
    url,
    data=json.dumps(payload).encode(),
    headers={"content-type": "application/json"},
    method="POST",
)
with urllib.request.urlopen(req, timeout=120) as resp:
    print(json.dumps(json.loads(resp.read()), indent=2, sort_keys=True))
PY
