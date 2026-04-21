#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

TALLY_FILE="${1:?usage: verify-tally.sh <tally-json>}"
PROPOSAL_FILE="${PROPOSAL_FILE:-$DAO_TREASURY_DIR/artifacts/proposal-2fc483c8e4ff.json}"
SNAPSHOT_FILE="${SNAPSHOT_FILE:-$DAO_TREASURY_DIR/artifacts/snapshot-block-59.json}"

"$SCRIPT_DIR/tally.py" verify \
  --rpc "$CKB_RPC_URL" \
  --proposal "$PROPOSAL_FILE" \
  --snapshot "$SNAPSHOT_FILE" \
  --tally "$TALLY_FILE"
