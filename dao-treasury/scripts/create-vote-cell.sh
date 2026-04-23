#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

DEPOSIT_OUTPOINT="${1:?usage: create-vote-cell.sh <deposit-outpoint> <yes|no|abstain>}"
CHOICE="${2:?usage: create-vote-cell.sh <deposit-outpoint> <yes|no|abstain>}"

: "${PROPOSAL_FILE:?set PROPOSAL_FILE to the proposal manifest json}"
: "${SNAPSHOT_FILE:?set SNAPSHOT_FILE to the snapshot json}"

summary="$("$SCRIPT_DIR/vote.py" create \
  --proposal "$PROPOSAL_FILE" \
  --snapshot "$SNAPSHOT_FILE" \
  --deposit-out-point "$DEPOSIT_OUTPOINT" \
  --choice "$CHOICE" \
  --output-dir "$DAO_TREASURY_DIR/artifacts")"

echo "$summary"

vote_path="$(printf '%s' "$summary" | jq -r '.vote_path')"

echo "vote artifact generated: $vote_path"
echo "typed vote submission requires the governance type transaction builder"
