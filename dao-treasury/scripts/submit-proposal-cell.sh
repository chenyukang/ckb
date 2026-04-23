#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

PROPOSAL_FILE="${1:?usage: submit-proposal-cell.sh <proposal-json> <type-code-outpoint>}"
TYPE_CODE_OUT_POINT="${2:?usage: submit-proposal-cell.sh <proposal-json> <type-code-outpoint>}"
PROPOSER_PRIVKEY="$DAO_TREASURY_DIR/accounts/proposer.privkey"
PROPOSER_ADDRESS="ckt1qzda0cr08m85hc8jlnfp3zer7xulejywt49kt2rr0vthywaa50xwsqgzca6d8x2rej8xf4kzc3e238llngchq4q20s5wz"

short_id="$(jq -r '.proposal_id[2:14]' "$PROPOSAL_FILE")"
cell_data_path="$DAO_TREASURY_DIR/artifacts/proposal-$short_id.cell-data.bin"
type_args="$(jq -r '.proposal_type_script.args' "$PROPOSAL_FILE")"
TYPE_CODE_HASH="${DAO_TREASURY_GOV_TYPE_CODE_HASH:-$(jq -r '.proposal_type_script.code_hash' "$PROPOSAL_FILE")}"
TYPE_HASH_TYPE="${DAO_TREASURY_GOV_TYPE_HASH_TYPE:-$(jq -r '.proposal_type_script.hash_type' "$PROPOSAL_FILE")}"
tx_file="$DAO_TREASURY_DIR/artifacts/submit-proposal-$short_id.tx.json"

summary="$("$SCRIPT_DIR/submit-typed-cell.py" \
  --ckb-cli "$CKB_CLI" \
  --rpc "$CKB_RPC_URL" \
  --privkey-path "$PROPOSER_PRIVKEY" \
  --from-address "$PROPOSER_ADDRESS" \
  --to-address "$PROPOSER_ADDRESS" \
  --capacity "${PROPOSAL_CELL_CAPACITY:-2000}" \
  --data-path "$cell_data_path" \
  --type-code-hash "$TYPE_CODE_HASH" \
  --type-hash-type "$TYPE_HASH_TYPE" \
  --type-args "$type_args" \
  --type-code-out-point "$TYPE_CODE_OUT_POINT" \
  --tx-file "$tx_file")"

echo "$summary"
tx_hash="$(printf '%s' "$summary" | jq -r '.tx_hash')"
"$SCRIPT_DIR/mine-until-committed.sh" "$tx_hash"
"$SCRIPT_DIR/proposal.py" record-chain --manifest "$PROPOSAL_FILE" --tx-hash "$tx_hash"
