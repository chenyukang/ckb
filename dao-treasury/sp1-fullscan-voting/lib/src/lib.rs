use anyhow::{bail, Context, Result};
use blake2b_ref::Blake2bBuilder;
use ckb_gen_types::{packed, prelude::*};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, VecDeque};

const CKB_HASH_PERSONALIZATION: &[u8] = b"ckb-default-hash";
const TRANSCRIPT_MAGIC: &str = "CKB_FULLSCAN_ZKVM_TRANSCRIPT_V1";
const PROPOSAL_MAGIC: &str = "CKB_FULLSCAN_ZKVM_PROPOSAL_V1";
const VOTE_MAGIC: &str = "CKB_FULLSCAN_ZKVM_VOTE_V1";
const REPORT_MAGIC: &str = "CKB_FULLSCAN_ZKVM_TALLY_REPORT_V1";
const DEPOSIT_PHASE_DATA: &str = "0x0000000000000000";
const EMPTY_TALLY_ROOT_TAG: &[u8] = b"CKB_FULLSCAN_ZKVM_EMPTY_TALLY_V1";

pub const FULLSCAN_PUBLIC_VALUES_BYTES: usize = 32 * 5 + 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FullscanPublicValues {
    pub proposal_type_script_hash: [u8; 32],
    pub proposal_data_hash: [u8; 32],
    pub start_block_hash: [u8; 32],
    pub end_block_hash: [u8; 32],
    pub report_root: [u8; 32],
    pub passed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FullscanVerificationOutput {
    pub public_values: FullscanPublicValues,
    pub result_json: Vec<u8>,
}

pub fn verify_transcript_bytes(transcript_bytes: &[u8]) -> Result<FullscanVerificationOutput> {
    let transcript: Value = serde_json::from_slice(transcript_bytes).context("parse transcript")?;
    let result = verify_transcript_value(&transcript)?;
    let public_values = get_path(&result, &["public_values"])?;
    Ok(FullscanVerificationOutput {
        public_values: FullscanPublicValues {
            proposal_type_script_hash: parse_hash_hex(str_path(
                public_values,
                &["proposal_type_script_hash"],
            )?)?,
            proposal_data_hash: parse_hash_hex(str_path(public_values, &["proposal_data_hash"])?)?,
            start_block_hash: parse_hash_hex(str_path(public_values, &["start_block_hash"])?)?,
            end_block_hash: parse_hash_hex(str_path(public_values, &["end_block_hash"])?)?,
            report_root: parse_hash_hex(str_path(public_values, &["report_root"])?)?,
            passed: bool_path(public_values, &["passed"])?,
        },
        result_json: serde_json::to_vec(&result).context("serialize verification result")?,
    })
}

pub fn encode_public_values(values: &FullscanPublicValues) -> [u8; FULLSCAN_PUBLIC_VALUES_BYTES] {
    let mut bytes = [0u8; FULLSCAN_PUBLIC_VALUES_BYTES];
    bytes[0..32].copy_from_slice(&values.proposal_type_script_hash);
    bytes[32..64].copy_from_slice(&values.proposal_data_hash);
    bytes[64..96].copy_from_slice(&values.start_block_hash);
    bytes[96..128].copy_from_slice(&values.end_block_hash);
    bytes[128..160].copy_from_slice(&values.report_root);
    bytes[160] = u8::from(values.passed);
    bytes
}

pub fn decode_public_values(bytes: &[u8]) -> Result<FullscanPublicValues> {
    if bytes.len() != FULLSCAN_PUBLIC_VALUES_BYTES {
        bail!(
            "invalid public values length: {} != {}",
            bytes.len(),
            FULLSCAN_PUBLIC_VALUES_BYTES
        );
    }
    Ok(FullscanPublicValues {
        proposal_type_script_hash: bytes[0..32].try_into().context("proposal hash bytes")?,
        proposal_data_hash: bytes[32..64]
            .try_into()
            .context("proposal data hash bytes")?,
        start_block_hash: bytes[64..96].try_into().context("start block hash bytes")?,
        end_block_hash: bytes[96..128].try_into().context("end block hash bytes")?,
        report_root: bytes[128..160].try_into().context("report root bytes")?,
        passed: match bytes[160] {
            0 => false,
            1 => true,
            other => bail!("invalid passed byte: {other}"),
        },
    })
}

pub fn format_hash_hex(bytes: &[u8; 32]) -> String {
    format!("0x{}", hex::encode(bytes))
}

pub fn verify_transcript_value(transcript: &Value) -> Result<Value> {
    if str_path(transcript, &["magic"])? != TRANSCRIPT_MAGIC {
        bail!("unexpected transcript magic");
    }

    let proposal_artifact = get_path(transcript, &["proposal"])?;
    let proposal_data = get_path(proposal_artifact, &["proposal_data"])?;
    if str_path(proposal_data, &["magic"])? != PROPOSAL_MAGIC {
        bail!("unexpected proposal magic");
    }
    let proposal_type_script = get_path(proposal_artifact, &["proposal_type_script"])?;
    let vote_type_script = get_path(proposal_artifact, &["vote_type_script"])?;
    let dao_type_hash = str_path(transcript, &["dao_type_hash"])?;
    let start_block = u64_path(transcript, &["start_block"])?;
    let end_block = u64_path(transcript, &["end_block"])?;
    if end_block < start_block {
        bail!("end_block is before start_block");
    }

    let blocks = get_path(transcript, &["blocks"])?
        .as_array()
        .context("blocks is not array")?;
    let expected_count = end_block - start_block + 1;
    if blocks.len() as u64 != expected_count {
        bail!(
            "block count mismatch: {} != expected {expected_count}",
            blocks.len()
        );
    }

    let evidence = get_path(transcript, &["cell_evidence"])?
        .as_object()
        .context("cell_evidence is not object")?;

    let mut previous_hash = None::<String>;
    let mut start_hash = None::<String>;
    let mut end_hash = None::<String>;
    let mut blocks_scanned = 0u64;
    let mut transactions_scanned = 0u64;
    let mut outputs_scanned = 0u64;
    let mut vote_outputs_seen = 0u64;
    let mut valid_votes = Vec::new();
    let mut invalid_votes = Vec::new();

    for (block_offset, block) in blocks.iter().enumerate() {
        let label = format!("blocks[{block_offset}]");
        verify_compact_block_witness(block, &label)?;
        verify_block_transaction_hashes(block, &label)?;

        let number = u64_path(block, &["header", "number"])?;
        let expected_number = start_block + block_offset as u64;
        if number != expected_number {
            bail!("{label} number mismatch: {number} != {expected_number}");
        }
        let header_hash = str_path(block, &["header", "hash"])?;
        if block_offset == 0 {
            start_hash = Some(header_hash.to_owned());
        }
        if let Some(parent) = &previous_hash {
            if str_path(block, &["header", "parent_hash"])? != parent {
                bail!("{label} parent_hash mismatch");
            }
        }
        previous_hash = Some(header_hash.to_owned());
        end_hash = Some(header_hash.to_owned());
        blocks_scanned += 1;

        let txs = get_path(block, &["transactions"])?
            .as_array()
            .context("block transactions is not array")?;
        for (tx_index, tx) in txs.iter().enumerate() {
            verify_raw_transaction_molecule_witness(
                tx,
                &format!("{label}.transactions[{tx_index}]"),
            )?;
            transactions_scanned += 1;
            let outputs = get_path(tx, &["outputs"])?
                .as_array()
                .context("transaction outputs is not array")?;
            let outputs_data = get_path(tx, &["outputs_data"])?
                .as_array()
                .context("transaction outputs_data is not array")?;
            if outputs.len() != outputs_data.len() {
                bail!("tx output/data length mismatch");
            }
            for (output_index, output) in outputs.iter().enumerate() {
                outputs_scanned += 1;
                if output.get("type") != Some(vote_type_script) {
                    continue;
                }
                vote_outputs_seen += 1;
                let vote = validate_article_vote(
                    proposal_artifact,
                    proposal_data,
                    dao_type_hash,
                    tx,
                    evidence,
                    number,
                    tx_index as u64,
                    output_index as u64,
                    outputs_data[output_index]
                        .as_str()
                        .context("output data is not string")?,
                )?;
                if bool_path(&vote, &["valid"])? {
                    valid_votes.push(vote);
                } else {
                    invalid_votes.push(vote);
                }
            }
        }
    }

    let tally = build_tally(
        proposal_artifact,
        proposal_data,
        start_block,
        start_hash.context("missing start hash")?,
        end_block,
        end_hash.context("missing end hash")?,
        blocks_scanned,
        transactions_scanned,
        outputs_scanned,
        vote_outputs_seen,
        valid_votes,
        invalid_votes,
    )?;
    let commitment = get_path(&tally, &["commitment"])?;
    let report_root = ckb_hash(&canonical_json(commitment)?);
    let public_values = json!({
        "proposal_type_script_hash": ckb_hash(&canonical_json(proposal_type_script)?),
        "proposal_data_hash": ckb_hash(&canonical_json(proposal_data)?),
        "start_block_hash": str_path(commitment, &["start_block_hash"])?,
        "end_block_hash": str_path(commitment, &["end_block_hash"])?,
        "report_root": report_root,
        "passed": bool_path(commitment, &["passed"])?,
    });

    Ok(json!({
        "magic": REPORT_MAGIC,
        "version": 1,
        "valid": true,
        "report_root": report_root,
        "public_values": public_values,
        "tally": tally,
        "limitations": [
            "The guest scans every transaction output in the supplied block window and validates DAO deposit cell-dep evidence for discovered vote outputs.",
            "The compact transactions_root witness binds the transaction hash list to the block header.",
            "Each JSON transaction is bound to a raw Molecule transaction witness by rehashing the raw transaction and comparing it with the compact transaction hash list.",
            "The JSON transaction fields used by the tally are checked against the raw Molecule transaction witness inside the guest."
        ],
    }))
}

fn validate_article_vote(
    proposal_artifact: &Value,
    proposal_data: &Value,
    dao_type_hash: &str,
    tx: &Value,
    evidence: &Map<String, Value>,
    block_number: u64,
    tx_index: u64,
    output_index: u64,
    output_data: &str,
) -> Result<Value> {
    let mut errors = Vec::new();
    let vote_data = match parse_cell_data(output_data) {
        Ok(value) => value,
        Err(err) => {
            errors.push(format!("invalid vote data: {err:#}"));
            json!({})
        }
    };
    if vote_data.get("magic").and_then(Value::as_str) != Some(VOTE_MAGIC) {
        errors.push("unexpected vote magic".to_owned());
    }
    if vote_data.get("proposal_type_id") != proposal_artifact.get("proposal_type_id") {
        errors.push("proposal_type_id mismatch".to_owned());
    }
    let choice = vote_data
        .get("choice")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if !choice_ids(proposal_data)?
        .iter()
        .any(|allowed| allowed == &choice)
    {
        errors.push(format!("invalid choice: {choice}"));
    }

    let input_locks = tx_input_locks(tx, evidence)?;
    let dep_cells = tx_dep_cells(tx, evidence)?;
    let mut matching_deposits = Vec::new();
    for dep_cell in dep_cells {
        if !is_dao_deposit_cell(&dep_cell, dao_type_hash) {
            continue;
        }
        let dep_lock = get_path(&dep_cell, &["output", "lock"])?;
        if input_locks.iter().any(|input_lock| input_lock == dep_lock) {
            matching_deposits.push(dep_cell);
        }
    }
    if matching_deposits.is_empty() {
        errors.push("no DAO deposit cell_dep with a lock matching an input lock".to_owned());
    }

    let mut weight = 0u128;
    let mut voter_key = Value::Null;
    if let Some(deposit) = matching_deposits
        .iter()
        .max_by_key(|cell| capacity_path(cell, &["output", "capacity"]).unwrap_or(0))
    {
        weight = capacity_path(deposit, &["output", "capacity"])?;
        voter_key = json!(ckb_hash(&canonical_json(get_path(
            deposit,
            &["output", "lock"]
        )?)?));
    }

    Ok(json!({
        "valid": errors.is_empty(),
        "errors": errors,
        "choice": choice,
        "weight_shannons": weight.to_string(),
        "voter_key": voter_key,
        "tx_hash": str_path(tx, &["hash"])?,
        "output_index": output_index,
        "out_point": {"tx_hash": str_path(tx, &["hash"])?, "index": format!("0x{output_index:x}")},
        "vote_data": vote_data,
        "block_number": block_number,
        "tx_index": tx_index,
    }))
}

fn tx_input_locks(tx: &Value, evidence: &Map<String, Value>) -> Result<Vec<Value>> {
    let mut locks = Vec::new();
    for input in get_path(tx, &["inputs"])?
        .as_array()
        .context("transaction inputs is not array")?
    {
        let previous_output = get_path(input, &["previous_output"])?;
        if str_path(previous_output, &["tx_hash"])? == format!("0x{}", "0".repeat(64)) {
            continue;
        }
        let cell = evidence_cell(evidence, previous_output)?;
        locks.push(get_path(cell, &["output", "lock"])?.clone());
    }
    Ok(locks)
}

fn tx_dep_cells(tx: &Value, evidence: &Map<String, Value>) -> Result<Vec<Value>> {
    let mut cells = Vec::new();
    for dep in get_path(tx, &["cell_deps"])?
        .as_array()
        .context("transaction cell_deps is not array")?
    {
        if str_path(dep, &["dep_type"])? != "code" {
            continue;
        }
        let cell = evidence_cell(evidence, get_path(dep, &["out_point"])?)?;
        cells.push(cell.clone());
    }
    Ok(cells)
}

fn evidence_cell<'a>(evidence: &'a Map<String, Value>, out_point: &Value) -> Result<&'a Value> {
    let key = normalize_outpoint_key_from_value(out_point)?;
    evidence
        .get(&key)
        .with_context(|| format!("missing cell evidence for {key}"))
}

