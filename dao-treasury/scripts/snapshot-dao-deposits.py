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
SNAPSHOT_MAGIC = "CKB_DAO_TREASURY_SNAPSHOT_V1"


def ckb_hash(data: bytes) -> str:
    return "0x" + blake2b(data, digest_size=32, person=CKB_HASH_PERSON).hexdigest()


def canonical_json(value) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode()


def parse_hex(value: str) -> int:
    if not isinstance(value, str) or not value.startswith("0x"):
        raise ValueError(f"expected hex string, got {value!r}")
    return int(value, 16)


def hex_index(index: int) -> str:
    return hex(index)


def ckb_to_shannons(capacity_hex: str) -> int:
    return parse_hex(capacity_hex)


def shannons_to_ckb_string(shannons: int) -> str:
    whole, frac = divmod(shannons, 100_000_000)
    if frac == 0:
        return str(whole)
    return f"{whole}.{frac:08d}".rstrip("0")


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
    capacity_shannons = ckb_to_shannons(output["capacity"])
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


def merkle_root(leaf_hashes):
    if not leaf_hashes:
        return ckb_hash(b"CKB_DAO_TREASURY_EMPTY_SNAPSHOT_V1")

    level = [bytes.fromhex(h[2:]) for h in leaf_hashes]
    while len(level) > 1:
        if len(level) % 2 == 1:
            level.append(level[-1])
        next_level = []
        for index in range(0, len(level), 2):
            next_level.append(bytes.fromhex(ckb_hash(level[index] + level[index + 1])[2:]))
        level = next_level
    return "0x" + level[0].hex()


def compute_snapshot_roots(snapshot_block_number: int, snapshot_block_hash: str, dao_type_hash: str, records):
    canonical_records = []
    for record in records:
        canonical_records.append(record)

    leaf_hashes = [ckb_hash(canonical_json(record)) for record in canonical_records]
    records_root = merkle_root(leaf_hashes)
    total_weight = sum(int(record["weight_shannons"]) for record in canonical_records)
    total_capacity = sum(int(record["capacity_shannons"]) for record in canonical_records)

    commitment = {
        "magic": SNAPSHOT_MAGIC,
        "snapshot_block_number": snapshot_block_number,
        "snapshot_block_hash": snapshot_block_hash,
        "dao_type_hash": dao_type_hash,
        "eligibility_phase": "deposit",
        "eligibility_output_data": DEPOSIT_PHASE_DATA,
        "weight": "capacity_shannons",
        "records_root": records_root,
        "record_count": len(canonical_records),
        "total_weight_shannons": str(total_weight),
        "total_capacity_shannons": str(total_capacity),
    }
    snapshot_root = ckb_hash(canonical_json(commitment))
    snapshot_id = snapshot_root
    return {
        "leaf_hashes": leaf_hashes,
        "records_root": records_root,
        "snapshot_root": snapshot_root,
        "snapshot_id": snapshot_id,
        "total_weight_shannons": str(total_weight),
        "total_capacity_shannons": str(total_capacity),
        "commitment": commitment,
    }


def build_snapshot(args):
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
    records.sort(key=lambda r: (r["deposit_block_number"], r["deposit_tx_index"], parse_hex(r["deposit_out_point"]["index"]), r["deposit_out_point"]["tx_hash"]))

    roots = compute_snapshot_roots(snapshot_block_number, snapshot_block_hash, dao_type_hash, records)

    snapshot = {
        "magic": SNAPSHOT_MAGIC,
        "version": 2,
        "snapshot_id": roots["snapshot_id"],
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
            "eligible_record_count": len(records),
            "total_capacity_shannons": roots["total_capacity_shannons"],
            "total_capacity_ckb": shannons_to_ckb_string(int(roots["total_capacity_shannons"])),
            "total_weight_shannons": roots["total_weight_shannons"],
        },
        "roots": {
            "records_root": roots["records_root"],
            "snapshot_root": roots["snapshot_root"],
        },
        "commitment": roots["commitment"],
        "records": records,
        "record_hashes": roots["leaf_hashes"],
    }
    if args.include_generated_at:
        snapshot["generated_at_unix"] = int(time.time())
    return snapshot


