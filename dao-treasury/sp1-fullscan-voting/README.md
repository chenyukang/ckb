# SP1 Full-Scan Voting PoC

This crate is the zkVM half of `fullscan-zkvm-voting/`.

The guest input is a full voting-window transcript:

```text
proposal artifact
DAO type hash
start/end block numbers
complete CKB JSON blocks for the window
cell evidence for vote tx inputs and cell_deps
```

The guest:

```text
1. verifies block number continuity and parent_hash continuity
2. verifies compact transaction-root commitments against each block header
3. rehashes each raw transaction Molecule witness and checks it against the compact tx hash list
4. checks the JSON transaction fields used by the tally against raw Molecule fields
5. scans every transaction output in the supplied window
6. discovers outputs matching the proposal vote type script
7. validates each discovered vote against DAO deposit cell_dep evidence
8. keeps the latest vote per voter lock key
9. computes choice weights, pass/fail, and report_root
10. commits public values: proposal hashes, start/end block hashes, report_root, passed
```

This is intentionally separate from the snapshot/tally MVP.

## Current Security Boundary

The guest now binds the supplied JSON transaction view to CKB's native transaction commitment:
the host runner adds raw transaction Molecule bytes for every transaction, and the guest rehashes
those bytes, verifies the compact block roots, and compares `version`, `cell_deps`, `header_deps`,
`inputs`, `outputs`, and `outputs_data` against the raw Molecule fields.

The remaining trust boundary is not transaction completeness inside a supplied block window; it is
whether the supplied window itself is the correct proposal window, and whether DAO weight should be
measured at vote time or under a stricter snapshot/start-block rule.

## Run

Build a transcript from RPC first:

```bash
dao-treasury/scripts/fullscan-zkvm-voting.sh build-transcript \
  --proposal dao-treasury/artifacts/fullscan-proposal.json \
  --start-block <proposal-block> \
  --end-block <proposal-block + duration> \
  --output dao-treasury/artifacts/fullscan-transcript.json
```

Then execute the guest:

```bash
dao-treasury/scripts/run-sp1-fullscan-voting.sh execute \
  dao-treasury/artifacts/fullscan-transcript.json
```

Core proof:

```bash
dao-treasury/scripts/run-sp1-fullscan-voting.sh core \
  dao-treasury/artifacts/fullscan-transcript.json
```

Latest local transcript result:

```text
execute_cycles = 6,427,243
report_root    = 0xc2ae68afae1958e9828643244b118696fe1b763926d375177648f3e1aa9bde28
passed         = true
core_fixture   = dao-treasury/artifacts/core-fullscan-voting-fixture-rawtx.json
```
