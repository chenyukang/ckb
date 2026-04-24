#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

TRANSCRIPT_FILE="${1:?usage: verify-zkvm-settlement-transcript.sh <transcript.json> [output.json]}"
OUTPUT_FILE="${2:-}"

args=(verify --transcript "$TRANSCRIPT_FILE")

if [[ -n "$OUTPUT_FILE" ]]; then
  args+=(--output "$OUTPUT_FILE")
fi

cargo run --quiet --manifest-path "$DAO_TREASURY_DIR/zkvm-poc/Cargo.toml" -- "${args[@]}"
