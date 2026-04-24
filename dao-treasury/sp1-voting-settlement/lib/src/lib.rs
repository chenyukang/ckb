use anyhow::{bail, Context, Result};
use blake2b_ref::Blake2bBuilder;
#[cfg(feature = "ckb-commitments")]
use ckb_jsonrpc_types::{
    BlockView as JsonBlockView, Either, HeaderView as JsonHeaderView,
    TransactionView as JsonTransactionView, TransactionWithStatusResponse,
};
#[cfg(feature = "ckb-commitments")]
use ckb_types::{
    core::{BlockView as CoreBlockView, HeaderView as CoreHeaderView},
    packed,
    prelude::*,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, VecDeque};

const CKB_HASH_PERSONALIZATION: &[u8] = b"ckb-default-hash";
const TRANSCRIPT_MAGIC: &str = "CKB_DAO_TREASURY_ZKVM_SETTLEMENT_TRANSCRIPT_V1";
const SNAPSHOT_MAGIC: &str = "CKB_DAO_TREASURY_SNAPSHOT_V3";
const SNAPSHOT_SOURCE_MAGIC: &str = "CKB_DAO_TREASURY_SNAPSHOT_SOURCE_V3";
const RECORD_PROOF_MAGIC: &str = "CKB_DAO_TREASURY_RECORD_PROOF_V3";
const DEPOSIT_PHASE_DATA: &str = "0x0000000000000000";
const EMPTY_RECORD_MAP_TAG: &[u8] = b"CKB_DAO_TREASURY_EMPTY_RECORD_MAP_V3";
const EMPTY_OWNER_INDEX_TAG: &[u8] = b"CKB_DAO_TREASURY_EMPTY_OWNER_INDEX_V3";
const EMPTY_TALLY_ROOT_TAG: &[u8] = b"CKB_DAO_TREASURY_EMPTY_TALLY_V1";