fn is_dao_deposit_cell(cell: &Value, dao_type_hash: &str) -> bool {
    let type_script = cell.get("output").and_then(|output| output.get("type"));
    type_script
        .and_then(|script| script.get("code_hash"))
        .and_then(Value::as_str)
        == Some(dao_type_hash)
        && cell.get("output_data").and_then(Value::as_str) == Some(DEPOSIT_PHASE_DATA)
}

fn build_tally(
    proposal_artifact: &Value,
    proposal_data: &Value,
    start_block: u64,
    start_block_hash: String,
    end_block: u64,
    end_block_hash: String,
    blocks_scanned: u64,
    transactions_scanned: u64,
    outputs_scanned: u64,
    vote_outputs_seen: u64,
    mut valid_votes: Vec<Value>,
    mut invalid_votes: Vec<Value>,
) -> Result<Value> {
    valid_votes.sort_by_key(vote_order);
    invalid_votes.sort_by_key(vote_order);
    let mut latest_by_voter = BTreeMap::<String, Value>::new();
    let mut superseded_votes = Vec::new();
    for vote in &valid_votes {
        let voter_key = str_path(vote, &["voter_key"])?.to_owned();
        if let Some(previous) = latest_by_voter.insert(voter_key, vote.clone()) {
            superseded_votes.push(previous);
        }
    }
    let mut counted_votes = latest_by_voter.into_values().collect::<Vec<_>>();
    counted_votes.sort_by_key(vote_order);
    superseded_votes.sort_by_key(vote_order);

    let choices = choice_ids(proposal_data)?;
    let mut weights: BTreeMap<String, u128> = choices.iter().map(|c| (c.clone(), 0)).collect();
    for vote in &counted_votes {
        let choice = str_path(vote, &["choice"])?;
        let weight = str_path(vote, &["weight_shannons"])?.parse::<u128>()?;
        *weights.entry(choice.to_owned()).or_insert(0) += weight;
    }
    let participation = weights.values().sum::<u128>();
    let yes = *weights.get("yes").unwrap_or(&0);
    let no = *weights.get("no").unwrap_or(&0);
    let minimal_requirement =
        str_path(proposal_data, &["minimal_requirement_shannons"])?.parse::<u128>()?;
    let passed = yes > no && participation >= minimal_requirement;
    let choice_weights_shannons = weights
        .iter()
        .map(|(choice, weight)| (choice.clone(), json!(weight.to_string())))
        .collect::<Map<_, _>>();

    let commitment = json!({
        "magic": REPORT_MAGIC,
        "version": 1,
        "proposal_type_id": str_path(proposal_artifact, &["proposal_type_id"])?,
        "start_block": start_block,
        "start_block_hash": start_block_hash,
        "end_block": end_block,
        "end_block_hash": end_block_hash,
        "blocks_scanned": blocks_scanned,
        "transactions_scanned": transactions_scanned,
        "outputs_scanned": outputs_scanned,
        "vote_outputs_seen": vote_outputs_seen,
        "valid_vote_count": valid_votes.len(),
        "counted_vote_count": counted_votes.len(),
        "superseded_vote_count": superseded_votes.len(),
        "invalid_vote_count": invalid_votes.len(),
        "choice_weights_shannons": choice_weights_shannons,
        "minimal_requirement_shannons": minimal_requirement.to_string(),
        "passed": passed,
        "counted_votes_root": merkle_root_records(&counted_votes)?,
        "superseded_votes_root": merkle_root_records(&superseded_votes)?,
        "invalid_votes_root": merkle_root_records(&invalid_votes)?,
    });
    Ok(json!({
        "commitment": commitment,
        "counted_votes": counted_votes,
        "superseded_votes": superseded_votes,
        "invalid_votes": invalid_votes,
    }))
}

