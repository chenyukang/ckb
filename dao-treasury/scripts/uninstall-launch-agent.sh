#!/usr/bin/env bash
set -euo pipefail

LABEL="com.ckb.dao-treasury-dev"
DOMAIN="gui/$(id -u)"

launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
echo "stopped $LABEL"
