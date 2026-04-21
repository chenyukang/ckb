#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

capacity() {
  local name="$1"
  local lock_arg="$2"
  echo "== $name capacity =="
  "$CKB_CLI" --url "$CKB_RPC_URL" wallet get-capacity --lock-arg "$lock_arg" --local-only
}

dao_deposits() {
  local name="$1"
  local address="$2"
  echo "== $name DAO deposits =="
  "$CKB_CLI" --url "$CKB_RPC_URL" dao query-deposited-cells --address "$address" --local-only
}

capacity "faucet" "0xc8328aabcd9b9e8e64fbc566c4385c3bdeb219d7"
capacity "alice" "0x7dec345bc7c2e18dbe47e07b362e6ff0d9b00f82"
capacity "bob" "0x977120455b83c232da8520c7db7da7aa29ef0125"
capacity "carol" "0x9a0e7b573eb5aba438d3853c7a3749bcf7820d04"
capacity "proposer" "0x02c774d39943cc8e64d6c2c472a89fff9a317054"
capacity "treasury-recipient" "0x7a5ab692c338254b7cc5b16a4817c53b77407172"

dao_deposits "alice" "ckt1qzda0cr08m85hc8jlnfp3zer7xulejywt49kt2rr0vthywaa50xwsqtaas69h37zuxxmu3lq0vmzumlsmxcqlqszq36s9"
dao_deposits "bob" "ckt1qzda0cr08m85hc8jlnfp3zer7xulejywt49kt2rr0vthywaa50xwsqvhwysy2kurcged4pfqcldhmfa298hszfgnkch54"
dao_deposits "carol" "ckt1qzda0cr08m85hc8jlnfp3zer7xulejywt49kt2rr0vthywaa50xwsqv6pea4w0444wjr35u983arwjdu77pq6pq3hsax3"
