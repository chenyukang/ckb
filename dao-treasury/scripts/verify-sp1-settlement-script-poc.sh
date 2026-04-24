#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
DAO_TREASURY_DIR=$(cd "$SCRIPT_DIR/.." && pwd)
REPO_ROOT=$(cd "$DAO_TREASURY_DIR/.." && pwd)

FIXTURE="${1:-$DAO_TREASURY_DIR/sp1-voting-settlement/artifacts/core-voting-settlement-fixture.json}"
EXPECTED_VK_HASH="${2:-}"
ALLOW_PLACEHOLDER="${ALLOW_PLACEHOLDER:-0}"

cmd=(
  cargo run --release
  --manifest-path "$DAO_TREASURY_DIR/sp1-settlement-script/Cargo.toml"
  --
  --fixture "$FIXTURE"
)

if [[ -n "$EXPECTED_VK_HASH" ]]; then
  cmd+=(--expected-vk-hash "$EXPECTED_VK_HASH")
fi

if [[ "$ALLOW_PLACEHOLDER" == "1" ]]; then
  cmd+=(--allow-placeholder-proof-verifier)
fi

cd "$REPO_ROOT"
proof_kind=$(jq -r '.proof_kind // ""' "$FIXTURE" 2>/dev/null || true)

if [[ "$proof_kind" == "core" ]]; then
  output_file=$(mktemp)
  if "${cmd[@]}" >"$output_file" 2>&1; then
    cat "$output_file"
    rm -f "$output_file"
    echo "expected core fixture to be rejected, but it was accepted" >&2
    exit 1
  fi
  cat "$output_file"
  if grep -q "not on-chain verifiable" "$output_file"; then
    rm -f "$output_file"
    echo "[sp1-settlement-script] expected rejection: core proofs are not on-chain verifiable"
    exit 0
  fi
  rm -f "$output_file"
  exit 1
fi

"${cmd[@]}"