fn parse_cell_data(data_hex: &str) -> Result<Value> {
    let data = hex::decode(trim_0x(data_hex)).context("decode cell data hex")?;
    serde_json::from_slice(&data).context("parse JSON cell data")
}

fn verify_block_transaction_hashes(block: &Value, label: &str) -> Result<()> {
    let txs = get_path(block, &["transactions"])?
        .as_array()
        .context("block transactions is not array")?;
    let tx_hashes = hash_array_path(get_path(block, &["compact"])?, &["transaction_hashes"])?;
    if txs.len() != tx_hashes.len() {
        bail!("{label} transaction count does not match compact tx hash count");
    }
    for (index, tx) in txs.iter().enumerate() {
        let expected = format_hash_hex(&tx_hashes[index]);
        let actual = str_path(tx, &["hash"])?;
        if actual != expected {
            bail!("{label} transaction hash mismatch at index {index}");
        }
    }
    Ok(())
}

fn verify_raw_transaction_molecule_witness(tx: &Value, label: &str) -> Result<()> {
    let raw_hex = str_path(tx, &["raw_transaction_molecule_hex"])?;
    let raw_bytes = hex::decode(trim_0x(raw_hex))
        .with_context(|| format!("{label} raw_transaction_molecule_hex is not valid hex"))?;
    if raw_bytes.is_empty() {
        bail!("{label} raw_transaction_molecule_hex is empty");
    }
    let computed_tx_hash = ckb_hash(&raw_bytes);
    let declared_tx_hash = str_path(tx, &["hash"])?;
    if computed_tx_hash != declared_tx_hash {
        bail!("{label} raw transaction hash mismatch: {computed_tx_hash} != {declared_tx_hash}");
    }
    let raw_tx = packed::RawTransaction::from_slice(&raw_bytes).with_context(|| {
        format!("{label} raw_transaction_molecule_hex is not a raw transaction")
    })?;
    verify_raw_transaction_fields(tx, raw_tx.as_reader(), label)?;
    Ok(())
}

