#!/usr/bin/env python3
import argparse
import json
import sys
import time
import urllib.error
import urllib.request
from hashlib import blake2b
from pathlib import Path
from types import SimpleNamespace


CKB_HASH_PERSON = b"ckb-default-hash"
DEPOSIT_PHASE_DATA = "0x0000000000000000"

SNAPSHOT_MAGIC = "CKB_DAO_TREASURY_SNAPSHOT_V3"
SNAPSHOT_SOURCE_MAGIC = "CKB_DAO_TREASURY_SNAPSHOT_SOURCE_V3"
SNAPSHOT_INDEX_MAGIC = "CKB_DAO_TREASURY_SNAPSHOT_INDEX_V3"
RECORD_PROOF_MAGIC = "CKB_DAO_TREASURY_RECORD_PROOF_V3"
OWNER_PROOF_MAGIC = "CKB_DAO_TREASURY_OWNER_PROOF_V3"


def ckb_hash(data: bytes) -> str:
    return "0x" + blake2b(data, digest_size=32, person=CKB_HASH_PERSON).hexdigest()


def canonical_json(value) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode()


def read_json(path: Path):
    return json.loads(path.read_text())


def write_json(path: Path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def parse_hex(value: str) -> int:
    if not isinstance(value, str) or not value.startswith("0x"):
        raise ValueError(f"expected hex string, got {value!r}")
    return int(value, 16)


def hex_index(index: int) -> str:
    return hex(index)


def normalize_outpoint_key(value: str) -> str:
    tx_hash, index = value.split(":", 1)
    if not tx_hash.startswith("0x"):
        raise ValueError(f"invalid tx hash in outpoint: {value}")
    index_int = int(index, 16) if index.startswith("0x") else int(index)
    return f"{tx_hash}:{hex_index(index_int)}"


def shannons_to_ckb_string(shannons: int) -> str:
    whole, frac = divmod(shannons, 100_000_000)
    if frac == 0:
        return str(whole)
    return f"{whole}.{frac:08d}".rstrip("0")


def source_path_for_snapshot(snapshot_path: Path) -> Path:
    return snapshot_path.with_name(f"{snapshot_path.stem}.source.json")


def index_path_for_snapshot(snapshot_path: Path) -> Path:
    return snapshot_path.with_name(f"{snapshot_path.stem}.index.json")


def record_proof_path(snapshot_path: Path, deposit_out_point_key: str) -> Path:
    short_key = ckb_hash(normalize_outpoint_key(deposit_out_point_key).encode())[2:14]
    return snapshot_path.with_name(f"{snapshot_path.stem}.record-proof-{short_key}.json")


def owner_proof_path(snapshot_path: Path, owner_key: str) -> Path:
    short_key = owner_key.removeprefix("0x")[:12]
    return snapshot_path.with_name(f"{snapshot_path.stem}.owner-proof-{short_key}.json")


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

    def consensus(self):
        return self.call("get_consensus")

    def tip_block_number(self) -> int:
        return parse_hex(self.call("get_tip_block_number"))

    def block_by_number(self, number: int):
        return self.call("get_block_by_number", [hex(number)])


def normalize_script(script):
    if script is None:
        return None
    return {
        "code_hash": script["code_hash"],
        "hash_type": script["hash_type"],
        "args": script["args"],
    }


def owner_key(lock) -> str:
    return ckb_hash(canonical_json(normalize_script(lock)))


def is_dao_type(script, dao_type_hash: str) -> bool:
    return (
        script is not None
        and script.get("code_hash") == dao_type_hash
        and script.get("hash_type") == "type"
        and script.get("args") == "0x"
    )


def out_point_key(out_point) -> str:
    tx_hash = out_point["tx_hash"]
    index = parse_hex(out_point["index"])
    return f"{tx_hash}:{hex_index(index)}"


def build_record(tx, output_index: int, output, output_data: str, block_number: int, tx_index: int):
    capacity_shannons = parse_hex(output["capacity"])
    tx_hash = tx["hash"]
    return {
        "deposit_out_point": {
            "tx_hash": tx_hash,
            "index": hex_index(output_index),
        },
        "deposit_out_point_key": f"{tx_hash}:{hex_index(output_index)}",
        "deposit_block_number": block_number,
        "deposit_tx_index": tx_index,
        "capacity_shannons": str(capacity_shannons),
        "capacity_ckb": shannons_to_ckb_string(capacity_shannons),
        "weight_shannons": str(capacity_shannons),
        "lock": normalize_script(output["lock"]),
        "owner_key": owner_key(output["lock"]),
        "type": normalize_script(output["type"]),
        "output_data": output_data,
    }


def replay_dao_live_set(rpc: RpcClient, snapshot_block_number: int, dao_type_hash: str):
    live = {}
    blocks = 0
    txs = 0
    dao_outputs_seen = 0
    dao_outputs_spent = 0

    for block_number in range(snapshot_block_number + 1):
        block = rpc.block_by_number(block_number)
        if block is None:
            raise RuntimeError(f"block {block_number} is missing")

        blocks += 1
        for tx_index, tx in enumerate(block["transactions"]):
            txs += 1

            for tx_input in tx["inputs"]:
                previous_output = tx_input["previous_output"]
                if previous_output["tx_hash"] == "0x" + "0" * 64:
                    continue
                key = out_point_key(previous_output)
                if key in live:
                    del live[key]
                    dao_outputs_spent += 1

            outputs = tx["outputs"]
            outputs_data = tx["outputs_data"]
            for output_index, output in enumerate(outputs):
                if is_dao_type(output.get("type"), dao_type_hash):
                    record = build_record(
                        tx,
                        output_index,
                        output,
                        outputs_data[output_index],
                        block_number,
                        tx_index,
                    )
                    live[record["deposit_out_point_key"]] = record
                    dao_outputs_seen += 1

    return live, {
        "blocks_scanned": blocks,
        "transactions_scanned": txs,
        "dao_outputs_seen": dao_outputs_seen,
        "dao_outputs_spent_before_snapshot": dao_outputs_spent,
    }


def merkle_root(leaf_hashes, empty_tag: bytes) -> str:
    if not leaf_hashes:
        return ckb_hash(empty_tag)

    level = [bytes.fromhex(h[2:]) for h in leaf_hashes]
    while len(level) > 1:
        if len(level) % 2 == 1:
            level.append(level[-1])
        next_level = []
        for index in range(0, len(level), 2):
            next_level.append(bytes.fromhex(ckb_hash(level[index] + level[index + 1])[2:]))
        level = next_level
    return "0x" + level[0].hex()


def merkle_branch(leaf_hashes, index: int):
    if index < 0 or index >= len(leaf_hashes):
        raise ValueError(f"leaf index out of range: {index}")

    level = [bytes.fromhex(h[2:]) for h in leaf_hashes]
    current_index = index
    proof = []
    while len(level) > 1:
        if len(level) % 2 == 1:
            level.append(level[-1])

        sibling_index = current_index ^ 1
        proof.append(
            {
                "position": "left" if sibling_index < current_index else "right",
                "hash": "0x" + level[sibling_index].hex(),
            }
        )

        next_level = []
        for pair_index in range(0, len(level), 2):
            next_level.append(bytes.fromhex(ckb_hash(level[pair_index] + level[pair_index + 1])[2:]))
        level = next_level
        current_index //= 2

    return proof


def verify_merkle_branch(leaf_hash: str, proof, expected_root: str):
    current = bytes.fromhex(leaf_hash[2:])
    for item in proof:
        sibling = bytes.fromhex(item["hash"][2:])
        if item["position"] == "left":
            current = bytes.fromhex(ckb_hash(sibling + current)[2:])
        elif item["position"] == "right":
            current = bytes.fromhex(ckb_hash(current + sibling)[2:])
        else:
            raise ValueError(f"invalid proof position: {item['position']}")
    actual = "0x" + current.hex()
    if actual != expected_root:
        raise ValueError(f"merkle proof root mismatch: {actual} != {expected_root}")
    return actual


def leaf_hash(key: str, value) -> str:
    return ckb_hash(canonical_json({"key": key, "value": value}))


def build_owner_entries(records):
    grouped = {}
    for record in records:
        key = record["owner_key"]
        entry = grouped.setdefault(
            key,
            {
                "owner_key": key,
                "lock": record["lock"],
                "lock_arg": record["lock"]["args"],
                "deposit_out_point_keys": [],
                "record_count": 0,
                "total_weight_shannons": "0",
                "total_capacity_shannons": "0",
            },
        )
        entry["deposit_out_point_keys"].append(normalize_outpoint_key(record["deposit_out_point_key"]))
        entry["record_count"] += 1
        entry["total_weight_shannons"] = str(int(entry["total_weight_shannons"]) + int(record["weight_shannons"]))
        entry["total_capacity_shannons"] = str(int(entry["total_capacity_shannons"]) + int(record["capacity_shannons"]))

    owner_entries = []
    for entry in grouped.values():
        entry["deposit_out_point_keys"].sort()
        entry["total_weight_ckb"] = shannons_to_ckb_string(int(entry["total_weight_shannons"]))
        owner_entries.append(entry)
    owner_entries.sort(key=lambda entry: entry["owner_key"])
    return owner_entries


def compute_snapshot_from_records(snapshot_block_number: int, snapshot_block_hash: str, dao_type_hash: str, records):
    records = sorted(records, key=lambda record: normalize_outpoint_key(record["deposit_out_point_key"]))
    owner_entries = build_owner_entries(records)

    record_leaf_hashes = [leaf_hash(normalize_outpoint_key(record["deposit_out_point_key"]), record) for record in records]
    owner_leaf_hashes = [leaf_hash(entry["owner_key"], entry) for entry in owner_entries]
    record_map_root = merkle_root(record_leaf_hashes, b"CKB_DAO_TREASURY_EMPTY_RECORD_MAP_V3")
    owner_index_root = merkle_root(owner_leaf_hashes, b"CKB_DAO_TREASURY_EMPTY_OWNER_INDEX_V3")

    total_weight = sum(int(record["weight_shannons"]) for record in records)
    total_capacity = sum(int(record["capacity_shannons"]) for record in records)

    commitment = {
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
        "record_count": len(records),
        "owner_count": len(owner_entries),
        "total_weight_shannons": str(total_weight),
        "total_capacity_shannons": str(total_capacity),
    }
    snapshot_root = ckb_hash(canonical_json(commitment))

    return {
        "snapshot_id": snapshot_root,
        "snapshot_root": snapshot_root,
        "record_map_root": record_map_root,
        "owner_index_root": owner_index_root,
        "record_leaf_hashes": record_leaf_hashes,
        "owner_leaf_hashes": owner_leaf_hashes,
        "records": records,
        "owner_entries": owner_entries,
        "total_weight_shannons": str(total_weight),
        "total_capacity_shannons": str(total_capacity),
        "commitment": commitment,
    }


def build_snapshot_artifacts(args):
    rpc = RpcClient(args.rpc)
    consensus = rpc.consensus()
    dao_type_hash = consensus["dao_type_hash"]
    genesis_hash = consensus["genesis_hash"]
    tip = rpc.tip_block_number()
    snapshot_block_number = tip if args.snapshot_block is None else args.snapshot_block

    if snapshot_block_number > tip:
        raise RuntimeError(f"snapshot block {snapshot_block_number} is above tip {tip}")

    snapshot_block = rpc.block_by_number(snapshot_block_number)
    snapshot_block_hash = snapshot_block["header"]["hash"]

    live, scan_stats = replay_dao_live_set(rpc, snapshot_block_number, dao_type_hash)
    records = [
        record
        for record in live.values()
        if record["output_data"] == DEPOSIT_PHASE_DATA
    ]
    computed = compute_snapshot_from_records(snapshot_block_number, snapshot_block_hash, dao_type_hash, records)

    snapshot = {
        "magic": SNAPSHOT_MAGIC,
        "version": 3,
        "snapshot_id": computed["snapshot_id"],
        "network": {
            "rpc_url": args.rpc,
            "genesis_hash": genesis_hash,
            "dao_type_hash": dao_type_hash,
        },
        "snapshot_block": {
            "number": snapshot_block_number,
            "hash": snapshot_block_hash,
        },
        "eligibility": {
            "dao_type_hash": dao_type_hash,
            "dao_type_hash_type": "type",
            "dao_type_args": "0x",
            "phase": "deposit",
            "output_data": DEPOSIT_PHASE_DATA,
            "weight": "capacity_shannons",
        },
        "stats": {
            **scan_stats,
            "eligible_record_count": len(computed["records"]),
            "owner_count": len(computed["owner_entries"]),
            "total_capacity_shannons": computed["total_capacity_shannons"],
            "total_capacity_ckb": shannons_to_ckb_string(int(computed["total_capacity_shannons"])),
            "total_weight_shannons": computed["total_weight_shannons"],
        },
        "roots": {
            "snapshot_root": computed["snapshot_root"],
            "record_map_root": computed["record_map_root"],
            "owner_index_root": computed["owner_index_root"],
        },
        "commitment": computed["commitment"],
    }
    if args.include_generated_at:
        snapshot["generated_at_unix"] = int(time.time())

    source = {
        "magic": SNAPSHOT_SOURCE_MAGIC,
        "version": 3,
        "snapshot_id": computed["snapshot_id"],
        "snapshot_root": computed["snapshot_root"],
        "snapshot_block": snapshot["snapshot_block"],
        "roots": snapshot["roots"],
        "commitment": computed["commitment"],
        "records": computed["records"],
        "owner_entries": computed["owner_entries"],
        "record_leaf_hashes": computed["record_leaf_hashes"],
        "owner_leaf_hashes": computed["owner_leaf_hashes"],
    }
    index = build_index_from_source(source)
    return snapshot, source, index


def record_proof_from_index(index, deposit_out_point_key: str):
    key = normalize_outpoint_key(deposit_out_point_key)
    if key not in index["by_out_point"]:
        raise ValueError(f"deposit outpoint not found in snapshot index: {deposit_out_point_key}")
    entry = index["by_out_point"][key]
    return {
        "magic": RECORD_PROOF_MAGIC,
        "version": 3,
        "snapshot_id": index["snapshot_id"],
        "snapshot_root": index["snapshot_root"],
        "record_map_root": index["roots"]["record_map_root"],
        "commitment": index["commitment"],
        "deposit_out_point_key": key,
        "record_index": entry["record_index"],
        "record_hash": entry["record_hash"],
        "record": entry["record"],
        "proof": entry["proof"],
    }


def owner_bundle_from_index(index, owner_key_or_lock_arg: str):
    key = owner_key_or_lock_arg
    if len(key) == 42:
        key = index["owner_key_by_lock_arg"].get(key)
        if key is None:
            raise ValueError(f"owner lock arg not found in snapshot index: {owner_key_or_lock_arg}")
    if key not in index["by_owner"]:
        raise ValueError(f"owner key not found in snapshot index: {owner_key_or_lock_arg}")

    owner = index["by_owner"][key]
    record_proofs = [
        record_proof_from_index(index, deposit_key)
        for deposit_key in owner["owner_entry"]["deposit_out_point_keys"]
    ]
    return {
        "magic": OWNER_PROOF_MAGIC,
        "version": 3,
        "snapshot_id": index["snapshot_id"],
        "snapshot_root": index["snapshot_root"],
        "owner_index_root": index["roots"]["owner_index_root"],
        "record_map_root": index["roots"]["record_map_root"],
        "commitment": index["commitment"],
        "owner_key": key,
        "owner_index": owner["owner_index"],
        "owner_hash": owner["owner_hash"],
        "owner_entry": owner["owner_entry"],
        "owner_proof": owner["owner_proof"],
        "record_proofs": record_proofs,
    }


def build_index_from_source(source):
    if source.get("magic") != SNAPSHOT_SOURCE_MAGIC:
        raise RuntimeError(f"unexpected source magic: {source.get('magic')}")
    records = source["records"]
    owner_entries = source["owner_entries"]
    record_leaf_hashes = source["record_leaf_hashes"]
    owner_leaf_hashes = source["owner_leaf_hashes"]

    by_out_point = {}
    for index, record in enumerate(records):
        key = normalize_outpoint_key(record["deposit_out_point_key"])
        by_out_point[key] = {
            "record_index": index,
            "record_hash": record_leaf_hashes[index],
            "record": record,
            "proof": merkle_branch(record_leaf_hashes, index),
        }

    by_owner = {}
    owner_key_by_lock_arg = {}
    for index, entry in enumerate(owner_entries):
        key = entry["owner_key"]
        by_owner[key] = {
            "owner_index": index,
            "owner_hash": owner_leaf_hashes[index],
            "owner_entry": entry,
            "owner_proof": merkle_branch(owner_leaf_hashes, index),
        }
        owner_key_by_lock_arg[entry["lock_arg"]] = key

    return {
        "magic": SNAPSHOT_INDEX_MAGIC,
        "version": 3,
        "snapshot_id": source["snapshot_id"],
        "snapshot_root": source["snapshot_root"],
        "snapshot_block": source["snapshot_block"],
        "roots": source["roots"],
        "commitment": source["commitment"],
        "record_count": len(records),
        "owner_count": len(owner_entries),
        "by_out_point": by_out_point,
        "by_owner": by_owner,
        "owner_key_by_lock_arg": owner_key_by_lock_arg,
    }


def verify_snapshot_object(snapshot):
    if snapshot.get("magic") != SNAPSHOT_MAGIC:
        raise RuntimeError(f"unexpected snapshot magic: {snapshot.get('magic')}")
    commitment = snapshot["commitment"]
    errors = []
    if commitment["record_map_root"] != snapshot["roots"]["record_map_root"]:
        errors.append("record_map_root mismatch")
    if commitment["owner_index_root"] != snapshot["roots"]["owner_index_root"]:
        errors.append("owner_index_root mismatch")
    snapshot_root = ckb_hash(canonical_json(commitment))
    if snapshot_root != snapshot["roots"]["snapshot_root"]:
        errors.append("snapshot_root mismatch")
    if snapshot_root != snapshot["snapshot_id"]:
        errors.append("snapshot_id mismatch")
    if errors:
        raise RuntimeError("; ".join(errors))
    return {
        "snapshot_id": snapshot_root,
        "snapshot_root": snapshot_root,
        "record_map_root": snapshot["roots"]["record_map_root"],
        "owner_index_root": snapshot["roots"]["owner_index_root"],
        "commitment": commitment,
    }


def verify_record_proof(proof):
    if proof.get("magic") != RECORD_PROOF_MAGIC:
        raise RuntimeError(f"unexpected record proof magic: {proof.get('magic')}")
    key = normalize_outpoint_key(proof["deposit_out_point_key"])
    record = proof["record"]
    actual_key = normalize_outpoint_key(record["deposit_out_point_key"])
    if actual_key != key:
        raise RuntimeError(f"record key mismatch: {actual_key} != {key}")
    record_hash = leaf_hash(key, record)
    if record_hash != proof["record_hash"]:
        raise RuntimeError(f"record hash mismatch: {record_hash} != {proof['record_hash']}")
    root = verify_merkle_branch(record_hash, proof["proof"], proof["record_map_root"])
    commitment = proof["commitment"]
    if commitment["record_map_root"] != root:
        raise RuntimeError("record_map_root does not match commitment")
    snapshot_root = ckb_hash(canonical_json(commitment))
    if snapshot_root != proof["snapshot_root"]:
        raise RuntimeError("snapshot_root mismatch")
    if snapshot_root != proof["snapshot_id"]:
        raise RuntimeError("snapshot_id mismatch")
    return {
        "snapshot_id": snapshot_root,
        "snapshot_root": snapshot_root,
        "record_map_root": root,
        "deposit_out_point_key": key,
        "record_hash": record_hash,
        "record": record,
    }


def verify_owner_bundle(bundle):
    if bundle.get("magic") != OWNER_PROOF_MAGIC:
        raise RuntimeError(f"unexpected owner proof magic: {bundle.get('magic')}")
    owner_key_value = bundle["owner_key"]
    owner_entry = bundle["owner_entry"]
    if owner_entry["owner_key"] != owner_key_value:
        raise RuntimeError("owner key mismatch")

    owner_hash = leaf_hash(owner_key_value, owner_entry)
    if owner_hash != bundle["owner_hash"]:
        raise RuntimeError("owner hash mismatch")
    owner_root = verify_merkle_branch(owner_hash, bundle["owner_proof"], bundle["owner_index_root"])
    commitment = bundle["commitment"]
    if commitment["owner_index_root"] != owner_root:
        raise RuntimeError("owner_index_root does not match commitment")
    snapshot_root = ckb_hash(canonical_json(commitment))
    if snapshot_root != bundle["snapshot_root"] or snapshot_root != bundle["snapshot_id"]:
        raise RuntimeError("snapshot root/id mismatch")

    expected_keys = list(owner_entry["deposit_out_point_keys"])
    actual_keys = []
    verified_records = []
    for proof in bundle["record_proofs"]:
        verified = verify_record_proof(proof)
        if verified["snapshot_id"] != snapshot_root:
            raise RuntimeError("record proof snapshot mismatch")
        record = verified["record"]
        if record["owner_key"] != owner_key_value:
            raise RuntimeError(f"record owner mismatch: {verified['deposit_out_point_key']}")
        actual_keys.append(verified["deposit_out_point_key"])
        verified_records.append(record)

    if sorted(actual_keys) != sorted(expected_keys):
        raise RuntimeError("owner bundle record list mismatch")

    return {
        "snapshot_id": snapshot_root,
        "snapshot_root": snapshot_root,
        "owner_index_root": owner_root,
        "owner_key": owner_key_value,
        "record_count": len(verified_records),
        "records": verified_records,
    }


def verify_source_against_snapshot(snapshot, source):
    computed = compute_snapshot_from_records(
        int(snapshot["snapshot_block"]["number"]),
        snapshot["snapshot_block"]["hash"],
        snapshot["network"]["dao_type_hash"],
        source["records"],
    )
    errors = []
    if computed["snapshot_root"] != snapshot["roots"]["snapshot_root"]:
        errors.append("snapshot_root mismatch")
    if computed["record_map_root"] != snapshot["roots"]["record_map_root"]:
        errors.append("record_map_root mismatch")
    if computed["owner_index_root"] != snapshot["roots"]["owner_index_root"]:
        errors.append("owner_index_root mismatch")
    if computed["records"] != source["records"]:
        errors.append("records ordering/content mismatch")
    if computed["owner_entries"] != source["owner_entries"]:
        errors.append("owner_entries mismatch")
    if errors:
        raise RuntimeError("; ".join(errors))
    return computed


def verify_snapshot_against_chain(path: Path, rpc_url: str):
    snapshot = read_json(path)
    verify_snapshot_object(snapshot)
    rebuilt, _source, _index = build_snapshot_artifacts(
        SimpleNamespace(
            rpc=rpc_url,
            snapshot_block=int(snapshot["snapshot_block"]["number"]),
            include_generated_at=False,
        )
    )
    errors = []
    if snapshot["network"]["genesis_hash"] != rebuilt["network"]["genesis_hash"]:
        errors.append("genesis_hash mismatch")
    if snapshot["network"]["dao_type_hash"] != rebuilt["network"]["dao_type_hash"]:
        errors.append("dao_type_hash mismatch")
    if snapshot["snapshot_block"]["hash"] != rebuilt["snapshot_block"]["hash"]:
        errors.append("snapshot_block hash mismatch")
    if snapshot["roots"] != rebuilt["roots"]:
        errors.append("roots mismatch")
    if snapshot["commitment"] != rebuilt["commitment"]:
        errors.append("commitment mismatch")
    if errors:
        raise RuntimeError("; ".join(errors))
    return rebuilt


def main():
    parser = argparse.ArgumentParser(description="Generate and verify proof-first DAO treasury snapshots.")
    subparsers = parser.add_subparsers(dest="command", required=True)

    generate = subparsers.add_parser("generate")
    generate.add_argument("--rpc", default="http://127.0.0.1:8114")
    generate.add_argument("--snapshot-block", type=int)
    generate.add_argument("--output", type=Path)
    generate.add_argument("--include-generated-at", action="store_true")

    verify = subparsers.add_parser("verify")
    verify.add_argument("snapshot", type=Path)
    verify.add_argument("--rpc", default="http://127.0.0.1:8114")
    verify.add_argument("--offline", action="store_true")
    verify.add_argument("--source", type=Path)

    prove_record = subparsers.add_parser("prove-record")
    prove_record.add_argument("index", type=Path)
    prove_record.add_argument("deposit_out_point_key")
    prove_record.add_argument("--output", type=Path)

    prove_owner = subparsers.add_parser("prove-owner")
    prove_owner.add_argument("index", type=Path)
    prove_owner.add_argument("owner_key_or_lock_arg")
    prove_owner.add_argument("--output", type=Path)

    verify_record = subparsers.add_parser("verify-record-proof")
    verify_record.add_argument("proof", type=Path)

    verify_owner = subparsers.add_parser("verify-owner-proof")
    verify_owner.add_argument("proof", type=Path)

    verify_index = subparsers.add_parser("verify-index")
    verify_index.add_argument("index", type=Path)

    args = parser.parse_args()

    if args.command == "generate":
        snapshot, source, index = build_snapshot_artifacts(args)
        output = args.output
        if output is None:
            output = Path(__file__).resolve().parents[1] / "artifacts" / f"snapshot-block-{snapshot['snapshot_block']['number']}.json"
        source_output = source_path_for_snapshot(output)
        index_output = index_path_for_snapshot(output)
        snapshot["artifacts"] = {
            "source": str(source_output),
            "index": str(index_output),
        }
        source["snapshot_artifact"] = str(output)
        index["snapshot_artifact"] = str(output)
        write_json(output, snapshot)
        write_json(source_output, source)
        write_json(index_output, index)
        print(f"snapshot_file: {output}")
        print(f"snapshot_source_file: {source_output}")
        print(f"snapshot_index_file: {index_output}")
        print(f"snapshot_id: {snapshot['snapshot_id']}")
        print(f"snapshot_block: {snapshot['snapshot_block']['number']}")
        print(f"snapshot_block_hash: {snapshot['snapshot_block']['hash']}")
        print(f"eligible_records: {snapshot['stats']['eligible_record_count']}")
        print(f"owners: {snapshot['stats']['owner_count']}")
        print(f"total_capacity_ckb: {snapshot['stats']['total_capacity_ckb']}")
        print(f"record_map_root: {snapshot['roots']['record_map_root']}")
        print(f"owner_index_root: {snapshot['roots']['owner_index_root']}")
        print(f"snapshot_root: {snapshot['roots']['snapshot_root']}")
        return

    if args.command == "verify":
        snapshot = read_json(args.snapshot)
        verified = verify_snapshot_object(snapshot)
        if args.source:
            source = read_json(args.source)
            verify_source_against_snapshot(snapshot, source)
        if args.offline:
            print(f"verified_snapshot_offline: {args.snapshot}")
            print(f"snapshot_id: {verified['snapshot_id']}")
            print(f"record_map_root: {verified['record_map_root']}")
            print(f"owner_index_root: {verified['owner_index_root']}")
            return
        rebuilt = verify_snapshot_against_chain(args.snapshot, args.rpc)
        print(f"verified_snapshot_against_chain: {args.snapshot}")
        print(f"snapshot_id: {rebuilt['snapshot_id']}")
        print(f"snapshot_block: {rebuilt['snapshot_block']['number']}")
        print(f"record_map_root: {rebuilt['roots']['record_map_root']}")
        print(f"owner_index_root: {rebuilt['roots']['owner_index_root']}")
        return

    if args.command == "prove-record":
        index = read_json(args.index)
        proof = record_proof_from_index(index, args.deposit_out_point_key)
        output = args.output or record_proof_path(Path(index["snapshot_artifact"]), args.deposit_out_point_key)
        write_json(output, proof)
        print(f"record_proof_file: {output}")
        print(f"snapshot_id: {proof['snapshot_id']}")
        print(f"deposit_out_point_key: {proof['deposit_out_point_key']}")
        print(f"record_hash: {proof['record_hash']}")
        return

    if args.command == "prove-owner":
        index = read_json(args.index)
        bundle = owner_bundle_from_index(index, args.owner_key_or_lock_arg)
        output = args.output or owner_proof_path(Path(index["snapshot_artifact"]), bundle["owner_key"])
        write_json(output, bundle)
        print(f"owner_proof_file: {output}")
        print(f"snapshot_id: {bundle['snapshot_id']}")
        print(f"owner_key: {bundle['owner_key']}")
        print(f"record_count: {len(bundle['record_proofs'])}")
        return

    if args.command == "verify-record-proof":
        proof = read_json(args.proof)
        verified = verify_record_proof(proof)
        print(f"verified_record_proof: {args.proof}")
        print(f"snapshot_id: {verified['snapshot_id']}")
        print(f"deposit_out_point_key: {verified['deposit_out_point_key']}")
        print(f"record_hash: {verified['record_hash']}")
        return

    if args.command == "verify-owner-proof":
        bundle = read_json(args.proof)
        verified = verify_owner_bundle(bundle)
        print(f"verified_owner_proof: {args.proof}")
        print(f"snapshot_id: {verified['snapshot_id']}")
        print(f"owner_key: {verified['owner_key']}")
        print(f"record_count: {verified['record_count']}")
        return

    if args.command == "verify-index":
        index = read_json(args.index)
        for owner in index["by_owner"].values():
            bundle = owner_bundle_from_index(index, owner["owner_entry"]["owner_key"])
            verify_owner_bundle(bundle)
        print(f"verified_snapshot_index: {args.index}")
        print(f"snapshot_id: {index['snapshot_id']}")
        print(f"record_count: {index['record_count']}")
        print(f"owner_count: {index['owner_count']}")
        return


if __name__ == "__main__":
    try:
        main()
    except Exception as err:
        print(f"error: {err}", file=sys.stderr)
        sys.exit(1)
