#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

INDEX_FILE="${1:?usage: snapshot-record-proof.sh <snapshot-index.json> <deposit-outpoint>}"
DEPOSIT_OUTPOINT="${2:?usage: snapshot-record-proof.sh <snapshot-index.json> <deposit-outpoint>}"

"$SCRIPT_DIR/snapshot-dao-deposits.py" prove-record "$INDEX_FILE" "$DEPOSIT_OUTPOINT"