pub const SETTLEMENT_PUBLIC_VALUES_BYTES: usize = 32 * 5;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettlementPublicValues {
    pub proposal_id: [u8; 32],
    pub snapshot_id: [u8; 32],
    pub snapshot_root: [u8; 32],
    pub tally_root: [u8; 32],
    pub settlement_root: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettlementVerificationOutput {
    pub public_values: SettlementPublicValues,
    pub result_json: Vec<u8>,
}

pub fn verify_transcript_bytes(transcript_bytes: &[u8]) -> Result<SettlementVerificationOutput> {
    let transcript: Value = serde_json::from_slice(transcript_bytes).context("parse transcript")?;
    let result = verify_transcript_value(&transcript)?;
    let commitment = get_path(&result, &["settlement_commitment"])?;
    let public_values = SettlementPublicValues {
        proposal_id: parse_hash_hex(str_path(commitment, &["proposal_id"])?)?,
        snapshot_id: parse_hash_hex(str_path(commitment, &["snapshot_id"])?)?,
        snapshot_root: parse_hash_hex(str_path(commitment, &["snapshot_root"])?)?,
        tally_root: parse_hash_hex(str_path(commitment, &["tally_root"])?)?,
        settlement_root: parse_hash_hex(str_path(&result, &["settlement_root"])?)?,
    };
    Ok(SettlementVerificationOutput {
        public_values,
        result_json: serde_json::to_vec(&result).context("serialize settlement output")?,
    })
}

pub fn encode_public_values(
    values: &SettlementPublicValues,
) -> [u8; SETTLEMENT_PUBLIC_VALUES_BYTES] {
    let mut bytes = [0u8; SETTLEMENT_PUBLIC_VALUES_BYTES];
    bytes[0..32].copy_from_slice(&values.proposal_id);
    bytes[32..64].copy_from_slice(&values.snapshot_id);
    bytes[64..96].copy_from_slice(&values.snapshot_root);
    bytes[96..128].copy_from_slice(&values.tally_root);
    bytes[128..160].copy_from_slice(&values.settlement_root);
    bytes
}

pub fn decode_public_values(bytes: &[u8]) -> Result<SettlementPublicValues> {
    if bytes.len() != SETTLEMENT_PUBLIC_VALUES_BYTES {
        bail!(
            "invalid public values length: {} != {}",
            bytes.len(),
            SETTLEMENT_PUBLIC_VALUES_BYTES
        );
    }
    Ok(SettlementPublicValues {
        proposal_id: bytes[0..32].try_into().context("proposal_id bytes")?,
        snapshot_id: bytes[32..64].try_into().context("snapshot_id bytes")?,
        snapshot_root: bytes[64..96].try_into().context("snapshot_root bytes")?,
        tally_root: bytes[96..128].try_into().context("tally_root bytes")?,
        settlement_root: bytes[128..160]
            .try_into()
            .context("settlement_root bytes")?,
    })
}

pub fn format_hash_hex(bytes: &[u8; 32]) -> String {
    format!("0x{}", hex::encode(bytes))
}

pub fn verify_transcript_value(transcript: &Value) -> Result<Value> {
    if str_path(transcript, &["magic"])? != TRANSCRIPT_MAGIC {
        bail!("unexpected transcript magic");
    }

    let public_inputs = get_path(transcript, &["public_inputs"])?;
    let proposal = get_path(transcript, &["proposal"])?;
    let snapshot = get_path(transcript, &["snapshot"])?;
    let snapshot_block_witness = get_path(transcript, &["snapshot_block_witness"])?;
    let header_chain_witness = get_path(transcript, &["header_chain_witness"])?;
    let source = get_path(transcript, &["snapshot_source"])?;

    verify_public_inputs(public_inputs, proposal, snapshot)?;
    let header_chain = verify_header_chain(header_chain_witness, public_inputs)?;
    verify_snapshot_block_witness(snapshot_block_witness, public_inputs, &header_chain)?;
    let snapshot_result = verify_snapshot_source(snapshot, source, public_inputs)?;
    let votes = verify_vote_witnesses(
        get_path(transcript, &["votes"])?
            .as_array()
            .context("votes is not array")?,
        proposal,
        public_inputs,
        &header_chain,
    )?;
    let tally = build_tally(proposal, public_inputs, votes)?;

    if str_path(&tally, &["tally_root"])? != str_path(public_inputs, &["tally_root"])? {
        bail!("tally_root mismatch");
    }
    if get_path(&tally, &["choice_weights_shannons"])?
        != get_path(public_inputs, &["choice_weights_shannons"])?
    {
        bail!("choice_weights_shannons mismatch");
    }

    let settlement_commitment = json!({
        "magic": "CKB_DAO_TREASURY_ZKVM_SETTLEMENT_OUTPUT_V1",
        "version": 1,
        "proof_model": str_path(public_inputs, &["proof_model"])?,
        "chain_inclusion_verified": bool_path(public_inputs, &["chain_inclusion_verified"])?,
        "proposal_id": str_path(public_inputs, &["proposal_id"])?,
        "snapshot_id": str_path(public_inputs, &["snapshot_id"])?,
        "snapshot_root": str_path(public_inputs, &["snapshot_root"])?,
        "snapshot_block_number": u64_path(public_inputs, &["snapshot_block_number"])?,
        "snapshot_block_hash": str_path(public_inputs, &["snapshot_block_hash"])?,
        "vote_start_block": u64_path(public_inputs, &["vote_start_block"])?,
        "vote_end_block": u64_path(public_inputs, &["vote_end_block"])?,
        "anchor_start_block_number": u64_path(public_inputs, &["anchor_start_block_number"])?,
        "anchor_start_block_hash": str_path(public_inputs, &["anchor_start_block_hash"])?,
        "anchor_end_block_number": u64_path(public_inputs, &["anchor_end_block_number"])?,
        "anchor_end_block_hash": str_path(public_inputs, &["anchor_end_block_hash"])?,
        "tally_root": str_path(public_inputs, &["tally_root"])?,
        "choice_weights_shannons": get_path(public_inputs, &["choice_weights_shannons"])?.clone(),
        "record_map_root": str_path(&snapshot_result, &["record_map_root"])?,
        "owner_index_root": str_path(&snapshot_result, &["owner_index_root"])?,
        "valid_vote_count": u64_path(&tally, &["valid_vote_count"])?,
        "counted_vote_count": u64_path(&tally, &["counted_vote_count"])?,
        "superseded_vote_count": u64_path(&tally, &["superseded_vote_count"])?,
    });
    let settlement_root = ckb_hash(&canonical_json(&settlement_commitment)?);

    Ok(json!({
        "magic": "CKB_DAO_TREASURY_ZKVM_SETTLEMENT_OUTPUT_V1",
        "version": 1,
        "valid": true,
        "settlement_root": settlement_root,
        "settlement_commitment": settlement_commitment,
        "snapshot": snapshot_result,
        "tally": tally,
        "limitations": [
            "This PoC verifies transcript consistency, snapshot roots, record proofs, tally roots, raw transaction Molecule bytes to CKB tx hashes, and compact CKB transaction-root inclusion for witnessed tx hashes.",
            "The guest does not yet parse every raw transaction Molecule field; JSON field checks are bound to the recomputed tx hash through the host-supplied raw transaction bytes."
        ],
    }))
}

fn verify_header_chain(headers: &Value, public_inputs: &Value) -> Result<BTreeMap<u64, String>> {
    let headers = headers
        .as_array()
        .context("header_chain_witness is not array")?;
    if headers.is_empty() {
        bail!("header_chain_witness is empty");
    }
    let mut chain = BTreeMap::new();
    let mut previous_number = None;
    let mut previous_hash = None::<String>;
    for header in headers {
        verify_header_view(header, "header_chain_witness")?;
        let number = u64_path(header, &["number"])?;
        let hash = str_path(header, &["hash"])?.to_owned();
        if let Some(prev_number) = previous_number {
            if number != prev_number + 1 {
                bail!("header chain number gap");
            }
        }
        if let Some(prev_hash) = &previous_hash {
            if str_path(header, &["parent_hash"])? != prev_hash {
                bail!("header chain parent_hash mismatch");
            }
        }
        previous_number = Some(number);
        previous_hash = Some(hash.clone());
        chain.insert(number, hash);
    }
    let first = headers.first().context("missing first header")?;
    let last = headers.last().context("missing last header")?;
    if u64_path(first, &["number"])? != u64_path(public_inputs, &["anchor_start_block_number"])? {
        bail!("anchor_start_block_number mismatch");
    }
    if str_path(first, &["hash"])? != str_path(public_inputs, &["anchor_start_block_hash"])? {
        bail!("anchor_start_block_hash mismatch");
    }
    if u64_path(last, &["number"])? != u64_path(public_inputs, &["anchor_end_block_number"])? {
        bail!("anchor_end_block_number mismatch");
    }
    if str_path(last, &["hash"])? != str_path(public_inputs, &["anchor_end_block_hash"])? {
        bail!("anchor_end_block_hash mismatch");
    }
    Ok(chain)
}

fn verify_snapshot_block_witness(
    block: &Value,
    public_inputs: &Value,
    header_chain: &BTreeMap<u64, String>,
) -> Result<()> {
    verify_block_witness(block, "snapshot_block_witness")?;
    if u64_path(block, &["header", "number"])?
        != u64_path(public_inputs, &["snapshot_block_number"])?
    {
        bail!("snapshot block witness number mismatch");
    }
    if str_path(block, &["header", "hash"])? != str_path(public_inputs, &["snapshot_block_hash"])? {
        bail!("snapshot block witness hash mismatch");
    }
    require_header_in_chain(
        header_chain,
        u64_path(block, &["header", "number"])?,
        str_path(block, &["header", "hash"])?,
    )?;
    Ok(())
}

fn verify_public_inputs(public_inputs: &Value, proposal: &Value, snapshot: &Value) -> Result<()> {
    if str_path(public_inputs, &["proposal_id"])? != str_path(proposal, &["proposal_id"])? {
        bail!("public proposal_id mismatch");
    }
    if str_path(public_inputs, &["snapshot_id"])? != str_path(snapshot, &["snapshot_id"])? {
        bail!("public snapshot_id mismatch");
    }
    if str_path(public_inputs, &["snapshot_root"])?
        != str_path(snapshot, &["roots", "snapshot_root"])?
    {
        bail!("public snapshot_root mismatch");
    }
    if str_path(public_inputs, &["record_map_root"])?
        != str_path(snapshot, &["roots", "record_map_root"])?
    {
        bail!("public record_map_root mismatch");
    }
    if str_path(public_inputs, &["owner_index_root"])?
        != str_path(snapshot, &["roots", "owner_index_root"])?
    {
        bail!("public owner_index_root mismatch");
    }
    if u64_path(public_inputs, &["snapshot_block_number"])?
        != u64_path(snapshot, &["snapshot_block", "number"])?
    {
        bail!("public snapshot_block_number mismatch");
    }
    if str_path(public_inputs, &["snapshot_block_hash"])?
        != str_path(snapshot, &["snapshot_block", "hash"])?
    {
        bail!("public snapshot_block_hash mismatch");
    }
    if u64_path(public_inputs, &["vote_start_block"])?
        != u64_path(proposal, &["manifest", "vote_window", "start_block"])?
    {
        bail!("public vote_start_block mismatch");
    }
    if u64_path(public_inputs, &["vote_end_block"])?
        != u64_path(proposal, &["manifest", "vote_window", "end_block"])?
    {
        bail!("public vote_end_block mismatch");
    }
    Ok(())
}

fn verify_snapshot_source(
    snapshot: &Value,
    source: &Value,
    public_inputs: &Value,
) -> Result<Value> {
    if str_path(snapshot, &["magic"])? != SNAPSHOT_MAGIC {
        bail!("unexpected snapshot magic");
    }
    if str_path(source, &["magic"])? != SNAPSHOT_SOURCE_MAGIC {
        bail!("unexpected snapshot source magic");
    }

    let records = get_path(source, &["records"])?
        .as_array()
        .context("snapshot source records is not array")?
        .clone();
    let computed = compute_snapshot_from_records(
        u64_path(public_inputs, &["snapshot_block_number"])?,
        str_path(public_inputs, &["snapshot_block_hash"])?,
        str_path(snapshot, &["network", "dao_type_hash"])?,
        records,
    )?;

    if str_path(&computed, &["snapshot_root"])? != str_path(public_inputs, &["snapshot_root"])? {
        bail!("computed snapshot_root mismatch");
    }
    if str_path(&computed, &["record_map_root"])? != str_path(public_inputs, &["record_map_root"])?
    {
        bail!("computed record_map_root mismatch");
    }
    if str_path(&computed, &["owner_index_root"])?
        != str_path(public_inputs, &["owner_index_root"])?
    {
        bail!("computed owner_index_root mismatch");
    }
    if get_path(&computed, &["commitment"])? != get_path(snapshot, &["commitment"])? {
        bail!("snapshot commitment mismatch");
    }
    if get_path(&computed, &["commitment"])? != get_path(source, &["commitment"])? {
        bail!("snapshot source commitment mismatch");
    }

    Ok(json!({
        "snapshot_id": str_path(&computed, &["snapshot_id"])?,
        "snapshot_root": str_path(&computed, &["snapshot_root"])?,
        "record_map_root": str_path(&computed, &["record_map_root"])?,
        "owner_index_root": str_path(&computed, &["owner_index_root"])?,
        "record_count": u64_path(&computed, &["record_count"])?,
        "owner_count": u64_path(&computed, &["owner_count"])?,
        "total_weight_shannons": str_path(&computed, &["total_weight_shannons"])?,
    }))
}

fn compute_snapshot_from_records(
    snapshot_block_number: u64,
    snapshot_block_hash: &str,
    dao_type_hash: &str,
    mut records: Vec<Value>,
) -> Result<Value> {
    records.sort_by_key(|record| {
        normalize_outpoint_key(
            record
                .get("deposit_out_point_key")
                .and_then(Value::as_str)
                .unwrap_or(""),
        )
        .unwrap_or_default()
    });
    let owner_entries = build_owner_entries(&records)?;

    let record_leaf_hashes = records
        .iter()
        .map(|record| leaf_hash(str_path(record, &["deposit_out_point_key"])?, record))
        .collect::<Result<Vec<_>>>()?;
    let owner_leaf_hashes = owner_entries
        .iter()
        .map(|entry| leaf_hash(str_path(entry, &["owner_key"])?, entry))
        .collect::<Result<Vec<_>>>()?;
    let record_map_root = merkle_root_from_hex_hashes(&record_leaf_hashes, EMPTY_RECORD_MAP_TAG)?;
    let owner_index_root = merkle_root_from_hex_hashes(&owner_leaf_hashes, EMPTY_OWNER_INDEX_TAG)?;
    let total_weight = records.iter().try_fold(0u128, |acc, record| {
        Ok::<_, anyhow::Error>(acc + str_path(record, &["weight_shannons"])?.parse::<u128>()?)
    })?;
    let total_capacity = records.iter().try_fold(0u128, |acc, record| {
        Ok::<_, anyhow::Error>(acc + str_path(record, &["capacity_shannons"])?.parse::<u128>()?)
    })?;

    let commitment = json!({
        "magic": SNAPSHOT_MAGIC,
        "version": 3,
        "snapshot_block_number": snapshot_block_number,
        "snapshot_block_hash": snapshot_block_hash,
        "dao_type_hash": dao_type_hash,
        "eligibility_phase": "deposit",
        "eligibility_output_data": DEPOSIT_PHASE_DATA,
        "weight": "capacity_shannons",
        "record_map_root": record_map_root,
        "owner_index_root": owner_index_root,
        "record_count": records.len(),
        "owner_count": owner_entries.len(),
        "total_weight_shannons": total_weight.to_string(),
        "total_capacity_shannons": total_capacity.to_string(),
    });
    let snapshot_root = ckb_hash(&canonical_json(&commitment)?);

    Ok(json!({
        "snapshot_id": snapshot_root,
        "snapshot_root": snapshot_root,
        "record_map_root": record_map_root,
        "owner_index_root": owner_index_root,
        "record_count": records.len(),
        "owner_count": owner_entries.len(),
        "total_weight_shannons": total_weight.to_string(),
        "total_capacity_shannons": total_capacity.to_string(),
        "commitment": commitment,
    }))
}

fn build_owner_entries(records: &[Value]) -> Result<Vec<Value>> {
    let mut grouped: BTreeMap<String, Value> = BTreeMap::new();
    for record in records {
        let owner_key = str_path(record, &["owner_key"])?;
        let entry = grouped.entry(owner_key.to_owned()).or_insert_with(|| {
            json!({
                "owner_key": owner_key,
                "lock": get_path(record, &["lock"]).cloned().unwrap_or(Value::Null),
                "lock_arg": get_path(record, &["lock", "args"]).cloned().unwrap_or(Value::Null),
                "deposit_out_point_keys": [],
                "record_count": 0,
                "total_weight_shannons": "0",
                "total_capacity_shannons": "0",
            })
        });
        let object = entry.as_object_mut().context("owner entry is not object")?;
        object
            .get_mut("deposit_out_point_keys")
            .and_then(Value::as_array_mut)
            .context("owner deposit_out_point_keys is not array")?
            .push(json!(normalize_outpoint_key(str_path(
                record,
                &["deposit_out_point_key"]
            )?)?));
        let record_count = object
            .get("record_count")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            + 1;
        object.insert("record_count".to_owned(), json!(record_count));
        let weight = object
            .get("total_weight_shannons")
            .and_then(Value::as_str)
            .unwrap_or("0")
            .parse::<u128>()?
            + str_path(record, &["weight_shannons"])?.parse::<u128>()?;
        let capacity = object
            .get("total_capacity_shannons")
            .and_then(Value::as_str)
            .unwrap_or("0")
            .parse::<u128>()?
            + str_path(record, &["capacity_shannons"])?.parse::<u128>()?;
        object.insert(
            "total_weight_shannons".to_owned(),
            json!(weight.to_string()),
        );
        object.insert(
            "total_capacity_shannons".to_owned(),
            json!(capacity.to_string()),
        );
    }

    let mut entries = grouped.into_values().collect::<Vec<_>>();
    for entry in &mut entries {
        let object = entry.as_object_mut().context("owner entry is not object")?;
        object
            .get_mut("deposit_out_point_keys")
            .and_then(Value::as_array_mut)
            .context("owner deposit_out_point_keys is not array")?
            .sort_by_key(|value| value.as_str().unwrap_or_default().to_owned());
        let total_weight = object
            .get("total_weight_shannons")
            .and_then(Value::as_str)
            .context("missing owner total_weight_shannons")?
            .parse::<u128>()?;
        object.insert(
            "total_weight_ckb".to_owned(),
            json!(shannons_to_ckb_string(total_weight)),
        );
    }
    entries.sort_by_key(|entry| {
        entry
            .get("owner_key")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    });
    Ok(entries)
}

fn verify_vote_witnesses(
    witnesses: &[Value],
    proposal: &Value,
    public_inputs: &Value,
    header_chain: &BTreeMap<u64, String>,
) -> Result<Vec<Value>> {
    let choices = choice_ids(proposal)?;
    let mut votes = Vec::new();
    for witness in witnesses {
        let vote_artifact = get_path(witness, &["vote"])?;
        let commitment = get_path(vote_artifact, &["commitment"])?;
        let mut unsigned_commitment = commitment.clone();
        let vote_id = unsigned_commitment
            .as_object_mut()
            .and_then(|object| object.remove("vote_id"))
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .context("vote_id missing")?;
        let recomputed_vote_id = ckb_hash(&canonical_json(&unsigned_commitment)?);
        if vote_id != recomputed_vote_id {
            bail!("vote_id mismatch for {}", vote_id);
        }
        if str_path(commitment, &["proposal_id"])? != str_path(public_inputs, &["proposal_id"])? {
            bail!("vote proposal_id mismatch");
        }
        if str_path(commitment, &["snapshot_id"])? != str_path(public_inputs, &["snapshot_id"])? {
            bail!("vote snapshot_id mismatch");
        }
        if str_path(commitment, &["snapshot_root"])? != str_path(public_inputs, &["snapshot_root"])?
        {
            bail!("vote snapshot_root mismatch");
        }
        let choice = str_path(commitment, &["choice"])?;
        if !choices.iter().any(|allowed| allowed == choice) {
            bail!("invalid vote choice: {choice}");
        }

        let deposit_key =
            normalize_outpoint_key(str_path(commitment, &["deposit_out_point_key"])?)?;
        let expected_args = format!(
            "0x01{}{}",
            trim_0x(str_path(public_inputs, &["proposal_id"])?),
            trim_0x(&ckb_hash(deposit_key.as_bytes()))
        );
        if str_path(vote_artifact, &["vote_type_script", "args"])? != expected_args {
            bail!("vote type args mismatch for {vote_id}");
        }
        let tx_hash = str_path(witness, &["tx_hash"])?;
        if tx_hash != str_path(witness, &["out_point", "tx_hash"])? {
            bail!("witness tx_hash does not match out_point tx_hash");
        }
        if tx_hash != str_path(vote_artifact, &["chain", "tx_hash"])? {
            bail!("witness tx_hash does not match vote artifact chain tx_hash");
        }

        let tx = get_path(witness, &["transaction"])?;
        verify_transaction_response(tx, "vote transaction witness")?;
        if tx_hash != str_path(tx, &["transaction", "hash"])? {
            bail!("transaction hash mismatch");
        }
        if u64_path(witness, &["block_number"])? != u64_path(tx, &["tx_status", "block_number"])? {
            bail!("transaction block_number mismatch");
        }
        if u64_path(witness, &["tx_index"])? != u64_path(tx, &["tx_status", "tx_index"])? {
            bail!("transaction tx_index mismatch");
        }
        let containing_block = get_path(witness, &["containing_block"])?;
        verify_block_witness(containing_block, "vote containing_block witness")?;
        if u64_path(containing_block, &["header", "number"])?
            != u64_path(tx, &["tx_status", "block_number"])?
        {
            bail!("containing block number mismatch");
        }
        if str_path(containing_block, &["header", "hash"])?
            != str_path(tx, &["tx_status", "block_hash"])?
        {
            bail!("containing block hash mismatch");
        }
        require_header_in_chain(
            header_chain,
            u64_path(containing_block, &["header", "number"])?,
            str_path(containing_block, &["header", "hash"])?,
        )?;
        verify_compact_tx_hash_at(
            containing_block,
            u64_path(witness, &["tx_index"])? as usize,
            tx_hash,
            "vote containing_block tx inclusion",
        )?;
        let output_index = u64_path(witness, &["output_index"])? as usize;
        let tx_outputs = get_path(tx, &["transaction", "outputs"])?
            .as_array()
            .context("transaction outputs is not array")?;
        let tx_outputs_data = get_path(tx, &["transaction", "outputs_data"])?
            .as_array()
            .context("transaction outputs_data is not array")?;
        let tx_output = tx_outputs
            .get(output_index)
            .context("vote output index out of range")?;
        let tx_output_data = tx_outputs_data
            .get(output_index)
            .and_then(Value::as_str)
            .context("vote output data missing")?;
        if get_path(tx_output, &["type"])? != get_path(vote_artifact, &["vote_type_script"])? {
            bail!("transaction output type script mismatch");
        }
        let expected_cell_data = format!("0x{}", hex::encode(canonical_json(commitment)?));
        if expected_cell_data != str_path(witness, &["expected_cell_data"])? {
            bail!("expected cell data mismatch");
        }
        if expected_cell_data != tx_output_data {
            bail!("transaction output data does not match vote commitment");
        }

        let verified = verify_record_proof(get_path(commitment, &["record_proof"])?)?;
        if str_path(&verified, &["snapshot_id"])? != str_path(public_inputs, &["snapshot_id"])? {
            bail!("record proof snapshot_id mismatch");
        }
        if str_path(&verified, &["deposit_out_point_key"])? != deposit_key {
            bail!("record proof deposit key mismatch");
        }
        if str_path(commitment, &["record_hash"])? != str_path(&verified, &["record_hash"])? {
            bail!("record_hash mismatch");
        }
        let record = get_path(&verified, &["record"])?;
        if get_path(commitment, &["voter_lock"])? != get_path(record, &["lock"])? {
            bail!("voter_lock does not match snapshot record");
        }
        if str_path(commitment, &["owner_key"])? != str_path(record, &["owner_key"])? {
            bail!("owner_key does not match snapshot record");
        }
        if str_path(commitment, &["weight_shannons"])? != str_path(record, &["weight_shannons"])? {
            bail!("weight does not match snapshot record");
        }

        let block_number = u64_path(witness, &["block_number"])?;
        let start = u64_path(public_inputs, &["vote_start_block"])?;
        let end = u64_path(public_inputs, &["vote_end_block"])?;
        if !(start..=end).contains(&block_number) {
            bail!("vote outside vote window");
        }

        let owner_input_cells = get_path(witness, &["owner_input_cells"])?
            .as_array()
            .context("owner_input_cells is not array")?;
        let tx_inputs = get_path(tx, &["transaction", "inputs"])?
            .as_array()
            .context("transaction inputs is not array")?;
        let mut has_owner_input = false;
        for owner_input in owner_input_cells {
            let input_index = u64_path(owner_input, &["input_index"])? as usize;
            let tx_input = tx_inputs
                .get(input_index)
                .context("owner input index out of range")?;
            if get_path(owner_input, &["previous_output"])?
                != get_path(tx_input, &["previous_output"])?
            {
                bail!("owner input previous_output mismatch");
            }
            let previous_transaction = get_path(owner_input, &["previous_transaction"])?;
            verify_transaction_response(
                previous_transaction,
                "owner input previous_transaction witness",
            )?;
            if str_path(previous_transaction, &["transaction", "hash"])?
                != str_path(owner_input, &["previous_output", "tx_hash"])?
            {
                bail!("previous transaction hash mismatch");
            }
            let previous_containing_block = get_path(owner_input, &["previous_containing_block"])?;
            verify_block_witness(
                previous_containing_block,
                "owner input previous_containing_block witness",
            )?;
            if u64_path(previous_containing_block, &["header", "number"])?
                != u64_path(previous_transaction, &["tx_status", "block_number"])?
            {
                bail!("previous containing block number mismatch");
            }
            if str_path(previous_containing_block, &["header", "hash"])?
                != str_path(previous_transaction, &["tx_status", "block_hash"])?
            {
                bail!("previous containing block hash mismatch");
            }
            require_header_in_chain(
                header_chain,
                u64_path(previous_containing_block, &["header", "number"])?,
                str_path(previous_containing_block, &["header", "hash"])?,
            )?;
            let previous_tx_index =
                u64_path(previous_transaction, &["tx_status", "tx_index"])? as usize;
            verify_compact_tx_hash_at(
                previous_containing_block,
                previous_tx_index,
                str_path(previous_transaction, &["transaction", "hash"])?,
                "owner previous_containing_block tx inclusion",
            )?;
            let previous_cell = get_path(owner_input, &["previous_cell"])?;
            if str_path(previous_cell, &["out_point_key"])?
                != normalize_outpoint_key_from_value(get_path(owner_input, &["previous_output"])?)?
            {
                bail!("owner input previous cell out_point_key mismatch");
            }
            let previous_output_index =
                u64_path(owner_input, &["previous_output", "index"])? as usize;
            let previous_outputs = get_path(previous_transaction, &["transaction", "outputs"])?
                .as_array()
                .context("previous transaction outputs is not array")?;
            let previous_outputs_data =
                get_path(previous_transaction, &["transaction", "outputs_data"])?
                    .as_array()
                    .context("previous transaction outputs_data is not array")?;
            if previous_outputs
                .get(previous_output_index)
                .context("previous output index out of range")?
                != get_path(previous_cell, &["output"])?
            {
                bail!("previous cell output mismatch");
            }
            if previous_outputs_data
                .get(previous_output_index)
                .and_then(Value::as_str)
                .context("previous output data missing")?
                != str_path(previous_cell, &["output_data"])?
            {
                bail!("previous cell output_data mismatch");
            }
            if get_path(previous_cell, &["output", "lock"])? == get_path(record, &["lock"])? {
                has_owner_input = true;
            }
        }
        if !has_owner_input {
            bail!("owner input lock does not match snapshot record lock");
        }

        votes.push(json!({
            "vote_id": vote_id,
            "deposit_out_point_key": deposit_key,
            "choice": choice,
            "weight_shannons": str_path(commitment, &["weight_shannons"])?,
            "voter_lock_arg": str_path(commitment, &["voter_lock", "args"])?,
            "out_point": get_path(witness, &["out_point"])?.clone(),
            "block_number": block_number,
            "tx_index": u64_path(witness, &["tx_index"])?,
            "output_index": u64_path(witness, &["output_index"])?,
        }));
    }
    Ok(votes)
}

fn verify_header_view(header: &Value, label: &str) -> Result<()> {
    #[cfg(not(feature = "ckb-commitments"))]
    {
        let _ = label;
        let _ = str_path(header, &["hash"])?;
        let _ = u64_path(header, &["number"])?;
        return Ok(());
    }

    #[cfg(feature = "ckb-commitments")]
    {
        let json_header: JsonHeaderView = serde_json::from_value(header.clone())
            .with_context(|| format!("parse {label} as jsonrpc header"))?;
        let core_header: CoreHeaderView = json_header.into();
        let computed_hash = packed_hex(&core_header.hash());
        let declared_hash = str_path(header, &["hash"])?;
        if computed_hash != declared_hash {
            bail!("{label} header hash mismatch: {computed_hash} != {declared_hash}");
        }
        Ok(())
    }
}

fn verify_block_witness(block: &Value, label: &str) -> Result<()> {
    #[cfg(not(feature = "ckb-commitments"))]
    {
        verify_compact_block_witness(block, label)?;
        return Ok(());
    }

    #[cfg(feature = "ckb-commitments")]
    {
        verify_compact_block_witness(block, label)?;

        let json_block: JsonBlockView = serde_json::from_value(block.clone())
            .with_context(|| format!("parse {label} as jsonrpc block"))?;
        for (index, tx) in json_block.transactions.iter().enumerate() {
            verify_transaction_view(tx, &format!("{label} transactions[{index}]"))?;
        }
        let core_block: CoreBlockView = json_block.into();

        let declared_hash = str_path(block, &["header", "hash"])?;
        let computed_hash = packed_hex(&core_block.hash());
        if computed_hash != declared_hash {
            bail!("{label} block hash mismatch: {computed_hash} != {declared_hash}");
        }

        let computed_transactions_root = packed_hex(&core_block.calc_transactions_root());
        let declared_transactions_root = str_path(block, &["header", "transactions_root"])?;
        if computed_transactions_root != declared_transactions_root {
            bail!(
            "{label} transactions_root mismatch: {computed_transactions_root} != {declared_transactions_root}"
        );
        }

        let computed_proposals_hash = packed_hex(&core_block.calc_proposals_hash());
        let declared_proposals_hash = str_path(block, &["header", "proposals_hash"])?;
        if computed_proposals_hash != declared_proposals_hash {
            bail!(
            "{label} proposals_hash mismatch: {computed_proposals_hash} != {declared_proposals_hash}"
        );
        }

        let computed_extra_hash = packed_hex(&core_block.calc_extra_hash().extra_hash());
        let declared_extra_hash = str_path(block, &["header", "extra_hash"])?;
        if computed_extra_hash != declared_extra_hash {
            bail!("{label} extra_hash mismatch: {computed_extra_hash} != {declared_extra_hash}");
        }

        Ok(())
    }
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

    let raw_root_hex = format_hash_hex(&raw_root);
    let witnesses_root_hex = format_hash_hex(&witnesses_root);
    let transactions_root_hex = format_hash_hex(&transactions_root);

    if raw_root_hex != str_path(compact, &["raw_transactions_root"])? {
        bail!("{label} compact raw_transactions_root mismatch");
    }
    if witnesses_root_hex != str_path(compact, &["witnesses_root"])? {
        bail!("{label} compact witnesses_root mismatch");
    }
    if transactions_root_hex != str_path(compact, &["transactions_root"])? {
        bail!("{label} compact transactions_root mismatch");
    }
    if transactions_root_hex != str_path(block, &["header", "transactions_root"])? {
        bail!("{label} header transactions_root mismatch");
    }

    Ok(())
}

fn verify_compact_tx_hash_at(
    block: &Value,
    tx_index: usize,
    expected_tx_hash: &str,
    label: &str,
) -> Result<()> {
    let compact = get_path(block, &["compact"])?;
    let tx_hashes = hash_array_path(compact, &["transaction_hashes"])?;
    let actual = tx_hashes
        .get(tx_index)
        .with_context(|| format!("{label} tx_index out of range"))?;
    let expected = parse_hash_hex(expected_tx_hash)
        .with_context(|| format!("{label} expected tx_hash is not a 32-byte hash"))?;
    if actual != &expected {
        bail!("{label} tx_hash mismatch");
    }
    Ok(())
}

fn verify_transaction_response(value: &Value, label: &str) -> Result<()> {
    verify_raw_transaction_molecule_witness(value, label)?;

    #[cfg(not(feature = "ckb-commitments"))]
    {
        let _ = label;
        let _ = get_path(value, &["transaction", "hash"])?;
        let _ = get_path(value, &["transaction", "outputs"])?
            .as_array()
            .context("transaction outputs is not array")?;
        let _ = get_path(value, &["transaction", "outputs_data"])?
            .as_array()
            .context("transaction outputs_data is not array")?;
        return Ok(());
    }

    #[cfg(feature = "ckb-commitments")]
    {
        let response: TransactionWithStatusResponse = serde_json::from_value(value.clone())
            .with_context(|| format!("parse {label} as jsonrpc transaction response"))?;
        let tx_view = json_transaction_from_response(&response)
            .with_context(|| format!("{label} missing json transaction payload"))?;
        verify_transaction_view(&tx_view, label)
    }
}

#[cfg(feature = "ckb-commitments")]
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
        Either::Right(_) => bail!("expected json transaction response, got hex payload"),
    }
}

