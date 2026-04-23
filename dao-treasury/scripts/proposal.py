#!/usr/bin/env python3
import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.request
from hashlib import blake2b
from pathlib import Path


CKB_HASH_PERSON = b"ckb-default-hash"
PROPOSAL_MAGIC = "CKB_GOV_PROPOSAL_V1"
PROPOSAL_COMMITMENT_MAGIC = "CKB_GOV_PROPOSAL_COMMITMENT_V1"
DEFAULT_GOVERNANCE_TYPE_CODE_HASH = "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5"
GOVERNANCE_TYPE_CODE_HASH = os.environ.get("DAO_TREASURY_GOV_TYPE_CODE_HASH", DEFAULT_GOVERNANCE_TYPE_CODE_HASH)
GOVERNANCE_TYPE_HASH_TYPE = os.environ.get("DAO_TREASURY_GOV_TYPE_HASH_TYPE", "data")
PROPOSAL_TYPE_ARGS_NAMESPACE = "00"
VOTE_TYPE_ARGS_NAMESPACE = "01"

SIGHASH_TYPE_HASH = "0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8"
DEFAULT_PROPOSER_LOCK_ARG = "0x02c774d39943cc8e64d6c2c472a89fff9a317054"


def ckb_hash(data: bytes) -> str:
    return "0x" + blake2b(data, digest_size=32, person=CKB_HASH_PERSON).hexdigest()


def canonical_json(value) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode()


def read_json(path: Path):
    return json.loads(path.read_text())


