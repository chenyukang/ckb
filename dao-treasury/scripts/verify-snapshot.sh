#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

SNAPSHOT_FILE="${1:?usage: verify-snapshot.sh <snapshot.json>}"
"$SCRIPT_DIR/snapshot-dao-deposits.py" verify --rpc "$CKB_RPC_URL" "$SNAPSHOT_FILE"