fn verify_raw_transaction_fields(
    tx: &Value,
    raw: packed::RawTransactionReader<'_>,
    label: &str,
) -> Result<()> {
    let raw_version = le_u32(raw.version().as_slice(), &format!("{label}.version"))?;
    let json_version = u64_path(tx, &["version"])?;
    if json_version != raw_version as u64 {
        bail!("{label} version mismatch: {json_version} != {raw_version}");
    }

    verify_raw_cell_deps(
        get_path(tx, &["cell_deps"])?
            .as_array()
            .context("transaction cell_deps is not array")?,
        raw.cell_deps(),
        &format!("{label}.cell_deps"),
    )?;
    verify_raw_header_deps(
        get_path(tx, &["header_deps"])?
            .as_array()
            .context("transaction header_deps is not array")?,
        raw.header_deps(),
        &format!("{label}.header_deps"),
    )?;
    verify_raw_inputs(
        get_path(tx, &["inputs"])?
            .as_array()
            .context("transaction inputs is not array")?,
        raw.inputs(),
        &format!("{label}.inputs"),
    )?;
    verify_raw_outputs(
        get_path(tx, &["outputs"])?
            .as_array()
            .context("transaction outputs is not array")?,
        raw.outputs(),
        &format!("{label}.outputs"),
    )?;
    verify_raw_outputs_data(
        get_path(tx, &["outputs_data"])?
            .as_array()
            .context("transaction outputs_data is not array")?,
        raw.outputs_data(),
        &format!("{label}.outputs_data"),
    )?;
    Ok(())
}

