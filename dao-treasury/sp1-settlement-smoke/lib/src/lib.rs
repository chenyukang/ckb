#![no_std]

extern crate alloc;

use alloc::{format, string::String};
use blake2b_ref::Blake2bBuilder;
use serde::{Deserialize, Serialize};

const CKB_HASH_PERSONALIZATION: &[u8] = b"ckb-default-hash";
pub const FIXED_HASH_BYTES: usize = 32;
pub const PUBLIC_VALUES_BYTES: usize = FIXED_HASH_BYTES * 4;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettlementInput {
    pub proposal_id: [u8; FIXED_HASH_BYTES],
    pub snapshot_id: [u8; FIXED_HASH_BYTES],
    pub tally_root: [u8; FIXED_HASH_BYTES],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettlementPublicValues {
    pub proposal_id: [u8; FIXED_HASH_BYTES],
    pub snapshot_id: [u8; FIXED_HASH_BYTES],
    pub tally_root: [u8; FIXED_HASH_BYTES],
    pub settlement_root: [u8; FIXED_HASH_BYTES],
}

pub fn compute_settlement_root(input: &SettlementInput) -> [u8; FIXED_HASH_BYTES] {
    let mut hasher = Blake2bBuilder::new(FIXED_HASH_BYTES)
        .personal(CKB_HASH_PERSONALIZATION)
        .build();
    hasher.update(&input.proposal_id);
    hasher.update(&input.snapshot_id);
    hasher.update(&input.tally_root);
    let mut result = [0u8; FIXED_HASH_BYTES];
    hasher.finalize(&mut result);
    result
}

pub fn public_values_from_input(input: &SettlementInput) -> SettlementPublicValues {
    SettlementPublicValues {
        proposal_id: input.proposal_id,
        snapshot_id: input.snapshot_id,
        tally_root: input.tally_root,
        settlement_root: compute_settlement_root(input),
    }
}

pub fn encode_public_values(values: &SettlementPublicValues) -> [u8; PUBLIC_VALUES_BYTES] {
    let mut bytes = [0u8; PUBLIC_VALUES_BYTES];
    bytes[0..32].copy_from_slice(&values.proposal_id);
    bytes[32..64].copy_from_slice(&values.snapshot_id);
    bytes[64..96].copy_from_slice(&values.tally_root);
    bytes[96..128].copy_from_slice(&values.settlement_root);
    bytes
}

pub fn decode_public_values(bytes: &[u8]) -> Result<SettlementPublicValues, String> {
    if bytes.len() != PUBLIC_VALUES_BYTES {
        return Err(format!(
            "invalid public values length: {} != {}",
            bytes.len(),
            PUBLIC_VALUES_BYTES
        ));
    }
    let mut proposal_id = [0u8; FIXED_HASH_BYTES];
    let mut snapshot_id = [0u8; FIXED_HASH_BYTES];
    let mut tally_root = [0u8; FIXED_HASH_BYTES];
    let mut settlement_root = [0u8; FIXED_HASH_BYTES];
    proposal_id.copy_from_slice(&bytes[0..32]);
    snapshot_id.copy_from_slice(&bytes[32..64]);
    tally_root.copy_from_slice(&bytes[64..96]);
    settlement_root.copy_from_slice(&bytes[96..128]);
    Ok(SettlementPublicValues {
        proposal_id,
        snapshot_id,
        tally_root,
        settlement_root,
    })
}

pub fn parse_hash_hex(value: &str) -> Result<[u8; FIXED_HASH_BYTES], String> {
    let trimmed = value.strip_prefix("0x").unwrap_or(value);
    if trimmed.len() != FIXED_HASH_BYTES * 2 {
        return Err(format!("expected 32-byte hex, got {} chars", trimmed.len()));
    }
    let decoded = hex::decode(trimmed).map_err(|err| format!("invalid hex: {err}"))?;
    let mut bytes = [0u8; FIXED_HASH_BYTES];
    bytes.copy_from_slice(&decoded);
    Ok(bytes)
}

pub fn format_hash_hex(bytes: &[u8; FIXED_HASH_BYTES]) -> String {
    format!("0x{}", hex::encode(bytes))
}
