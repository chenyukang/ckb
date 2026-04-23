use anyhow::{Context, Result, anyhow, bail};
use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use blake2b_ref::Blake2bBuilder;
use clap::Parser;
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const SERVICE_MAGIC: &str = "CKB_GOV_TALLY_SERVICE_MVP_V1";
const CKB_HASH_PERSONALIZATION: &[u8] = b"ckb-default-hash";
const SNAPSHOT_MAGIC: &str = "CKB_DAO_TREASURY_SNAPSHOT_V3";
const SNAPSHOT_INDEX_MAGIC: &str = "CKB_DAO_TREASURY_SNAPSHOT_INDEX_V3";
const RECORD_PROOF_MAGIC: &str = "CKB_DAO_TREASURY_RECORD_PROOF_V3";
const OWNER_PROOF_MAGIC: &str = "CKB_DAO_TREASURY_OWNER_PROOF_V3";
const VOTE_TYPE_ARGS_NAMESPACE: &str = "01";
const PROPOSAL_TYPE_ARGS_PREFIX: &str = "0x00";
const DEFAULT_GOVERNANCE_TYPE_CODE_HASH: &str =
    "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5";
const DEFAULT_GOVERNANCE_TYPE_HASH_TYPE: &str = "data";
const DEPOSIT_PHASE_DATA: &str = "0x0000000000000000";
const EMPTY_TALLY_ROOT_TAG: &[u8] = b"CKB_DAO_TREASURY_EMPTY_TALLY_V1";

#[derive(Parser, Debug)]
#[command(about = "Run the DAO treasury MVP Tally JSON-RPC service.")]
struct Args {
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    #[arg(long, default_value_t = 8124)]
    port: u16,

    #[arg(long)]
    ckb_rpc: Option<String>,

    #[arg(long)]
    artifacts_dir: Option<PathBuf>,

    #[arg(long)]
    governance_type_code_hash: Option<String>,

    #[arg(long)]
    governance_type_hash_type: Option<String>,

    #[arg(long, default_value_t = 100)]
    page_limit: u64,
}

#[derive(Clone)]
struct CkbRpc {
    url: String,
    client: reqwest::Client,
    next_id: Arc<AtomicU64>,
}

