# SP1 Voting Settlement

This directory runs the real DAO treasury voting settlement transcript inside an SP1 guest.

It builds on the smaller smoke test in `../sp1-settlement-smoke`, but replaces the toy input with
the real transcript produced by:

```text
dao-treasury/scripts/create-zkvm-settlement-transcript.sh
```

Current goal:

```text
transcript JSON
  -> host adds compact block inclusion witnesses
  -> SP1 guest verifies snapshot / vote / tally settlement transcript
  -> SP1 guest recomputes CKB tx hashes from raw transaction Molecule bytes
  -> SP1 guest verifies compact CKB transactions_root inclusion for witnessed tx hashes
  -> guest commits fixed public values
  -> host executes or proves the guest
```

Current public values are five 32-byte hashes:

```text
proposal_id
snapshot_id
snapshot_root
tally_root
settlement_root
```

## Current local status

This path now runs against the real demo transcript:

```text
artifacts/zkvm-settlement-transcript-c27689d3b084.json
```

Successful local commands:

```bash
dao-treasury/scripts/run-sp1-voting-settlement.sh execute
dao-treasury/scripts/run-sp1-voting-settlement.sh core
```

Latest successful public values:

```text
proposal_id     = 0xc27689d3b08472f648e59d96e2519174e65127e2264ee1ebd898f84d434bbaf7
snapshot_id     = 0x0441f2798b6324b284e9d0016d37dc35df34d855bc750f0ce20a542f83c4d830
snapshot_root   = 0x0441f2798b6324b284e9d0016d37dc35df34d855bc750f0ce20a542f83c4d830
tally_root      = 0xa42570824d60f590a69afd6b46c6c3d903a6dfc14bb2126959259108f1b732be
settlement_root = 0x86f66621dcadedde2cd802e0a239b2af969315c4cdff2c468100183635363add
vk_hash         = 0x0075b03e9929a2df39af133d41111d2ce6887ec0cec0f9960a1a99184f2be45e
execute cycles  = 16430991
```

The generated core proof fixture is:

```text
sp1-voting-settlement/artifacts/core-voting-settlement-fixture.json
```

The `settlement_root` matches the existing non-SP1 transcript verifier output.

Plonk status: local Plonk wrapping currently requires Docker for SP1's gnark FFI path. The wrapper
checks `docker info` before starting Plonk proving so it fails early when Docker is not running. On
Apple Silicon, the SP1 gnark Docker image is currently amd64-only, so the wrapper sets
`DOCKER_DEFAULT_PLATFORM=linux/amd64` unless the environment already overrides it.

Groth16 mode is also wired in the host wrapper and writes
`groth16-voting-settlement-fixture.json` when it succeeds.

Latest local Plonk attempt with Docker memory raised to about 22 GB reached gnark's
`constraint system solver done` stage, then the Docker process exited with status 137 and no
`plonk-voting-settlement-fixture.json` was produced. The Plonk circuit artifacts are now fully
installed under `~/.sp1/circuits/plonk/v6.1.0/`; the remaining blocker is the local Docker memory
peak during proof generation.

## Current boundary

The SP1 guest currently proves the real governance settlement logic:

```text
- public input consistency
- header chain witness consistency by declared hashes
- snapshot source -> snapshot roots
- record proof verification
- vote commitment validation
- owner input witness consistency
- raw transaction Molecule bytes -> CKB tx hash recomputation for vote txs and owner previous txs
- compact CKB block `transactions_root` recomputation from `transaction_hashes` and `witness_hashes`
- tx-index inclusion for vote tx hashes and owner previous tx hashes
- duplicate-vote handling
- tally root and settlement root
```

The SP1 guest now avoids pulling full CKB JSON/Molecule dependencies into the RISC-V build. Instead,
the host derives raw transaction Molecule bytes and compact inclusion witnesses from CKB-native data
before execution. The guest hashes the raw transaction bytes with CKB's default blake2b
personalization, then recomputes CKB's CBMT-style `raw_transactions_root`, `witnesses_root`, and
final `transactions_root`.

Current remaining boundary: the guest still does not parse every raw transaction Molecule field. The
vote/owner JSON checks are now tied to a guest-recomputed tx hash, but replacing those JSON checks
with compact Molecule field extraction would make the witness format tighter.
