#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

INDEX_FILE="${1:?usage: snapshot-owner-proof.sh <snapshot-index.json> <owner-key-or-lock-arg>}"
OWNER="${2:?usage: snapshot-owner-proof.sh <snapshot-index.json> <owner-key-or-lock-arg>}"

"$SCRIPT_DIR/snapshot-dao-deposits.py" prove-owner "$INDEX_FILE" "$OWNER"
