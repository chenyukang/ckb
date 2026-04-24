use anyhow::{Context, Result};
use ckb_jsonrpc_types::BlockView as JsonBlockView;
use ckb_jsonrpc_types::{
    Either, TransactionView as JsonTransactionView, TransactionWithStatusResponse,
};
use ckb_types::{core::BlockView as CoreBlockView, packed, prelude::*};
use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sp1_sdk::ProvingKey;
use sp1_sdk::{
    blocking::{ProveRequest, Prover, ProverClient},
    include_elf, Elf, HashableKey, SP1Proof, SP1ProofWithPublicValues, SP1Stdin,
};
use sp1_voting_settlement_lib::{
    decode_public_values, encode_public_values, format_hash_hex, verify_transcript_bytes,
    SettlementPublicValues,
};
use std::{fs, path::PathBuf};

const VOTING_SETTLEMENT_ELF: Elf = include_elf!("sp1-voting-settlement-program");

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
            .execute(VOTING_SETTLEMENT_ELF, stdin)
            .run()
            .expect("failed to execute SP1 guest");
        let public_values = decode_and_check(&expected.public_values, output.as_slice());
        println!("Voting settlement guest executed successfully.");
        print_public_values(&public_values);
        println!("Number of cycles: {}", report.total_instruction_count());
        return;
    }

    let pk = client
        .setup(VOTING_SETTLEMENT_ELF)
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
        "Successfully generated and verified {:?} voting settlement proof.",
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

fn decode_and_check(
    expected: &SettlementPublicValues,
    public_values: &[u8],
) -> SettlementPublicValues {
    let decoded = decode_public_values(public_values).expect("decode public values");
    if &decoded != expected {
        panic!("guest public values do not match host-verified transcript values");
    }
    decoded
}

fn print_public_values(values: &SettlementPublicValues) {
    println!("proposal_id: {}", format_hash_hex(&values.proposal_id));
    println!("snapshot_id: {}", format_hash_hex(&values.snapshot_id));
    println!("snapshot_root: {}", format_hash_hex(&values.snapshot_root));
    println!("tally_root: {}", format_hash_hex(&values.tally_root));
    println!(
        "settlement_root: {}",
        format_hash_hex(&values.settlement_root)
    );
}

fn build_fixture(
    args: &Args,
    public_values: &SettlementPublicValues,
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
        proposal_id: format_hash_hex(&public_values.proposal_id),
        snapshot_id: format_hash_hex(&public_values.snapshot_id),
        snapshot_root: format_hash_hex(&public_values.snapshot_root),
        tally_root: format_hash_hex(&public_values.tally_root),
        settlement_root: format_hash_hex(&public_values.settlement_root),
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
        ProofSystem::Core => "core-voting-settlement-fixture.json",
        ProofSystem::Plonk => "plonk-voting-settlement-fixture.json",
        ProofSystem::Groth16 => "groth16-voting-settlement-fixture.json",
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
        serde_json::from_slice(&transcript_bytes).context("parse settlement transcript json")?;
    add_compact_inclusion_witnesses(&mut transcript)?;
    serde_json::to_vec(&transcript).context("serialize guest transcript")
}

fn add_compact_inclusion_witnesses(transcript: &mut Value) -> Result<()> {
    add_compact_block_witness(
        transcript
            .get_mut("snapshot_block_witness")
            .context("missing snapshot_block_witness")?,
        "snapshot_block_witness",
    )?;

    let votes = transcript
        .get_mut("votes")
        .and_then(Value::as_array_mut)
        .context("votes is not array")?;
    for (vote_index, vote) in votes.iter_mut().enumerate() {
        add_transaction_molecule_witness(
            vote.get_mut("transaction")
                .with_context(|| format!("votes[{vote_index}] missing transaction"))?,
            &format!("votes[{vote_index}].transaction"),
        )?;

        add_compact_block_witness(
            vote.get_mut("containing_block")
                .with_context(|| format!("votes[{vote_index}] missing containing_block"))?,
            &format!("votes[{vote_index}].containing_block"),
        )?;

        let owner_input_cells = vote
            .get_mut("owner_input_cells")
            .and_then(Value::as_array_mut)
            .with_context(|| format!("votes[{vote_index}].owner_input_cells is not array"))?;
        for (input_index, owner_input) in owner_input_cells.iter_mut().enumerate() {
            add_transaction_molecule_witness(
                owner_input
                    .get_mut("previous_transaction")
                    .with_context(|| {
                        format!(
                            "votes[{vote_index}].owner_input_cells[{input_index}] missing previous_transaction"
                        )
                    })?,
                &format!("votes[{vote_index}].owner_input_cells[{input_index}].previous_transaction"),
            )?;

            add_compact_block_witness(
                owner_input
                    .get_mut("previous_containing_block")
                    .with_context(|| {
                        format!(
                            "votes[{vote_index}].owner_input_cells[{input_index}] missing previous_containing_block"
                        )
                    })?,
                &format!("votes[{vote_index}].owner_input_cells[{input_index}].previous_containing_block"),
            )?;
        }
    }

    Ok(())
}

fn add_transaction_molecule_witness(response: &mut Value, label: &str) -> Result<()> {
    let transaction_response: TransactionWithStatusResponse =
        serde_json::from_value(response.clone())
            .with_context(|| format!("parse {label} as CKB jsonrpc transaction response"))?;
    let tx_view = json_transaction_from_response(&transaction_response)
        .with_context(|| format!("{label} missing json transaction payload"))?;
    let packed_tx: packed::Transaction = tx_view.inner.clone().into();
    let raw_tx = packed_tx.raw();
    let raw_hash = packed_hex(&raw_tx.calc_tx_hash());
    let declared_hash = format!("0x{}", hex::encode(tx_view.hash.0));
    if raw_hash != declared_hash {
        anyhow::bail!("{label} raw transaction hash mismatch: {raw_hash} != {declared_hash}");
    }

    response
        .get_mut("transaction")
        .and_then(Value::as_object_mut)
        .with_context(|| format!("{label}.transaction is not an object"))?
        .insert(
            "raw_transaction_molecule_hex".to_owned(),
            Value::String(format!("0x{}", hex::encode(raw_tx.as_slice()))),
        );
    Ok(())
}

fn json_transaction_from_response(
    response: &TransactionWithStatusResponse,
) -> Result<JsonTransactionView> {
    match response
        .transaction
        .clone()
        .context("transaction response missing transaction")?
        .inner
    {
        Either::Left(tx_view) => Ok(tx_view),
        Either::Right(_) => anyhow::bail!("expected json transaction response, got hex payload"),
    }
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
