#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

PID_FILE="$DAO_TREASURY_DIR/ckb-node.pid"

if [[ ! -f "$PID_FILE" ]]; then
  echo "no pid file: $PID_FILE"
  exit 0
fi

pid="$(cat "$PID_FILE")"
if [[ -z "$pid" ]]; then
  rm -f "$PID_FILE"
  echo "empty pid file removed"
  exit 0
fi

if ! kill -0 "$pid" 2>/dev/null; then
  rm -f "$PID_FILE"
  echo "stale pid file removed: $pid"
  exit 0
fi

kill "$pid"
for _ in $(seq 1 20); do
  if ! kill -0 "$pid" 2>/dev/null; then
    rm -f "$PID_FILE"
    echo "stopped ckb node: pid=$pid"
    exit 0
  fi
  sleep 0.5
done

echo "ckb node did not stop in time: pid=$pid" >&2
exit 1
