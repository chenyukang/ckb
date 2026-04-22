#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

VOTER="${1:?usage: voter-options.sh <alice|bob|carol|lock-arg>}"
shift

cd "$DAO_TREASURY_DIR/.."
"$SCRIPT_DIR/voter-options.py" "$VOTER" \
  --rpc "$CKB_RPC_URL" \
  --snapshot-dir "dao-treasury/artifacts" \
  "$@"
