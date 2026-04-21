#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

TX_HASH="${1:?usage: mine-until-committed.sh <tx-hash> [max-blocks]}"
MAX_BLOCKS="${2:-12}"

tx_status() {
  "$CKB_CLI" --url "$CKB_RPC_URL" rpc get_transaction \
    --hash "$TX_HASH" \
    --output-format json \
    --local-only 2>/dev/null \
    | sed -n 's/.*"status": *"\([^"]*\)".*/\1/p' \
    | tail -1
}

for i in $(seq 0 "$MAX_BLOCKS"); do
  status="$(tx_status || true)"
  if [[ "$status" == "committed" ]]; then
    echo "$TX_HASH committed"
    exit 0
  fi

  if [[ "$i" == "$MAX_BLOCKS" ]]; then
    break
  fi

  echo "tx status: ${status:-unknown}; mining one block..."
  "$CKB_BIN" -C "$DAO_TREASURY_DIR" miner --limit 1 >/dev/null
done

echo "$TX_HASH was not committed after $MAX_BLOCKS mined blocks" >&2
exit 1