#[cfg(feature = "ckb-commitments")]
fn verify_transaction_view(tx_view: &JsonTransactionView, label: &str) -> Result<()> {
    let packed_tx: packed::Transaction = tx_view.inner.clone().into();
    let core_tx = packed_tx.into_view();
    let computed_tx_hash = packed_hex(&core_tx.hash());
    let declared_tx_hash = fixed_hash_hex(&tx_view.hash.0);
    if computed_tx_hash != declared_tx_hash {
        bail!("{label} tx hash mismatch: {computed_tx_hash} != {declared_tx_hash}");
    }
    Ok(())
}

fn verify_raw_transaction_molecule_witness(value: &Value, label: &str) -> Result<()> {
    let raw_hex = str_path(value, &["transaction", "raw_transaction_molecule_hex"])?;
    let raw_bytes = hex::decode(trim_0x(raw_hex))
        .with_context(|| format!("{label} raw_transaction_molecule_hex is not valid hex"))?;
    if raw_bytes.is_empty() {
        bail!("{label} raw_transaction_molecule_hex is empty");
    }
    let computed_tx_hash = ckb_hash(&raw_bytes);
    let declared_tx_hash = str_path(value, &["transaction", "hash"])?;
    if computed_tx_hash != declared_tx_hash {
        bail!("{label} raw transaction hash mismatch: {computed_tx_hash} != {declared_tx_hash}");
    }
    Ok(())
}

