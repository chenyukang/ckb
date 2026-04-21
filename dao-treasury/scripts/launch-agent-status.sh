#!/usr/bin/env bash
set -euo pipefail

LABEL="com.ckb.dao-treasury-dev"
DOMAIN="gui/$(id -u)"

launchctl print "$DOMAIN/$LABEL"
