# Full-Scan zkVM Voting PoC

This is a separate PoC for the article-style voting design where zkVM proves the tally by scanning
the complete voting block window.

It intentionally does not preserve compatibility with the existing snapshot/tally MVP.

## Goal

```text
proposal block + duration window
  -> feed every block in the window to the zkVM guest
  -> guest verifies header continuity and transactions_root
  -> guest verifies each transaction's raw Molecule bytes and JSON fields
  -> guest parses every transaction output
  -> guest finds every matching vote cell
  -> guest validates each vote against DAO deposit evidence
  -> guest computes pass/fail
  -> proposal type script verifies the SP1 proof and unlocks treasury execution
```

Compared with the existing MVP, this route is stronger on completeness: the guest discovers vote
cells itself instead of trusting a tally service to provide the vote witness set. The cost is that
proving grows with the number of blocks, transactions, and outputs in the voting window.

## Article-Compatible Model

Proposal cell data contains:

```text
duration
vote type script
expired_time
description
receiver
amount
minimal_requirement
```

Vote cell data contains:

```text
choice = yes | no
```

Vote validity follows the article:

```text
1. the vote transaction has an input lock proving voter control
2. cell_deps include a DAO deposit cell
3. the DAO deposit lock equals one input lock
4. the DAO deposit capacity is the vote weight
5. later votes overwrite earlier votes from the same voter key
```

## Current Implementation

The first utility is a host-side full-scan verifier:

```text
fullscan.py
```

Commands:

```bash
dao-treasury/scripts/fullscan-zkvm-voting.sh create-proposal \
  --duration 80 \
  --minimal-requirement-shannons 1 \
  --output dao-treasury/artifacts/fullscan-proposal.json

dao-treasury/scripts/fullscan-zkvm-voting.sh create-vote \
  --proposal dao-treasury/artifacts/fullscan-proposal.json \
  --choice yes \
  --output dao-treasury/artifacts/fullscan-vote-alice.json

dao-treasury/scripts/fullscan-zkvm-voting.sh scan \
  --proposal dao-treasury/artifacts/fullscan-proposal.json \
  --start-block <proposal-block> \
  --end-block <proposal-block + duration> \
  --output dao-treasury/artifacts/fullscan-report.json

dao-treasury/scripts/fullscan-zkvm-voting.sh build-transcript \
  --proposal dao-treasury/artifacts/fullscan-proposal.json \
  --start-block <proposal-block> \
  --end-block <proposal-block + duration> \
  --output dao-treasury/artifacts/fullscan-transcript.json
```

The `scan` command currently:

```text
1. reads every block in the selected window from RPC
2. checks parent_hash continuity
3. scans every transaction output
4. finds outputs matching the proposal's vote type script
5. parses vote data
6. inspects the vote transaction's cell_deps
7. finds DAO deposit cell_deps whose lock matches an input lock
8. uses DAO deposit capacity as voting weight
9. keeps the latest valid vote per voter lock key
10. computes pass/fail and a report_root
```

This remains useful as a fast host-side checker for the same transcript shape used by the guest.

The SP1 guest scaffold now lives in:

```text
sp1-fullscan-voting/
```

It consumes the `build-transcript` output, scans every transaction output in the supplied block
window, validates DAO-deposit evidence for discovered vote outputs, computes pass/fail, and commits
public values. The host runner enriches each JSON transaction with raw transaction Molecule bytes;
the guest rehashes those bytes, checks the hash against the compact block transaction list, and
checks the JSON fields used by the tally against the raw Molecule fields.

Submit article-style cells:

```bash
dao-treasury/scripts/submit-fullscan-proposal-cell.sh \
  dao-treasury/artifacts/fullscan-proposal.json \
  <type-code-outpoint>

dao-treasury/scripts/submit-fullscan-vote-cell.sh \
  alice \
  dao-treasury/artifacts/fullscan-vote-alice.json \
  <dao-deposit-outpoint> \
  <type-code-outpoint>
```

Execute the SP1 guest:

```bash
dao-treasury/scripts/run-sp1-fullscan-voting.sh execute \
  dao-treasury/artifacts/fullscan-transcript.json
```

Run the full local-chain demo:

```bash
dao-treasury/scripts/run-fullscan-zkvm-demo.sh
```

The demo resets the local dev-chain data under this worktree, starts CKB, funds throwaway accounts,
creates DAO deposits, submits article-style fullscan proposal/vote cells, builds a full-window
transcript, runs SP1 execute, and then generates a core proof.

Latest local-chain run:

```text
window                  = blocks 27..39
blocks_scanned          = 13
transactions_scanned    = 17
vote_outputs_seen       = 3
valid_vote_count        = 3
counted_vote_count      = 3
choice_weights_shannons = yes 7000000000000, no 2000000000000
passed                  = true
sp1_execute_cycles      = 6,427,243
sp1_core_fixture        = dao-treasury/artifacts/core-fullscan-voting-fixture-rawtx.json
sp1_report_root         = 0xc2ae68afae1958e9828643244b118696fe1b763926d375177648f3e1aa9bde28
```

The Python host report and the SP1 guest fixture currently expose different commitment schemas, so
they intentionally have different root fields in `fullscan-demo-summary.json`. Use the SP1
`report_root` as the zkVM public value.

## Important Open Problems

```text
1. Decide whether vote-time DAO deposit weight is acceptable or whether we add a snapshot/start-block age rule.
2. Add article-style treasury cell release and proposal type-script envelope.
3. Link the proposal type script envelope to SP1 public values.
4. Benchmark larger windows to find the practical proof-time envelope.
```
