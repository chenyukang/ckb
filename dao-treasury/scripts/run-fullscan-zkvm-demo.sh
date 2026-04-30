#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

ARTIFACTS_DIR="$DAO_TREASURY_DIR/artifacts"
SUMMARY_FILE="$ARTIFACTS_DIR/fullscan-demo-summary.json"
GOV_CODE_PATH="$DAO_TREASURY_DIR/../script/testdata/always_success"

log() {
  printf '\n== %s ==\n' "$*"
}

info() {
  printf '  %s\n' "$*"
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

mine_until_committed() {
  local tx_hash="$1"
  "$SCRIPT_DIR/mine-until-committed.sh" "$tx_hash" >/dev/null
  info "committed: $tx_hash"
}

to_decimal() {
  local value="$1"
  if [[ "$value" == 0x* ]]; then
    printf '%d\n' "$((16#${value#0x}))"
  else
    printf '%d\n' "$value"
  fi
}

key_info() {
  "$CKB_CLI" util key-info \
    --privkey-path "$1" \
    --local-only \
    --output-format json \
    2>/dev/null \
    | sed -n '/^{/,$p'
}

key_address() {
  key_info "$1" | jq -r '.address.testnet'
}

key_lock_arg() {
  key_info "$1" | jq -r '.lock_arg'
}

write_key() {
  local name="$1"
  local key="$2"
  printf '%s\n' "$key" >"$DAO_TREASURY_DIR/accounts/$name.privkey"
}

reset_demo_data() {
  log "Reset fullscan demo data"
  "$SCRIPT_DIR/stop-node.sh" >/dev/null 2>&1 || true
  rm -rf "$DAO_TREASURY_DIR/data" "$DAO_TREASURY_DIR/logs"
  rm -f "$DAO_TREASURY_DIR/ckb-node.pid"
  mkdir -p "$DAO_TREASURY_DIR/accounts" "$ARTIFACTS_DIR"
  find "$ARTIFACTS_DIR" -mindepth 1 ! -name README.md -exec rm -rf {} +
  info "removed data/, logs/, and old generated artifacts"
}

write_demo_accounts() {
  log "Write throwaway dev accounts"
  write_key "faucet" "0xd00c06bfd800d27397002dca6fb0993d5ba6399b4238b2f29ee9deb97593d2bc"
  write_key "alice" "0x1111111111111111111111111111111111111111111111111111111111111111"
  write_key "bob" "0x2222222222222222222222222222222222222222222222222222222222222222"
  write_key "carol" "0x3333333333333333333333333333333333333333333333333333333333333333"
  write_key "proposer" "0x4444444444444444444444444444444444444444444444444444444444444444"
  chmod 600 "$DAO_TREASURY_DIR"/accounts/*.privkey

  ALICE_ADDRESS="$(key_address "$DAO_TREASURY_DIR/accounts/alice.privkey")"
  BOB_ADDRESS="$(key_address "$DAO_TREASURY_DIR/accounts/bob.privkey")"
  CAROL_ADDRESS="$(key_address "$DAO_TREASURY_DIR/accounts/carol.privkey")"
  PROPOSER_ADDRESS="$(key_address "$DAO_TREASURY_DIR/accounts/proposer.privkey")"
  ALICE_LOCK="$(key_lock_arg "$DAO_TREASURY_DIR/accounts/alice.privkey")"
  BOB_LOCK="$(key_lock_arg "$DAO_TREASURY_DIR/accounts/bob.privkey")"
  CAROL_LOCK="$(key_lock_arg "$DAO_TREASURY_DIR/accounts/carol.privkey")"
  PROPOSER_LOCK="$(key_lock_arg "$DAO_TREASURY_DIR/accounts/proposer.privkey")"

  info "alice: $ALICE_LOCK"
  info "bob: $BOB_LOCK"
  info "carol: $CAROL_LOCK"
  info "proposer: $PROPOSER_LOCK"
}

start_node() {
  log "Start CKB dev chain"
  "$SCRIPT_DIR/start-node-bg.sh"
  wait_for_node
  info "tip: $(tip)"
}

fund_account() {
  local name="$1"
  local address="$2"
  local capacity="$3"
  info "funding $name with $capacity CKB"
  local tx_hash
  tx_hash="$("$CKB_CLI" --url "$CKB_RPC_URL" wallet transfer \
    --privkey-path "$DAO_TREASURY_DIR/accounts/faucet.privkey" \
    --to-address "$address" \
    --capacity "$capacity" \
    --local-only)"
  mine_until_committed "$tx_hash"
}

fund_accounts() {
  log "Fund fullscan demo accounts"
  fund_account "alice" "$ALICE_ADDRESS" "100000"
  fund_account "bob" "$BOB_ADDRESS" "100000"
  fund_account "carol" "$CAROL_ADDRESS" "100000"
  fund_account "proposer" "$PROPOSER_ADDRESS" "10000"
  info "tip after funding: $(tip)"
}

deploy_governance_type_code() {
  log "Deploy governance type code"
  GOVERNANCE_TYPE_CODE_HASH="$("$CKB_CLI" util blake2b \
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

  GOVERNANCE_TYPE_CODE_OUT_POINT="$tx_hash:0"
  export DAO_TREASURY_GOV_TYPE_CODE_HASH="$GOVERNANCE_TYPE_CODE_HASH"
  export DAO_TREASURY_GOV_TYPE_HASH_TYPE="data"
  info "code_hash: $GOVERNANCE_TYPE_CODE_HASH"
  info "code_out_point: $GOVERNANCE_TYPE_CODE_OUT_POINT"
}

create_dao_deposit() {
  local voter="$1"
  local capacity="$2"
  info "$voter deposits $capacity CKB"
  local tx_hash
  tx_hash="$("$CKB_CLI" --url "$CKB_RPC_URL" dao deposit \
    --privkey-path "$DAO_TREASURY_DIR/accounts/$voter.privkey" \
    --capacity "$capacity" \
    --local-only)"
  mine_until_committed "$tx_hash"
  printf '%s:0\n' "$tx_hash"
}

create_dao_deposits() {
  log "Create DAO deposits"
  ALICE_DEPOSIT="$(create_dao_deposit "alice" "30000" | tail -1)"
  BOB_DEPOSIT="$(create_dao_deposit "bob" "40000" | tail -1)"
  CAROL_DEPOSIT="$(create_dao_deposit "carol" "20000" | tail -1)"
  info "alice deposit: $ALICE_DEPOSIT"
  info "bob deposit: $BOB_DEPOSIT"
  info "carol deposit: $CAROL_DEPOSIT"
}

tx_block_number() {
  local tx_hash="$1"
  "$CKB_CLI" --url "$CKB_RPC_URL" rpc get_transaction \
    --hash "$tx_hash" \
    --output-format json \
    --local-only \
    | jq -r '.tx_status.block_number' \
    | while read -r value; do to_decimal "$value"; done
}

extract_tx_hash() {
  awk '
    /^\{/ { capture = 1 }
    capture { print }
    /^\}/ { exit }
  ' | jq -r '.tx_hash'
}

submit_fullscan_proposal() {
  log "Create and submit fullscan proposal"
  PROPOSAL_FILE="$ARTIFACTS_DIR/fullscan-proposal.json"
  "$SCRIPT_DIR/fullscan-zkvm-voting.sh" create-proposal \
    --duration 12 \
    --minimal-requirement-shannons 1 \
    --owner-lock-blake160 "$PROPOSER_LOCK" \
    --output "$PROPOSAL_FILE"

  local submit_output
  submit_output="$("$SCRIPT_DIR/submit-fullscan-proposal-cell.sh" \
    "$PROPOSAL_FILE" \
    "$GOVERNANCE_TYPE_CODE_OUT_POINT")"
  printf '%s\n' "$submit_output"
  PROPOSAL_TX="$(printf '%s\n' "$submit_output" | extract_tx_hash)"
  PROPOSAL_BLOCK="$(tx_block_number "$PROPOSAL_TX")"
  VOTE_START_BLOCK="$PROPOSAL_BLOCK"
  VOTE_END_BLOCK="$((PROPOSAL_BLOCK + 12))"
  info "proposal_tx: $PROPOSAL_TX"
  info "vote window: $VOTE_START_BLOCK..$VOTE_END_BLOCK"
}

submit_fullscan_vote() {
  local voter="$1"
  local deposit="$2"
  local choice="$3"
  local vote_file="$ARTIFACTS_DIR/fullscan-vote-$voter-$choice.json"

  log "Create and submit fullscan vote: $voter -> $choice"
  "$SCRIPT_DIR/fullscan-zkvm-voting.sh" create-vote \
    --proposal "$PROPOSAL_FILE" \
    --choice "$choice" \
    --output "$vote_file"
  "$SCRIPT_DIR/submit-fullscan-vote-cell.sh" \
    "$voter" \
    "$vote_file" \
    "$deposit" \
    "$GOVERNANCE_TYPE_CODE_OUT_POINT"
}

submit_fullscan_votes() {
  submit_fullscan_vote "alice" "$ALICE_DEPOSIT" "yes"
  submit_fullscan_vote "bob" "$BOB_DEPOSIT" "yes"
  submit_fullscan_vote "carol" "$CAROL_DEPOSIT" "no"
}

finish_vote_window() {
  log "Advance to vote window end"
  local current
  current="$(tip)"
  if (( current < VOTE_END_BLOCK )); then
    "$SCRIPT_DIR/generate-blocks.sh" "$((VOTE_END_BLOCK - current))" >/dev/null
  fi
  info "tip: $(tip)"
}

build_and_verify_transcript() {
  log "Build fullscan transcript and host report"
  TRANSCRIPT_FILE="$ARTIFACTS_DIR/fullscan-transcript.json"
  REPORT_FILE="$ARTIFACTS_DIR/fullscan-report.json"
  "$SCRIPT_DIR/fullscan-zkvm-voting.sh" build-transcript \
    --proposal "$PROPOSAL_FILE" \
    --start-block "$VOTE_START_BLOCK" \
    --end-block "$VOTE_END_BLOCK" \
    --output "$TRANSCRIPT_FILE"
  "$SCRIPT_DIR/fullscan-zkvm-voting.sh" scan \
    --proposal "$PROPOSAL_FILE" \
    --start-block "$VOTE_START_BLOCK" \
    --end-block "$VOTE_END_BLOCK" \
    --output "$REPORT_FILE" >/dev/null
  FULLSCAN_HOST_REPORT_ROOT="$(jq -r '.report_root' "$REPORT_FILE")"
  FULLSCAN_PASSED="$(jq -r '.commitment.passed' "$REPORT_FILE")"
  info "host_report_root: $FULLSCAN_HOST_REPORT_ROOT"
  info "passed: $FULLSCAN_PASSED"
}

run_sp1_execute() {
  log "Run SP1 fullscan execute"
  SP1_EXECUTE_LOG="$ARTIFACTS_DIR/fullscan-sp1-execute.log"
  "$SCRIPT_DIR/run-sp1-fullscan-voting.sh" execute "$TRANSCRIPT_FILE" \
    | tee "$SP1_EXECUTE_LOG"
  SP1_REPORT_ROOT="$(sed -n 's/^report_root: //p' "$SP1_EXECUTE_LOG" | tail -1)"
  SP1_PASSED="$(sed -n 's/^passed: //p' "$SP1_EXECUTE_LOG" | tail -1)"
  SP1_EXECUTE_CYCLES="$(sed -n 's/^Number of cycles: //p' "$SP1_EXECUTE_LOG" | tail -1)"
  if [[ "$SP1_PASSED" != "$FULLSCAN_PASSED" ]]; then
    echo "SP1 pass/fail mismatch: $SP1_PASSED != $FULLSCAN_PASSED" >&2
    exit 1
  fi
  info "sp1_report_root: $SP1_REPORT_ROOT"
  info "sp1_execute_cycles: $SP1_EXECUTE_CYCLES"
}

run_sp1_core() {
  log "Run SP1 fullscan core proof"
  SP1_CORE_FIXTURE="$ARTIFACTS_DIR/core-fullscan-voting-fixture.json"
  "$SCRIPT_DIR/run-sp1-fullscan-voting.sh" core "$TRANSCRIPT_FILE" \
    --output "$SP1_CORE_FIXTURE"
  SP1_PROOF_KIND="$(jq -r '.proof_kind' "$SP1_CORE_FIXTURE")"
  SP1_VK_HASH="$(jq -r '.vk_hash' "$SP1_CORE_FIXTURE")"
  SP1_PUBLIC_VALUES_LEN="$(jq -r '.public_values_len' "$SP1_CORE_FIXTURE")"
  info "core fixture: $SP1_CORE_FIXTURE"
}

write_summary() {
  log "Write fullscan demo summary"
  jq -n \
    --arg rpc_url "$CKB_RPC_URL" \
    --arg tip "$(tip)" \
    --arg gov_code_hash "$GOVERNANCE_TYPE_CODE_HASH" \
    --arg gov_code_out_point "$GOVERNANCE_TYPE_CODE_OUT_POINT" \
    --arg proposal_file "$PROPOSAL_FILE" \
    --arg proposal_tx "$PROPOSAL_TX" \
    --arg proposal_block "$PROPOSAL_BLOCK" \
    --arg vote_start_block "$VOTE_START_BLOCK" \
    --arg vote_end_block "$VOTE_END_BLOCK" \
    --arg alice_deposit "$ALICE_DEPOSIT" \
    --arg bob_deposit "$BOB_DEPOSIT" \
    --arg carol_deposit "$CAROL_DEPOSIT" \
    --arg transcript "$TRANSCRIPT_FILE" \
    --arg report "$REPORT_FILE" \
    --arg host_report_root "$FULLSCAN_HOST_REPORT_ROOT" \
    --arg sp1_report_root "$SP1_REPORT_ROOT" \
    --arg passed "$FULLSCAN_PASSED" \
    --arg execute_log "$SP1_EXECUTE_LOG" \
    --arg core_fixture "$SP1_CORE_FIXTURE" \
    --arg proof_kind "$SP1_PROOF_KIND" \
    --arg vk_hash "$SP1_VK_HASH" \
    --arg public_values_len "$SP1_PUBLIC_VALUES_LEN" \
    --arg cycles "$SP1_EXECUTE_CYCLES" \
    --slurpfile report_json "$REPORT_FILE" \
    '{
      rpc_url: $rpc_url,
      tip_block_number: ($tip | tonumber),
      governance_type: {
        code_hash: $gov_code_hash,
        hash_type: "data",
        code_out_point: $gov_code_out_point
      },
      proposal: {
        file: $proposal_file,
        tx_hash: $proposal_tx,
        block_number: ($proposal_block | tonumber),
        vote_start_block: ($vote_start_block | tonumber),
        vote_end_block: ($vote_end_block | tonumber)
      },
      deposits: {
        alice: $alice_deposit,
        bob: $bob_deposit,
        carol: $carol_deposit
      },
      fullscan: {
        transcript: $transcript,
        report: $report,
        host_report_root: $host_report_root,
        sp1_report_root: $sp1_report_root,
        passed: ($passed == "true"),
        sp1_execute_log: $execute_log,
        sp1_core_fixture: $core_fixture,
        proof_kind: $proof_kind,
        vk_hash: $vk_hash,
        public_values_len: ($public_values_len | tonumber),
        cycles: ($cycles | tonumber),
        blocks_scanned: $report_json[0].commitment.blocks_scanned,
        transactions_scanned: $report_json[0].commitment.transactions_scanned,
        outputs_scanned: $report_json[0].commitment.outputs_scanned,
        vote_outputs_seen: $report_json[0].commitment.vote_outputs_seen,
        valid_vote_count: $report_json[0].commitment.valid_vote_count,
        counted_vote_count: $report_json[0].commitment.counted_vote_count,
        invalid_vote_count: $report_json[0].commitment.invalid_vote_count,
        choice_weights_shannons: $report_json[0].commitment.choice_weights_shannons,
        start_block: $report_json[0].commitment.start_block,
        end_block: $report_json[0].commitment.end_block,
        start_block_hash: $report_json[0].commitment.start_block_hash,
        end_block_hash: $report_json[0].commitment.end_block_hash
      }
    }' >"$SUMMARY_FILE"
  info "summary: $SUMMARY_FILE"
}

main() {
  reset_demo_data
  write_demo_accounts
  start_node
  fund_accounts
  deploy_governance_type_code
  create_dao_deposits
  submit_fullscan_proposal
  submit_fullscan_votes
  finish_vote_window
  build_and_verify_transcript
  run_sp1_execute
  if [[ "${RUN_CORE_PROOF:-1}" == "1" ]]; then
    run_sp1_core
  fi
  write_summary

  log "Fullscan zkVM demo completed"
  info "host_report_root: $FULLSCAN_HOST_REPORT_ROOT"
  info "sp1_report_root: $SP1_REPORT_ROOT"
  info "passed: $FULLSCAN_PASSED"
}

main "$@"
