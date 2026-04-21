#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

for key in "$DAO_TREASURY_DIR"/accounts/*.privkey; do
  name="$(basename "$key" .privkey)"
  echo "== $name =="
  "$CKB_CLI" util key-info --privkey-path "$key" --local-only
done