fn require_header_in_chain(
    header_chain: &BTreeMap<u64, String>,
    number: u64,
    hash: &str,
) -> Result<()> {
    let expected = header_chain
        .get(&number)
        .with_context(|| format!("header chain missing block {number}"))?;
    if expected != hash {
        bail!("header chain hash mismatch at block {number}");
    }
    Ok(())
}

fn verify_record_proof(proof: &Value) -> Result<Value> {
    if str_path(proof, &["magic"])? != RECORD_PROOF_MAGIC {
        bail!("unexpected record proof magic");
    }
    let key = normalize_outpoint_key(str_path(proof, &["deposit_out_point_key"])?)?;
    let record = get_path(proof, &["record"])?;
    let actual_key = normalize_outpoint_key(str_path(record, &["deposit_out_point_key"])?)?;
    if actual_key != key {
        bail!("record key mismatch");
    }
    let record_hash = leaf_hash(&key, record)?;
    if record_hash != str_path(proof, &["record_hash"])? {
        bail!("record hash mismatch");
    }
    let root = verify_merkle_branch(
        &record_hash,
        get_path(proof, &["proof"])?
            .as_array()
            .context("record proof is not array")?,
        str_path(proof, &["record_map_root"])?,
    )?;
    let commitment = get_path(proof, &["commitment"])?;
    if str_path(commitment, &["record_map_root"])? != root {
        bail!("record_map_root does not match commitment");
    }
    let snapshot_root = ckb_hash(&canonical_json(commitment)?);
    if snapshot_root != str_path(proof, &["snapshot_root"])? {
        bail!("record proof snapshot_root mismatch");
    }
    if snapshot_root != str_path(proof, &["snapshot_id"])? {
        bail!("record proof snapshot_id mismatch");
    }
    Ok(json!({
        "snapshot_id": snapshot_root,
        "snapshot_root": snapshot_root,
        "record_map_root": root,
        "deposit_out_point_key": key,
        "record_hash": record_hash,
        "record": record.clone(),
    }))
}

