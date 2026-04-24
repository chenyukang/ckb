# SP1 Settlement Script PoC

This crate models the CKB settlement type script boundary for the DAO treasury SP1 proof.

It intentionally keeps the cryptographic SP1 Plonk/Groth16 verifier behind a tiny trait. The current
local fixture is a core proof, and SP1 core proofs do not have on-chain proof bytes. So this PoC now
does the script-side checks that are independent from the verifier port:

```text
- fixture shape
- proof kind must be Plonk/Groth16 for strict on-chain mode
- on-chain proof payload must be present
- public values must be exactly 160 bytes
- proposal_id / snapshot_id / snapshot_root / tally_root / settlement_root must match public values
- verifying key hash can be pinned by the script args equivalent
```

The command rejects the current core fixture in strict mode, which is the expected behavior:

```bash
dao-treasury/scripts/verify-sp1-settlement-script-poc.sh
```

When a `plonk-voting-settlement-fixture.json` or `groth16-voting-settlement-fixture.json` exists,
pass it as the first argument. The remaining missing piece is replacing the placeholder verifier
trait implementation with the CKB-VM SP1 verifier port.

To exercise the envelope against a Plonk/Groth16 fixture before the verifier port lands:

```bash
ALLOW_PLACEHOLDER=1 dao-treasury/scripts/verify-sp1-settlement-script-poc.sh \
  dao-treasury/sp1-voting-settlement/artifacts/plonk-voting-settlement-fixture.json
```

Without `ALLOW_PLACEHOLDER=1`, Plonk/Groth16 fixtures intentionally fail at the explicit
`real CKB-VM SP1 verifier is not linked yet` boundary.
