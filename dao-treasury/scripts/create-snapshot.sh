#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

SNAPSHOT_BLOCK="${1:-}"

args=(
  generate
  --rpc "$CKB_RPC_URL"
)

if [[ -n "$SNAPSHOT_BLOCK" ]]; then
  args+=(--snapshot-block "$SNAPSHOT_BLOCK")
fi

"$SCRIPT_DIR/snapshot-dao-deposits.py" "${args[@]}"
