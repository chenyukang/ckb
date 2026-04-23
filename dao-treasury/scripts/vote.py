#!/usr/bin/env python3
import argparse
import importlib.util
import json
import sys
import time
import urllib.error
import urllib.request
from hashlib import blake2b
from pathlib import Path

import proposal as proposal_lib


CKB_HASH_PERSON = b"ckb-default-hash"
VOTE_MAGIC = "CKB_GOV_VOTE_V2"

SIGHASH_TYPE_HASH = "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8"


def load_snapshot_lib():
    path = Path(__file__).with_name("snapshot-dao-deposits.py")
    spec = importlib.util.spec_from_file_location("snapshot_dao_deposits", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


snapshot_lib = load_snapshot_lib()


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
    return snapshot_lib.normalize_outpoint_key(value)


def outpoint_from_key(value: str):
    tx_hash, index = normalize_outpoint_key(value).split(":", 1)
    return {"tx_hash": tx_hash, "index": index}


def parse_cell_data(data_hex: str):
    data = bytes.fromhex(data_hex.removeprefix("0x"))
    return json.loads(data)


def cell_data(commitment) -> bytes:
    return canonical_json(commitment)


def choice_ids(proposal):
    return [choice["id"] for choice in proposal["manifest"]["choices"]]


def snapshot_index_path(snapshot_path: Path) -> Path:
    return snapshot_path.with_name(f"{snapshot_path.stem}.index.json")


def load_record_proof(snapshot_path: Path, snapshot_index_path_value: Path, deposit_out_point_key: str):
    index_path = snapshot_index_path_value or snapshot_index_path(snapshot_path)
    index = read_json(index_path)
    proof = snapshot_lib.record_proof_from_index(index, deposit_out_point_key)
    verified = snapshot_lib.verify_record_proof(proof)
    return proof, verified["record"], index_path


def build_commitment(proposal, snapshot, record, record_proof, choice: str):
    proposal_id = proposal["proposal_id"]
    snapshot_id = snapshot["snapshot_id"]
    deposit_key = normalize_outpoint_key(record["deposit_out_point_key"])
    commitment = {
        "magic": VOTE_MAGIC,
        "version": 2,
        "proposal_id": proposal_id,
        "snapshot_id": snapshot_id,
        "snapshot_root": snapshot["roots"]["snapshot_root"],
        "deposit_out_point": outpoint_from_key(deposit_key),
        "deposit_out_point_key": deposit_key,
        "choice": choice,
        "voter_lock": record["lock"],
        "owner_key": record["owner_key"],
        "weight_shannons": record["weight_shannons"],
        "record_hash": record_proof["record_hash"],
        "record_proof": record_proof,
    }
    commitment["vote_id"] = ckb_hash(canonical_json(commitment))
    return commitment


def create_vote(args):
    proposal = read_json(args.proposal)
    snapshot = read_json(args.snapshot)
    choices = choice_ids(proposal)
    if args.choice not in choices:
        raise ValueError(f"choice {args.choice!r} is not one of {choices}")
    if proposal["manifest"]["snapshot"]["snapshot_id"] != snapshot["snapshot_id"]:
        raise ValueError("proposal snapshot_id does not match snapshot file")

    record_proof, record, index_path = load_record_proof(args.snapshot, args.snapshot_index, args.deposit_out_point)
    if record_proof["snapshot_id"] != snapshot["snapshot_id"]:
        raise ValueError("record proof snapshot_id does not match snapshot file")
    if record_proof["snapshot_root"] != snapshot["roots"]["snapshot_root"]:
        raise ValueError("record proof snapshot_root does not match snapshot file")

    commitment = build_commitment(proposal, snapshot, record, record_proof, args.choice)
    vote_type_script = proposal_lib.vote_type_script(
        commitment["proposal_id"],
        commitment["deposit_out_point_key"],
    )
    short_id = commitment["vote_id"][2:14]
    output_dir = args.output_dir
    vote_path = output_dir / f"vote-{short_id}.json"
    cell_data_path = output_dir / f"vote-{short_id}.cell-data.bin"

    envelope = {
        "magic": VOTE_MAGIC,
        "version": 2,
        "vote_id": commitment["vote_id"],
        "proposal_id": commitment["proposal_id"],
        "snapshot_id": commitment["snapshot_id"],
        "choice": args.choice,
        "snapshot_index_path": str(index_path),
        "vote_type_script": vote_type_script,
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
                "record_hash": commitment["record_hash"],
                "vote_type_script": vote_type_script,
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

    def get_block_by_number(self, block_number: int):
        return self.call("get_block_by_number", [hex(block_number)])


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
    record = None
    if vote_id != recomputed_vote_id:
        errors.append("vote_id mismatch")
    if commitment["proposal_id"] != proposal["proposal_id"]:
        errors.append("proposal_id mismatch")
    if commitment["snapshot_id"] != snapshot["snapshot_id"]:
        errors.append("snapshot_id mismatch")
    if commitment["snapshot_root"] != snapshot["roots"]["snapshot_root"]:
        errors.append("snapshot_root mismatch")
    if commitment["choice"] not in choice_ids(proposal):
        errors.append("invalid choice")

    expected_type_script = proposal_lib.vote_type_script(
        commitment["proposal_id"],
        commitment["deposit_out_point_key"],
    )
    if not proposal_lib.script_equal(cell["output"].get("type"), expected_type_script):
        errors.append("vote cell type script mismatch")

    try:
        verified = snapshot_lib.verify_record_proof(commitment["record_proof"])
        record = verified["record"]
        if verified["snapshot_id"] != snapshot["snapshot_id"]:
            errors.append("record proof snapshot_id mismatch")
        if normalize_outpoint_key(commitment["deposit_out_point_key"]) != verified["deposit_out_point_key"]:
            errors.append("deposit_out_point_key mismatch")
        if commitment["record_hash"] != verified["record_hash"]:
            errors.append("record_hash mismatch")
        if not script_equal(commitment["voter_lock"], record["lock"]):
            errors.append("voter_lock does not match proven snapshot record")
        if commitment["owner_key"] != record["owner_key"]:
            errors.append("owner_key does not match proven snapshot record")
        if commitment["weight_shannons"] != record["weight_shannons"]:
            errors.append("weight does not match proven snapshot record")
    except Exception as err:
        errors.append(f"record proof invalid: {err}")

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
            errors.append("vote tx does not spend an input with the proven snapshot record lock")

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
        "owner_key": commitment["owner_key"],
        "record_hash": commitment["record_hash"],
        "out_point": cell["out_point"],
        "block_number": block_number,
        "tx_index": int(cell["tx_index"], 16),
        "output_index": int(cell["out_point"]["index"], 16),
    }


def discover_votes(rpc: RpcClient, proposal, snapshot, limit: int = 100):
    del limit
    votes = []
    vote_type_args_prefix = proposal_lib.vote_type_args_prefix(proposal["proposal_id"])
    tip = rpc.get_tip_block_number()
    vote_window = proposal["manifest"]["vote_window"]
    end_block = min(tip, vote_window["end_block"])
    if end_block < vote_window["start_block"]:
        return votes

    for block_number in range(vote_window["start_block"], end_block + 1):
        block = rpc.get_block_by_number(block_number)
        if block is None:
            continue
        for tx_index, tx in enumerate(block["transactions"]):
            for output_index, output in enumerate(tx["outputs"]):
                if not proposal_lib.script_has_args_prefix(output.get("type"), vote_type_args_prefix):
                    continue
                output_data = tx["outputs_data"][output_index]
                cell = {
                    "output": output,
                    "output_data": output_data,
                    "out_point": {
                        "tx_hash": tx["hash"],
                        "index": hex(output_index),
                    },
                    "block_number": hex(block_number),
                    "tx_index": hex(tx_index),
                }
                try:
                    result = verify_vote_cell(rpc, proposal, snapshot, cell)
                except Exception as err:
                    result = {
                        "valid": False,
                        "errors": [str(err)],
                        "out_point": cell.get("out_point"),
                        "block_number": block_number,
                        "tx_index": tx_index,
                        "output_index": output_index,
                    }
                votes.append(result)

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
    parser = argparse.ArgumentParser(description="Create, discover, and verify DAO treasury v2 vote artifacts.")
    subparsers = parser.add_subparsers(dest="command", required=True)

    create = subparsers.add_parser("create")
    create.add_argument("--proposal", type=Path, required=True)
    create.add_argument("--snapshot", type=Path, required=True)
    create.add_argument("--snapshot-index", type=Path)
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
