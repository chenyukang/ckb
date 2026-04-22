#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

PROOF_FILE="${1:?usage: verify-owner-proof.sh <owner-proof.json>}"

"$SCRIPT_DIR/snapshot-dao-deposits.py" verify-owner-proof "$PROOF_FILE"
