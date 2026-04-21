#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

LIMIT="${1:-6}"
exec "$CKB_BIN" -C "$DAO_TREASURY_DIR" miner --limit "$LIMIT"
