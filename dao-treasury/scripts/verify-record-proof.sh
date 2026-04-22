#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

PROOF_FILE="${1:?usage: verify-record-proof.sh <record-proof.json>}"

"$SCRIPT_DIR/snapshot-dao-deposits.py" verify-record-proof "$PROOF_FILE"