fn verify_raw_cell_deps(
    json_deps: &[Value],
    raw_deps: packed::CellDepVecReader<'_>,
    label: &str,
) -> Result<()> {
    if json_deps.len() != raw_deps.len() {
        bail!(
            "{label} length mismatch: {} != {}",
            json_deps.len(),
            raw_deps.len()
        );
    }
    for (index, json_dep) in json_deps.iter().enumerate() {
        let raw_dep = raw_deps.get_unchecked(index);
        verify_out_point_json(
            get_path(json_dep, &["out_point"])?,
            raw_dep.out_point(),
            &format!("{label}[{index}].out_point"),
        )?;
        let raw_dep_type = dep_type_name(read_byte(
            raw_dep.dep_type(),
            &format!("{label}[{index}].dep_type"),
        )?)?;
        let json_dep_type = str_path(json_dep, &["dep_type"])?;
        if json_dep_type != raw_dep_type {
            bail!("{label}[{index}].dep_type mismatch: {json_dep_type} != {raw_dep_type}");
        }
    }
    Ok(())
}

fn verify_raw_header_deps(
    json_deps: &[Value],
    raw_deps: packed::Byte32VecReader<'_>,
    label: &str,
) -> Result<()> {
    if json_deps.len() != raw_deps.len() {
        bail!(
            "{label} length mismatch: {} != {}",
            json_deps.len(),
            raw_deps.len()
        );
    }
    for (index, json_dep) in json_deps.iter().enumerate() {
        let raw_dep = hex_slice(raw_deps.get_unchecked(index).as_slice());
        let json_dep = json_dep
            .as_str()
            .with_context(|| format!("{label}[{index}] is not string"))?;
        if json_dep != raw_dep {
            bail!("{label}[{index}] mismatch: {json_dep} != {raw_dep}");
        }
    }
    Ok(())
}

fn verify_raw_inputs(
    json_inputs: &[Value],
    raw_inputs: packed::CellInputVecReader<'_>,
    label: &str,
) -> Result<()> {
    if json_inputs.len() != raw_inputs.len() {
        bail!(
            "{label} length mismatch: {} != {}",
            json_inputs.len(),
            raw_inputs.len()
        );
    }
    for (index, json_input) in json_inputs.iter().enumerate() {
        let raw_input = raw_inputs.get_unchecked(index);
        let raw_since = le_u64(
            raw_input.since().as_slice(),
            &format!("{label}[{index}].since"),
        )?;
        let json_since = u64_path(json_input, &["since"])?;
        if json_since != raw_since {
            bail!("{label}[{index}].since mismatch: {json_since} != {raw_since}");
        }
        verify_out_point_json(
            get_path(json_input, &["previous_output"])?,
            raw_input.previous_output(),
            &format!("{label}[{index}].previous_output"),
        )?;
    }
    Ok(())
}

