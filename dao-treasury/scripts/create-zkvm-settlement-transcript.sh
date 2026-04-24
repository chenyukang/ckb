#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

PROPOSAL_FILE="${1:?usage: create-zkvm-settlement-transcript.sh <proposal.json> <snapshot.json> <tally.json> [output.json]}"
SNAPSHOT_FILE="${2:?usage: create-zkvm-settlement-transcript.sh <proposal.json> <snapshot.json> <tally.json> [output.json]}"
TALLY_FILE="${3:?usage: create-zkvm-settlement-transcript.sh <proposal.json> <snapshot.json> <tally.json> [output.json]}"
OUTPUT_FILE="${4:-}"

args=(
  --rpc "$CKB_RPC_URL"
  --proposal "$PROPOSAL_FILE"
  --snapshot "$SNAPSHOT_FILE"
  --tally "$TALLY_FILE"
)

if [[ -n "$OUTPUT_FILE" ]]; then
  args+=(--output "$OUTPUT_FILE")
fi

"$SCRIPT_DIR/create-zkvm-settlement-transcript.py" "${args[@]}"
