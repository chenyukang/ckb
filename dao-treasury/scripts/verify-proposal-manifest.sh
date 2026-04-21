#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

MANIFEST="${1:?usage: verify-proposal-manifest.sh <proposal-manifest.json>}"
"$SCRIPT_DIR/proposal.py" verify-manifest "$MANIFEST"
