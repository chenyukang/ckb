#![no_main]

sp1_zkvm::entrypoint!(main);

use sp1_settlement_smoke_lib::{SettlementInput, encode_public_values, public_values_from_input};

pub fn main() {
    let input = sp1_zkvm::io::read::<SettlementInput>();
    let public_values = public_values_from_input(&input);
    let bytes = encode_public_values(&public_values);
    sp1_zkvm::io::commit_slice(&bytes);
}
