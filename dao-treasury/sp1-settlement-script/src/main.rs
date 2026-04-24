use anyhow::{bail, Context, Result};
use clap::Parser;
use serde::Deserialize;
use std::{fs, path::PathBuf};

const PUBLIC_VALUES_LEN: usize = 32 * 5;
const ONCHAIN_PROOF_PREFIX_LEN: usize = 4;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(long)]
    fixture: PathBuf,

    #[arg(long)]
    expected_vk_hash: Option<String>,

    #[arg(long)]
    allow_placeholder_proof_verifier: bool,
}

#[derive(Debug, Deserialize)]
struct Sp1ProofFixture {
    mode: String,
    proof_kind: String,
    proposal_id: String,
    snapshot_id: String,
    snapshot_root: String,
    tally_root: String,
    settlement_root: String,
    public_values_hex: String,
    public_values_len: usize,
    vk_hash: String,
    onchain_proof_hex: Option<String>,
    onchain_proof_len: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SettlementPublicValues {
    proposal_id: [u8; 32],
    snapshot_id: [u8; 32],
    snapshot_root: [u8; 32],
    tally_root: [u8; 32],
    settlement_root: [u8; 32],
}

trait Sp1OnchainVerifier {
    fn verify(&self, vk_hash: &[u8; 32], public_values: &[u8], proof: &[u8]) -> Result<()>;
}

struct PlaceholderSp1OnchainVerifier;

impl Sp1OnchainVerifier for PlaceholderSp1OnchainVerifier {
    fn verify(&self, _vk_hash: &[u8; 32], _public_values: &[u8], proof: &[u8]) -> Result<()> {
        if proof.len() <= ONCHAIN_PROOF_PREFIX_LEN {
            bail!("on-chain proof payload is too short");
        }
        Ok(())
    }
}

struct MissingSp1OnchainVerifier;

impl Sp1OnchainVerifier for MissingSp1OnchainVerifier {
    fn verify(&self, _vk_hash: &[u8; 32], _public_values: &[u8], _proof: &[u8]) -> Result<()> {
        bail!("real CKB-VM SP1 verifier is not linked yet")
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let fixture = read_fixture(&args.fixture)?;
    let verifier: Box<dyn Sp1OnchainVerifier> = if args.allow_placeholder_proof_verifier {
        Box::new(PlaceholderSp1OnchainVerifier)
    } else {
        Box::new(MissingSp1OnchainVerifier)
    };
    verify_fixture(
        &fixture,
        args.expected_vk_hash.as_deref(),
        verifier.as_ref(),
    )?;
    println!("SP1 settlement script PoC accepted fixture.");
    println!("mode: {}", fixture.mode);
    println!("proof_kind: {}", fixture.proof_kind);
    println!("vk_hash: {}", fixture.vk_hash);
    println!("settlement_root: {}", fixture.settlement_root);
    Ok(())
}

fn read_fixture(path: &PathBuf) -> Result<Sp1ProofFixture> {
    let bytes = fs::read(path).with_context(|| format!("read fixture {}", path.display()))?;
    serde_json::from_slice(&bytes).context("parse SP1 proof fixture")
}

fn verify_fixture(
    fixture: &Sp1ProofFixture,
    expected_vk_hash: Option<&str>,
    verifier: &dyn Sp1OnchainVerifier,
) -> Result<()> {
    require_onchain_proof_kind(&fixture.proof_kind)?;
    if fixture.onchain_proof_len == 0 {
        bail!("fixture has no on-chain proof bytes; generate a Plonk or Groth16 proof");
    }
    let proof_hex = fixture
        .onchain_proof_hex
        .as_deref()
        .context("fixture missing onchain_proof_hex")?;
    let proof = decode_hex(proof_hex).context("decode onchain_proof_hex")?;
    if proof.len() != fixture.onchain_proof_len {
        bail!(
            "onchain_proof_len mismatch: {} != {}",
            proof.len(),
            fixture.onchain_proof_len
        );
    }

    let public_values =
        decode_hex(&fixture.public_values_hex).context("decode public_values_hex")?;
    if public_values.len() != PUBLIC_VALUES_LEN {
        bail!(
            "public_values length mismatch: {} != {}",
            public_values.len(),
            PUBLIC_VALUES_LEN
        );
    }
    if fixture.public_values_len != PUBLIC_VALUES_LEN {
        bail!(
            "fixture public_values_len mismatch: {} != {}",
            fixture.public_values_len,
            PUBLIC_VALUES_LEN
        );
    }
    let decoded = decode_public_values(&public_values)?;
    require_hash_field("proposal_id", &fixture.proposal_id, &decoded.proposal_id)?;
    require_hash_field("snapshot_id", &fixture.snapshot_id, &decoded.snapshot_id)?;
    require_hash_field(
        "snapshot_root",
        &fixture.snapshot_root,
        &decoded.snapshot_root,
    )?;
    require_hash_field("tally_root", &fixture.tally_root, &decoded.tally_root)?;
    require_hash_field(
        "settlement_root",
        &fixture.settlement_root,
        &decoded.settlement_root,
    )?;

    let vk_hash = parse_hash_hex(&fixture.vk_hash).context("parse fixture vk_hash")?;
    if let Some(expected) = expected_vk_hash {
        let expected = parse_hash_hex(expected).context("parse expected_vk_hash")?;
        if vk_hash != expected {
            bail!("vk_hash does not match script args");
        }
    }
    verifier.verify(&vk_hash, &public_values, &proof)
}

fn require_onchain_proof_kind(proof_kind: &str) -> Result<()> {
    match proof_kind {
        "plonk" | "groth16" => Ok(()),
        other => {
            bail!("proof kind {other:?} is not on-chain verifiable; expected plonk or groth16")
        }
    }
}

fn decode_public_values(bytes: &[u8]) -> Result<SettlementPublicValues> {
    if bytes.len() != PUBLIC_VALUES_LEN {
        bail!("invalid public values length");
    }
    Ok(SettlementPublicValues {
        proposal_id: bytes[0..32].try_into().context("proposal_id")?,
        snapshot_id: bytes[32..64].try_into().context("snapshot_id")?,
        snapshot_root: bytes[64..96].try_into().context("snapshot_root")?,
        tally_root: bytes[96..128].try_into().context("tally_root")?,
        settlement_root: bytes[128..160].try_into().context("settlement_root")?,
    })
}

fn require_hash_field(label: &str, hex_value: &str, expected: &[u8; 32]) -> Result<()> {
    let actual = parse_hash_hex(hex_value).with_context(|| format!("parse {label}"))?;
    if &actual != expected {
        bail!("{label} does not match public values");
    }
    Ok(())
}

fn parse_hash_hex(value: &str) -> Result<[u8; 32]> {
    let decoded = decode_hex(value)?;
    if decoded.len() != 32 {
        bail!("expected 32-byte hash, got {} bytes", decoded.len());
    }
    Ok(decoded.try_into().expect("length checked"))
}

fn decode_hex(value: &str) -> Result<Vec<u8>> {
    hex::decode(value.strip_prefix("0x").unwrap_or(value)).context("decode hex")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(byte: u8) -> String {
        format!("0x{}", hex::encode([byte; 32]))
    }

    fn fixture(proof_kind: &str) -> Sp1ProofFixture {
        let proposal_id = hash(1);
        let snapshot_id = hash(2);
        let snapshot_root = hash(3);
        let tally_root = hash(4);
        let settlement_root = hash(5);
        let public_values_hex = format!(
            "0x{}{}{}{}{}",
            hex::encode([1u8; 32]),
            hex::encode([2u8; 32]),
            hex::encode([3u8; 32]),
            hex::encode([4u8; 32]),
            hex::encode([5u8; 32]),
        );
        Sp1ProofFixture {
            mode: proof_kind.to_owned(),
            proof_kind: proof_kind.to_owned(),
            proposal_id,
            snapshot_id,
            snapshot_root,
            tally_root,
            settlement_root,
            public_values_hex,
            public_values_len: PUBLIC_VALUES_LEN,
            vk_hash: hash(9),
            onchain_proof_hex: Some("0x010203040506".to_owned()),
            onchain_proof_len: 6,
        }
    }

    #[test]
    fn rejects_core_proof() {
        let fixture = fixture("core");
        let err = verify_fixture(&fixture, None, &PlaceholderSp1OnchainVerifier).unwrap_err();
        assert!(err.to_string().contains("not on-chain verifiable"));
    }

    #[test]
    fn accepts_plonk_envelope_with_placeholder_verifier() {
        let fixture = fixture("plonk");
        verify_fixture(
            &fixture,
            Some("0x0909090909090909090909090909090909090909090909090909090909090909"),
            &PlaceholderSp1OnchainVerifier,
        )
        .unwrap();
    }

    #[test]
    fn rejects_public_value_mismatch() {
        let mut fixture = fixture("groth16");
        fixture.tally_root = hash(7);
        let err = verify_fixture(&fixture, None, &PlaceholderSp1OnchainVerifier).unwrap_err();
        assert!(err.to_string().contains("tally_root does not match"));
    }

    #[test]
    fn real_verifier_boundary_is_explicit() {
        let fixture = fixture("plonk");
        let err = verify_fixture(&fixture, None, &MissingSp1OnchainVerifier).unwrap_err();
        assert!(err.to_string().contains("real CKB-VM SP1 verifier"));
    }
}
