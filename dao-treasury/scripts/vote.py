#!/usr/bin/env python3
import argparse
import json
import sys
import time
import urllib.error
import urllib.request
from hashlib import blake2b
from pathlib import Path


CKB_HASH_PERSON = b"ckb-default-hash"
VOTE_MAGIC = "CKB_GOV_VOTE_V1"
VOTE_CELL_PREFIX = b"CKB_GOV_VOTE_V1\n"

SIGHASH_TYPE_HASH = "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8"


def ckb_hash(data: bytes) -> str:
    return "0x" + blake2b(data, digest_size=32, person=CKB_HASH_PERSON).hexdigest()


def canonical_json(value) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode()


def read_json(path: Path):
    return json.loads(path.read_text())


def write_json(path: Path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def normalize_outpoint_key(value: str) -> str:
    tx_hash, index = value.split(":", 1)
    if not tx_hash.startswith("0x"):
        raise ValueError(f"invalid tx hash in outpoint: {value}")
    if index.startswith("0x"):
        index_int = int(index, 16)
    else:
        index_int = int(index)
    return f"{tx_hash}:{hex(index_int)}"


def outpoint_from_key(value: str):
    tx_hash, index = normalize_outpoint_key(value).split(":", 1)
    return {"tx_hash": tx_hash, "index": index}


def parse_cell_data(data_hex: str):
    data = bytes.fromhex(data_hex.removeprefix("0x"))
    if not data.startswith(VOTE_CELL_PREFIX):
        raise ValueError("not a vote cell")
    return json.loads(data[len(VOTE_CELL_PREFIX) :])


def cell_data(commitment) -> bytes:
    return VOTE_CELL_PREFIX + canonical_json(commitment)


def choice_ids(proposal):
    return [choice["id"] for choice in proposal["manifest"]["choices"]]


def snapshot_record(snapshot, deposit_out_point_key: str):
    normalized = normalize_outpoint_key(deposit_out_point_key)
    for record in snapshot["records"]:
        if normalize_outpoint_key(record["deposit_out_point_key"]) == normalized:
            return record
    raise ValueError(f"deposit outpoint not found in snapshot: {deposit_out_point_key}")


def build_commitment(proposal, snapshot, record, choice: str):
    proposal_id = proposal["proposal_id"]
    snapshot_id = snapshot["snapshot_id"]
    deposit_key = normalize_outpoint_key(record["deposit_out_point_key"])
    commitment = {
        "magic": VOTE_MAGIC,
        "version": 1,
        "proposal_id": proposal_id,
        "snapshot_id": snapshot_id,
        "snapshot_root": snapshot["roots"]["snapshot_root"],
        "deposit_out_point": outpoint_from_key(deposit_key),
        "deposit_out_point_key": deposit_key,
        "choice": choice,
        "voter_lock": record["lock"],
        "weight_shannons": record["weight_shannons"],
    }
    commitment["vote_id"] = ckb_hash(canonical_json(commitment))
    return commitment


def create_vote(args):
    proposal = read_json(args.proposal)
    snapshot = read_json(args.snapshot)
    record = snapshot_record(snapshot, args.deposit_out_point)
    choices = choice_ids(proposal)
    if args.choice not in choices:
        raise ValueError(f"choice {args.choice!r} is not one of {choices}")
    if proposal["manifest"]["snapshot"]["snapshot_id"] != snapshot["snapshot_id"]:
        raise ValueError("proposal snapshot_id does not match snapshot file")

    commitment = build_commitment(proposal, snapshot, record, args.choice)
    short_id = commitment["vote_id"][2:14]
    output_dir = args.output_dir
    vote_path = output_dir / f"vote-{short_id}.json"
    cell_data_path = output_dir / f"vote-{short_id}.cell-data.bin"

    envelope = {
        "magic": VOTE_MAGIC,
        "version": 1,
        "vote_id": commitment["vote_id"],
        "proposal_id": commitment["proposal_id"],
        "snapshot_id": commitment["snapshot_id"],
        "choice": args.choice,
        "commitment": commitment,
    }
    write_json(vote_path, envelope)
    cell_data_path.write_bytes(cell_data(commitment))

    print(
        json.dumps(
            {
                "vote_id": commitment["vote_id"],
                "proposal_id": commitment["proposal_id"],
                "snapshot_id": commitment["snapshot_id"],
                "deposit_out_point_key": commitment["deposit_out_point_key"],
                "choice": commitment["choice"],
                "voter_lock_arg": commitment["voter_lock"]["args"],
                "weight_shannons": commitment["weight_shannons"],
                "vote_path": str(vote_path),
                "cell_data_path": str(cell_data_path),
            },
            indent=2,
            sort_keys=True,
        )
    )


class RpcClient:
    def __init__(self, url: str):
        self.url = url
        self.next_id = 1

    def call(self, method: str, params=None):
        if params is None:
            params = []
        payload = {
            "id": self.next_id,
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }
        self.next_id += 1
        req = urllib.request.Request(
            self.url,
            data=json.dumps(payload).encode(),
            headers={"content-type": "application/json"},
            method="POST",
        )
        try:
            with urllib.request.urlopen(req, timeout=30) as resp:
                body = json.loads(resp.read())
        except urllib.error.URLError as err:
            raise RuntimeError(f"RPC request failed: {err}") from err
        if body.get("error"):
            raise RuntimeError(f"RPC {method} error: {body['error']}")
        return body["result"]

    def get_cells(self, search_key, order="asc", limit=100, after=None):
        params = [search_key, order, hex(limit)]
        if after is not None:
            params.append(after)
        return self.call("get_cells", params)

    def get_transaction(self, tx_hash: str):
        return self.call("get_transaction", [tx_hash])

    def get_tip_block_number(self):
        return int(self.call("get_tip_block_number"), 16)


def get_previous_output_lock(rpc: RpcClient, out_point):
    tx_hash = out_point["tx_hash"]
    index = int(out_point["index"], 16)
    tx = rpc.get_transaction(tx_hash)
    if tx is None or tx.get("transaction") is None:
        raise RuntimeError(f"cannot load previous tx: {tx_hash}")
    return tx["transaction"]["outputs"][index]["lock"]


def script_equal(left, right) -> bool:
    return (
        left.get("code_hash") == right.get("code_hash")
        and left.get("hash_type") == right.get("hash_type")
        and left.get("args") == right.get("args")
    )


def verify_vote_cell(rpc: RpcClient, proposal, snapshot, cell):
    commitment = parse_cell_data(cell["output_data"])
    vote_id = commitment.pop("vote_id", None)
    recomputed_vote_id = ckb_hash(canonical_json(commitment))
    commitment["vote_id"] = vote_id

    errors = []
    if vote_id != recomputed_vote_id:
        errors.append("vote_id mismatch")
    if commitment["proposal_id"] != proposal["proposal_id"]:
        errors.append("proposal_id mismatch")
    if commitment["snapshot_id"] != snapshot["snapshot_id"]:
        errors.append("snapshot_id mismatch")
    if commitment["choice"] not in choice_ids(proposal):
        errors.append("invalid choice")

    try:
        record = snapshot_record(snapshot, commitment["deposit_out_point_key"])
        if not script_equal(commitment["voter_lock"], record["lock"]):
            errors.append("voter_lock does not match snapshot record")
        if commitment["weight_shannons"] != record["weight_shannons"]:
            errors.append("weight does not match snapshot record")
    except Exception as err:
        record = None
        errors.append(str(err))

    block_number = int(cell["block_number"], 16)
    vote_window = proposal["manifest"]["vote_window"]
    if not (vote_window["start_block"] <= block_number <= vote_window["end_block"]):
        errors.append("vote cell is outside vote window")

    tx_hash = cell["out_point"]["tx_hash"]
    tx = rpc.get_transaction(tx_hash)
    if tx is None or tx.get("transaction") is None:
        errors.append("cannot load vote tx")
    else:
        has_owner_input = False
        for tx_input in tx["transaction"]["inputs"]:
            previous_output = tx_input["previous_output"]
            if previous_output["tx_hash"] == "0x" + "0" * 64:
                continue
            previous_lock = get_previous_output_lock(rpc, previous_output)
            if record is not None and script_equal(previous_lock, record["lock"]):
                has_owner_input = True
                break
        if not has_owner_input:
            errors.append("vote tx does not spend an input with the snapshot record lock")

    return {
        "valid": not errors,
        "errors": errors,
        "vote_id": vote_id,
        "proposal_id": commitment["proposal_id"],
        "snapshot_id": commitment["snapshot_id"],
        "deposit_out_point_key": commitment["deposit_out_point_key"],
        "choice": commitment["choice"],
        "weight_shannons": commitment["weight_shannons"],
        "voter_lock_arg": commitment["voter_lock"]["args"],
        "out_point": cell["out_point"],
        "block_number": block_number,
        "tx_index": int(cell["tx_index"], 16),
        "output_index": int(cell["out_point"]["index"], 16),
    }


def discover_votes(rpc: RpcClient, proposal, snapshot, limit: int = 100):
    prefix_hex = "0x" + VOTE_CELL_PREFIX.hex()
    locks = []
    seen = set()
    for record in snapshot["records"]:
        lock = record["lock"]
        key = canonical_json(lock).decode()
        if key not in seen:
            seen.add(key)
            locks.append(lock)

    votes = []
    for lock in locks:
        search_key = {
            "script": lock,
            "script_type": "lock",
            "script_search_mode": "exact",
            "filter": {
                "output_data": prefix_hex,
                "output_data_filter_mode": "prefix",
            },
            "with_data": True,
        }
        cursor = None
        while True:
            page = rpc.get_cells(search_key, limit=limit, after=cursor)
            objects = page["objects"]
            for cell in objects:
                try:
                    result = verify_vote_cell(rpc, proposal, snapshot, cell)
                except Exception as err:
                    result = {
                        "valid": False,
                        "errors": [str(err)],
                        "out_point": cell.get("out_point"),
                        "block_number": int(cell["block_number"], 16),
                    }
                votes.append(result)
            if len(objects) < limit:
                break
            cursor = page["last_cursor"]

    votes.sort(key=lambda vote: (vote.get("block_number", 0), vote.get("tx_index", 0), vote.get("output_index", 0)))
    return votes


def discover(args):
    rpc = RpcClient(args.rpc)
    proposal = read_json(args.proposal)
    snapshot = read_json(args.snapshot)
    votes = discover_votes(rpc, proposal, snapshot, args.limit)
    print(json.dumps({"votes": votes}, indent=2, sort_keys=True))


def record_chain(args):
    envelope = read_json(args.vote)
    envelope["chain"] = {
        "tx_hash": args.tx_hash,
        "submitted_at_unix": int(time.time()),
    }
    write_json(args.vote, envelope)
    print(f"recorded_chain_tx: {args.tx_hash}")
    print(f"vote: {args.vote}")


def main():
    parser = argparse.ArgumentParser(description="Create, discover, and verify DAO treasury vote MVP artifacts.")
    subparsers = parser.add_subparsers(dest="command", required=True)

    create = subparsers.add_parser("create")
    create.add_argument("--proposal", type=Path, required=True)
    create.add_argument("--snapshot", type=Path, required=True)
    create.add_argument("--deposit-out-point", required=True)
    create.add_argument("--choice", required=True)
    create.add_argument("--output-dir", type=Path, required=True)

    discover_parser = subparsers.add_parser("discover")
    discover_parser.add_argument("--rpc", default="http://127.0.0.1:8114")
    discover_parser.add_argument("--proposal", type=Path, required=True)
    discover_parser.add_argument("--snapshot", type=Path, required=True)
    discover_parser.add_argument("--limit", type=int, default=100)

    record = subparsers.add_parser("record-chain")
    record.add_argument("--vote", type=Path, required=True)
    record.add_argument("--tx-hash", required=True)

    args = parser.parse_args()
    if args.command == "create":
        create_vote(args)
    elif args.command == "discover":
        discover(args)
    elif args.command == "record-chain":
        record_chain(args)


if __name__ == "__main__":
    try:
        main()
    except Exception as err:
        print(f"error: {err}", file=sys.stderr)
        sys.exit(1)
