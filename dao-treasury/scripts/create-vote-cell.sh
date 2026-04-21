#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

VOTER="${1:?usage: create-vote-cell.sh <alice|bob|carol> <deposit-outpoint> <yes|no|abstain>}"
DEPOSIT_OUTPOINT="${2:?usage: create-vote-cell.sh <alice|bob|carol> <deposit-outpoint> <yes|no|abstain>}"
CHOICE="${3:?usage: create-vote-cell.sh <alice|bob|carol> <deposit-outpoint> <yes|no|abstain>}"

PROPOSAL_FILE="${PROPOSAL_FILE:-$DAO_TREASURY_DIR/artifacts/proposal-2fc483c8e4ff.json}"
SNAPSHOT_FILE="${SNAPSHOT_FILE:-$DAO_TREASURY_DIR/artifacts/snapshot-block-59.json}"
VOTE_CAPACITY="${VOTE_CELL_CAPACITY:-1000}"

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

summary="$("$SCRIPT_DIR/vote.py" create \
  --proposal "$PROPOSAL_FILE" \
  --snapshot "$SNAPSHOT_FILE" \
  --deposit-out-point "$DEPOSIT_OUTPOINT" \
  --choice "$CHOICE" \
  --output-dir "$DAO_TREASURY_DIR/artifacts")"

echo "$summary"

vote_path="$(printf '%s' "$summary" | jq -r '.vote_path')"
cell_data_path="$(printf '%s' "$summary" | jq -r '.cell_data_path')"

tx_hash="$("$CKB_CLI" --url "$CKB_RPC_URL" wallet transfer \
  --privkey-path "$PRIVKEY" \
  --to-address "$ADDRESS" \
  --to-data-path "$cell_data_path" \
  --capacity "$VOTE_CAPACITY" \
  --local-only)"

echo "vote_tx_hash: $tx_hash"
"$SCRIPT_DIR/mine-until-committed.sh" "$tx_hash"
"$SCRIPT_DIR/vote.py" record-chain --vote "$vote_path" --tx-hash "$tx_hash"
