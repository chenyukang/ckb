#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export DAO_TREASURY_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
export CKB_RPC_URL="${CKB_RPC_URL:-http://127.0.0.1:8114}"
export CKB_CLI="${CKB_CLI:-ckb-cli}"
export CKB_BIN="${CKB_BIN:-ckb}"
