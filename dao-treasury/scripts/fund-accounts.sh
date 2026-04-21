#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

if [[ "${CONFIRM:-}" != "1" ]]; then
  echo "This script sends funds on the local dev chain. Re-run with CONFIRM=1." >&2
  exit 1
fi

transfer() {
  local name="$1"
  local address="$2"
  local capacity="$3"

  echo "funding $name with $capacity CKB"
  tx_hash="$("$CKB_CLI" --url "$CKB_RPC_URL" wallet transfer \
    --privkey-path "$DAO_TREASURY_DIR/accounts/faucet.privkey" \
    --to-address "$address" \
    --capacity "$capacity" \
    --local-only)"
  echo "$tx_hash"
  "$SCRIPT_DIR/mine-until-committed.sh" "$tx_hash"
}

transfer "alice" "ckt1qzda0cr08m85hc8jlnfp3zer7xulejywt49kt2rr0vthywaa50xwsqtaas69h37zuxxmu3lq0vmzumlsmxcqlqszq36s9" "100000"
transfer "bob" "ckt1qzda0cr08m85hc8jlnfp3zer7xulejywt49kt2rr0vthywaa50xwsqvhwysy2kurcged4pfqcldhmfa298hszfgnkch54" "120000"
transfer "carol" "ckt1qzda0cr08m85hc8jlnfp3zer7xulejywt49kt2rr0vthywaa50xwsqv6pea4w0444wjr35u983arwjdu77pq6pq3hsax3" "80000"
transfer "proposer" "ckt1qzda0cr08m85hc8jlnfp3zer7xulejywt49kt2rr0vthywaa50xwsqgzca6d8x2rej8xf4kzc3e238llngchq4q20s5wz" "5000"
transfer "treasury-recipient" "ckt1qzda0cr08m85hc8jlnfp3zer7xulejywt49kt2rr0vthywaa50xwsqt6t2mf9secy49he3d3dfyp03fmwaq8zus53wqem" "1000"