impl CkbRpc {
    fn new(url: String) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("build CKB RPC client")?;
        Ok(Self {
            url,
            client,
            next_id: Arc::new(AtomicU64::new(1)),
        })
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let payload = json!({
            "id": id,
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let response: Value = self
            .client
            .post(&self.url)
            .json(&payload)
            .send()
            .await
            .with_context(|| format!("CKB RPC request failed: {method}"))?
            .error_for_status()
            .with_context(|| format!("CKB RPC http error: {method}"))?
            .json()
            .await
            .with_context(|| format!("decode CKB RPC response: {method}"))?;
        if !response.get("error").unwrap_or(&Value::Null).is_null() {
            bail!("CKB RPC {method} error: {}", response["error"]);
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }

    async fn get_tip_block_number(&self) -> Result<u64> {
        hex_to_u64(
            self.call("get_tip_block_number", json!([]))
                .await?
                .as_str()
                .context("get_tip_block_number result is not string")?,
        )
    }

    async fn get_consensus(&self) -> Result<Value> {
        self.call("get_consensus", json!([])).await
    }

    async fn get_cells(
        &self,
        search_key: Value,
        limit: u64,
        after: Option<String>,
    ) -> Result<Value> {
        let mut params = vec![search_key, json!("asc"), json!(to_hex(limit))];
        if let Some(cursor) = after {
            params.push(json!(cursor));
        }
        self.call("get_cells", Value::Array(params)).await
    }

    async fn get_block_by_number(&self, number: u64) -> Result<Value> {
        self.call("get_block_by_number", json!([to_hex(number)]))
            .await
    }

    async fn get_transaction(&self, tx_hash: &str) -> Result<Value> {
        self.call("get_transaction", json!([tx_hash])).await
    }
}

#[derive(Clone)]
struct TallyService {
    rpc: CkbRpc,
    ckb_rpc_url: String,
    artifacts_dir: PathBuf,
    repo_root: PathBuf,
    governance_type_code_hash: String,
    governance_type_hash_type: String,
    page_limit: u64,
}

impl TallyService {
    async fn new(
        ckb_rpc_url: String,
        artifacts_dir: PathBuf,
        governance_type_code_hash: String,
        governance_type_hash_type: String,
        page_limit: u64,
    ) -> Result<Self> {
        let artifacts_dir = artifacts_dir
            .canonicalize()
            .with_context(|| format!("canonicalize artifacts dir {}", artifacts_dir.display()))?;
        let dao_treasury_dir = artifacts_dir
            .parent()
            .context("artifacts dir has no parent")?
            .to_path_buf();
        let repo_root = dao_treasury_dir
            .parent()
            .context("dao-treasury dir has no parent")?
            .to_path_buf();
        Ok(Self {
            rpc: CkbRpc::new(ckb_rpc_url.clone())?,
            ckb_rpc_url,
            artifacts_dir,
            repo_root,
            governance_type_code_hash,
            governance_type_hash_type,
            page_limit,
        })
    }

    fn governance_type_script(&self, args: String) -> Value {
        json!({
            "code_hash": self.governance_type_code_hash,
            "hash_type": self.governance_type_hash_type,
            "args": args,
        })
    }

    fn proposal_type_script(&self, proposal_id: &str) -> Value {
        self.governance_type_script(format!("0x00{}", trim_0x(proposal_id)))
    }

    fn vote_type_args_prefix(&self, proposal_id: &str) -> String {
        format!("0x{}{}", VOTE_TYPE_ARGS_NAMESPACE, trim_0x(proposal_id))
    }

    fn vote_type_script(&self, proposal_id: &str, deposit_out_point_key: &str) -> Value {
        let deposit_hash = ckb_hash(deposit_out_point_key.as_bytes());
        self.governance_type_script(format!(
            "{}{}",
            self.vote_type_args_prefix(proposal_id),
            trim_0x(&deposit_hash)
        ))
    }

    fn resolve_artifact_path(&self, value: &str) -> PathBuf {
        let path = PathBuf::from(value);
        if path.is_absolute() {
            return path;
        }
        let repo_relative = self.repo_root.join(&path);
        if repo_relative.exists() {
            return repo_relative;
        }
        self.artifacts_dir.join(path)
    }

    async fn tip(&self) -> Result<u64> {
        self.rpc.get_tip_block_number().await
    }

    async fn dao_type_hash(&self) -> Result<String> {
        let consensus = self.rpc.get_consensus().await?;
        str_path(&consensus, &["dao_type_hash"]).map(ToOwned::to_owned)
    }

    async fn get_tip(&self) -> Result<Value> {
        let tip = self.tip().await?;
        Ok(json!({
            "magic": SERVICE_MAGIC,
            "ckb_rpc_url": self.ckb_rpc_url,
            "tip_block_number": tip,
            "next_block_number": tip + 1,
            "artifacts_dir": self.artifacts_dir,
            "generated_at_unix": now_unix(),
        }))
    }

    async fn discover_proposals(&self, limit: u64) -> Result<Vec<Value>> {
        let search_key = json!({
            "script": self.governance_type_script(PROPOSAL_TYPE_ARGS_PREFIX.to_owned()),
            "script_type": "type",
            "script_search_mode": "prefix",
            "with_data": true,
        });

        let mut proposals = Vec::new();
        let mut cursor = None;
        loop {
            let page = self
                .rpc
                .get_cells(search_key.clone(), limit, cursor)
                .await?;
            let objects = page
                .get("objects")
                .and_then(Value::as_array)
                .context("get_cells result missing objects")?;
            for cell in objects {
                let commitment = parse_cell_data(str_path(cell, &["output_data"])?)
                    .context("parse proposal cell data")?;
                let proposal_id = str_path(&commitment, &["proposal_id"])?;
                let manifest_hash = str_path(&commitment, &["manifest_hash"])?;
                let manifest_uri = str_path(&commitment, &["manifest_uri"])?;
                let manifest_path = self.resolve_artifact_path(manifest_uri);
                let envelope = if manifest_path.exists() {
                    Some(read_json_file(&manifest_path)?)
                } else {
                    None
                };
                let manifest_hash_verified = envelope
                    .as_ref()
                    .and_then(|value| value.get("manifest_hash"))
                    .and_then(Value::as_str)
                    .map(|value| value == manifest_hash)
                    .unwrap_or(false);
                let expected_type_script = self.proposal_type_script(proposal_id);
                let type_script = cell
                    .get("output")
                    .and_then(|value| value.get("type"))
                    .cloned()
                    .unwrap_or(Value::Null);
                let title = envelope
                    .as_ref()
                    .and_then(|value| value.get("manifest"))
                    .and_then(|value| value.get("title"))
                    .cloned()
                    .unwrap_or(Value::Null);
                let summary = envelope
                    .as_ref()
                    .and_then(|value| value.get("manifest"))
                    .and_then(|value| value.get("summary"))
                    .cloned()
                    .unwrap_or(Value::Null);

                proposals.push(json!({
                    "proposal_id": proposal_id,
                    "snapshot_id": str_path(&commitment, &["snapshot_id"])?,
                    "vote_start_block": u64_path(&commitment, &["vote_start_block"])?,
                    "vote_end_block": u64_path(&commitment, &["vote_end_block"])?,
                    "manifest_hash": manifest_hash,
                    "manifest_uri": manifest_uri,
                    "manifest_path": manifest_path,
                    "manifest_found": envelope.is_some(),
                    "manifest_hash_verified": manifest_hash_verified,
                    "type_script": type_script,
                    "expected_type_script": expected_type_script,
                    "type_script_verified": script_equal(
                        cell.get("output").and_then(|value| value.get("type")),
                        Some(&expected_type_script),
                    ),
                    "title": title,
                    "summary": summary,
                    "envelope": envelope.unwrap_or(Value::Null),
                    "out_point": cell.get("out_point").cloned().unwrap_or(Value::Null),
                    "block_number": hex_to_u64(str_path(cell, &["block_number"])?)?,
                    "tx_index": hex_to_u64(str_path(cell, &["tx_index"])?)?,
                }));
            }
            if objects.len() < limit as usize {
                break;
            }
            cursor = page
                .get("last_cursor")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
        }

        proposals.sort_by_key(|value| {
            (
                value
                    .get("block_number")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                value.get("tx_index").and_then(Value::as_u64).unwrap_or(0),
            )
        });
        Ok(proposals)
    }

    async fn get_proposals(&self, params: &Map<String, Value>) -> Result<Value> {
        let include_manifest = bool_param(params, "include_manifest", false);
        let limit = u64_param(params, "limit", self.page_limit)?;
        let proposals = self
            .discover_proposals(limit)
            .await?
            .into_iter()
            .map(|item| compact_proposal(&item, include_manifest))
            .collect::<Result<Vec<_>>>()?;
        Ok(json!({
            "tip_block_number": self.tip().await?,
            "proposal_count": proposals.len(),
            "proposals": proposals,
        }))
    }

    async fn find_proposal(&self, proposal_id: &str) -> Result<(Value, Value)> {
        for item in self.discover_proposals(self.page_limit).await? {
            if str_path(&item, &["proposal_id"])? == proposal_id {
                let envelope = item.get("envelope").cloned().unwrap_or(Value::Null);
                if envelope.is_null() {
                    bail!("proposal manifest not found: {proposal_id}");
                }
                return Ok((item, envelope));
            }
        }
        bail!("proposal not found: {proposal_id}")
    }

    async fn get_proposal(&self, params: &Map<String, Value>) -> Result<Value> {
        let proposal_id = required_str(params, "proposal_id")?;
        let include_manifest = bool_param(params, "include_manifest", true);
        let (item, _proposal) = self.find_proposal(proposal_id).await?;
        Ok(json!({
            "tip_block_number": self.tip().await?,
            "proposal": compact_proposal(&item, include_manifest)?,
        }))
    }

    fn find_snapshot(&self, snapshot_id: &str) -> Result<(PathBuf, Value)> {
        for entry in fs::read_dir(&self.artifacts_dir)
            .with_context(|| format!("read artifacts dir {}", self.artifacts_dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if !name.starts_with("snapshot-")
                || !name.ends_with(".json")
                || name.contains(".source.")
                || name.contains(".index.")
                || name.contains(".record-proof-")
                || name.contains(".owner-proof-")
            {
                continue;
            }
            let snapshot = read_json_file(&path)?;
            if snapshot.get("magic").and_then(Value::as_str) == Some(SNAPSHOT_MAGIC)
                && snapshot.get("snapshot_id").and_then(Value::as_str) == Some(snapshot_id)
            {
                return Ok((path, snapshot));
            }
        }
        bail!("snapshot not found: {snapshot_id}")
    }

    async fn get_snapshot(&self, params: &Map<String, Value>) -> Result<Value> {
        let snapshot_id = required_str(params, "snapshot_id")?;
        let (snapshot_path, snapshot) = self.find_snapshot(snapshot_id)?;
        let index_path = snapshot_index_path(&snapshot_path);
        Ok(json!({
            "snapshot_path": snapshot_path,
            "snapshot_index_path": index_path,
            "snapshot_index_found": index_path.exists(),
            "snapshot": snapshot,
        }))
    }

    async fn get_owner_proof(&self, params: &Map<String, Value>) -> Result<Value> {
        let snapshot_id = required_str(params, "snapshot_id")?;
        let owner = required_str(params, "owner_key_or_lock_arg")?;
        let (snapshot_path, _snapshot) = self.find_snapshot(snapshot_id)?;
        let index_path = snapshot_index_path(&snapshot_path);
        let index = read_json_file(&index_path)?;
        let bundle = owner_bundle_from_index(&index, owner)?;
        let verified = verify_owner_bundle(&bundle)?;
        Ok(json!({
            "snapshot_path": snapshot_path,
            "snapshot_index_path": index_path,
            "verified": verified,
            "owner_proof": bundle,
        }))
    }

    async fn get_record_proof(&self, params: &Map<String, Value>) -> Result<Value> {
        let snapshot_id = required_str(params, "snapshot_id")?;
        let deposit = required_str(params, "deposit_out_point_key")?;
        let (snapshot_path, _snapshot) = self.find_snapshot(snapshot_id)?;
        let index_path = snapshot_index_path(&snapshot_path);
        let index = read_json_file(&index_path)?;
        let proof = record_proof_from_index(&index, deposit)?;
        let verified = verify_record_proof(&proof)?;
        Ok(json!({
            "snapshot_path": snapshot_path,
            "snapshot_index_path": index_path,
            "verified": verified,
            "record_proof": proof,
        }))
    }

    async fn get_votes(&self, params: &Map<String, Value>) -> Result<Value> {
        let proposal_id = required_str(params, "proposal_id")?;
        let (_item, proposal) = self.find_proposal(proposal_id).await?;
        let snapshot_id = str_path(&proposal, &["manifest", "snapshot", "snapshot_id"])?;
        let (snapshot_path, snapshot) = self.find_snapshot(snapshot_id)?;
        let tip = self.tip().await?;
        let scan = scan_range(&proposal, tip, None)?;
        let (votes, stats) = self
            .discover_historical_votes(&proposal, &snapshot, &scan)
            .await?;
        Ok(json!({
            "tip_block_number": tip,
            "proposal_id": proposal_id,
            "snapshot_id": snapshot_id,
            "snapshot_path": snapshot_path,
            "scan": scan,
            "scan_stats": stats,
            "vote_count": votes.len(),
            "votes": votes,
        }))
    }

    async fn get_tally(&self, params: &Map<String, Value>) -> Result<Value> {
        let proposal_id = required_str(params, "proposal_id")?;
        let include_votes = bool_param(params, "include_votes", false);
        let scan_end_block = optional_u64_param(params, "scan_end_block")?;
        let (_item, proposal) = self.find_proposal(proposal_id).await?;
        let snapshot_id = str_path(&proposal, &["manifest", "snapshot", "snapshot_id"])?;
        let (snapshot_path, snapshot) = self.find_snapshot(snapshot_id)?;
        let tip = self.tip().await?;
        let scan = scan_range(&proposal, tip, scan_end_block)?;
        let (votes, stats) = self
            .discover_historical_votes(&proposal, &snapshot, &scan)
            .await?;
        let report = build_tally_report(&proposal, &snapshot, votes, &scan, stats)?;
        Ok(json!({
            "snapshot_path": snapshot_path,
            "tally": compact_tally_report(&report, include_votes),
        }))
    }

    async fn get_voter_options(&self, params: &Map<String, Value>) -> Result<Value> {
        let voter = required_str(params, "voter")?;
        let lock_arg = normalize_account_or_lock_arg(voter)?;
        let limit = u64_param(params, "limit", self.page_limit)?;
        let tip = self.tip().await?;
        let live_cells = self.query_live_dao_deposits(&lock_arg, limit).await?;
        let live_keys = live_cells
            .iter()
            .filter_map(|cell| cell.get("out_point_key").and_then(Value::as_str))
            .map(normalize_outpoint_key)
            .collect::<Result<BTreeSet<_>>>()?;

        let mut proposals = Vec::new();
        for discovered in self.discover_proposals(limit).await? {
            if !discovered
                .get("manifest_found")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || !discovered
                    .get("type_script_verified")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                || !discovered
                    .get("manifest_hash_verified")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            {
                continue;
            }

            let snapshot_id = str_path(&discovered, &["snapshot_id"])?;
            let Ok((snapshot_path, snapshot)) = self.find_snapshot(snapshot_id) else {
                continue;
            };
            let index_path = snapshot_index_path(&snapshot_path);
            if !index_path.exists() {
                continue;
            }
            let proposal = discovered.get("envelope").cloned().unwrap_or(Value::Null);
            let index = read_json_file(&index_path)?;

            let (owner_proof_valid, owner_proof_error, owner_records) =
                match owner_bundle_from_index(&index, &lock_arg)
                    .and_then(|bundle| verify_owner_bundle(&bundle))
                {
                    Ok(verified) => (
                        true,
                        Value::Null,
                        verified
                            .get("records")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default(),
                    ),
                    Err(err) => (false, json!(err.to_string()), Vec::new()),
                };

            let latest_votes = self
                .latest_votes_by_deposit(&proposal, &snapshot, tip)
                .await?;
            let mut eligible_records = Vec::new();
            for record in owner_records {
                let key = normalize_outpoint_key(str_path(&record, &["deposit_out_point_key"])?)?;
                let latest_vote = latest_votes.get(&key);
                eligible_records.push(json!({
                    "deposit_out_point_key": key,
                    "weight_shannons": str_path(&record, &["weight_shannons"])?,
                    "weight_ckb": shannons_to_ckb_string(str_path(&record, &["weight_shannons"])?.parse::<u128>()?),
                    "snapshot_deposit_block_number": u64_path(&record, &["deposit_block_number"])?,
                    "live_now": live_keys.contains(&key),
                    "latest_vote": latest_vote.map(|vote| json!({
                        "choice": vote.get("choice").cloned().unwrap_or(Value::Null),
                        "vote_id": vote.get("vote_id").cloned().unwrap_or(Value::Null),
                        "block_number": vote.get("block_number").cloned().unwrap_or(Value::Null),
                    })).unwrap_or(Value::Null),
                }));
            }

            let eligible_weight = eligible_records
                .iter()
                .filter_map(|record| record.get("weight_shannons").and_then(Value::as_str))
                .map(|value| value.parse::<u128>())
                .collect::<std::result::Result<Vec<_>, _>>()?
                .into_iter()
                .sum::<u128>();
            let (status, can_vote_next_block) = proposal_status(
                tip,
                u64_path(&discovered, &["vote_start_block"])?,
                u64_path(&discovered, &["vote_end_block"])?,
            );
            proposals.push(json!({
                "proposal_id": str_path(&discovered, &["proposal_id"])?,
                "title": discovered.get("title").cloned().unwrap_or(Value::Null),
                "summary": discovered.get("summary").cloned().unwrap_or(Value::Null),
                "status": status,
                "can_vote_next_block": can_vote_next_block && !eligible_records.is_empty(),
                "vote_start_block": u64_path(&discovered, &["vote_start_block"])?,
                "vote_end_block": u64_path(&discovered, &["vote_end_block"])?,
                "snapshot_id": snapshot_id,
                "snapshot_path": snapshot_path,
                "snapshot_index_path": index_path,
                "owner_proof_valid": owner_proof_valid,
                "owner_proof_error": owner_proof_error,
                "eligible_record_count": eligible_records.len(),
                "eligible_weight_ckb": shannons_to_ckb_string(eligible_weight),
                "eligible_records": eligible_records,
            }));
        }

        Ok(json!({
            "voter": voter,
            "voter_lock_arg": lock_arg,
            "tip_block_number": tip,
            "next_block_number": tip + 1,
            "live_dao_deposit_count": live_cells.len(),
            "live_dao_deposits": live_cells,
            "proposals": proposals,
        }))
    }

    async fn query_live_dao_deposits(&self, lock_arg: &str, limit: u64) -> Result<Vec<Value>> {
        let dao_type_hash = self.dao_type_hash().await?;
        let search_key = json!({
            "script": {
                "code_hash": dao_type_hash,
                "hash_type": "type",
                "args": "0x",
            },
            "script_type": "type",
            "script_search_mode": "exact",
            "filter": {
                "output_data": DEPOSIT_PHASE_DATA,
                "output_data_filter_mode": "exact",
            },
            "with_data": true,
        });
        let mut cells = Vec::new();
        let mut cursor = None;
        loop {
            let page = self
                .rpc
                .get_cells(search_key.clone(), limit, cursor)
                .await?;
            let objects = page
                .get("objects")
                .and_then(Value::as_array)
                .context("get_cells result missing objects")?;
            for cell in objects {
                if str_path(cell, &["output", "lock", "args"])? != lock_arg {
                    continue;
                }
                let capacity = hex_to_u64(str_path(cell, &["output", "capacity"])?)? as u128;
                let out_point = cell.get("out_point").cloned().unwrap_or(Value::Null);
                let out_point_key = out_point_key(&out_point)?;
                cells.push(json!({
                    "out_point": out_point,
                    "out_point_key": out_point_key,
                    "capacity_shannons": capacity.to_string(),
                    "capacity_ckb": shannons_to_ckb_string(capacity),
                    "block_number": hex_to_u64(str_path(cell, &["block_number"])?)?,
                    "tx_index": hex_to_u64(str_path(cell, &["tx_index"])?)?,
                }));
            }
            if objects.len() < limit as usize {
                break;
            }
            cursor = page
                .get("last_cursor")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
        }
        cells.sort_by_key(|cell| {
            (
                cell.get("block_number")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                cell.get("tx_index").and_then(Value::as_u64).unwrap_or(0),
                cell.get("out_point")
                    .and_then(|value| value.get("index"))
                    .and_then(Value::as_str)
                    .and_then(|value| hex_to_u64(value).ok())
                    .unwrap_or(0),
            )
        });
        Ok(cells)
    }

    async fn latest_votes_by_deposit(
        &self,
        proposal: &Value,
        snapshot: &Value,
        tip: u64,
    ) -> Result<BTreeMap<String, Value>> {
        let scan_end = tip.min(u64_path(
            proposal,
            &["manifest", "vote_window", "end_block"],
        )?);
        let scan = scan_range(proposal, tip, Some(scan_end))?;
        let (votes, _stats) = self
            .discover_historical_votes(proposal, snapshot, &scan)
            .await?;
        let mut latest = BTreeMap::new();
        for vote in votes
            .into_iter()
            .filter(|vote| vote.get("valid").and_then(Value::as_bool) == Some(true))
        {
            latest.insert(
                normalize_outpoint_key(str_path(&vote, &["deposit_out_point_key"])?)?,
                vote,
            );
        }
        Ok(latest)
    }

    async fn discover_historical_votes(
        &self,
        proposal: &Value,
        snapshot: &Value,
        scan: &Value,
    ) -> Result<(Vec<Value>, Value)> {
        let start = u64_path(scan, &["start_block"])?;
        let end = u64_path(scan, &["end_block"])?;
        let mut votes = Vec::new();
        let mut blocks_scanned = 0u64;
        let mut transactions_scanned = 0u64;
        let mut vote_outputs_seen = 0u64;

        if end < start {
            return Ok((
                votes,
                json!({
                    "blocks_scanned": 0,
                    "transactions_scanned": 0,
                    "vote_outputs_seen": 0,
                }),
            ));
        }

        let proposal_id = str_path(proposal, &["proposal_id"])?;
        let vote_args_prefix = self.vote_type_args_prefix(proposal_id);
        for block_number in start..=end {
            let block = self.rpc.get_block_by_number(block_number).await?;
            if block.is_null() {
                bail!("block {block_number} is missing");
            }
            blocks_scanned += 1;
            let transactions = block
                .get("transactions")
                .and_then(Value::as_array)
                .context("block missing transactions")?;
            for (tx_index, tx) in transactions.iter().enumerate() {
                transactions_scanned += 1;
                let outputs = tx
                    .get("outputs")
                    .and_then(Value::as_array)
                    .context("transaction missing outputs")?;
                let outputs_data = tx
                    .get("outputs_data")
                    .and_then(Value::as_array)
                    .context("transaction missing outputs_data")?;
                for (output_index, output) in outputs.iter().enumerate() {
                    if !self.script_has_args_prefix(output.get("type"), &vote_args_prefix) {
                        continue;
                    }
                    vote_outputs_seen += 1;
                    let cell = json!({
                        "output": output,
                        "output_data": outputs_data.get(output_index).cloned().unwrap_or(Value::Null),
                        "out_point": {
                            "tx_hash": str_path(tx, &["hash"])?,
                            "index": to_hex(output_index as u64),
                        },
                        "block_number": to_hex(block_number),
                        "tx_index": to_hex(tx_index as u64),
                    });
                    let vote = match self.verify_vote_cell(proposal, snapshot, &cell).await {
                        Ok(value) => value,
                        Err(err) => json!({
                            "valid": false,
                            "errors": [err.to_string()],
                            "out_point": cell.get("out_point").cloned().unwrap_or(Value::Null),
                            "block_number": block_number,
                            "tx_index": tx_index,
                            "output_index": output_index,
                        }),
                    };
                    votes.push(vote);
                }
            }
        }
        votes.sort_by_key(vote_order);
        Ok((
            votes,
            json!({
                "blocks_scanned": blocks_scanned,
                "transactions_scanned": transactions_scanned,
                "vote_outputs_seen": vote_outputs_seen,
            }),
        ))
    }

    fn script_has_args_prefix(&self, script: Option<&Value>, args_prefix: &str) -> bool {
        let Some(script) = script else {
            return false;
        };
        script.get("code_hash").and_then(Value::as_str)
            == Some(self.governance_type_code_hash.as_str())
            && script.get("hash_type").and_then(Value::as_str)
                == Some(self.governance_type_hash_type.as_str())
            && script
                .get("args")
                .and_then(Value::as_str)
                .map(|args| args.starts_with(args_prefix))
                .unwrap_or(false)
    }

    async fn verify_vote_cell(
        &self,
        proposal: &Value,
        snapshot: &Value,
        cell: &Value,
    ) -> Result<Value> {
        let mut commitment =
            parse_cell_data(str_path(cell, &["output_data"])?).context("parse vote cell data")?;
        let vote_id = commitment
            .as_object_mut()
            .and_then(|object| object.remove("vote_id"))
            .and_then(|value| value.as_str().map(ToOwned::to_owned));
        let recomputed_vote_id = ckb_hash(&canonical_json(&commitment)?);
        if let Some(vote_id) = &vote_id {
            commitment
                .as_object_mut()
                .context("vote commitment is not object")?
                .insert("vote_id".to_owned(), json!(vote_id));
        }

        let mut errors = Vec::new();
        let mut verified_record = None;
        if vote_id.as_deref() != Some(&recomputed_vote_id) {
            errors.push("vote_id mismatch".to_owned());
        }
        if str_path(&commitment, &["proposal_id"]).ok()
            != Some(str_path(proposal, &["proposal_id"])?)
        {
            errors.push("proposal_id mismatch".to_owned());
        }
        if str_path(&commitment, &["snapshot_id"]).ok()
            != Some(str_path(snapshot, &["snapshot_id"])?)
        {
            errors.push("snapshot_id mismatch".to_owned());
        }
        if str_path(&commitment, &["snapshot_root"]).ok()
            != Some(str_path(snapshot, &["roots", "snapshot_root"])?)
        {
            errors.push("snapshot_root mismatch".to_owned());
        }
        let choices = choice_ids(proposal)?;
        if !choices
            .iter()
            .any(|choice| choice == str_path(&commitment, &["choice"]).unwrap_or(""))
        {
            errors.push("invalid choice".to_owned());
        }

        let expected_type_script = self.vote_type_script(
            str_path(&commitment, &["proposal_id"])?,
            str_path(&commitment, &["deposit_out_point_key"])?,
        );
        if !script_equal(
            cell.get("output").and_then(|value| value.get("type")),
            Some(&expected_type_script),
        ) {
            errors.push("vote cell type script mismatch".to_owned());
        }

        match verify_record_proof(get_path(&commitment, &["record_proof"])?) {
            Ok(verified) => {
                if str_path(&verified, &["snapshot_id"])? != str_path(snapshot, &["snapshot_id"])? {
                    errors.push("record proof snapshot_id mismatch".to_owned());
                }
                if normalize_outpoint_key(str_path(&commitment, &["deposit_out_point_key"])?)?
                    != str_path(&verified, &["deposit_out_point_key"])?
                {
                    errors.push("deposit_out_point_key mismatch".to_owned());
                }
                if str_path(&commitment, &["record_hash"])?
                    != str_path(&verified, &["record_hash"])?
                {
                    errors.push("record_hash mismatch".to_owned());
                }
                let record = get_path(&verified, &["record"])?.clone();
                if !script_equal(
                    Some(get_path(&commitment, &["voter_lock"])?),
                    Some(get_path(&record, &["lock"])?),
                ) {
                    errors.push("voter_lock does not match proven snapshot record".to_owned());
                }
                if str_path(&commitment, &["owner_key"])? != str_path(&record, &["owner_key"])? {
                    errors.push("owner_key does not match proven snapshot record".to_owned());
                }
                if str_path(&commitment, &["weight_shannons"])?
                    != str_path(&record, &["weight_shannons"])?
                {
                    errors.push("weight does not match proven snapshot record".to_owned());
                }
                verified_record = Some(record);
            }
            Err(err) => errors.push(format!("record proof invalid: {err}")),
        }

        let block_number = hex_to_u64(str_path(cell, &["block_number"])?)?;
        let vote_start = u64_path(proposal, &["manifest", "vote_window", "start_block"])?;
        let vote_end = u64_path(proposal, &["manifest", "vote_window", "end_block"])?;
        if !(vote_start..=vote_end).contains(&block_number) {
            errors.push("vote cell is outside vote window".to_owned());
        }

        let tx_hash = str_path(cell, &["out_point", "tx_hash"])?;
        match self.rpc.get_transaction(tx_hash).await {
            Ok(tx_response) => {
                let mut has_owner_input = false;
                if let Some(inputs) = tx_response
                    .get("transaction")
                    .and_then(|value| value.get("inputs"))
                    .and_then(Value::as_array)
                {
                    for input in inputs {
                        let previous_output = get_path(input, &["previous_output"])?;
                        if str_path(previous_output, &["tx_hash"])?
                            == format!("0x{}", "0".repeat(64))
                        {
                            continue;
                        }
                        let previous_lock = self.get_previous_output_lock(previous_output).await?;
                        if let Some(record) = &verified_record {
                            if script_equal(
                                Some(&previous_lock),
                                Some(get_path(record, &["lock"])?),
                            ) {
                                has_owner_input = true;
                                break;
                            }
                        }
                    }
                }
                if !has_owner_input {
                    errors.push(
                        "vote tx does not spend an input with the proven snapshot record lock"
                            .to_owned(),
                    );
                }
            }
            Err(_) => errors.push("cannot load vote tx".to_owned()),
        }

        let vote_id_value = vote_id.map(Value::String).unwrap_or(Value::Null);
        Ok(json!({
            "valid": errors.is_empty(),
            "errors": errors,
            "vote_id": vote_id_value,
            "proposal_id": str_path(&commitment, &["proposal_id"])?,
            "snapshot_id": str_path(&commitment, &["snapshot_id"])?,
            "deposit_out_point_key": str_path(&commitment, &["deposit_out_point_key"])?,
            "choice": str_path(&commitment, &["choice"])?,
            "weight_shannons": str_path(&commitment, &["weight_shannons"])?,
            "voter_lock_arg": str_path(&commitment, &["voter_lock", "args"])?,
            "owner_key": str_path(&commitment, &["owner_key"])?,
            "record_hash": str_path(&commitment, &["record_hash"])?,
            "out_point": cell.get("out_point").cloned().unwrap_or(Value::Null),
            "block_number": block_number,
            "tx_index": hex_to_u64(str_path(cell, &["tx_index"])?)?,
            "output_index": hex_to_u64(str_path(cell, &["out_point", "index"])?)?,
        }))
    }

    async fn get_previous_output_lock(&self, out_point: &Value) -> Result<Value> {
        let tx_hash = str_path(out_point, &["tx_hash"])?;
        let index = hex_to_u64(str_path(out_point, &["index"])?)? as usize;
        let tx = self.rpc.get_transaction(tx_hash).await?;
        tx.get("transaction")
            .and_then(|value| value.get("outputs"))
            .and_then(Value::as_array)
            .and_then(|outputs| outputs.get(index))
            .and_then(|output| output.get("lock"))
            .cloned()
            .ok_or_else(|| anyhow!("cannot load previous tx output lock: {tx_hash}:{index}"))
    }

    async fn dispatch(&self, method: &str, params: &Map<String, Value>) -> Result<Value> {
        match method {
            "tally.get_tip" => self.get_tip().await,
            "tally.get_proposals" => self.get_proposals(params).await,
            "tally.get_proposal" => self.get_proposal(params).await,
            "tally.get_snapshot" => self.get_snapshot(params).await,
            "tally.get_owner_proof" => self.get_owner_proof(params).await,
            "tally.get_record_proof" => self.get_record_proof(params).await,
            "tally.get_votes" => self.get_votes(params).await,
            "tally.get_tally" => self.get_tally(params).await,
            "tally.get_voter_options" => self.get_voter_options(params).await,
            _ => bail!("method not found: {method}"),
        }
    }
}

fn compact_proposal(item: &Value, include_manifest: bool) -> Result<Value> {
    let mut proposal = json!({
        "proposal_id": str_path(item, &["proposal_id"])?,
        "snapshot_id": str_path(item, &["snapshot_id"])?,
        "vote_start_block": u64_path(item, &["vote_start_block"])?,
        "vote_end_block": u64_path(item, &["vote_end_block"])?,
        "manifest_hash": item.get("manifest_hash").cloned().unwrap_or(Value::Null),
        "manifest_uri": item.get("manifest_uri").cloned().unwrap_or(Value::Null),
        "manifest_path": item.get("manifest_path").cloned().unwrap_or(Value::Null),
        "manifest_found": item.get("manifest_found").cloned().unwrap_or(Value::Null),
        "manifest_hash_verified": item.get("manifest_hash_verified").cloned().unwrap_or(Value::Null),
        "type_script_verified": item.get("type_script_verified").cloned().unwrap_or(Value::Null),
        "title": item.get("title").cloned().unwrap_or(Value::Null),
        "summary": item.get("summary").cloned().unwrap_or(Value::Null),
        "out_point": item.get("out_point").cloned().unwrap_or(Value::Null),
        "block_number": u64_path(item, &["block_number"])?,
        "tx_index": u64_path(item, &["tx_index"])?,
    });
    if include_manifest {
        proposal
            .as_object_mut()
            .context("compact proposal is not object")?
            .insert(
                "envelope".to_owned(),
                item.get("envelope").cloned().unwrap_or(Value::Null),
            );
    }
    Ok(proposal)
}

fn compact_tally_report(report: &Value, include_votes: bool) -> Value {
    let mut value = json!({
        "magic": report["magic"],
        "version": report["version"],
        "proposal_id": report["proposal_id"],
        "snapshot_id": report["snapshot_id"],
        "snapshot_root": report["snapshot_root"],
        "tally_root": report["tally_root"],
        "is_final": report["is_final"],
        "generated_at_unix": report["generated_at_unix"],
        "generated_at_tip_block": report["generated_at_tip_block"],
        "scan_stats": report["scan_stats"],
        "commitment": report["commitment"],
        "choice_weights_shannons": report["choice_weights_shannons"],
        "choice_weights_ckb": report["choice_weights_ckb"],
    });
    if include_votes {
        let object = value.as_object_mut().expect("object");
        object.insert("valid_votes".to_owned(), report["valid_votes"].clone());
        object.insert("counted_votes".to_owned(), report["counted_votes"].clone());
        object.insert(
            "superseded_votes".to_owned(),
            report["superseded_votes"].clone(),
        );
        object.insert("invalid_votes".to_owned(), report["invalid_votes"].clone());
    }
    value
}

fn scan_range(
    proposal: &Value,
    tip_block_number: u64,
    scan_end_block: Option<u64>,
) -> Result<Value> {
    let start_block = u64_path(proposal, &["manifest", "vote_window", "start_block"])?;
    let final_end_block = u64_path(proposal, &["manifest", "vote_window", "end_block"])?;
    let end_block = match scan_end_block {
        Some(end) => {
            if end > final_end_block {
                bail!("scan end block {end} is after vote window end {final_end_block}");
            }
            if end > tip_block_number {
                bail!("scan end block {end} is above chain tip {tip_block_number}");
            }
            end
        }
        None => final_end_block.min(tip_block_number),
    };
    Ok(json!({
        "start_block": start_block,
        "end_block": end_block,
        "vote_end_block": final_end_block,
        "tip_block_number": tip_block_number,
        "is_final": tip_block_number >= final_end_block && end_block == final_end_block,
    }))
}

fn build_tally_report(
    proposal: &Value,
    snapshot: &Value,
    votes: Vec<Value>,
    scan: &Value,
    scan_stats: Value,
) -> Result<Value> {
    let choices = choice_ids(proposal)?;
    let mut valid_votes = Vec::new();
    let mut invalid_votes = Vec::new();
    for vote in votes {
        if vote.get("valid").and_then(Value::as_bool).unwrap_or(false) {
            valid_votes.push(vote);
        } else {
            invalid_votes.push(vote);
        }
    }
    valid_votes.sort_by_key(vote_order);

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
    invalid_votes.sort_by_key(vote_order);

    let mut choice_weights = BTreeMap::new();
    for choice in &choices {
        choice_weights.insert(choice.clone(), 0u128);
    }
    for vote in &counted_votes {
        let choice = str_path(vote, &["choice"])?;
        let weight = str_path(vote, &["weight_shannons"])?.parse::<u128>()?;
        *choice_weights.entry(choice.to_owned()).or_insert(0) += weight;
    }

    let counted_records = counted_votes
        .iter()
        .map(compact_vote)
        .collect::<Result<Vec<_>>>()?;
    let superseded_records = superseded_votes
        .iter()
        .map(compact_vote)
        .collect::<Result<Vec<_>>>()?;
    let invalid_records = invalid_votes
        .iter()
        .map(compact_invalid_vote)
        .collect::<Result<Vec<_>>>()?;

    let choice_weights_shannons = choice_weights
        .iter()
        .map(|(key, value)| (key.clone(), json!(value.to_string())))
        .collect::<Map<_, _>>();
    let choice_weights_ckb = choice_weights
        .iter()
        .map(|(key, value)| (key.clone(), json!(shannons_to_ckb_string(*value))))
        .collect::<Map<_, _>>();

    let commitment = json!({
        "magic": "CKB_GOV_TALLY_V1",
        "version": 1,
        "proposal_id": str_path(proposal, &["proposal_id"])?,
        "snapshot_id": str_path(snapshot, &["snapshot_id"])?,
        "snapshot_root": str_path(snapshot, &["roots", "snapshot_root"])?,
        "vote_window": get_path(proposal, &["manifest", "vote_window"])?.clone(),
        "discovery_method": "historical_block_scan",
        "scan_range": {
            "start_block": u64_path(scan, &["start_block"])?,
            "end_block": u64_path(scan, &["end_block"])?,
        },
        "is_final": bool_path(scan, &["is_final"])?,
        "valid_vote_count": valid_votes.len(),
        "counted_vote_count": counted_records.len(),
        "superseded_vote_count": superseded_records.len(),
        "invalid_vote_count": invalid_records.len(),
        "choice_weights_shannons": choice_weights_shannons,
        "counted_votes_root": merkle_root_records(&counted_records)?,
        "superseded_votes_root": merkle_root_records(&superseded_records)?,
        "invalid_votes_root": merkle_root_records(&invalid_records)?,
    });
    let tally_root = ckb_hash(&canonical_json(&commitment)?);

    Ok(json!({
        "magic": "CKB_GOV_TALLY_V1",
        "version": 1,
        "proposal_id": str_path(proposal, &["proposal_id"])?,
        "snapshot_id": str_path(snapshot, &["snapshot_id"])?,
        "snapshot_root": str_path(snapshot, &["roots", "snapshot_root"])?,
        "tally_root": tally_root,
        "is_final": bool_path(scan, &["is_final"])?,
        "generated_at_unix": now_unix(),
        "generated_at_tip_block": u64_path(scan, &["tip_block_number"])?,
        "scan_stats": scan_stats,
        "commitment": commitment,
        "choice_weights_shannons": Value::Object(choice_weights_shannons),
        "choice_weights_ckb": Value::Object(choice_weights_ckb),
        "valid_votes": valid_votes.iter().map(compact_vote).collect::<Result<Vec<_>>>()?,
        "counted_votes": counted_records,
        "superseded_votes": superseded_records,
        "invalid_votes": invalid_records,
    }))
}

fn compact_vote(vote: &Value) -> Result<Value> {
    Ok(json!({
        "vote_id": vote.get("vote_id").cloned().unwrap_or(Value::Null),
        "deposit_out_point_key": vote.get("deposit_out_point_key").cloned().unwrap_or(Value::Null),
        "choice": vote.get("choice").cloned().unwrap_or(Value::Null),
        "weight_shannons": vote.get("weight_shannons").cloned().unwrap_or(Value::Null),
        "voter_lock_arg": vote.get("voter_lock_arg").cloned().unwrap_or(Value::Null),
        "out_point": vote.get("out_point").cloned().unwrap_or(Value::Null),
        "block_number": vote.get("block_number").cloned().unwrap_or(Value::Null),
        "tx_index": vote.get("tx_index").cloned().unwrap_or(Value::Null),
        "output_index": vote.get("output_index").cloned().unwrap_or(Value::Null),
    }))
}

fn compact_invalid_vote(vote: &Value) -> Result<Value> {
    let mut compact = compact_vote(vote)?;
    compact
        .as_object_mut()
        .context("compact vote is not object")?
        .insert(
            "errors".to_owned(),
            vote.get("errors").cloned().unwrap_or_else(|| json!([])),
        );
    Ok(compact)
}

fn record_proof_from_index(index: &Value, deposit_out_point_key: &str) -> Result<Value> {
    if index.get("magic").and_then(Value::as_str) != Some(SNAPSHOT_INDEX_MAGIC) {
        bail!("unexpected snapshot index magic");
    }
    let key = normalize_outpoint_key(deposit_out_point_key)?;
    let entry = get_path(index, &["by_out_point", &key])
        .with_context(|| format!("deposit outpoint not found in snapshot index: {key}"))?;
    Ok(json!({
        "magic": RECORD_PROOF_MAGIC,
        "version": 3,
        "snapshot_id": str_path(index, &["snapshot_id"])?,
        "snapshot_root": str_path(index, &["snapshot_root"])?,
        "record_map_root": str_path(index, &["roots", "record_map_root"])?,
        "commitment": get_path(index, &["commitment"])?.clone(),
        "deposit_out_point_key": key,
        "record_index": get_path(entry, &["record_index"])?.clone(),
        "record_hash": str_path(entry, &["record_hash"])?,
        "record": get_path(entry, &["record"])?.clone(),
        "proof": get_path(entry, &["proof"])?.clone(),
    }))
}

fn owner_bundle_from_index(index: &Value, owner_key_or_lock_arg: &str) -> Result<Value> {
    if index.get("magic").and_then(Value::as_str) != Some(SNAPSHOT_INDEX_MAGIC) {
        bail!("unexpected snapshot index magic");
    }
    let key = if owner_key_or_lock_arg.len() == 42 {
        str_path(index, &["owner_key_by_lock_arg", owner_key_or_lock_arg])
            .with_context(|| {
                format!("owner lock arg not found in snapshot index: {owner_key_or_lock_arg}")
            })?
            .to_owned()
    } else {
        owner_key_or_lock_arg.to_owned()
    };
    let owner = get_path(index, &["by_owner", &key])
        .with_context(|| format!("owner key not found in snapshot index: {key}"))?;
    let deposit_keys = get_path(owner, &["owner_entry", "deposit_out_point_keys"])?
        .as_array()
        .context("owner entry deposit_out_point_keys is not array")?;
    let mut record_proofs = Vec::new();
    for deposit_key in deposit_keys {
        record_proofs.push(record_proof_from_index(
            index,
            deposit_key
                .as_str()
                .context("deposit_out_point_key is not string")?,
        )?);
    }
    Ok(json!({
        "magic": OWNER_PROOF_MAGIC,
        "version": 3,
        "snapshot_id": str_path(index, &["snapshot_id"])?,
        "snapshot_root": str_path(index, &["snapshot_root"])?,
        "owner_index_root": str_path(index, &["roots", "owner_index_root"])?,
        "record_map_root": str_path(index, &["roots", "record_map_root"])?,
        "commitment": get_path(index, &["commitment"])?.clone(),
        "owner_key": key,
        "owner_index": get_path(owner, &["owner_index"])?.clone(),
        "owner_hash": str_path(owner, &["owner_hash"])?,
        "owner_entry": get_path(owner, &["owner_entry"])?.clone(),
        "owner_proof": get_path(owner, &["owner_proof"])?.clone(),
        "record_proofs": record_proofs,
    }))
}

fn verify_record_proof(proof: &Value) -> Result<Value> {
    if proof.get("magic").and_then(Value::as_str) != Some(RECORD_PROOF_MAGIC) {
        bail!("unexpected record proof magic: {:?}", proof.get("magic"));
    }
    let key = normalize_outpoint_key(str_path(proof, &["deposit_out_point_key"])?)?;
    let record = get_path(proof, &["record"])?;
    let actual_key = normalize_outpoint_key(str_path(record, &["deposit_out_point_key"])?)?;
    if actual_key != key {
        bail!("record key mismatch: {actual_key} != {key}");
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
        bail!("snapshot_root mismatch");
    }
    if snapshot_root != str_path(proof, &["snapshot_id"])? {
        bail!("snapshot_id mismatch");
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

fn verify_owner_bundle(bundle: &Value) -> Result<Value> {
    if bundle.get("magic").and_then(Value::as_str) != Some(OWNER_PROOF_MAGIC) {
        bail!("unexpected owner proof magic: {:?}", bundle.get("magic"));
    }
    let owner_key = str_path(bundle, &["owner_key"])?;
    let owner_entry = get_path(bundle, &["owner_entry"])?;
    if str_path(owner_entry, &["owner_key"])? != owner_key {
        bail!("owner key mismatch");
    }
    let owner_hash = leaf_hash(owner_key, owner_entry)?;
    if owner_hash != str_path(bundle, &["owner_hash"])? {
        bail!("owner hash mismatch");
    }
    let owner_root = verify_merkle_branch(
        &owner_hash,
        get_path(bundle, &["owner_proof"])?
            .as_array()
            .context("owner proof is not array")?,
        str_path(bundle, &["owner_index_root"])?,
    )?;
    let commitment = get_path(bundle, &["commitment"])?;
    if str_path(commitment, &["owner_index_root"])? != owner_root {
        bail!("owner_index_root does not match commitment");
    }
    let snapshot_root = ckb_hash(&canonical_json(commitment)?);
    if snapshot_root != str_path(bundle, &["snapshot_root"])?
        || snapshot_root != str_path(bundle, &["snapshot_id"])?
    {
        bail!("snapshot root/id mismatch");
    }

    let expected_keys = get_path(owner_entry, &["deposit_out_point_keys"])?
        .as_array()
        .context("owner_entry deposit_out_point_keys is not array")?
        .iter()
        .map(|value| {
            normalize_outpoint_key(
                value
                    .as_str()
                    .context("deposit_out_point_key is not string")?,
            )
        })
        .collect::<Result<BTreeSet<_>>>()?;
    let mut actual_keys = BTreeSet::new();
    let mut verified_records = Vec::new();
    for proof in get_path(bundle, &["record_proofs"])?
        .as_array()
        .context("record_proofs is not array")?
    {
        let verified = verify_record_proof(proof)?;
        if str_path(&verified, &["snapshot_id"])? != snapshot_root {
            bail!("record proof snapshot mismatch");
        }
        let record = get_path(&verified, &["record"])?.clone();
        if str_path(&record, &["owner_key"])? != owner_key {
            bail!(
                "record owner mismatch: {}",
                str_path(&verified, &["deposit_out_point_key"])?
            );
        }
        actual_keys.insert(str_path(&verified, &["deposit_out_point_key"])?.to_owned());
        verified_records.push(record);
    }
    if actual_keys != expected_keys {
        bail!("owner bundle record list mismatch");
    }
    Ok(json!({
        "snapshot_id": snapshot_root,
        "snapshot_root": snapshot_root,
        "owner_index_root": owner_root,
        "owner_key": owner_key,
        "record_count": verified_records.len(),
        "records": verified_records,
    }))
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

fn merkle_root_records(records: &[Value]) -> Result<String> {
    if records.is_empty() {
        return Ok(ckb_hash(EMPTY_TALLY_ROOT_TAG));
    }
    let leaf_hashes = records
        .iter()
        .map(|record| Ok(hex::decode(trim_0x(&ckb_hash(&canonical_json(record)?)))?))
        .collect::<Result<Vec<_>>>()?;
    merkle_root_from_hashes(leaf_hashes)
}

fn merkle_root_from_hashes(mut level: Vec<Vec<u8>>) -> Result<String> {
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

fn leaf_hash(key: &str, value: &Value) -> Result<String> {
    Ok(ckb_hash(&canonical_json(&json!({
        "key": key,
        "value": value,
    }))?))
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

fn parse_cell_data(data_hex: &str) -> Result<Value> {
    let data = hex::decode(trim_0x(data_hex)).context("decode cell data hex")?;
    serde_json::from_slice(&data).context("decode cell data json")
}

fn read_json_file(path: &Path) -> Result<Value> {
    let data = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&data).with_context(|| format!("parse {}", path.display()))
}

fn snapshot_index_path(snapshot_path: &Path) -> PathBuf {
    let stem = snapshot_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("snapshot");
    snapshot_path.with_file_name(format!("{stem}.index.json"))
}

fn script_equal(left: Option<&Value>, right: Option<&Value>) -> bool {
    let (Some(left), Some(right)) = (left, right) else {
        return false;
    };
    left.get("code_hash").and_then(Value::as_str) == right.get("code_hash").and_then(Value::as_str)
        && left.get("hash_type").and_then(Value::as_str)
            == right.get("hash_type").and_then(Value::as_str)
        && left.get("args").and_then(Value::as_str) == right.get("args").and_then(Value::as_str)
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
            return hex_to_u64(text);
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

fn required_str<'a>(params: &'a Map<String, Value>, name: &str) -> Result<&'a str> {
    params
        .get(name)
        .and_then(Value::as_str)
        .with_context(|| format!("missing required string param: {name}"))
}

fn bool_param(params: &Map<String, Value>, name: &str, default: bool) -> bool {
    params.get(name).and_then(Value::as_bool).unwrap_or(default)
}

fn u64_param(params: &Map<String, Value>, name: &str, default: u64) -> Result<u64> {
    match params.get(name) {
        Some(value) if value.is_u64() => Ok(value.as_u64().unwrap()),
        Some(value) if value.is_string() => Ok(value.as_str().unwrap().parse::<u64>()?),
        Some(_) => bail!("param {name} must be u64"),
        None => Ok(default),
    }
}

fn optional_u64_param(params: &Map<String, Value>, name: &str) -> Result<Option<u64>> {
    match params.get(name) {
        Some(value) if value.is_u64() => Ok(value.as_u64()),
        Some(value) if value.is_string() => Ok(Some(value.as_str().unwrap().parse::<u64>()?)),
        Some(Value::Null) | None => Ok(None),
        Some(_) => bail!("param {name} must be u64"),
    }
}

fn hex_to_u64(value: &str) -> Result<u64> {
    u64::from_str_radix(trim_0x(value), 16).with_context(|| format!("parse hex u64: {value}"))
}

fn to_hex(value: u64) -> String {
    format!("0x{value:x}")
}

fn trim_0x(value: &str) -> &str {
    value.strip_prefix("0x").unwrap_or(value)
}

fn normalize_outpoint_key(value: &str) -> Result<String> {
    let (tx_hash, index) = value
        .split_once(':')
        .with_context(|| format!("invalid outpoint key: {value}"))?;
    if !tx_hash.starts_with("0x") {
        bail!("invalid tx hash in outpoint: {value}");
    }
    let index = if index.starts_with("0x") {
        hex_to_u64(index)?
    } else {
        index.parse::<u64>()?
    };
    Ok(format!("{tx_hash}:{}", to_hex(index)))
}

fn out_point_key(out_point: &Value) -> Result<String> {
    Ok(format!(
        "{}:{}",
        str_path(out_point, &["tx_hash"])?,
        to_hex(hex_to_u64(str_path(out_point, &["index"])?)?)
    ))
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

fn proposal_status(tip: u64, start_block: u64, end_block: u64) -> (&'static str, bool) {
    let next_block = tip + 1;
    if next_block < start_block {
        ("pending", false)
    } else if (start_block..=end_block).contains(&next_block) {
        ("open", true)
    } else {
        ("ended", false)
    }
}

fn normalize_account_or_lock_arg(value: &str) -> Result<String> {
    let lower = value.to_ascii_lowercase();
    let lock_arg = match lower.as_str() {
        "alice" => "0x7dec345bc7c2e18dbe47e07b362e6ff0d9b00f82",
        "bob" => "0x977120455b83c232da8520c7db7da7aa29ef0125",
        "carol" => "0x9a0e7b573eb5aba438d3853c7a3749bcf7820d04",
        "proposer" => "0x02c774d39943cc8e64d6c2c472a89fff9a317054",
        "treasury-recipient" => "0x7a5ab692c338254b7cc5b16a4817c53b77407172",
        _ => value,
    };
    if lock_arg.starts_with("0x") && lock_arg.len() == 42 {
        Ok(lock_arg.to_owned())
    } else {
        bail!("unknown account or lock arg: {value}")
    }
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

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn params_object(value: Option<Value>) -> Result<Map<String, Value>> {
    match value {
        None | Some(Value::Null) => Ok(Map::new()),
        Some(Value::Object(object)) => Ok(object),
        Some(Value::Array(mut values)) if values.len() == 1 && values[0].is_object() => {
            Ok(values.remove(0).as_object().cloned().unwrap())
        }
        Some(_) => bail!("params must be an object"),
    }
}

fn jsonrpc_result(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

fn jsonrpc_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message.into(),
        },
    })
}

#[derive(Clone)]
struct AppState {
    service: Arc<TallyService>,
}

async fn health(State(state): State<AppState>) -> Json<Value> {
    match state.service.get_tip().await {
        Ok(value) => Json(json!({"ok": true, "service": value})),
        Err(err) => Json(json!({"ok": false, "error": err.to_string()})),
    }
}

async fn rpc_handler(State(state): State<AppState>, Json(payload): Json<Value>) -> Json<Value> {
    if let Value::Array(items) = payload {
        let mut responses = Vec::new();
        for item in items {
            responses.push(handle_jsonrpc_request(state.service.clone(), item).await);
        }
        return Json(Value::Array(responses));
    }
    Json(handle_jsonrpc_request(state.service, payload).await)
}

async fn handle_jsonrpc_request(service: Arc<TallyService>, request: Value) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    if request.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return jsonrpc_error(id, -32600, "invalid request");
    }
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        return jsonrpc_error(id, -32600, "invalid request");
    };
    let params = match params_object(request.get("params").cloned()) {
        Ok(params) => params,
        Err(err) => return jsonrpc_error(id, -32602, err.to_string()),
    };
    if !matches!(
        method,
        "tally.get_tip"
            | "tally.get_proposals"
            | "tally.get_proposal"
            | "tally.get_snapshot"
            | "tally.get_owner_proof"
            | "tally.get_record_proof"
            | "tally.get_votes"
            | "tally.get_tally"
            | "tally.get_voter_options"
    ) {
        return jsonrpc_error(id, -32601, format!("method not found: {method}"));
    }
    match service.dispatch(method, &params).await {
        Ok(result) => jsonrpc_result(id, result),
        Err(err) => jsonrpc_error(id, -32000, err.to_string()),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let ckb_rpc = args
        .ckb_rpc
        .or_else(|| env::var("CKB_RPC_URL").ok())
        .unwrap_or_else(|| "http://127.0.0.1:8114".to_owned());
    let artifacts_dir = args
        .artifacts_dir
        .or_else(|| {
            env::var("DAO_TREASURY_ARTIFACTS_DIR")
                .ok()
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| PathBuf::from("dao-treasury/artifacts"));
    let governance_type_code_hash = args
        .governance_type_code_hash
        .or_else(|| env::var("DAO_TREASURY_GOV_TYPE_CODE_HASH").ok())
        .unwrap_or_else(|| DEFAULT_GOVERNANCE_TYPE_CODE_HASH.to_owned());
    let governance_type_hash_type = args
        .governance_type_hash_type
        .or_else(|| env::var("DAO_TREASURY_GOV_TYPE_HASH_TYPE").ok())
        .unwrap_or_else(|| DEFAULT_GOVERNANCE_TYPE_HASH_TYPE.to_owned());
    let service = Arc::new(
        TallyService::new(
            ckb_rpc.clone(),
            artifacts_dir,
            governance_type_code_hash,
            governance_type_hash_type,
            args.page_limit,
        )
        .await?,
    );
    let state = AppState { service };
    let app = Router::new()
        .route("/health", get(health))
        .route("/", post(rpc_handler))
        .with_state(state);
    let addr: SocketAddr = format!("{}:{}", args.host, args.port).parse()?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "magic": SERVICE_MAGIC,
            "listen": format!("http://{addr}"),
            "ckb_rpc_url": ckb_rpc,
        }))?
    );
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
