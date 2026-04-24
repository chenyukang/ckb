use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use sp1_sdk::{
    Elf, HashableKey, SP1Proof, SP1ProofWithPublicValues, SP1Stdin,
    blocking::{ProveRequest, Prover, ProverClient},
    include_elf,
};
use sp1_sdk::ProvingKey;
use sp1_settlement_smoke_lib::{
    SettlementInput, SettlementPublicValues, compute_settlement_root, decode_public_values,
    format_hash_hex, parse_hash_hex, public_values_from_input,
};
use std::{fs, path::PathBuf};

const SETTLEMENT_SMOKE_ELF: Elf = include_elf!("sp1-settlement-smoke-program");

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
enum ProofSystem {
    Core,
    Plonk,
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

    #[arg(long, default_value = "0xc27689d3b08472f648e59d96e2519174e65127e2264ee1ebd898f84d434bbaf7")]
    proposal_id: String,

    #[arg(long, default_value = "0x0441f2798b6324b284e9d0016d37dc35df34d855bc750f0ce20a542f83c4d830")]
    snapshot_id: String,

    #[arg(long, default_value = "0xa42570824d60f590a69afd6b46c6c3d903a6dfc14bb2126959259108f1b732be")]
    tally_root: String,

    #[arg(long)]
    output: Option<PathBuf>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
struct ProofFixture {
    mode: String,
    proof_kind: String,
    proposal_id: String,
    snapshot_id: String,
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
    let client = ProverClient::from_env();
    let input = build_input(&args).unwrap_or_else(|err| {
        eprintln!("invalid input: {err}");
        std::process::exit(1);
    });

    let mut stdin = SP1Stdin::new();
    stdin.write(&input);

    if args.execute {
        let (output, report) = client.execute(SETTLEMENT_SMOKE_ELF, stdin).run().unwrap();
        let public_values = decode_and_check(&input, output.as_slice()).unwrap();
        println!("Program executed successfully.");
        print_public_values(&public_values);
        println!("Number of cycles: {}", report.total_instruction_count());
        return;
    }

    let pk = client
        .setup(SETTLEMENT_SMOKE_ELF)
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
    };

    client
        .verify(&proof, pk.verifying_key(), None)
        .expect("failed to verify proof");

    let public_values = decode_and_check(&input, proof.public_values.as_slice()).unwrap();
    println!("Successfully generated and verified {:?} proof.", args.system);
    print_public_values(&public_values);

    let fixture = build_fixture(&args, &input, &public_values, &proof, &pk);
    let output_path = args.output.unwrap_or_else(|| default_output_path(args.system));
    write_fixture(&output_path, &fixture).expect("failed to write proof fixture");
    println!("fixture: {}", output_path.display());
}

fn build_input(args: &Args) -> Result<SettlementInput, String> {
    Ok(SettlementInput {
        proposal_id: parse_hash_hex(&args.proposal_id)?,
        snapshot_id: parse_hash_hex(&args.snapshot_id)?,
        tally_root: parse_hash_hex(&args.tally_root)?,
    })
}

fn decode_and_check(
    input: &SettlementInput,
    public_values: &[u8],
) -> Result<SettlementPublicValues, String> {
    let decoded = decode_public_values(public_values)?;
    let expected = public_values_from_input(input);
    if decoded != expected {
        return Err("decoded public values do not match expected settlement values".to_owned());
    }
    let settlement_root = compute_settlement_root(input);
    if decoded.settlement_root != settlement_root {
        return Err("decoded settlement_root mismatch".to_owned());
    }
    Ok(decoded)
}

fn print_public_values(values: &SettlementPublicValues) {
    println!("proposal_id: {}", format_hash_hex(&values.proposal_id));
    println!("snapshot_id: {}", format_hash_hex(&values.snapshot_id));
    println!("tally_root: {}", format_hash_hex(&values.tally_root));
    println!("settlement_root: {}", format_hash_hex(&values.settlement_root));
}

fn build_fixture(
    args: &Args,
    input: &SettlementInput,
    public_values: &SettlementPublicValues,
    proof: &SP1ProofWithPublicValues,
    pk: &impl ProvingKey,
) -> ProofFixture {
    let (proof_kind, onchain_proof_hex, onchain_proof_len) = match &proof.proof {
        SP1Proof::Core(_) => ("core".to_owned(), None, 0),
        SP1Proof::Compressed(_) => ("compressed".to_owned(), None, 0),
        SP1Proof::Plonk(_) => {
            let bytes = proof.bytes();
            ("plonk".to_owned(), Some(format!("0x{}", hex::encode(&bytes))), bytes.len())
        }
        SP1Proof::Groth16(_) => {
            let bytes = proof.bytes();
            ("groth16".to_owned(), Some(format!("0x{}", hex::encode(&bytes))), bytes.len())
        }
    };

    ProofFixture {
        mode: match args.system {
            ProofSystem::Core => "core".to_owned(),
            ProofSystem::Plonk => "plonk".to_owned(),
        },
        proof_kind,
        proposal_id: format_hash_hex(&input.proposal_id),
        snapshot_id: format_hash_hex(&input.snapshot_id),
        tally_root: format_hash_hex(&input.tally_root),
        settlement_root: format_hash_hex(&public_values.settlement_root),
        public_values_hex: format!("0x{}", hex::encode(proof.public_values.as_slice())),
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
        ProofSystem::Core => "core-proof-fixture.json",
        ProofSystem::Plonk => "plonk-proof-fixture.json",
    };
    artifacts_dir.join(file_name)
}

fn write_fixture(path: &PathBuf, fixture: &ProofFixture) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(fixture).expect("serialize fixture"))
}
