# DAO Treasury zkVM Settlement PoC

This crate is the first step toward a zkVM-based settlement path.

It intentionally does not bind the MVP to a specific zkVM framework yet. Instead, it implements a deterministic "guest-shaped" verifier:

```text
settlement transcript
  -> verify snapshot roots from snapshot records
  -> verify vote proofs against snapshot_root
  -> recompute tally_root
  -> emit settlement_root
```

The current PoC proves arithmetic and commitment consistency for a transcript. It does not yet prove that every witness item is included in the canonical CKB chain. Chain inclusion, header anchoring, and checkpoint transitions are the next PoC step.

Run it through the wrappers:

```bash
dao-treasury/scripts/create-zkvm-settlement-transcript.sh \
  dao-treasury/artifacts/proposal-<short-id>.json \
  dao-treasury/artifacts/snapshot-block-<N>.json \
  dao-treasury/artifacts/tally-<short-id>-block-<M>.json

dao-treasury/scripts/verify-zkvm-settlement-transcript.sh \
  dao-treasury/artifacts/zkvm-settlement-transcript-<short-id>.json
```

For the end-to-end local flow, use the unified wrapper:

```bash
dao-treasury/scripts/run-zkvm-poc.sh --fresh-demo
```

That command will:

```text
1. reset and rerun the local governance demo
2. reuse demo-summary.json to locate proposal / snapshot / tally artifacts
3. create zkVM settlement transcript
4. verify transcript with the Rust guest-shaped verifier
5. print settlement_root / proof_model / anchor block range
```