fn verify_raw_outputs(
    json_outputs: &[Value],
    raw_outputs: packed::CellOutputVecReader<'_>,
    label: &str,
) -> Result<()> {
    if json_outputs.len() != raw_outputs.len() {
        bail!(
            "{label} length mismatch: {} != {}",
            json_outputs.len(),
            raw_outputs.len()
        );
    }
    for (index, json_output) in json_outputs.iter().enumerate() {
        let raw_output = raw_outputs.get_unchecked(index);
        let raw_capacity = le_u64(
            raw_output.capacity().as_slice(),
            &format!("{label}[{index}].capacity"),
        )?;
        let json_capacity = capacity_path(json_output, &["capacity"])?;
        if json_capacity != raw_capacity as u128 {
            bail!("{label}[{index}].capacity mismatch: {json_capacity} != {raw_capacity}");
        }
        verify_script_json(
            get_path(json_output, &["lock"])?,
            raw_output.lock(),
            &format!("{label}[{index}].lock"),
        )?;
        match raw_output.type_().to_opt() {
            Some(raw_type) => verify_script_json(
                get_path(json_output, &["type"])?,
                raw_type,
                &format!("{label}[{index}].type"),
            )?,
            None => {
                if !get_path(json_output, &["type"])?.is_null() {
                    bail!("{label}[{index}].type mismatch: expected null");
                }
            }
        }
    }
    Ok(())
}

fn verify_raw_outputs_data(
    json_outputs_data: &[Value],
    raw_outputs_data: packed::BytesVecReader<'_>,
    label: &str,
) -> Result<()> {
    if json_outputs_data.len() != raw_outputs_data.len() {
        bail!(
            "{label} length mismatch: {} != {}",
            json_outputs_data.len(),
            raw_outputs_data.len()
        );
    }
    for (index, json_data) in json_outputs_data.iter().enumerate() {
        let raw_data = hex_slice(raw_outputs_data.get_unchecked(index).raw_data());
        let json_data = json_data
            .as_str()
            .with_context(|| format!("{label}[{index}] is not string"))?;
        if json_data != raw_data {
            bail!("{label}[{index}] mismatch");
        }
    }
    Ok(())
}

fn verify_out_point_json(
    json_out_point: &Value,
    raw_out_point: packed::OutPointReader<'_>,
    label: &str,
) -> Result<()> {
    let raw_tx_hash = hex_slice(raw_out_point.tx_hash().as_slice());
    let json_tx_hash = str_path(json_out_point, &["tx_hash"])?;
    if json_tx_hash != raw_tx_hash {
        bail!("{label}.tx_hash mismatch: {json_tx_hash} != {raw_tx_hash}");
    }
    let raw_index = le_u32(raw_out_point.index().as_slice(), &format!("{label}.index"))?;
    let json_index = u64_path(json_out_point, &["index"])?;
    if json_index != raw_index as u64 {
        bail!("{label}.index mismatch: {json_index} != {raw_index}");
    }
    Ok(())
}

fn verify_script_json(
    json_script: &Value,
    raw_script: packed::ScriptReader<'_>,
    label: &str,
) -> Result<()> {
    let raw_code_hash = hex_slice(raw_script.code_hash().as_slice());
    let json_code_hash = str_path(json_script, &["code_hash"])?;
    if json_code_hash != raw_code_hash {
        bail!("{label}.code_hash mismatch: {json_code_hash} != {raw_code_hash}");
    }

    let raw_hash_type = script_hash_type_name(read_byte(
        raw_script.hash_type(),
        &format!("{label}.hash_type"),
    )?)?;
    let json_hash_type = str_path(json_script, &["hash_type"])?;
    if json_hash_type != raw_hash_type {
        bail!("{label}.hash_type mismatch: {json_hash_type} != {raw_hash_type}");
    }

    let raw_args = hex_slice(raw_script.args().raw_data());
    let json_args = str_path(json_script, &["args"])?;
    if json_args != raw_args {
        bail!("{label}.args mismatch");
    }
    Ok(())
}

fn verify_compact_block_witness(block: &Value, label: &str) -> Result<()> {
    let compact = get_path(block, &["compact"])?;
    let tx_hashes = hash_array_path(compact, &["transaction_hashes"])?;
    let witness_hashes = hash_array_path(compact, &["witness_hashes"])?;
    if tx_hashes.len() != witness_hashes.len() {
        bail!("{label} compact tx_hashes/witness_hashes length mismatch");
    }
    if let Some(tx_count) = compact.get("tx_count").and_then(Value::as_u64) {
        if tx_count as usize != tx_hashes.len() {
            bail!("{label} compact tx_count mismatch");
        }
    }

    let raw_root = ckb_cbmt_root(&tx_hashes);
    let witnesses_root = ckb_cbmt_root(&witness_hashes);
    let transactions_root = ckb_cbmt_root(&[raw_root, witnesses_root]);
    if format_hash_hex(&raw_root) != str_path(compact, &["raw_transactions_root"])? {
        bail!("{label} compact raw_transactions_root mismatch");
    }
    if format_hash_hex(&witnesses_root) != str_path(compact, &["witnesses_root"])? {
        bail!("{label} compact witnesses_root mismatch");
    }
    if format_hash_hex(&transactions_root) != str_path(compact, &["transactions_root"])? {
        bail!("{label} compact transactions_root mismatch");
    }
    if format_hash_hex(&transactions_root) != str_path(block, &["header", "transactions_root"])? {
        bail!("{label} header transactions_root mismatch");
    }
    Ok(())
}