fn build_tally(
    proposal: &Value,
    public_inputs: &Value,
    mut valid_votes: Vec<Value>,
) -> Result<Value> {
    valid_votes.sort_by_key(vote_order);
    let choices = choice_ids(proposal)?;
    let mut latest_by_deposit = BTreeMap::new();
    let mut superseded_votes = Vec::new();
    for vote in &valid_votes {
        let key = normalize_outpoint_key(str_path(vote, &["deposit_out_point_key"])?)?;
        if let Some(previous) = latest_by_deposit.insert(key, vote.clone()) {
            superseded_votes.push(previous);
        }
    }
    let mut counted_votes = latest_by_deposit.into_values().collect::<Vec<_>>();
    counted_votes.sort_by_key(vote_order);
    superseded_votes.sort_by_key(vote_order);

    let mut choice_weights: BTreeMap<String, u128> =
        choices.iter().map(|choice| (choice.clone(), 0)).collect();
    for vote in &counted_votes {
        let choice = str_path(vote, &["choice"])?;
        let weight = str_path(vote, &["weight_shannons"])?.parse::<u128>()?;
        *choice_weights.entry(choice.to_owned()).or_insert(0) += weight;
    }
    let choice_weights_shannons = choice_weights
        .iter()
        .map(|(choice, weight)| (choice.clone(), json!(weight.to_string())))
        .collect::<Map<_, _>>();

    let invalid_votes = Vec::<Value>::new();
    let commitment = json!({
        "magic": "CKB_GOV_TALLY_V1",
        "version": 1,
        "proposal_id": str_path(public_inputs, &["proposal_id"])?,
        "snapshot_id": str_path(public_inputs, &["snapshot_id"])?,
        "snapshot_root": str_path(public_inputs, &["snapshot_root"])?,
        "vote_window": get_path(proposal, &["manifest", "vote_window"])?.clone(),
        "discovery_method": "historical_block_scan",
        "scan_range": {
            "start_block": u64_path(public_inputs, &["vote_start_block"])?,
            "end_block": u64_path(public_inputs, &["vote_end_block"])?,
        },
        "is_final": true,
        "valid_vote_count": valid_votes.len(),
        "counted_vote_count": counted_votes.len(),
        "superseded_vote_count": superseded_votes.len(),
        "invalid_vote_count": invalid_votes.len(),
        "choice_weights_shannons": choice_weights_shannons,
        "counted_votes_root": merkle_root_records(&counted_votes)?,
        "superseded_votes_root": merkle_root_records(&superseded_votes)?,
        "invalid_votes_root": merkle_root_records(&invalid_votes)?,
    });
    let tally_root = ckb_hash(&canonical_json(&commitment)?);
    Ok(json!({
        "tally_root": tally_root,
        "commitment": commitment,
        "choice_weights_shannons": Value::Object(choice_weights_shannons),
        "valid_vote_count": valid_votes.len(),
        "counted_vote_count": counted_votes.len(),
        "superseded_vote_count": superseded_votes.len(),
        "invalid_vote_count": invalid_votes.len(),
    }))
}

