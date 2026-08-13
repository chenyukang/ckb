# CKB DAO Treasury Implementation Plan

Last updated: 2026-08-13

Overall status: FUNCTIONAL PROTOTYPE IMPLEMENTED; PRODUCTION VALIDATION IN PROGRESS

## Scope and boundaries

- `~/code/ckb` owns hardfork activation, secondary-issuance accounting, Treasury Cell creation, block assembly, and consensus verification.
- Tally accumulation and proposal passing rules remain in Rust CKB contracts rather than node-native Rust validation.
- Historical burned issuance is never recreated. Activation applies only to the future would-be-burned portion attributable to target blocks at or after activation.
- `secondary_epoch_reward` remains unchanged.

## Implementation steps

| Step | Status | Deliverable |
|---|---|---|
| 1. Freeze accounting and activation rules | COMPLETED | Executable issuance decomposition, activation-boundary rules, and golden vectors |
| 2. Prepare implementation branches | COMPLETED | `dao-treasury-implementation` in CKB and `dao-treasury-contracts` in Treasury Lab |
| 3. Add consensus parameters and issuance decomposition | COMPLETED | Hardfork-gated Treasury parameters and shared reward calculator |
| 4. Create and verify Treasury Cell outputs | COMPLETED | Block assembler, CellbaseVerifier, and RewardVerifier changes |
| 5. Implement Rust contracts | COMPLETED | Proposal, vote, tally, challenge, result/policy, treasury, burn, config, and grant contracts |
| 6. Implement Rust builder and watcher | COMPLETED | CBMT/SMT builder, cross-block watcher, verified CKB RPC adapter, and transaction submission |
| 7. Add end-to-end and adversarial tests | IN PROGRESS | Activation and contract coverage complete; explicit reorg and live devnet scenarios remain |
| 8. Benchmark and freeze production limits | IN PROGRESS | Independent-voter baseline measured; complex event shapes and final parameters remain |

## Step 1 checklist

- [x] Verify the exact current formula for miner, DAO compensation, and would-be-burned secondary issuance.
- [x] Determine whether live Nervos DAO deposits require new deterministic derived state.
- [x] Define activation against the finalized reward target block, not merely the block carrying the Cellbase output.
- [x] Define deterministic rounding and remainder handling.
- [x] Select a Treasury Cell emission cadence that avoids one permanent UTXO per block.
- [x] Add golden-vector tests covering epoch boundaries, finalization delay, activation, and no retrospective issuance.

## Initial implementation parameters

These are development defaults, not final mainnet values:

```text
treasury_ratio              = 100% of future would-be-burned issuance
activation                  = configurable on dev/test chains
emission_interval           = 100 target blocks (development default)
max_events_per_tally_batch  = 100 hard cap; 15-18 independent votes recommended at current target
max_batch_sequence          = 128
max_batch_cycles_target     = 50M to 60M
```

## Decision log

- 2026-08-13: Keep secondary issuance baseline unchanged.
- 2026-08-13: Do not recreate historical burned issuance.
- 2026-08-13: Keep optimistic batch tally and passing-policy logic in CKB contracts.
- 2026-08-13: Avoid a consensus-critical Tally Index in the node.
- 2026-08-13: Investigate aggregated Treasury Cell emission before accepting per-block emission because of UTXO growth, occupied-capacity feedback, and payout input count.
- 2026-08-13: Track counted capacity only for live DAO deposit-phase cells (DAO type and eight zero data bytes); withdrawing-phase cells no longer accrue compensation.
- 2026-08-13: Derive treasury share as `floor(g2 * (C - U - D) / C)`, where `D` is live DAO counted capacity at the target block's parent. Miner and treasury shares round down; remainder stays in the DAO reserve.
- 2026-08-13: Store `D` by block hash as deterministic, rebuildable `BlockExt` state so competing forks retain independent values and canonical reorg selection requires no separate index database.
- 2026-08-13: Aggregate 100 target blocks per Treasury Cell by default; activation and interval boundaries are defined by finalized target block numbers.
- 2026-08-13: Require `activation_block_number > 0`. A node upgrade must schedule activation after its historical-state migration; genesis or retrospective treasury activation is rejected.

## Implementation log