fn compact_vote(vote: &Value) -> Result<Value> {
    Ok(json!({
        "choice": get_path(vote, &["choice"])?.clone(),
        "weight_shannons": get_path(vote, &["weight_shannons"])?.clone(),
        "voter_key": get_path(vote, &["voter_key"])?.clone(),
        "out_point": get_path(vote, &["out_point"])?.clone(),
        "block_number": get_path(vote, &["block_number"])?.clone(),
        "tx_index": get_path(vote, &["tx_index"])?.clone(),
        "output_index": get_path(vote, &["output_index"])?.clone(),
    }))
}

fn merkle_root_records(records: &[Value]) -> Result<String> {
    if records.is_empty() {
        return Ok(ckb_hash(EMPTY_TALLY_ROOT_TAG));
    }
    let leaf_hashes = records
        .iter()
        .map(|record| Ok(ckb_hash(&canonical_json(&compact_vote(record)?)?)))
        .collect::<Result<Vec<_>>>()?;
    merkle_root_from_hex_hashes(&leaf_hashes)
}

fn merkle_root_from_hex_hashes(leaf_hashes: &[String]) -> Result<String> {
    let mut level = leaf_hashes
        .iter()
        .map(|hash| hex::decode(trim_0x(hash)).context("decode leaf hash"))
        .collect::<Result<Vec<_>>>()?;
    while level.len() > 1 {
        if level.len() % 2 == 1 {
            level.push(level.last().context("missing last hash")?.clone());
        }
        let mut next = Vec::new();
        for pair in level.chunks(2) {
            next.push(ckb_hash_raw([pair[0].clone(), pair[1].clone()].concat()));
        }
        level = next;
    }
    Ok(format!("0x{}", hex::encode(&level[0])))
}

fn choice_ids(proposal_data: &Value) -> Result<Vec<String>> {
    get_path(proposal_data, &["choices"])?
        .as_array()
        .context("proposal choices is not array")?
        .iter()
        .map(|choice| {
            choice
                .as_str()
                .map(ToOwned::to_owned)
                .context("choice is not string")
        })
        .collect()
}

fn vote_order(vote: &Value) -> (u64, u64, u64) {
    (
        vote.get("block_number")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        vote.get("tx_index").and_then(Value::as_u64).unwrap_or(0),
        vote.get("output_index")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    )
}

fn hash_array_path(value: &Value, path: &[&str]) -> Result<Vec<[u8; 32]>> {
    get_path(value, path)?
        .as_array()
        .with_context(|| format!("json path {} is not array", path.join(".")))?
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let hash = item
                .as_str()
                .with_context(|| format!("json path {}[{index}] is not string", path.join(".")))?;
            parse_hash_hex(hash).with_context(|| {
                format!(
                    "json path {}[{index}] is not a 32-byte hash",
                    path.join(".")
                )
            })
        })
        .collect()
}

fn ckb_cbmt_root(leaves: &[[u8; 32]]) -> [u8; 32] {
    if leaves.is_empty() {
        return [0u8; 32];
    }
    let mut queue = VecDeque::with_capacity((leaves.len() + 1) >> 1);
    let mut iter = leaves.rchunks_exact(2);
    while let Some(pair) = iter.next() {
        queue.push_back(merge_hashes(&pair[0], &pair[1]));
    }
    if let Some(leaf) = iter.remainder().first() {
        queue.push_front(*leaf);
    }
    while queue.len() > 1 {
        let right = queue.pop_front().expect("queue has right item");
        let left = queue.pop_front().expect("queue has left item");
        queue.push_back(merge_hashes(&left, &right));
    }
    queue.pop_front().expect("non-empty queue")
}

fn merge_hashes(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut data = [0u8; 64];
    data[..32].copy_from_slice(left);
    data[32..].copy_from_slice(right);
    ckb_hash_raw(data)
        .try_into()
        .expect("ckb hash output is 32 bytes")
}

fn capacity_path(value: &Value, path: &[&str]) -> Result<u128> {
    let text = str_path(value, path)?;
    if text.starts_with("0x") {
        return u128::from_str_radix(trim_0x(text), 16).context("parse capacity hex");
    }
    text.parse::<u128>().context("parse capacity integer")
}

fn le_u32(bytes: &[u8], label: &str) -> Result<u32> {
    let bytes: [u8; 4] = bytes
        .try_into()
        .with_context(|| format!("{label} is not 4 bytes"))?;
    Ok(u32::from_le_bytes(bytes))
}

fn le_u64(bytes: &[u8], label: &str) -> Result<u64> {
    let bytes: [u8; 8] = bytes
        .try_into()
        .with_context(|| format!("{label} is not 8 bytes"))?;
    Ok(u64::from_le_bytes(bytes))
}

fn read_byte(reader: packed::ByteReader<'_>, label: &str) -> Result<u8> {
    let bytes = reader.as_slice();
    if bytes.len() != 1 {
        bail!("{label} is not one byte");
    }
    Ok(bytes[0])
}

