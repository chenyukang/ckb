use anyhow::{Context, Result};
use ckb_jsonrpc_types::{BlockView as JsonBlockView, TransactionView as JsonTransactionView};
use ckb_types::{core::BlockView as CoreBlockView, packed, prelude::*};
use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sp1_fullscan_voting_lib::{
    decode_public_values, encode_public_values, format_hash_hex, verify_transcript_bytes,
    FullscanPublicValues,
};
use sp1_sdk::ProvingKey;
use sp1_sdk::{
    blocking::{ProveRequest, Prover, ProverClient},
    include_elf, Elf, HashableKey, SP1Proof, SP1ProofWithPublicValues, SP1Stdin,
};
use std::{fs, path::PathBuf};

const FULLSCAN_VOTING_ELF: Elf = include_elf!("sp1-fullscan-voting-program");

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
enum ProofSystem {
    Core,
    Plonk,
    Groth16,
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(long)]
    execute: bool,

    #[arg(long)]
    prove: bool,

    #[arg(long, value_enum, default_value = "core")]
    system: ProofSystem,

    #[arg(long)]
    transcript: PathBuf,

    #[arg(long)]
    output: Option<PathBuf>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
struct ProofFixture {
    mode: String,
    proof_kind: String,
    transcript: String,
    proposal_type_script_hash: String,
    proposal_data_hash: String,
    start_block_hash: String,
    end_block_hash: String,
    report_root: String,
    passed: bool,
    public_values_hex: String,
    public_values_len: usize,
    vk_hash: String,
    onchain_proof_hex: Option<String>,
    onchain_proof_len: usize,
}

fn main() {
    let args = Args::parse();
    if args.execute == args.prove {
        eprintln!("must choose exactly one of --execute or --prove");
        std::process::exit(1);
    }

    sp1_sdk::utils::setup_logger();
    let transcript_bytes = load_guest_transcript(&args.transcript).unwrap_or_else(|err| {
        eprintln!(
            "failed to prepare transcript {}: {err:#}",
            args.transcript.display()
        );
        std::process::exit(1);
    });
    let expected = verify_transcript_bytes(&transcript_bytes).unwrap_or_else(|err| {
        eprintln!("host precheck failed: {err:#}");
        std::process::exit(1);
    });

    let mut stdin = SP1Stdin::new();
    stdin.write(&transcript_bytes);

    let client = ProverClient::from_env();
    if args.execute {
        let (output, report) = client
            .execute(FULLSCAN_VOTING_ELF, stdin)
            .run()
            .expect("failed to execute SP1 guest");
        let public_values = decode_and_check(&expected.public_values, output.as_slice());
        println!("Full-scan voting guest executed successfully.");
        print_public_values(&public_values);
        println!("Number of cycles: {}", report.total_instruction_count());
        return;
    }

    let pk = client
        .setup(FULLSCAN_VOTING_ELF)
        .expect("failed to setup elf");
    let proof = match args.system {
        ProofSystem::Core => client
            .prove(&pk, stdin)
            .run()
            .expect("failed to generate core proof"),
        ProofSystem::Plonk => client
            .prove(&pk, stdin)
            .plonk()
            .run()
            .expect("failed to generate plonk proof"),
        ProofSystem::Groth16 => client
            .prove(&pk, stdin)
            .groth16()
            .run()
            .expect("failed to generate groth16 proof"),
    };
    client
        .verify(&proof, pk.verifying_key(), None)
        .expect("failed to verify proof");

    let public_values = decode_and_check(&expected.public_values, proof.public_values.as_slice());
    println!(
        "Successfully generated and verified {:?} full-scan voting proof.",
        args.system
    );
    print_public_values(&public_values);

    let fixture = build_fixture(&args, &public_values, &proof, &pk);
    let output_path = args
        .output
        .unwrap_or_else(|| default_output_path(args.system));
    write_fixture(&output_path, &fixture).expect("failed to write proof fixture");
    println!("fixture: {}", output_path.display());
}

fn decode_and_check(expected: &FullscanPublicValues, public_values: &[u8]) -> FullscanPublicValues {
    let decoded = decode_public_values(public_values).expect("decode public values");
    if &decoded != expected {
        panic!("guest public values do not match host-verified transcript values");
    }
    decoded
}

fn print_public_values(values: &FullscanPublicValues) {
    println!(
        "proposal_type_script_hash: {}",
        format_hash_hex(&values.proposal_type_script_hash)
    );
    println!(
        "proposal_data_hash: {}",
        format_hash_hex(&values.proposal_data_hash)
    );
    println!(
        "start_block_hash: {}",
        format_hash_hex(&values.start_block_hash)
    );
    println!(
        "end_block_hash: {}",
        format_hash_hex(&values.end_block_hash)
    );
    println!("report_root: {}", format_hash_hex(&values.report_root));
    println!("passed: {}", values.passed);
}

