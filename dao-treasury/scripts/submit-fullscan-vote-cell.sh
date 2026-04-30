#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

VOTER="${1:?usage: submit-fullscan-vote-cell.sh <alice|bob|carol> <vote-json> <dao-deposit-outpoint> <type-code-outpoint>}"
VOTE_FILE="${2:?usage: submit-fullscan-vote-cell.sh <alice|bob|carol> <vote-json> <dao-deposit-outpoint> <type-code-outpoint>}"
DAO_DEPOSIT_OUT_POINT="${3:?usage: submit-fullscan-vote-cell.sh <alice|bob|carol> <vote-json> <dao-deposit-outpoint> <type-code-outpoint>}"
TYPE_CODE_OUT_POINT="${4:?usage: submit-fullscan-vote-cell.sh <alice|bob|carol> <vote-json> <dao-deposit-outpoint> <type-code-outpoint>}"

case "$VOTER" in
  alice)
    PRIVKEY="$DAO_TREASURY_DIR/accounts/alice.privkey"
    ;;
  bob)
    PRIVKEY="$DAO_TREASURY_DIR/accounts/bob.privkey"
    ;;
  carol)
    PRIVKEY="$DAO_TREASURY_DIR/accounts/carol.privkey"
    ;;
  *)
    echo "unknown voter: $VOTER" >&2
    exit 1
    ;;
esac

key_address() {
  "$CKB_CLI" util key-info \
    --privkey-path "$1" \
    --local-only \
    --output-format json \
    2>/dev/null \
    | sed -n '/^{/,$p' \
    | jq -r '.address.testnet'
}

ADDRESS="$(key_address "$PRIVKEY")"

short_id="$(jq -r '.vote_data | @json' "$VOTE_FILE" | shasum -a 256 | awk '{print substr($1, 1, 12)}')"
cell_data_path="${VOTE_FILE%.json}.cell-data.bin"
type_args="$(jq -r '.vote_type_script.args' "$VOTE_FILE")"
TYPE_CODE_HASH="${DAO_TREASURY_GOV_TYPE_CODE_HASH:-$(jq -r '.vote_type_script.code_hash' "$VOTE_FILE")}"
TYPE_HASH_TYPE="${DAO_TREASURY_GOV_TYPE_HASH_TYPE:-$(jq -r '.vote_type_script.hash_type' "$VOTE_FILE")}"
tx_file="$DAO_TREASURY_DIR/artifacts/submit-fullscan-vote-$short_id.tx.json"

summary="$("$SCRIPT_DIR/submit-typed-cell.py" \
  --ckb-cli "$CKB_CLI" \
  --rpc "$CKB_RPC_URL" \
  --privkey-path "$PRIVKEY" \
  --from-address "$ADDRESS" \
  --to-address "$ADDRESS" \
  --capacity "${VOTE_CELL_CAPACITY:-5000}" \
  --data-path "$cell_data_path" \
  --type-code-hash "$TYPE_CODE_HASH" \
  --type-hash-type "$TYPE_HASH_TYPE" \
  --type-args "$type_args" \
  --type-code-out-point "$TYPE_CODE_OUT_POINT" \
  --extra-cell-dep-out-point "$DAO_DEPOSIT_OUT_POINT" \
  --tx-file "$tx_file")"

echo "$summary"
tx_hash="$(printf '%s' "$summary" | jq -r '.tx_hash')"
"$SCRIPT_DIR/mine-until-committed.sh" "$tx_hash"
