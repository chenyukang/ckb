#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

ARTIFACTS_DIR="$DAO_TREASURY_DIR/artifacts"
DEMO_SUMMARY="$ARTIFACTS_DIR/demo-summary.json"
TALLY_PID_FILE="$DAO_TREASURY_DIR/tally-service.pid"
TALLY_RPC_URL="${TALLY_RPC_URL:-http://127.0.0.1:8124}"
TALLY_SERVICE_PORT="${TALLY_SERVICE_PORT:-8124}"
GOV_CODE_PATH="$DAO_TREASURY_DIR/../script/testdata/always_success"

ALICE_LOCK="0x7dec345bc7c2e18dbe47e07b362e6ff0d9b00f82"
BOB_LOCK="0x977120455b83c232da8520c7db7da7aa29ef0125"
CAROL_LOCK="0x9a0e7b573eb5aba438d3853c7a3749bcf7820d04"
PROPOSER_ADDRESS="ckt1qzda0cr08m85hc8jlnfp3zer7xulejywt49kt2rr0vthywaa50xwsqgzca6d8x2rej8xf4kzc3e238llngchq4q20s5wz"

log() {
  printf '\n== %s ==\n' "$*"
}

info() {
  printf '  %s\n' "$*"
}

json_rpc() {
  local method="$1"
  local params
  if [[ $# -ge 2 ]]; then
    params="$2"
  else
    params="{}"
  fi
  "$SCRIPT_DIR/tally-rpc.sh" "$method" "$params"
}

stop_tally_service() {
  if [[ -f "$TALLY_PID_FILE" ]]; then
    local pid
    pid="$(cat "$TALLY_PID_FILE")"
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
      kill "$pid" >/dev/null 2>&1 || true
      for _ in $(seq 1 20); do
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.2
      done
    fi
    rm -f "$TALLY_PID_FILE"
  fi
}

stop_node() {
  "$SCRIPT_DIR/uninstall-launch-agent.sh" >/dev/null 2>&1 || true
  "$SCRIPT_DIR/stop-node.sh" >/dev/null 2>&1 || true
}

reset_demo_data() {
  log "Reset demo data"
  stop_tally_service
  stop_node
  rm -rf "$DAO_TREASURY_DIR/data" "$DAO_TREASURY_DIR/logs"
  mkdir -p "$ARTIFACTS_DIR"
  find "$ARTIFACTS_DIR" -mindepth 1 ! -name README.md -exec rm -rf {} +
  rm -f "$DAO_TREASURY_DIR/ckb-node.pid"
  info "removed data/, logs/, and old artifacts"
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

tip() {
  "$CKB_CLI" --url "$CKB_RPC_URL" rpc get_tip_block_number --local-only
}

advance_to_block() {
  local target="$1"
  local current
  current="$(tip)"
  if (( current >= target )); then
    info "tip already at block $current"
    return 0
  fi
  local count=$((target - current))
  info "mining $count blocks to reach block $target"
  "$SCRIPT_DIR/generate-blocks.sh" "$count" >/dev/null
  info "tip: $(tip)"
}

mine_until_committed() {
  local tx_hash="$1"
  "$SCRIPT_DIR/mine-until-committed.sh" "$tx_hash" >/dev/null
  info "committed: $tx_hash"
}

start_node() {
  log "Start CKB dev chain"
  "$SCRIPT_DIR/start-node-bg.sh"
  wait_for_node
  info "tip: $(tip)"
}

fund_accounts() {
  log "Fund demo accounts"
  CONFIRM=1 "$SCRIPT_DIR/fund-accounts.sh"
  info "tip after funding: $(tip)"
}

deploy_governance_type_code() {
  log "Deploy governance type code"
  local code_hash
  code_hash="$("$CKB_CLI" util blake2b \
    --binary-path "$GOV_CODE_PATH" \
    --local-only \
    --output-format json | jq -r '.')"

  local tx_hash
  tx_hash="$("$CKB_CLI" --url "$CKB_RPC_URL" wallet transfer \
    --privkey-path "$DAO_TREASURY_DIR/accounts/proposer.privkey" \
    --to-address "$PROPOSER_ADDRESS" \
    --capacity "1000" \
    --to-data-path "$GOV_CODE_PATH" \
    --local-only)"
  mine_until_committed "$tx_hash"

  GOVERNANCE_TYPE_CODE_HASH="$code_hash"
  GOVERNANCE_TYPE_CODE_OUT_POINT="$tx_hash:0"
  export DAO_TREASURY_GOV_TYPE_CODE_HASH="$GOVERNANCE_TYPE_CODE_HASH"
  export DAO_TREASURY_GOV_TYPE_HASH_TYPE="data"

  info "code_hash: $GOVERNANCE_TYPE_CODE_HASH"
  info "code_out_point: $GOVERNANCE_TYPE_CODE_OUT_POINT"
}

create_deposits() {
  log "Create DAO deposits"
  CONFIRM=1 "$SCRIPT_DIR/create-dao-deposits.sh"
  info "tip after deposits: $(tip)"
}

dao_cells_json() {
  "$CKB_CLI" --url "$CKB_RPC_URL" --output-format json rpc get_cells \
    --json-path "$SCRIPT_DIR/search-key-dao-deposits.json" \
    --order asc \
    --limit 100 \
    --local-only
}

outpoint_for() {
  local lock="$1"
  local capacity="$2"
  jq -r --arg lock "$lock" --arg capacity "$capacity" '
    .objects[]
    | select(.output.lock.args == $lock and .output.capacity == $capacity)
    | "\(.out_point.tx_hash):\(.out_point.index)"
  ' "$ARTIFACTS_DIR/dao-live-cells.json" | head -1
}

record_dao_deposits() {
  log "Record DAO live cells"
  dao_cells_json >"$ARTIFACTS_DIR/dao-live-cells.json"
  jq -r '
    .objects[]
    | [
        .block_number,
        .output.capacity,
        .output.lock.args,
        "\(.out_point.tx_hash):\(.out_point.index)"
      ]
    | @tsv
  ' "$ARTIFACTS_DIR/dao-live-cells.json" \
    | awk 'BEGIN { printf "%-8s %-12s %-44s %s\n", "Block", "Capacity", "Lock Arg", "Out Point" }
           { printf "%-8s %-12s %-44s %s\n", $1, $2, $3, $4 }'

  ALICE_DEPOSIT="$(outpoint_for "$ALICE_LOCK" "30000.0")"
  BOB_DEPOSIT_40K="$(outpoint_for "$BOB_LOCK" "40000.0")"
  BOB_DEPOSIT_25K="$(outpoint_for "$BOB_LOCK" "25000.0")"
  CAROL_DEPOSIT="$(outpoint_for "$CAROL_LOCK" "20000.0")"

  if [[ -z "$ALICE_DEPOSIT" || -z "$BOB_DEPOSIT_40K" || -z "$BOB_DEPOSIT_25K" || -z "$CAROL_DEPOSIT" ]]; then
    echo "failed to identify all demo DAO deposits" >&2
    exit 1
  fi
}

create_snapshot() {
  log "Create shared snapshot"
  SNAPSHOT_BLOCK="$(tip)"
  local output
  output="$("$SCRIPT_DIR/create-snapshot.sh" "$SNAPSHOT_BLOCK")"
  printf '%s\n' "$output"
  SNAPSHOT_FILE="$(printf '%s\n' "$output" | awk -F': ' '/^snapshot_file:/ {print $2}')"
  SNAPSHOT_ID="$(printf '%s\n' "$output" | awk -F': ' '/^snapshot_id:/ {print $2}')"
  SNAPSHOT_ROOT="$(printf '%s\n' "$output" | awk -F': ' '/^snapshot_root:/ {print $2}')"
  info "snapshot_file: $SNAPSHOT_FILE"
}

create_and_submit_proposal() {
  log "Create and submit Proposal Session Cell"
  local summary
  summary="$("$SCRIPT_DIR/proposal.py" create-sample \
    --snapshot "$SNAPSHOT_FILE" \
    --output-dir "$ARTIFACTS_DIR")"
  printf '%s\n' "$summary"

  PROPOSAL_FILE="$(printf '%s\n' "$summary" | jq -r '.manifest_path')"
  PROPOSAL_ID="$(printf '%s\n' "$summary" | jq -r '.proposal_id')"
  VOTE_START_BLOCK="$(printf '%s\n' "$summary" | jq -r '.vote_start_block')"
  VOTE_END_BLOCK="$(printf '%s\n' "$summary" | jq -r '.vote_end_block')"

  local submit_output
  submit_output="$("$SCRIPT_DIR/submit-proposal-cell.sh" "$PROPOSAL_FILE" "$GOVERNANCE_TYPE_CODE_OUT_POINT")"
  printf '%s\n' "$submit_output"

  PROPOSAL_TX="$(jq -r '.chain.tx_hash' "$PROPOSAL_FILE")"
  info "proposal_id: $PROPOSAL_ID"
  info "proposal_tx: $PROPOSAL_TX"
  info "vote window: $VOTE_START_BLOCK..$VOTE_END_BLOCK"
}

advance_to_vote_window() {
  log "Advance to voting window"
  advance_to_block "$((VOTE_START_BLOCK - 1))"
}

create_vote() {
  local voter="$1"
  local deposit="$2"
  local choice="$3"

  log "Create and submit vote: $voter -> $choice"
  local summary
  summary="$(PROPOSAL_FILE="$PROPOSAL_FILE" SNAPSHOT_FILE="$SNAPSHOT_FILE" "$SCRIPT_DIR/vote.py" create \
    --proposal "$PROPOSAL_FILE" \
    --snapshot "$SNAPSHOT_FILE" \
    --deposit-out-point "$deposit" \
    --choice "$choice" \
    --output-dir "$ARTIFACTS_DIR")"
  printf '%s\n' "$summary"

  local vote_file
  vote_file="$(printf '%s\n' "$summary" | jq -r '.vote_path')"
  "$SCRIPT_DIR/submit-vote-cell.sh" "$voter" "$vote_file" "$GOVERNANCE_TYPE_CODE_OUT_POINT"
}

submit_votes() {
  create_vote "alice" "$ALICE_DEPOSIT" "yes"
  create_vote "bob" "$BOB_DEPOSIT_40K" "yes"
  create_vote "bob" "$BOB_DEPOSIT_25K" "no"
  create_vote "carol" "$CAROL_DEPOSIT" "no"
}

finish_vote_window() {
  log "Advance to vote end"
  advance_to_block "$VOTE_END_BLOCK"
}

create_and_verify_tally_artifact() {
  log "Create and verify independent tally artifact"
  local output
  output="$(PROPOSAL_FILE="$PROPOSAL_FILE" SNAPSHOT_FILE="$SNAPSHOT_FILE" "$SCRIPT_DIR/tally-proposal.sh")"
  printf '%s\n' "$output"
  TALLY_FILE="$(printf '%s\n' "$output" | jq -r '.tally_path')"
  TALLY_ROOT="$(printf '%s\n' "$output" | jq -r '.tally_root')"

  PROPOSAL_FILE="$PROPOSAL_FILE" SNAPSHOT_FILE="$SNAPSHOT_FILE" "$SCRIPT_DIR/verify-tally.sh" "$TALLY_FILE"
  info "tally_file: $TALLY_FILE"
}

start_tally_service() {
  log "Start Rust Tally service"
  stop_tally_service
  TALLY_SERVICE_PORT="$TALLY_SERVICE_PORT" nohup "$SCRIPT_DIR/start-tally-service.sh" \
    >"$DAO_TREASURY_DIR/logs/tally-service.log" 2>&1 &
  local pid="$!"
  echo "$pid" >"$TALLY_PID_FILE"

  for _ in $(seq 1 60); do
    if curl -sSf "$TALLY_RPC_URL/health" >/dev/null 2>&1; then
      info "tally service: $TALLY_RPC_URL pid=$pid"
      return 0
    fi
    sleep 1
  done
  echo "Tally service did not become ready; log: $DAO_TREASURY_DIR/logs/tally-service.log" >&2
  exit 1
}

query_tally_service() {
  log "Query Rust Tally service"
  local proposals voter_options tally owner_proof
  proposals="$(json_rpc tally.get_proposals '{"include_manifest":false}')"
  voter_options="$(json_rpc tally.get_voter_options '{"voter":"alice"}')"
  tally="$(json_rpc tally.get_tally "{\"proposal_id\":\"$PROPOSAL_ID\"}")"
  owner_proof="$(json_rpc tally.get_owner_proof "{\"snapshot_id\":\"$SNAPSHOT_ID\",\"owner_key_or_lock_arg\":\"$ALICE_LOCK\"}")"

  SERVICE_TALLY_ROOT="$(printf '%s\n' "$tally" | jq -r '.result.tally.tally_root')"
  SERVICE_YES="$(printf '%s\n' "$tally" | jq -r '.result.tally.choice_weights_ckb.yes')"
  SERVICE_NO="$(printf '%s\n' "$tally" | jq -r '.result.tally.choice_weights_ckb.no')"
  SERVICE_ABSTAIN="$(printf '%s\n' "$tally" | jq -r '.result.tally.choice_weights_ckb.abstain')"
  SERVICE_IS_FINAL="$(printf '%s\n' "$tally" | jq -r '.result.tally.is_final')"
  ALICE_WEIGHT="$(printf '%s\n' "$voter_options" | jq -r '.result.proposals[0].eligible_weight_ckb')"
  OWNER_RECORD_COUNT="$(printf '%s\n' "$owner_proof" | jq -r '.result.verified.record_count')"

  printf '%s\n' "$proposals" >"$ARTIFACTS_DIR/service-proposals.json"
  printf '%s\n' "$voter_options" >"$ARTIFACTS_DIR/service-alice-options.json"
  printf '%s\n' "$tally" >"$ARTIFACTS_DIR/service-tally.json"
  printf '%s\n' "$owner_proof" >"$ARTIFACTS_DIR/service-alice-owner-proof.json"

  info "proposal_count: $(printf '%s\n' "$proposals" | jq -r '.result.proposal_count')"
  info "alice eligible weight: $ALICE_WEIGHT CKB"
  info "alice owner proof record_count: $OWNER_RECORD_COUNT"
  info "service tally_root: $SERVICE_TALLY_ROOT"
  info "weights: yes=$SERVICE_YES no=$SERVICE_NO abstain=$SERVICE_ABSTAIN"
  info "is_final: $SERVICE_IS_FINAL"

  if [[ "$SERVICE_TALLY_ROOT" != "$TALLY_ROOT" ]]; then
    echo "service tally root mismatch: $SERVICE_TALLY_ROOT != $TALLY_ROOT" >&2
    exit 1
  fi
}

write_summary() {
  log "Write demo summary"
  jq -n \
    --arg rpc_url "$CKB_RPC_URL" \
    --arg tally_rpc_url "$TALLY_RPC_URL" \
    --arg tip "$(tip)" \
    --arg gov_code_hash "$GOVERNANCE_TYPE_CODE_HASH" \
    --arg gov_code_out_point "$GOVERNANCE_TYPE_CODE_OUT_POINT" \
    --arg snapshot_file "$SNAPSHOT_FILE" \
    --arg snapshot_id "$SNAPSHOT_ID" \
    --arg snapshot_root "$SNAPSHOT_ROOT" \
    --arg snapshot_block "$SNAPSHOT_BLOCK" \
    --arg proposal_file "$PROPOSAL_FILE" \
    --arg proposal_id "$PROPOSAL_ID" \
    --arg proposal_tx "$PROPOSAL_TX" \
    --arg vote_start_block "$VOTE_START_BLOCK" \
    --arg vote_end_block "$VOTE_END_BLOCK" \
    --arg tally_file "$TALLY_FILE" \
    --arg tally_root "$TALLY_ROOT" \
    --arg alice_deposit "$ALICE_DEPOSIT" \
    --arg bob_deposit_40k "$BOB_DEPOSIT_40K" \
    --arg bob_deposit_25k "$BOB_DEPOSIT_25K" \
    --arg carol_deposit "$CAROL_DEPOSIT" \
    --arg yes "$SERVICE_YES" \
    --arg no "$SERVICE_NO" \
    --arg abstain "$SERVICE_ABSTAIN" \
    '{
      rpc_url: $rpc_url,
      tally_rpc_url: $tally_rpc_url,
      tip_block_number: ($tip | tonumber),
      governance_type: {
        code_hash: $gov_code_hash,
        hash_type: "data",
        code_out_point: $gov_code_out_point
      },
      deposits: {
        alice: $alice_deposit,
        bob_40000: $bob_deposit_40k,
        bob_25000: $bob_deposit_25k,
        carol: $carol_deposit
      },
      snapshot: {
        file: $snapshot_file,
        snapshot_id: $snapshot_id,
        snapshot_root: $snapshot_root,
        block_number: ($snapshot_block | tonumber)
      },
      proposal: {
        file: $proposal_file,
        proposal_id: $proposal_id,
        tx_hash: $proposal_tx,
        vote_start_block: ($vote_start_block | tonumber),
        vote_end_block: ($vote_end_block | tonumber)
      },
      tally: {
        file: $tally_file,
        tally_root: $tally_root,
        weights_ckb: {
          yes: $yes,
          no: $no,
          abstain: $abstain
        }
      }
    }' >"$DEMO_SUMMARY"
  info "summary: $DEMO_SUMMARY"
}

main() {
  reset_demo_data
  start_node
  fund_accounts
  deploy_governance_type_code
  create_deposits
  record_dao_deposits
  create_snapshot
  create_and_submit_proposal
  advance_to_vote_window
  submit_votes
  finish_vote_window
  create_and_verify_tally_artifact
  start_tally_service
  query_tally_service
  write_summary

  log "Demo completed"
  info "CKB RPC: $CKB_RPC_URL"
  info "Tally RPC: $TALLY_RPC_URL"
  info "proposal_id: $PROPOSAL_ID"
  info "snapshot_id: $SNAPSHOT_ID"
  info "tally_root: $TALLY_ROOT"
  info "final weights: yes=$SERVICE_YES no=$SERVICE_NO abstain=$SERVICE_ABSTAIN"
}

main "$@"
