#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

ARTIFACTS_DIR="$DAO_TREASURY_DIR/artifacts"
SUMMARY_FILE="$ARTIFACTS_DIR/demo-summary.json"

REFRESH_DEMO=0
TRANSCRIPT_FILE=""
OUTPUT_FILE=""
NODE_STARTED_BY_SCRIPT=0

usage() {
  cat <<'EOF'
usage: run-zkvm-poc.sh [--fresh-demo] [--transcript <path>] [--output <path>]

  --fresh-demo         reset local demo chain and rerun run-demo.sh first
  --transcript <path>  override transcript output path
  --output <path>      override settlement output path
EOF
}

log() {
  printf '\n== %s ==\n' "$*"
}

info() {
  printf '  %s\n' "$*"
}

tip() {
  "$CKB_CLI" --url "$CKB_RPC_URL" rpc get_tip_block_number --local-only
}

wait_for_node() {
  for _ in $(seq 1 60); do
    if "$CKB_CLI" --url "$CKB_RPC_URL" rpc get_tip_block_number --local-only >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  echo "CKB node did not become ready" >&2
  exit 1
}

ensure_node() {
  if "$CKB_CLI" --url "$CKB_RPC_URL" rpc get_tip_block_number --local-only >/dev/null 2>&1; then
    info "ckb node already running, tip=$(tip)"
    return 0
  fi

  log "Start local CKB node"
  "$SCRIPT_DIR/start-node-bg.sh" >/dev/null
  wait_for_node
  NODE_STARTED_BY_SCRIPT=1
  info "tip: $(tip)"
}

cleanup() {
  if [[ "$NODE_STARTED_BY_SCRIPT" == "1" ]]; then
    "$SCRIPT_DIR/stop-node.sh" >/dev/null 2>&1 || true
  fi
}

trap cleanup EXIT

while [[ $# -gt 0 ]]; do
  case "$1" in
    --fresh-demo)
      REFRESH_DEMO=1
      shift
      ;;
    --transcript)
      TRANSCRIPT_FILE="${2:?missing value for --transcript}"
      shift 2
      ;;
    --output)
      OUTPUT_FILE="${2:?missing value for --output}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [[ "$REFRESH_DEMO" == "1" || ! -f "$SUMMARY_FILE" ]]; then
  log "Run governance demo"
  "$SCRIPT_DIR/run-demo.sh"
fi

if [[ ! -f "$SUMMARY_FILE" ]]; then
  echo "missing demo summary: $SUMMARY_FILE" >&2
  exit 1
fi

PROPOSAL_FILE="$(jq -r '.proposal.file' "$SUMMARY_FILE")"
SNAPSHOT_FILE="$(jq -r '.snapshot.file' "$SUMMARY_FILE")"
TALLY_FILE="$(jq -r '.tally.file' "$SUMMARY_FILE")"
PROPOSAL_ID="$(jq -r '.proposal.proposal_id' "$SUMMARY_FILE")"
SNAPSHOT_ID="$(jq -r '.snapshot.snapshot_id' "$SUMMARY_FILE")"
TALLY_ROOT="$(jq -r '.tally.tally_root' "$SUMMARY_FILE")"

if [[ -z "$TRANSCRIPT_FILE" ]]; then
  short_id="${PROPOSAL_ID:2:12}"
  TRANSCRIPT_FILE="$ARTIFACTS_DIR/zkvm-settlement-transcript-${short_id}.json"
fi

if [[ -z "$OUTPUT_FILE" ]]; then
  short_id="${PROPOSAL_ID:2:12}"
  OUTPUT_FILE="$ARTIFACTS_DIR/zkvm-settlement-output-${short_id}.json"
fi

ensure_node

log "Create zkVM settlement transcript"
info "proposal: $PROPOSAL_FILE"
info "snapshot: $SNAPSHOT_FILE"
info "tally: $TALLY_FILE"
info "output: $TRANSCRIPT_FILE"
"$SCRIPT_DIR/create-zkvm-settlement-transcript.sh" \
  "$PROPOSAL_FILE" \
  "$SNAPSHOT_FILE" \
  "$TALLY_FILE" \
  "$TRANSCRIPT_FILE"

log "Verify zkVM settlement transcript"
info "transcript: $TRANSCRIPT_FILE"
info "output: $OUTPUT_FILE"
"$SCRIPT_DIR/verify-zkvm-settlement-transcript.sh" \
  "$TRANSCRIPT_FILE" \
  "$OUTPUT_FILE" >/dev/null

PROOF_MODEL="$(jq -r '.settlement_commitment.proof_model' "$OUTPUT_FILE")"
SETTLEMENT_ROOT="$(jq -r '.settlement_root' "$OUTPUT_FILE")"
ANCHOR_START="$(jq -r '.settlement_commitment.anchor_start_block_number' "$OUTPUT_FILE")"
ANCHOR_END="$(jq -r '.settlement_commitment.anchor_end_block_number' "$OUTPUT_FILE")"
VALID="$(jq -r '.valid' "$OUTPUT_FILE")"

log "zkVM PoC completed"
info "proposal_id: $PROPOSAL_ID"
info "snapshot_id: $SNAPSHOT_ID"
info "tally_root: $TALLY_ROOT"
info "proof_model: $PROOF_MODEL"
info "anchor blocks: $ANCHOR_START..$ANCHOR_END"
info "settlement_root: $SETTLEMENT_ROOT"
info "valid: $VALID"
info "transcript_file: $TRANSCRIPT_FILE"
info "settlement_output: $OUTPUT_FILE"