fn dep_type_name(value: u8) -> Result<&'static str> {
    match value {
        0 => Ok("code"),
        1 => Ok("dep_group"),
        other => bail!("invalid dep_type byte: {other}"),
    }
}

fn script_hash_type_name(value: u8) -> Result<String> {
    match value {
        0 => Ok("data".to_owned()),
        1 => Ok("type".to_owned()),
        even if even % 2 == 0 => Ok(format!("data{}", even / 2)),
        other => bail!("invalid script hash_type byte: {other}"),
    }
}

fn get_path<'a>(value: &'a Value, path: &[&str]) -> Result<&'a Value> {
    let mut current = value;
    for key in path {
        current = current
            .get(*key)
            .with_context(|| format!("missing json path {}", path.join(".")))?;
    }
    Ok(current)
}

fn str_path<'a>(value: &'a Value, path: &[&str]) -> Result<&'a str> {
    get_path(value, path)?
        .as_str()
        .with_context(|| format!("json path {} is not string", path.join(".")))
}

fn u64_path(value: &Value, path: &[&str]) -> Result<u64> {
    let value = get_path(value, path)?;
    if let Some(number) = value.as_u64() {
        return Ok(number);
    }
    if let Some(text) = value.as_str() {
        if text.starts_with("0x") {
            return u64::from_str_radix(trim_0x(text), 16)
                .with_context(|| format!("parse hex u64: {text}"));
        }
        return text.parse::<u64>().context("parse integer string");
    }
    bail!("json path {} is not u64", path.join("."))
}

fn bool_path(value: &Value, path: &[&str]) -> Result<bool> {
    get_path(value, path)?
        .as_bool()
        .with_context(|| format!("json path {} is not bool", path.join(".")))
}

fn trim_0x(value: &str) -> &str {
    value.strip_prefix("0x").unwrap_or(value)
}

fn hex_slice(bytes: &[u8]) -> String {
    format!("0x{}", hex::encode(bytes))
}

fn parse_hash_hex(value: &str) -> Result<[u8; 32]> {
    let trimmed = trim_0x(value);
    if trimmed.len() != 64 {
        bail!("expected 32-byte hex value, got {} chars", trimmed.len());
    }
    let decoded = hex::decode(trimmed).context("decode hash hex")?;
    decoded
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid decoded hash length"))
}

fn normalize_outpoint_key_from_value(out_point: &Value) -> Result<String> {
    Ok(format!(
        "{}:0x{:x}",
        str_path(out_point, &["tx_hash"])?,
        u64_path(out_point, &["index"])?,
    ))
}

fn ckb_hash(data: &[u8]) -> String {
    format!("0x{}", hex::encode(ckb_hash_raw(data)))
}

fn ckb_hash_raw(data: impl AsRef<[u8]>) -> Vec<u8> {
    let mut ret = [0u8; 32];
    let mut hasher = Blake2bBuilder::new(32)
        .personal(CKB_HASH_PERSONALIZATION)
        .build();
    hasher.update(data.as_ref());
    hasher.finalize(&mut ret);
    ret.to_vec()
}

fn canonical_json(value: &Value) -> Result<Vec<u8>> {
    serde_json::to_vec(value).context("serialize canonical JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(byte: u8) -> String {
        format!("0x{}", hex::encode([byte; 32]))
    }

    #[test]
    fn verifies_empty_window_transcript() {
        let zero = [0u8; 32];
        let transactions_root = format_hash_hex(&ckb_cbmt_root(&[zero, zero]));
        let transcript = json!({
            "magic": TRANSCRIPT_MAGIC,
            "version": 1,
            "dao_type_hash": hash(0x82),
            "start_block": 1,
            "end_block": 1,
            "proposal": {
                "proposal_type_id": "0x1111111111111111111111111111111111111111",
                "proposal_type_script": {
                    "code_hash": hash(0x28),
                    "hash_type": "data",
                    "args": "0x10"
                },
                "vote_type_script": {
                    "code_hash": hash(0x28),
                    "hash_type": "data",
                    "args": "0x11"
                },
                "proposal_data": {
                    "magic": PROPOSAL_MAGIC,
                    "version": 1,
                    "duration": 0,
                    "choices": ["yes", "no"],
                    "minimal_requirement_shannons": "0"
                }
            },
            "blocks": [{
                "header": {
                    "number": "0x1",
                    "hash": hash(0xaa),
                    "parent_hash": hash(0xbb),
                    "transactions_root": transactions_root
                },
                "transactions": [],
                "compact": {
                    "tx_count": 0,
                    "transaction_hashes": [],
                    "witness_hashes": [],
                    "raw_transactions_root": hash(0),
                    "witnesses_root": hash(0),
                    "transactions_root": transactions_root
                }
            }],
            "cell_evidence": {}
        });

        let result = verify_transcript_value(&transcript).expect("valid transcript");
        assert_eq!(result["tally"]["commitment"]["blocks_scanned"], json!(1));
        assert_eq!(result["public_values"]["passed"], json!(false));
    }
}