fn build_fixture(
    args: &Args,
    public_values: &FullscanPublicValues,
    proof: &SP1ProofWithPublicValues,
    pk: &impl ProvingKey,
) -> ProofFixture {
    let (proof_kind, onchain_proof_hex, onchain_proof_len) = match &proof.proof {
        SP1Proof::Core(_) => ("core".to_owned(), None, 0),
        SP1Proof::Compressed(_) => ("compressed".to_owned(), None, 0),
        SP1Proof::Plonk(_) => {
            let bytes = proof.bytes();
            (
                "plonk".to_owned(),
                Some(format!("0x{}", hex::encode(&bytes))),
                bytes.len(),
            )
        }
        SP1Proof::Groth16(_) => {
            let bytes = proof.bytes();
            (
                "groth16".to_owned(),
                Some(format!("0x{}", hex::encode(&bytes))),
                bytes.len(),
            )
        }
    };
    ProofFixture {
        mode: match args.system {
            ProofSystem::Core => "core".to_owned(),
            ProofSystem::Plonk => "plonk".to_owned(),
            ProofSystem::Groth16 => "groth16".to_owned(),
        },
        proof_kind,
        transcript: args.transcript.display().to_string(),
        proposal_type_script_hash: format_hash_hex(&public_values.proposal_type_script_hash),
        proposal_data_hash: format_hash_hex(&public_values.proposal_data_hash),
        start_block_hash: format_hash_hex(&public_values.start_block_hash),
        end_block_hash: format_hash_hex(&public_values.end_block_hash),
        report_root: format_hash_hex(&public_values.report_root),
        passed: public_values.passed,
        public_values_hex: format!("0x{}", hex::encode(encode_public_values(public_values))),
        public_values_len: proof.public_values.as_slice().len(),
        vk_hash: pk.verifying_key().bytes32().to_string(),
        onchain_proof_hex,
        onchain_proof_len,
    }
}

fn default_output_path(system: ProofSystem) -> PathBuf {
    let artifacts_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../artifacts");
    let _ = fs::create_dir_all(&artifacts_dir);
    let file_name = match system {
        ProofSystem::Core => "core-fullscan-voting-fixture.json",
        ProofSystem::Plonk => "plonk-fullscan-voting-fixture.json",
        ProofSystem::Groth16 => "groth16-fullscan-voting-fixture.json",
    };
    artifacts_dir.join(file_name)
}

fn write_fixture(path: &PathBuf, fixture: &ProofFixture) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        path,
        serde_json::to_vec_pretty(fixture).expect("serialize fixture"),
    )
}

fn load_guest_transcript(path: &PathBuf) -> Result<Vec<u8>> {
    let transcript_bytes =
        fs::read(path).with_context(|| format!("read transcript {}", path.display()))?;
    let mut transcript: Value =
        serde_json::from_slice(&transcript_bytes).context("parse fullscan transcript json")?;
    add_transaction_molecule_witnesses(&mut transcript)?;
    add_compact_block_witnesses(&mut transcript)?;
    serde_json::to_vec(&transcript).context("serialize guest transcript")
}

fn add_transaction_molecule_witnesses(transcript: &mut Value) -> Result<()> {
    let blocks = transcript
        .get_mut("blocks")
        .and_then(Value::as_array_mut)
        .context("blocks is not array")?;
    for (block_index, block) in blocks.iter_mut().enumerate() {
        let txs = block
            .get_mut("transactions")
            .and_then(Value::as_array_mut)
            .with_context(|| format!("blocks[{block_index}].transactions is not array"))?;
        for (tx_index, tx) in txs.iter_mut().enumerate() {
            add_transaction_molecule_witness(
                tx,
                &format!("blocks[{block_index}].transactions[{tx_index}]"),
            )?;
        }
    }
    Ok(())
}

fn add_transaction_molecule_witness(tx: &mut Value, label: &str) -> Result<()> {
    let tx_view: JsonTransactionView = serde_json::from_value(tx.clone())
        .with_context(|| format!("parse {label} as CKB jsonrpc transaction"))?;
    let packed_tx: packed::Transaction = tx_view.inner.clone().into();
    let raw_tx = packed_tx.raw();
    let raw_hash = packed_hex(&raw_tx.calc_tx_hash());
    let declared_hash = format!("0x{}", hex::encode(tx_view.hash.0));
    if raw_hash != declared_hash {
        anyhow::bail!("{label} raw transaction hash mismatch: {raw_hash} != {declared_hash}");
    }

    tx.as_object_mut()
        .with_context(|| format!("{label} is not an object"))?
        .insert(
            "raw_transaction_molecule_hex".to_owned(),
            Value::String(format!("0x{}", hex::encode(raw_tx.as_slice()))),
        );
    Ok(())
}

fn add_compact_block_witnesses(transcript: &mut Value) -> Result<()> {
    let blocks = transcript
        .get_mut("blocks")
        .and_then(Value::as_array_mut)
        .context("blocks is not array")?;
    for (index, block) in blocks.iter_mut().enumerate() {
        add_compact_block_witness(block, &format!("blocks[{index}]"))?;
    }
    Ok(())
}

fn add_compact_block_witness(block: &mut Value, label: &str) -> Result<()> {
    let json_block: JsonBlockView = serde_json::from_value(block.clone())
        .with_context(|| format!("parse {label} as CKB jsonrpc block"))?;
    let core_block: CoreBlockView = json_block.into();
    let tx_hashes = core_block
        .tx_hashes()
        .iter()
        .map(packed_hex)
        .collect::<Vec<_>>();
    let witness_hashes = core_block
        .tx_witness_hashes()
        .iter()
        .map(packed_hex)
        .collect::<Vec<_>>();
    let compact = json!({
        "tx_count": tx_hashes.len(),
        "transaction_hashes": tx_hashes,
        "witness_hashes": witness_hashes,
        "raw_transactions_root": packed_hex(&core_block.calc_raw_transactions_root()),
        "witnesses_root": packed_hex(&core_block.calc_witnesses_root()),
        "transactions_root": packed_hex(&core_block.calc_transactions_root()),
    });
    block
        .as_object_mut()
        .with_context(|| format!("{label} is not an object"))?
        .insert("compact".to_owned(), compact);
    Ok(())
}

fn packed_hex<T: Entity>(entity: &T) -> String {
    format!("0x{}", hex::encode(entity.as_slice()))
}
