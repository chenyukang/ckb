#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

SNAPSHOT_FILE="${1:?usage: create-proposal-cell.sh <snapshot-json>}"

summary="$("$SCRIPT_DIR/proposal.py" create-sample \
  --snapshot "$SNAPSHOT_FILE" \
  --output-dir "$DAO_TREASURY_DIR/artifacts")"

echo "$summary"

manifest_path="$(printf '%s' "$summary" | jq -r '.manifest_path')"

echo "proposal artifact generated: $manifest_path"
echo "typed proposal submission requires the governance type transaction builder"