def verify_snapshot(path: Path):
    snapshot = json.loads(path.read_text())
    required_magic = snapshot.get("magic")
    if required_magic != SNAPSHOT_MAGIC:
        raise RuntimeError(f"unexpected snapshot magic: {required_magic}")

    records = snapshot["records"]
    roots = compute_snapshot_roots(
        int(snapshot["snapshot_block"]["number"]),
        snapshot["snapshot_block"]["hash"],
        snapshot["network"]["dao_type_hash"],
        records,
    )
    errors = []
    if snapshot["roots"]["records_root"] != roots["records_root"]:
        errors.append(f"records_root mismatch: {snapshot['roots']['records_root']} != {roots['records_root']}")
    if snapshot["roots"]["snapshot_root"] != roots["snapshot_root"]:
        errors.append(f"snapshot_root mismatch: {snapshot['roots']['snapshot_root']} != {roots['snapshot_root']}")
    if snapshot.get("snapshot_id") != roots["snapshot_id"]:
        errors.append(f"snapshot_id mismatch: {snapshot.get('snapshot_id')} != {roots['snapshot_id']}")
    if snapshot.get("record_hashes") != roots["leaf_hashes"]:
        errors.append("record_hashes mismatch")
    if errors:
        raise RuntimeError("; ".join(errors))
    return roots


def verify_snapshot_against_chain(path: Path, rpc_url: str):
    snapshot = json.loads(path.read_text())
    offline_roots = verify_snapshot(path)
    rebuilt = build_snapshot(
        SimpleNamespace(
            rpc=rpc_url,
            snapshot_block=int(snapshot["snapshot_block"]["number"]),
            output=None,
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
    if snapshot["roots"]["records_root"] != rebuilt["roots"]["records_root"]:
        errors.append("chain records_root mismatch")
    if snapshot["roots"]["snapshot_root"] != rebuilt["roots"]["snapshot_root"]:
        errors.append("chain snapshot_root mismatch")
    if snapshot["records"] != rebuilt["records"]:
        errors.append("records mismatch")
    if errors:
        raise RuntimeError("; ".join(errors))
    return offline_roots, rebuilt


def main():
    parser = argparse.ArgumentParser(description="Generate or verify DAO treasury voting snapshots.")
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

    args = parser.parse_args()

    if args.command == "generate":
        snapshot = build_snapshot(args)
        output = args.output
        if output is None:
            output = Path(__file__).resolve().parents[1] / "artifacts" / f"snapshot-block-{snapshot['snapshot_block']['number']}.json"
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(snapshot, indent=2, sort_keys=True) + "\n")
        print(f"snapshot_file: {output}")
        print(f"snapshot_id: {snapshot['snapshot_id']}")
        print(f"snapshot_block: {snapshot['snapshot_block']['number']}")
        print(f"snapshot_block_hash: {snapshot['snapshot_block']['hash']}")
        print(f"eligible_records: {snapshot['stats']['eligible_record_count']}")
        print(f"total_capacity_ckb: {snapshot['stats']['total_capacity_ckb']}")
        print(f"records_root: {snapshot['roots']['records_root']}")
        print(f"snapshot_root: {snapshot['roots']['snapshot_root']}")
        return

    if args.offline:
        roots = verify_snapshot(args.snapshot)
        print(f"verified_offline: {args.snapshot}")
        print(f"records_root: {roots['records_root']}")
        print(f"snapshot_root: {roots['snapshot_root']}")
        return

    roots, rebuilt = verify_snapshot_against_chain(args.snapshot, args.rpc)
    print(f"verified_against_chain: {args.snapshot}")
    print(f"snapshot_id: {rebuilt['snapshot_id']}")
    print(f"snapshot_block: {rebuilt['snapshot_block']['number']}")
    print(f"snapshot_block_hash: {rebuilt['snapshot_block']['hash']}")
    print(f"eligible_records: {rebuilt['stats']['eligible_record_count']}")
    print(f"records_root: {roots['records_root']}")
    print(f"snapshot_root: {roots['snapshot_root']}")


if __name__ == "__main__":
    try:
        main()
    except Exception as err:
        print(f"error: {err}", file=sys.stderr)
        sys.exit(1)