def write_json(path: Path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def parse_cell_data(data_hex: str):
    data = bytes.fromhex(data_hex.removeprefix("0x"))
    return json.loads(data)


def cell_data_hex(commitment) -> str:
    data = canonical_json(commitment)
    return "0x" + data.hex()


def governance_type_script(args_hex: str):
    return {
        "code_hash": GOVERNANCE_TYPE_CODE_HASH,
        "hash_type": GOVERNANCE_TYPE_HASH_TYPE,
        "args": args_hex,
    }


def proposal_type_args_prefix() -> str:
    return "0x" + PROPOSAL_TYPE_ARGS_NAMESPACE


def proposal_type_args(proposal_id: str) -> str:
    return proposal_type_args_prefix() + proposal_id.removeprefix("0x")


def proposal_type_script(proposal_id: str):
    return governance_type_script(proposal_type_args(proposal_id))


def vote_type_args_prefix(proposal_id: str) -> str:
    return "0x" + VOTE_TYPE_ARGS_NAMESPACE + proposal_id.removeprefix("0x")


def deposit_key_hash(deposit_out_point_key: str) -> str:
    return ckb_hash(deposit_out_point_key.encode())


def vote_type_args(proposal_id: str, deposit_out_point_key: str) -> str:
    return vote_type_args_prefix(proposal_id) + deposit_key_hash(deposit_out_point_key).removeprefix("0x")


def vote_type_script(proposal_id: str, deposit_out_point_key: str):
    return governance_type_script(vote_type_args(proposal_id, deposit_out_point_key))


def script_equal(left, right) -> bool:
    if left is None or right is None:
        return False
    return (
        left.get("code_hash") == right.get("code_hash")
        and left.get("hash_type") == right.get("hash_type")
        and left.get("args") == right.get("args")
    )


def script_has_args_prefix(script, args_prefix: str) -> bool:
    if script is None:
        return False
    return (
        script.get("code_hash") == GOVERNANCE_TYPE_CODE_HASH
        and script.get("hash_type") == GOVERNANCE_TYPE_HASH_TYPE
        and script.get("args", "").startswith(args_prefix)
    )


def default_manifest(snapshot, args):
    choices = [
        {"id": "yes", "label": "Yes"},
        {"id": "no", "label": "No"},
        {"id": "abstain", "label": "Abstain"},
    ]
    snapshot_block = int(snapshot["snapshot_block"]["number"])
    vote_start = args.vote_start_block if args.vote_start_block is not None else snapshot_block + 20
    vote_end = args.vote_end_block if args.vote_end_block is not None else snapshot_block + 80
    return {
        "magic": PROPOSAL_MAGIC,
        "version": 1,
        "title": args.title,
        "summary": args.summary,
        "body": args.body,
        "discussion_uri": args.discussion_uri,
        "snapshot": {
            "snapshot_id": snapshot["snapshot_id"],
            "snapshot_root": snapshot["roots"]["snapshot_root"],
            "snapshot_block_number": snapshot_block,
            "snapshot_block_hash": snapshot["snapshot_block"]["hash"],
            "record_map_root": snapshot["roots"]["record_map_root"],
            "owner_index_root": snapshot["roots"]["owner_index_root"],
            "snapshot_schema": snapshot["magic"],
        },
        "vote_window": {
            "start_block": vote_start,
            "end_block": vote_end,
        },
        "choices": choices,
        "created_by": {
            "lock": {
                "code_hash": SIGHASH_TYPE_HASH,
                "hash_type": "type",
                "args": args.proposer_lock_arg,
            }
        },
    }


def manifest_hash(manifest) -> str:
    return ckb_hash(canonical_json(manifest))


def proposal_id_from_manifest_hash(value: str) -> str:
    return ckb_hash(canonical_json({"magic": "CKB_GOV_PROPOSAL_ID_V1", "manifest_hash": value}))


def commitment_for_manifest(proposal_id: str, manifest_hash_value: str, manifest_uri: str, manifest):
    choices = [choice["id"] for choice in manifest["choices"]]
    snapshot = manifest["snapshot"]
    vote_window = manifest["vote_window"]
    return {
        "magic": PROPOSAL_COMMITMENT_MAGIC,
        "version": 1,
        "proposal_id": proposal_id,
        "manifest_hash": manifest_hash_value,
        "manifest_uri": manifest_uri,
        "snapshot_id": snapshot["snapshot_id"],
        "snapshot_root": snapshot["snapshot_root"],
        "record_map_root": snapshot["record_map_root"],
        "owner_index_root": snapshot["owner_index_root"],
        "snapshot_block_number": snapshot["snapshot_block_number"],
        "snapshot_block_hash": snapshot["snapshot_block_hash"],
        "choices": choices,
        "choices_hash": ckb_hash(canonical_json(choices)),
        "vote_start_block": vote_window["start_block"],
        "vote_end_block": vote_window["end_block"],
    }


def create_sample(args):
    snapshot = read_json(args.snapshot)
    manifest = default_manifest(snapshot, args)
    m_hash = manifest_hash(manifest)
    proposal_id = proposal_id_from_manifest_hash(m_hash)
    short_id = proposal_id[2:14]

    output_dir = args.output_dir
    output_dir.mkdir(parents=True, exist_ok=True)
    manifest_path = output_dir / f"proposal-{short_id}.json"
    cell_data_path = output_dir / f"proposal-{short_id}.cell-data.bin"
    manifest_uri = args.manifest_uri or str(manifest_path)

    commitment = commitment_for_manifest(proposal_id, m_hash, manifest_uri, manifest)
    envelope = {
        "magic": PROPOSAL_MAGIC,
        "version": 1,
        "proposal_id": proposal_id,
        "manifest_hash": m_hash,
        "proposal_type_script": proposal_type_script(proposal_id),
        "commitment": commitment,
        "manifest": manifest,
    }
    write_json(manifest_path, envelope)
    cell_data_path.write_bytes(canonical_json(commitment))

    summary = {
        "proposal_id": proposal_id,
        "short_id": short_id,
        "manifest_hash": m_hash,
        "manifest_path": str(manifest_path),
        "cell_data_path": str(cell_data_path),
        "cell_data_hex": cell_data_hex(commitment),
        "snapshot_id": snapshot["snapshot_id"],
        "snapshot_root": snapshot["roots"]["snapshot_root"],
        "proposal_type_script": proposal_type_script(proposal_id),
        "vote_start_block": manifest["vote_window"]["start_block"],
        "vote_end_block": manifest["vote_window"]["end_block"],
    }
    print(json.dumps(summary, indent=2, sort_keys=True))


def verify_manifest(args):
    envelope = read_json(args.manifest)
    manifest = envelope["manifest"]
    m_hash = manifest_hash(manifest)
    proposal_id = proposal_id_from_manifest_hash(m_hash)
    errors = []
    if envelope["manifest_hash"] != m_hash:
        errors.append("manifest_hash mismatch")
    if envelope["proposal_id"] != proposal_id:
        errors.append("proposal_id mismatch")
    expected_commitment = commitment_for_manifest(
        proposal_id,
        m_hash,
        envelope["commitment"]["manifest_uri"],
        manifest,
    )
    if envelope["commitment"] != expected_commitment:
        errors.append("commitment mismatch")
    if errors:
        raise RuntimeError("; ".join(errors))
    print(f"verified_manifest: {args.manifest}")
    print(f"proposal_id: {proposal_id}")
    print(f"manifest_hash: {m_hash}")


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


def discover(args):
    rpc = RpcClient(args.rpc)
    search_key = {
        "script": governance_type_script(proposal_type_args_prefix()),
        "script_type": "type",
        "script_search_mode": "prefix",
        "with_data": True,
    }

    proposals = []
    cursor = None
    while True:
        page = rpc.get_cells(search_key, limit=args.limit, after=cursor)
        objects = page["objects"]
        for cell in objects:
            commitment = parse_cell_data(cell["output_data"])
            expected_type_script = proposal_type_script(commitment["proposal_id"])
            item = {
                "proposal_id": commitment["proposal_id"],
                "manifest_hash": commitment["manifest_hash"],
                "manifest_uri": commitment["manifest_uri"],
                "snapshot_id": commitment["snapshot_id"],
                "vote_start_block": commitment["vote_start_block"],
                "vote_end_block": commitment["vote_end_block"],
                "expected_type_script": expected_type_script,
                "type_script": cell["output"].get("type"),
                "type_script_verified": script_equal(cell["output"].get("type"), expected_type_script),
                "out_point": cell["out_point"],
                "block_number": int(cell["block_number"], 16),
                "tx_index": int(cell["tx_index"], 16),
            }
            manifest_path = Path(commitment["manifest_uri"])
            if manifest_path.exists():
                envelope = read_json(manifest_path)
                item["manifest_found"] = True
                item["manifest_hash_verified"] = envelope.get("manifest_hash") == commitment["manifest_hash"]
                item["title"] = envelope["manifest"].get("title")
                item["summary"] = envelope["manifest"].get("summary")
            else:
                item["manifest_found"] = False
                item["manifest_hash_verified"] = False
            proposals.append(item)

        if len(objects) < args.limit:
            break
        cursor = page["last_cursor"]

    print(json.dumps({"proposals": proposals}, indent=2, sort_keys=True))


def record_chain(args):
    envelope = read_json(args.manifest)
    chain = {
        "tx_hash": args.tx_hash,
        "submitted_at_unix": int(time.time()),
    }
    envelope["chain"] = chain
    write_json(args.manifest, envelope)
    print(f"recorded_chain_tx: {args.tx_hash}")
    print(f"manifest: {args.manifest}")


def main():
    parser = argparse.ArgumentParser(description="Create, verify, and discover DAO treasury proposal MVP artifacts.")
    subparsers = parser.add_subparsers(dest="command", required=True)

    create = subparsers.add_parser("create-sample")
    create.add_argument("--snapshot", type=Path, required=True)
    create.add_argument("--output-dir", type=Path, required=True)
    create.add_argument("--manifest-uri")
    create.add_argument("--proposer-lock-arg", default=DEFAULT_PROPOSER_LOCK_ARG)
    create.add_argument("--vote-start-block", type=int)
    create.add_argument("--vote-end-block", type=int)
    create.add_argument("--title", default="DAO Treasury Activation MVP")
    create.add_argument(
        "--summary",
        default="Local MVP proposal for testing DAO treasury governance voting over a shared DAO snapshot.",
    )
    create.add_argument(
        "--body",
        default=(
            "This local proposal is a development artifact. It references the shared DAO snapshot "
            "and is used to test proposal discovery, vote creation, and tally verification."
        ),
    )
    create.add_argument("--discussion-uri", default="https://talk.nervos.org/t/pre-rfc-discussion-activating-the-nervos-dao-treasury/10143")

    verify = subparsers.add_parser("verify-manifest")
    verify.add_argument("manifest", type=Path)

    discover_parser = subparsers.add_parser("discover")
    discover_parser.add_argument("--rpc", default="http://127.0.0.1:8114")
    discover_parser.add_argument("--limit", type=int, default=100)

    record = subparsers.add_parser("record-chain")
    record.add_argument("--manifest", type=Path, required=True)
    record.add_argument("--tx-hash", required=True)

    args = parser.parse_args()
    if args.command == "create-sample":
        create_sample(args)
    elif args.command == "verify-manifest":
        verify_manifest(args)
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
