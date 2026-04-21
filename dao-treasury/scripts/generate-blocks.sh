#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

COUNT="${1:-1}"
for _ in $(seq 1 "$COUNT"); do
  "$CKB_CLI" --url "$CKB_RPC_URL" rpc generate_block --local-only >/dev/null
done
"$SCRIPT_DIR/status.sh"
