#!/usr/bin/env python3
import argparse
import importlib.util
import json
import sys
from pathlib import Path

import proposal as proposal_lib
import tally as tally_lib
import vote as vote_lib


ACCOUNTS = {
    "alice": "0x7dec345bc7c2e18dbe47e07b362e6ff0d9b00f82",
    "bob": "0x977120455b83c232da8520c7db7da7aa29ef0125",
    "carol": "0x9a0e7b573eb5aba438d3853c7a3749bcf7820d04",
    "proposer": "0x02c774d39943cc8e64d6c2c472a89fff9a317054",
    "treasury-recipient": "0x7a5ab692c338254b7cc5b16a4817c53b77407172",
}

DEPOSIT_PHASE_DATA = "0x0000000000000000"
DAO_TYPE_HASH = "0x82d76d1b75fe2fd9a27dfbaa65a039221a380d76c926f378d3f81cf3e7e13f2e"


def load_snapshot_lib():
    path = Path(__file__).with_name("snapshot-dao-deposits.py")
    spec = importlib.util.spec_from_file_location("snapshot_dao_deposits", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


snapshot_lib = load_snapshot_lib()


def normalize_account_or_lock_arg(value: str) -> str:
    lowered = value.lower()
    if lowered in ACCOUNTS:
        return ACCOUNTS[lowered]
    if value.startswith("0x") and len(value) == 42:
        return value
    raise ValueError(f"unknown account or lock arg: {value}")


def shannons_to_ckb_string(shannons: int) -> str:
    whole, frac = divmod(shannons, 100_000_000)
    if frac == 0:
        return str(whole)
    return f"{whole}.{frac:08d}".rstrip("0")


def query_live_dao_deposits(rpc: vote_lib.RpcClient, lock_arg: str, limit: int):
    search_key = {
        "script": {
            "code_hash": DAO_TYPE_HASH,
            "hash_type": "type",
            "args": "0x",
        },
        "script_type": "type",
        "script_search_mode": "exact",
        "filter": {
            "output_data": DEPOSIT_PHASE_DATA,
            "output_data_filter_mode": "exact",
        },
        "with_data": True,
    }

    cells = []
    cursor = None
    while True:
        page = rpc.get_cells(search_key, limit=limit, after=cursor)
        for cell in page["objects"]:
            output = cell["output"]
            if output["lock"]["args"] != lock_arg:
                continue
            capacity = int(output["capacity"], 16)
            out_point = cell["out_point"]
            cells.append(
                {
                    "out_point": out_point,
                    "out_point_key": f"{out_point['tx_hash']}:{out_point['index']}",
                    "capacity_shannons": str(capacity),
                    "capacity_ckb": shannons_to_ckb_string(capacity),
                    "block_number": int(cell["block_number"], 16),
                    "tx_index": int(cell["tx_index"], 16),
                }
            )
        if len(page["objects"]) < limit:
            break
        cursor = page["last_cursor"]

    cells.sort(key=lambda cell: (cell["block_number"], cell["tx_index"], int(cell["out_point"]["index"], 16)))
    return cells


def discover_proposals(rpc: vote_lib.RpcClient, proposer_lock_arg: str, limit: int):
    prefix_hex = "0x" + proposal_lib.PROPOSAL_CELL_PREFIX.hex()
    search_key = {
        "script": {
            "code_hash": proposal_lib.SIGHASH_TYPE_HASH,
            "hash_type": "type",
            "args": proposer_lock_arg,
        },
        "script_type": "lock",
        "script_search_mode": "exact",
        "filter": {
            "output_data": prefix_hex,
            "output_data_filter_mode": "prefix",
        },
        "with_data": True,
    }

    proposals = []
    cursor = None
    while True:
        page = rpc.get_cells(search_key, limit=limit, after=cursor)
        for cell in page["objects"]:
            commitment = proposal_lib.parse_cell_data(cell["output_data"])
            manifest_path = Path(commitment["manifest_uri"])
            envelope = proposal_lib.read_json(manifest_path) if manifest_path.exists() else None
            proposals.append(
                {
                    "proposal_id": commitment["proposal_id"],
                    "snapshot_id": commitment["snapshot_id"],
                    "vote_start_block": commitment["vote_start_block"],
                    "vote_end_block": commitment["vote_end_block"],
                    "manifest_path": str(manifest_path),
                    "manifest_found": envelope is not None,
                    "manifest_hash_verified": bool(envelope and envelope.get("manifest_hash") == commitment["manifest_hash"]),
                    "title": envelope["manifest"].get("title") if envelope else None,
                    "summary": envelope["manifest"].get("summary") if envelope else None,
                    "envelope": envelope,
                    "out_point": cell["out_point"],
                    "block_number": int(cell["block_number"], 16),
                    "tx_index": int(cell["tx_index"], 16),
                }
            )
        if len(page["objects"]) < limit:
            break
        cursor = page["last_cursor"]

    proposals.sort(key=lambda item: (item["block_number"], item["tx_index"]))
    return proposals


def find_snapshot(snapshot_dir: Path, snapshot_id: str):
    for path in sorted(snapshot_dir.glob("snapshot-*.json")):
        snapshot = vote_lib.read_json(path)
        if snapshot.get("magic") == snapshot_lib.SNAPSHOT_MAGIC and snapshot.get("snapshot_id") == snapshot_id:
            return path, snapshot
    return None, None


def snapshot_index_path(snapshot_path: Path) -> Path:
    return snapshot_path.with_name(f"{snapshot_path.stem}.index.json")


def proposal_status(tip: int, start_block: int, end_block: int):
    next_block = tip + 1
    if next_block < start_block:
        return "pending", False
    if start_block <= next_block <= end_block:
        return "open", True
    return "ended", False


def latest_votes_by_deposit(rpc, proposal, snapshot, tip):
    scan = tally_lib.scan_range(proposal, tip, min(tip, proposal["manifest"]["vote_window"]["end_block"]))
    votes, _stats = tally_lib.discover_historical_votes(rpc, proposal, snapshot, scan)
    latest = {}
    for item in [vote for vote in votes if vote.get("valid")]:
        key = vote_lib.normalize_outpoint_key(item["deposit_out_point_key"])
        latest[key] = item
    return latest


def build_report(args):
    rpc = vote_lib.RpcClient(args.rpc)
    lock_arg = normalize_account_or_lock_arg(args.voter)
    tip = rpc.get_tip_block_number()
    live_cells = query_live_dao_deposits(rpc, lock_arg, args.limit)
    live_keys = {vote_lib.normalize_outpoint_key(cell["out_point_key"]) for cell in live_cells}

    proposals = []
    for discovered in discover_proposals(rpc, args.proposer_lock_arg, args.limit):
        if not discovered["envelope"]:
            if not args.include_incomplete:
                continue
            proposals.append(
                {
                    "proposal_id": discovered["proposal_id"],
                    "title": discovered["title"],
                    "status": "manifest_missing",
                    "can_vote_next_block": False,
                    "eligible_records": [],
                }
            )
            continue

        if not discovered["manifest_hash_verified"]:
            if not args.include_incomplete:
                continue
            proposals.append(
                {
                    "proposal_id": discovered["proposal_id"],
                    "title": discovered["title"],
                    "status": "manifest_hash_mismatch",
                    "can_vote_next_block": False,
                    "manifest_path": discovered["manifest_path"],
                    "eligible_records": [],
                }
            )
            continue

        snapshot_path, snapshot = find_snapshot(args.snapshot_dir, discovered["snapshot_id"])
        status, can_vote = proposal_status(tip, discovered["vote_start_block"], discovered["vote_end_block"])
        if snapshot is None:
            if not args.include_incomplete:
                continue
            proposals.append(
                {
                    "proposal_id": discovered["proposal_id"],
                    "title": discovered["title"],
                    "status": status,
                    "can_vote_next_block": False,
                    "snapshot_found": False,
                    "eligible_records": [],
                }
            )
            continue

        index_path = snapshot_index_path(snapshot_path)
        if not index_path.exists():
            if not args.include_incomplete:
                continue
            proposals.append(
                {
                    "proposal_id": discovered["proposal_id"],
                    "title": discovered["title"],
                    "status": status,
                    "can_vote_next_block": False,
                    "snapshot_found": True,
                    "snapshot_index_found": False,
                    "snapshot_index_path": str(index_path),
                    "eligible_records": [],
                }
            )
            continue

        proposal = discovered["envelope"]
        index = vote_lib.read_json(index_path)
        try:
            owner_bundle = snapshot_lib.owner_bundle_from_index(index, lock_arg)
            verified_owner = snapshot_lib.verify_owner_bundle(owner_bundle)
            owner_proof_valid = True
            owner_proof_error = None
            owner_records = verified_owner["records"]
        except Exception as err:
            owner_proof_valid = False
            owner_proof_error = str(err)
            owner_records = []

        latest_votes = latest_votes_by_deposit(rpc, proposal, snapshot, tip)
        eligible_records = []
        for record in owner_records:
            key = vote_lib.normalize_outpoint_key(record["deposit_out_point_key"])
            latest_vote = latest_votes.get(key)
            eligible_records.append(
                {
                    "deposit_out_point_key": key,
                    "weight_shannons": record["weight_shannons"],
                    "weight_ckb": shannons_to_ckb_string(int(record["weight_shannons"])),
                    "snapshot_deposit_block_number": record["deposit_block_number"],
                    "live_now": key in live_keys,
                    "latest_vote": {
                        "choice": latest_vote["choice"],
                        "vote_id": latest_vote["vote_id"],
                        "block_number": latest_vote["block_number"],
                    }
                    if latest_vote
                    else None,
                }
            )

        proposals.append(
            {
                "proposal_id": discovered["proposal_id"],
                "title": discovered["title"],
                "summary": discovered["summary"],
                "status": status,
                "can_vote_next_block": can_vote and bool(eligible_records),
                "vote_start_block": discovered["vote_start_block"],
                "vote_end_block": discovered["vote_end_block"],
                "snapshot_id": discovered["snapshot_id"],
                "snapshot_path": str(snapshot_path),
                "snapshot_index_path": str(index_path),
                "owner_proof_valid": owner_proof_valid,
                "owner_proof_error": owner_proof_error,
                "eligible_record_count": len(eligible_records),
                "eligible_weight_ckb": shannons_to_ckb_string(sum(int(record["weight_shannons"]) for record in eligible_records)),
                "eligible_records": eligible_records,
            }
        )

    return {
        "voter": args.voter,
        "voter_lock_arg": lock_arg,
        "tip_block_number": tip,
        "next_block_number": tip + 1,
        "live_dao_deposit_count": len(live_cells),
        "live_dao_deposits": live_cells,
        "proposals": proposals,
    }


def main():
    parser = argparse.ArgumentParser(description="List DAO live cells and proposal voting options for a voter.")
    parser.add_argument("voter", help="alice, bob, carol, or a 20-byte lock arg hex")
    parser.add_argument("--rpc", default="http://127.0.0.1:8114")
    parser.add_argument("--snapshot-dir", type=Path, default=Path("dao-treasury/artifacts"))
    parser.add_argument("--proposer-lock-arg", default=proposal_lib.DEFAULT_PROPOSER_LOCK_ARG)
    parser.add_argument("--limit", type=int, default=100)
    parser.add_argument(
        "--include-incomplete",
        action="store_true",
        help="include on-chain proposal cells whose local manifest, snapshot, or index is missing or unverifiable",
    )
    args = parser.parse_args()
    print(json.dumps(build_report(args), indent=2, sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except Exception as err:
        print(f"error: {err}", file=sys.stderr)
        sys.exit(1)
