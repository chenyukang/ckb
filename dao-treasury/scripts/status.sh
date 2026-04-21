#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

echo "RPC: $CKB_RPC_URL"
"$CKB_CLI" --url "$CKB_RPC_URL" rpc get_tip_block_number --local-only
"$CKB_CLI" --url "$CKB_RPC_URL" rpc get_indexer_tip --local-only || true
