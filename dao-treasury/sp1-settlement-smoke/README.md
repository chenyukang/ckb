# SP1 Settlement Smoke

This is a minimal SP1 integration PoC for the DAO treasury work.

It does not prove the full voting settlement logic yet. Instead, it proves a very small
"settlement-shaped" statement:

```text
given proposal_id, snapshot_id, tally_root
compute settlement_root = ckb_hash(proposal_id || snapshot_id || tally_root)
commit all four values as public outputs
```

Why this exists:

```text
1. prove we can run an SP1 guest / host flow locally
2. produce proof bytes + public values we can later hand to the CKB-VM SP1 verifier
3. keep the first integration tiny before moving the real transcript verifier into SP1
```

Current focus:

```text
- execute and core proof are the primary smoke-test path
- plonk remains in the CLI because we want that path later
- local plonk proving may still require extra Go/C toolchain setup on macOS
```

## Current local status

As of the latest run on this machine:

```text
- execute: works
- core proof: works
- plonk: starts correctly, but first run downloads a large local artifact bundle under ~/.sp1/circuits/plonk/v6.1.0/
```

Current successful smoke values:

```text
proposal_id     = 0xc27689d3b08472f648e59d96e2519174e65127e2264ee1ebd898f84d434bbaf7
snapshot_id     = 0x0441f2798b6324b284e9d0016d37dc35df34d855bc750f0ce20a542f83c4d830
tally_root      = 0xa42570824d60f590a69afd6b46c6c3d903a6dfc14bb2126959259108f1b732be
settlement_root = 0x245aac0ef18175b41d9b8690a0d441d9ec00ebae8e29b2073d1dd92933077ca0
vk_hash         = 0x00cc6c59e6c52be035c0bffd14764f21881a1950ed95fa44835fd5c1c194539c
```

The current exported core fixture is:

```text
sp1-settlement-smoke/artifacts/core-proof-fixture.json
```

## Layout

```text
lib/      shared types and settlement_root computation
program/  SP1 guest
script/   SP1 host (execute / prove / verify / export artifacts)
artifacts/ generated fixtures
```

## Commands

Execute without proving:

```bash
cd dao-treasury/sp1-settlement-smoke/script
cargo run --release -- --execute
```

Or use the wrapper from the repo root:

```bash
dao-treasury/scripts/run-sp1-settlement-smoke.sh execute
```

Generate and verify a core proof:

```bash
cd dao-treasury/sp1-settlement-smoke/script
cargo run --release -- --prove --system core
```

Wrapper:

```bash
dao-treasury/scripts/run-sp1-settlement-smoke.sh core
```

Generate and verify a PLONK proof:

```bash
cd dao-treasury/sp1-settlement-smoke/script
cargo run --release -- --prove --system plonk
```

Wrapper:

```bash
dao-treasury/scripts/run-sp1-settlement-smoke.sh plonk
```

Artifacts are written to:

```text
dao-treasury/sp1-settlement-smoke/artifacts/
```
