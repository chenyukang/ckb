#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

: "${PROPOSAL_FILE:?set PROPOSAL_FILE to the proposal manifest json}"
: "${SNAPSHOT_FILE:?set SNAPSHOT_FILE to the snapshot json}"

"$SCRIPT_DIR/vote.py" discover \
  --rpc "$CKB_RPC_URL" \
  --proposal "$PROPOSAL_FILE" \
  --snapshot "$SNAPSHOT_FILE"
