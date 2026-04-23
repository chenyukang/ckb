#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

VOTER="${1:?usage: submit-vote-cell.sh <alice|bob|carol> <vote-json> <type-code-outpoint>}"
VOTE_FILE="${2:?usage: submit-vote-cell.sh <alice|bob|carol> <vote-json> <type-code-outpoint>}"
TYPE_CODE_OUT_POINT="${3:?usage: submit-vote-cell.sh <alice|bob|carol> <vote-json> <type-code-outpoint>}"

case "$VOTER" in
  alice)
    PRIVKEY="$DAO_TREASURY_DIR/accounts/alice.privkey"
    ADDRESS="ckt1qzda0cr08m85hc8jlnfp3zer7xulejywt49kt2rr0vthywaa50xwsqtaas69h37zuxxmu3lq0vmzumlsmxcqlqszq36s9"
    ;;
  bob)
    PRIVKEY="$DAO_TREASURY_DIR/accounts/bob.privkey"
    ADDRESS="ckt1qzda0cr08m85hc8jlnfp3zer7xulejywt49kt2rr0vthywaa50xwsqvhwysy2kurcged4pfqcldhmfa298hszfgnkch54"
    ;;
  carol)
    PRIVKEY="$DAO_TREASURY_DIR/accounts/carol.privkey"
    ADDRESS="ckt1qzda0cr08m85hc8jlnfp3zer7xulejywt49kt2rr0vthywaa50xwsqv6pea4w0444wjr35u983arwjdu77pq6pq3hsax3"
    ;;
  *)
    echo "unknown voter: $VOTER" >&2
    exit 1
    ;;
esac

short_id="$(jq -r '.vote_id[2:14]' "$VOTE_FILE")"
cell_data_path="$DAO_TREASURY_DIR/artifacts/vote-$short_id.cell-data.bin"
type_args="$(jq -r '.vote_type_script.args' "$VOTE_FILE")"
TYPE_CODE_HASH="${DAO_TREASURY_GOV_TYPE_CODE_HASH:-$(jq -r '.vote_type_script.code_hash' "$VOTE_FILE")}"
TYPE_HASH_TYPE="${DAO_TREASURY_GOV_TYPE_HASH_TYPE:-$(jq -r '.vote_type_script.hash_type' "$VOTE_FILE")}"
tx_file="$DAO_TREASURY_DIR/artifacts/submit-vote-$short_id.tx.json"

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
  --tx-file "$tx_file")"

echo "$summary"
tx_hash="$(printf '%s' "$summary" | jq -r '.tx_hash')"
"$SCRIPT_DIR/mine-until-committed.sh" "$tx_hash"
"$SCRIPT_DIR/vote.py" record-chain --vote "$VOTE_FILE" --tx-hash "$tx_hash"
