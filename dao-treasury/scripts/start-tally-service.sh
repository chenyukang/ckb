#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

TALLY_SERVICE_HOST="${TALLY_SERVICE_HOST:-127.0.0.1}"
TALLY_SERVICE_PORT="${TALLY_SERVICE_PORT:-8124}"

cd "$DAO_TREASURY_DIR/.."

exec cargo run \
  --manifest-path "$DAO_TREASURY_DIR/tally-service/Cargo.toml" \
  -- \
  --host "$TALLY_SERVICE_HOST" \
  --port "$TALLY_SERVICE_PORT" \
  --ckb-rpc "$CKB_RPC_URL" \
  --artifacts-dir "$DAO_TREASURY_DIR/artifacts"
