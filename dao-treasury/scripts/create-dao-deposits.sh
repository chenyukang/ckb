#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

if [[ "${CONFIRM:-}" != "1" ]]; then
  echo "This script creates DAO deposits on the local dev chain. Re-run with CONFIRM=1." >&2
  exit 1
fi

deposit() {
  local name="$1"
  local key="$2"
  local capacity="$3"

  echo "$name deposits $capacity CKB"
  tx_hash="$("$CKB_CLI" --url "$CKB_RPC_URL" dao deposit \
    --privkey-path "$DAO_TREASURY_DIR/accounts/$key.privkey" \
    --capacity "$capacity" \
    --local-only)"
  echo "$tx_hash"
  "$SCRIPT_DIR/mine-until-committed.sh" "$tx_hash"
}

deposit "alice" "alice" "30000"
deposit "bob" "bob" "40000"
deposit "bob" "bob" "25000"
deposit "carol" "carol" "20000"
