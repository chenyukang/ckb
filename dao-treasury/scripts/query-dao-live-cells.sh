#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

MODE="${1:-deposits}"
case "$MODE" in
  deposits)
    SEARCH_KEY="$SCRIPT_DIR/search-key-dao-deposits.json"
    ;;
  all)
    SEARCH_KEY="$SCRIPT_DIR/search-key-dao-all.json"
    ;;
  *)
    echo "usage: query-dao-live-cells.sh [deposits|all]" >&2
    exit 1
    ;;
esac

"$CKB_CLI" --url "$CKB_RPC_URL" rpc get_cells \
  --json-path "$SEARCH_KEY" \
  --order asc \
  --limit "${LIMIT:-100}" \
  --local-only
