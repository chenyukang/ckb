#![no_main]

sp1_zkvm::entrypoint!(main);

use sp1_voting_settlement_lib::{encode_public_values, verify_transcript_bytes};

pub fn main() {
    let transcript_bytes = sp1_zkvm::io::read::<Vec<u8>>();
    let output = verify_transcript_bytes(&transcript_bytes).expect("valid settlement transcript");
    let public_values = encode_public_values(&output.public_values);
    sp1_zkvm::io::commit_slice(&public_values);
}
