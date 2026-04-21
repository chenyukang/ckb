#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

SNAPSHOT_FILE="${1:-$DAO_TREASURY_DIR/artifacts/snapshot-block-59.json}"
PROPOSER_PRIVKEY="$DAO_TREASURY_DIR/accounts/proposer.privkey"
PROPOSER_ADDRESS="ckt1qzda0cr08m85hc8jlnfp3zer7xulejywt49kt2rr0vthywaa50xwsqgzca6d8x2rej8xf4kzc3e238llngchq4q20s5wz"

summary="$("$SCRIPT_DIR/proposal.py" create-sample \
  --snapshot "$SNAPSHOT_FILE" \
  --output-dir "$DAO_TREASURY_DIR/artifacts")"

echo "$summary"

manifest_path="$(printf '%s' "$summary" | jq -r '.manifest_path')"
cell_data_path="$(printf '%s' "$summary" | jq -r '.cell_data_path')"

tx_hash="$("$CKB_CLI" --url "$CKB_RPC_URL" wallet transfer \
  --privkey-path "$PROPOSER_PRIVKEY" \
  --to-address "$PROPOSER_ADDRESS" \
  --to-data-path "$cell_data_path" \
  --capacity "${PROPOSAL_CELL_CAPACITY:-1000}" \
  --local-only)"

echo "proposal_tx_hash: $tx_hash"
"$SCRIPT_DIR/mine-until-committed.sh" "$tx_hash"
"$SCRIPT_DIR/proposal.py" record-chain --manifest "$manifest_path" --tx-hash "$tx_hash"
