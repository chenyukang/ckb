#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

PROPOSAL_FILE="${PROPOSAL_FILE:-$DAO_TREASURY_DIR/artifacts/proposal-fca04aaecee4.json}"
SNAPSHOT_FILE="${SNAPSHOT_FILE:-$DAO_TREASURY_DIR/artifacts/snapshot-block-139.json}"

"$SCRIPT_DIR/vote.py" discover \
  --rpc "$CKB_RPC_URL" \
  --proposal "$PROPOSAL_FILE" \
  --snapshot "$SNAPSHOT_FILE"