fn compact_vote(vote: &Value) -> Result<Value> {
    Ok(json!({
        "vote_id": get_path(vote, &["vote_id"])?.clone(),
        "deposit_out_point_key": get_path(vote, &["deposit_out_point_key"])?.clone(),
        "choice": get_path(vote, &["choice"])?.clone(),
        "weight_shannons": get_path(vote, &["weight_shannons"])?.clone(),
        "voter_lock_arg": get_path(vote, &["voter_lock_arg"])?.clone(),
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
    merkle_root_from_hex_hashes(&leaf_hashes, EMPTY_TALLY_ROOT_TAG)
}

fn merkle_root_from_hex_hashes(leaf_hashes: &[String], empty_tag: &[u8]) -> Result<String> {
    if leaf_hashes.is_empty() {
        return Ok(ckb_hash(empty_tag));
    }
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

fn verify_merkle_branch(leaf_hash: &str, proof: &[Value], expected_root: &str) -> Result<String> {
    let mut current = hex::decode(trim_0x(leaf_hash)).context("decode leaf hash")?;
    for item in proof {
        let sibling =
            hex::decode(trim_0x(str_path(item, &["hash"])?)).context("decode sibling hash")?;
        current = match str_path(item, &["position"])? {
            "left" => ckb_hash_raw([sibling, current].concat()),
            "right" => ckb_hash_raw([current, sibling].concat()),
            other => bail!("invalid proof position: {other}"),
        };
    }
    let actual = format!("0x{}", hex::encode(current));
    if actual != expected_root {
        bail!("merkle proof root mismatch: {actual} != {expected_root}");
    }
    Ok(actual)
}

fn leaf_hash(key: &str, value: &Value) -> Result<String> {
    Ok(ckb_hash(&canonical_json(&json!({
        "key": key,
        "value": value,
    }))?))
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

fn choice_ids(proposal: &Value) -> Result<Vec<String>> {
    get_path(proposal, &["manifest", "choices"])?
        .as_array()
        .context("proposal choices is not array")?
        .iter()
        .map(|choice| {
            choice
                .get("id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .context("choice missing id")
        })
        .collect()
}

fn shannons_to_ckb_string(shannons: u128) -> String {
    let whole = shannons / 100_000_000;
    let frac = shannons % 100_000_000;
    if frac == 0 {
        whole.to_string()
    } else {
        format!("{whole}.{frac:08}")
            .trim_end_matches('0')
            .to_owned()
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

fn normalize_outpoint_key(value: &str) -> Result<String> {
    let (tx_hash, index) = value
        .split_once(':')
        .with_context(|| format!("invalid outpoint key: {value}"))?;
    if !tx_hash.starts_with("0x") {
        bail!("invalid tx hash in outpoint: {value}");
    }
    let index = if index.starts_with("0x") {
        u64::from_str_radix(trim_0x(index), 16)?
    } else {
        index.parse::<u64>()?
    };
    Ok(format!("{tx_hash}:0x{index:x}"))
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

#[cfg(feature = "ckb-commitments")]
fn packed_hex<T: Entity>(entity: &T) -> String {
    fixed_hash_hex(entity.as_slice())
}

#[cfg(feature = "ckb-commitments")]
fn fixed_hash_hex(bytes: &[u8]) -> String {
    format!("0x{}", hex::encode(bytes))
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