- 2026-08-13: Created this plan and started Step 1.
- 2026-08-13: Created `dao-treasury-implementation` from `develop` in `~/code/ckb`; all implementation uses Git branches rather than worktrees.
- 2026-08-13: Cloned `nervosnetwork/ckb-treasury-lab` to `~/code/ckb-treasury-lab` and created `dao-treasury-contracts` at `88bb25efe81a93b652bb263d40e2921a8338412a`.
- 2026-08-13: Added executable secondary-issuance decomposition and DAO deposit-capacity delta calculation to `ckb-dao`.
- 2026-08-13: Added optional chain-spec treasury parameters: target-block activation, emission interval, and consensus lock script.
- 2026-08-13: Added per-block-hash `DaoTreasuryState` in the existing RocksDB block-extension column, including canonical-history migration and lazy side-fork reconstruction.
- 2026-08-13: Added deterministic pending-amount aggregation. Amounts below minimum cell capacity roll into the next interval without loss.
- 2026-08-13: Added Treasury Cell construction as cellbase output index 1, contextual reward verification, and DAO `S` deduction when treasury issuance is materialized.
- 2026-08-13: Completed Steps 1, 3, and 4; started Rust contract implementation.
- 2026-08-13: Added explicit chain-spec rejection for genesis activation and zero emission intervals.
- 2026-08-13: Implemented ordinary Rust CKB contracts for immutable config, proposal lifecycle, votes, operator-bound TallySession batches, final-only omission challenges, result policy, Treasury payout/burn, and grant timelocks.
- 2026-08-13: Defined `VoteRecord` as the committed preimage containing voter, direction, amount, event cursor, and all DAO outpoints. It supports deterministic revote and DAO-spend removal.
- 2026-08-13: Implemented a shared canonical V1 codec plus CBMT and compiled-SMT verification used by both contracts and the off-chain Rust builder.
- 2026-08-13: Implemented the block-feed watcher core, batch witness construction, candidate construction, and omitted-vote challenge proof construction. RPC polling and transaction submission remain.
- 2026-08-13: Replaced stale node-native and per-block-Treasury documentation with the implemented optimistic batch design.
- 2026-08-13: Added a blocking CKB RPC adapter that fetches serialized canonical blocks, recomputes and checks raw/witness transaction roots, preserves exact header-dep ordering, and submits assembled settlement transactions.
- 2026-08-13: Fixed watcher state continuity across blocks so a later DAO spend invalidates a vote discovered in an earlier block of the same batch.
- 2026-08-13: Added state-preserving empty batches so a proposal with no votes, or a final empty suffix, can still enter the challenge period.
- 2026-08-13: Replaced the DAO-spend challenge's historical-vote condition with proofs that the final DAO SMT still counts the spent outpoint and the spend event is absent. This avoids false slashing after a superseding revote.
- 2026-08-13: Made the DAO type code hash and hash type proposal parameters instead of hard-coding one network, and rejected VoteTxs that spend a DAO cell while using it as voting weight.
- 2026-08-13: Synchronized the implemented V1 design to the Notion page `On chain voting with batch settlement txs` and re-fetched it for verification.
- 2026-08-13: Added a real fork-switch test. Treasury state remains independently addressable for both sibling block hashes after the side branch becomes canonical.
- 2026-08-13: Changed historical migration from per-input transaction lookup to a linear replay with an in-memory live-deposit outpoint map; new databases also derive genesis DAO deposits instead of assuming `D = 0`.

## Validation log

- 2026-08-13: `cargo test -p ckb-dao` passed (15 tests).
- 2026-08-13: `cargo test -p ckb-store --lib` passed (9 tests).
- 2026-08-13: `cargo test -p ckb-migrate --lib` passed.
- 2026-08-13: Treasury block-template integration test passed through activation, two-target aggregation, finalization delay, cellbase emission, and full block verification.
- 2026-08-13: `cargo check --workspace` passed.
- 2026-08-13: Full `ckb-verification` run passed 73/75; the two existing timing-sensitive header tests passed when rerun individually.
- 2026-08-13: `treasury-common` and `tally-builder` unit tests passed, including CBMT, old/new SMT proof compatibility, RPC block-root validation, empty settlement, and cross-block DAO-spend tracking.
- 2026-08-13: RISC-V builds completed for config, proposal, vote, tally, policy, Treasury lock, and grant lock contracts.
- 2026-08-13: `ckb-testtool` passed builder-generated final-batch settlement, empty-window final settlement, omitted-vote bond slash, exact Treasury payout, and expired burn tests.
- 2026-08-13: Final `make clippy && make test` passed for off-chain crates and all seven RISC-V contracts. Contract tests: 8 passed and the cycle benchmark remained intentionally ignored; builder/common tests: 13 passed.
- 2026-08-13: Preliminary independent-voter benchmark measured 3.25M-3.27M CKB-VM cycles per vote. The 1/10/50/100-vote cases used 3.25M/32.33M/163.07M/326.97M cycles and 670/7,505/41,111/85,387 witness bytes.
- 2026-08-13: Final CKB `cargo check --workspace` passed. Targeted `cargo clippy --no-deps --all-targets -- -D warnings` passed for all nine modified packages.
- 2026-08-13: A dependency-inclusive CKB clippy run remains blocked by five pre-existing `needless_return` warnings in unmodified `ckb-script` files.
- 2026-08-13: Final CKB tests passed: DAO 15, store 10, migration 1, reward calculator 3, Treasury chain integration 2, plus focused chain-spec and cellbase-verifier tests.
