#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

PID_FILE="$DAO_TREASURY_DIR/ckb-node.pid"
LOG_FILE="$DAO_TREASURY_DIR/logs/ckb-node.stdout.log"

mkdir -p "$DAO_TREASURY_DIR/logs"

if [[ -f "$PID_FILE" ]]; then
  pid="$(cat "$PID_FILE")"
  if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
    echo "ckb node already running: pid=$pid"
    "$SCRIPT_DIR/status.sh"
    exit 0
  fi
fi

nohup "$CKB_BIN" -C "$DAO_TREASURY_DIR" run >>"$LOG_FILE" 2>&1 &
pid="$!"
echo "$pid" >"$PID_FILE"

echo "started ckb node: pid=$pid"
echo "log: $LOG_FILE"
sleep 2
"$SCRIPT_DIR/status.sh"
