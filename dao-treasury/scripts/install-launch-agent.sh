#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

LABEL="com.ckb.dao-treasury-dev"
DOMAIN="gui/$(id -u)"
PLIST="$DAO_TREASURY_DIR/launchd/$LABEL.plist"

mkdir -p "$DAO_TREASURY_DIR/logs"

launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
launchctl bootstrap "$DOMAIN" "$PLIST"
launchctl kickstart -k "$DOMAIN/$LABEL"

sleep 2
"$SCRIPT_DIR/status.sh"
