#!/usr/bin/env python3
import argparse
import json
import os
import urllib.request
from hashlib import blake2b
from pathlib import Path


CKB_HASH_PERSON = b"ckb-default-hash"
PROPOSAL_MAGIC = "CKB_FULLSCAN_ZKVM_PROPOSAL_V1"
VOTE_MAGIC = "CKB_FULLSCAN_ZKVM_VOTE_V1"
REPORT_MAGIC = "CKB_FULLSCAN_ZKVM_TALLY_REPORT_V1"
TRANSCRIPT_MAGIC = "CKB_FULLSCAN_ZKVM_TRANSCRIPT_V1"
DEFAULT_GOVERNANCE_TYPE_CODE_HASH = "0x28e83a1277d48add8e72fadaa9248559e1b632bab2bd60b27955ebc4c03800a5"
GOVERNANCE_TYPE_CODE_HASH = os.environ.get(
    "DAO_TREASURY_GOV_TYPE_CODE_HASH", DEFAULT_GOVERNANCE_TYPE_CODE_HASH
)
GOVERNANCE_TYPE_HASH_TYPE = os.environ.get("DAO_TREASURY_GOV_TYPE_HASH_TYPE", "data")
DAO_TYPE_HASH = os.environ.get(
    "DAO_TREASURY_DAO_TYPE_HASH",
    "0x82d76d1b75fe2fd9a27dfbaa65a039221a380d76c926f378d3f81cf3e7e13f2e",
)
PROPOSAL_NAMESPACE = "10"
VOTE_NAMESPACE = "11"
DEPOSIT_PHASE_DATA = "0x0000000000000000"


def ckb_hash(data: bytes) -> str:
    return "0x" + blake2b(data, digest_size=32, person=CKB_HASH_PERSON).hexdigest()


def canonical_json(value) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode()


def read_json(path: Path):
    return json.loads(path.read_text())


def write_json(path: Path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def cell_data_hex(value) -> str:
    return "0x" + canonical_json(value).hex()


def parse_cell_data(data_hex: str):
    if data_hex in ("0x", ""):
        return None
    return json.loads(bytes.fromhex(data_hex.removeprefix("0x")))


def shannons_to_ckb_string(shannons: int) -> str:
    whole, frac = divmod(shannons, 100_000_000)
    if frac == 0:
        return str(whole)
    return f"{whole}.{frac:08d}".rstrip("0")


def normalize_hex(value: str, bytes_len: int) -> str:
    raw = value.removeprefix("0x")
    if len(raw) != bytes_len * 2:
        raise ValueError(f"expected {bytes_len} bytes, got {len(raw) // 2}: {value}")
    return "0x" + raw.lower()


def governance_type_script(args_hex: str):
    return {
        "code_hash": GOVERNANCE_TYPE_CODE_HASH,
        "hash_type": GOVERNANCE_TYPE_HASH_TYPE,
        "args": args_hex,
    }


def proposal_type_args(type_id_blake160: str, owner_lock_blake160: str, sp1_vk_hash: str) -> str:
    return (
        "0x"
        + PROPOSAL_NAMESPACE
        + normalize_hex(type_id_blake160, 20).removeprefix("0x")
        + normalize_hex(owner_lock_blake160, 20).removeprefix("0x")
        + normalize_hex(sp1_vk_hash, 32).removeprefix("0x")
    )


def proposal_type_id_from_args(args_hex: str) -> str:
    raw = args_hex.removeprefix("0x")
    if not raw.startswith(PROPOSAL_NAMESPACE) or len(raw) < 2 + 40:
        raise ValueError("invalid fullscan proposal type args")
    return "0x" + raw[2 : 2 + 40]


def vote_type_args(proposal_type_id: str) -> str:
    return "0x" + VOTE_NAMESPACE + normalize_hex(proposal_type_id, 20).removeprefix("0x")


def script_equal(left, right) -> bool:
    return (
        left is not None
        and right is not None
        and left.get("code_hash") == right.get("code_hash")
        and left.get("hash_type") == right.get("hash_type")
        and left.get("args") == right.get("args")
    )


def script_key(script) -> str:
    return ckb_hash(canonical_json(script))


def outpoint_key(out_point) -> str:
    index = out_point["index"]
    index_value = int(index, 16) if isinstance(index, str) and index.startswith("0x") else int(index)
    return f"{out_point['tx_hash']}:0x{index_value:x}"


class RpcClient:
    def __init__(self, url: str):
        self.url = url
        self.next_id = 1

    def call(self, method: str, params=None):
        payload = {
            "id": self.next_id,
            "jsonrpc": "2.0",
            "method": method,
            "params": params or [],
        }
        self.next_id += 1
        req = urllib.request.Request(
            self.url,
            data=json.dumps(payload).encode(),
            headers={"content-type": "application/json"},
        )
        with urllib.request.urlopen(req) as resp:
            result = json.loads(resp.read())
        if "error" in result:
            raise RuntimeError(result["error"])
        return result["result"]

    def get_block_by_number(self, number: int):
        return self.call("get_block_by_number", [hex(number)])

    def get_transaction(self, tx_hash: str):
        return self.call("get_transaction", [tx_hash])


def previous_cell(rpc: RpcClient, out_point):
    tx = rpc.get_transaction(out_point["tx_hash"])
    if tx is None or tx.get("transaction") is None:
        raise ValueError(f"missing previous tx {out_point['tx_hash']}")
    index = int(out_point["index"], 16) if isinstance(out_point["index"], str) else int(out_point["index"])
    outputs = tx["transaction"]["outputs"]
    outputs_data = tx["transaction"]["outputs_data"]
    if index >= len(outputs):
        raise ValueError(f"previous output index out of range: {outpoint_key(out_point)}")
    return {
        "out_point": out_point,
        "output": outputs[index],
        "output_data": outputs_data[index],
        "transaction": tx,
    }


def compact_cell_evidence(cell):
    return {
        "out_point": cell["out_point"],
        "output": cell["output"],
        "output_data": cell["output_data"],
    }


def is_dao_deposit_cell(cell) -> bool:
    type_script = cell["output"].get("type")
    return (
        type_script is not None
        and type_script.get("code_hash") == DAO_TYPE_HASH
        and cell.get("output_data") == DEPOSIT_PHASE_DATA
    )


def tx_input_locks(rpc: RpcClient, tx):
    locks = []
    for tx_input in tx["inputs"]:
        previous_output = tx_input["previous_output"]
        if previous_output["tx_hash"] == "0x" + "0" * 64:
            continue
        cell = previous_cell(rpc, previous_output)
        locks.append(cell["output"]["lock"])
    return locks


def tx_dep_cells(rpc: RpcClient, tx):
    cells = []
    for dep in tx.get("cell_deps", []):
        # This PoC treats dep_group as not directly inspectable DAO evidence.
        if dep.get("dep_type") != "code":
            continue
        try:
            cells.append(previous_cell(rpc, dep["out_point"]))
        except Exception as err:
            cells.append({"error": str(err), "out_point": dep["out_point"]})
    return cells


def validate_article_vote(rpc: RpcClient, proposal, tx, output_index: int, output, output_data: str):
    errors = []
    commitment = None
    try:
        commitment = parse_cell_data(output_data)
    except Exception as err:
        errors.append(f"invalid vote data: {err}")
    if not isinstance(commitment, dict):
        errors.append("vote data is not JSON object")
        commitment = {}
    if commitment.get("magic") != VOTE_MAGIC:
        errors.append("unexpected vote magic")
    choice = commitment.get("choice")
    choices = proposal["choices"]
    if choice not in choices:
        errors.append(f"invalid choice: {choice}")

    input_locks = tx_input_locks(rpc, tx)
    dep_cells = tx_dep_cells(rpc, tx)
    matching_deposits = []
    for dep_cell in dep_cells:
        if dep_cell.get("error"):
            continue
        if not is_dao_deposit_cell(dep_cell):
            continue
        dep_lock = dep_cell["output"]["lock"]
        if any(script_equal(dep_lock, input_lock) for input_lock in input_locks):
            matching_deposits.append(dep_cell)
    if not matching_deposits:
        errors.append("no DAO deposit cell_dep with a lock matching an input lock")

    # The article uses the DAO deposit amount as the voting weight. If multiple matching deposits
    # are included, this PoC uses the largest one to avoid accidental double counting in one vote.
    weight = 0
    voter_key = None
    if matching_deposits:
        deposit = max(matching_deposits, key=lambda cell: int(cell["output"]["capacity"], 16))
        weight = int(deposit["output"]["capacity"], 16)
        voter_key = script_key(deposit["output"]["lock"])

    vote = {
        "valid": not errors,
        "errors": errors,
        "choice": choice,
        "weight_shannons": str(weight),
        "weight_ckb": shannons_to_ckb_string(weight),
        "voter_key": voter_key,
        "tx_hash": tx["hash"],
        "output_index": output_index,
        "out_point": {"tx_hash": tx["hash"], "index": hex(output_index)},
        "vote_data": commitment,
    }
    return vote


def create_proposal(args):
    type_id_seed = args.type_id_seed or ckb_hash(args.title.encode())[:42]
    type_id_blake160 = "0x" + ckb_hash(type_id_seed.encode()).removeprefix("0x")[:40]
    type_args = proposal_type_args(type_id_blake160, args.owner_lock_blake160, args.sp1_vk_hash)
    proposal_type_script = governance_type_script(type_args)
    proposal_type_id = proposal_type_id_from_args(type_args)
    vote_type_script = governance_type_script(vote_type_args(proposal_type_id))
    proposal_data = {
        "magic": PROPOSAL_MAGIC,
        "version": 1,
        "title": args.title,
        "description": args.description,
        "duration": args.duration,
        "expired_time": args.expired_time,
        "choices": ["yes", "no"],
        "vote_type_script": vote_type_script,
        "receiver": args.receiver,
        "amount_shannons": args.amount_shannons,
        "minimal_requirement_shannons": args.minimal_requirement_shannons,
    }
    artifact = {
        "magic": PROPOSAL_MAGIC,
        "version": 1,
        "proposal_type_id": proposal_type_id,
        "proposal_type_script": proposal_type_script,
        "vote_type_script": vote_type_script,
        "proposal_data": proposal_data,
    }
    output = args.output
    write_json(output, artifact)
    data_path = output.with_suffix(".cell-data.bin")
    data_path.write_bytes(canonical_json(proposal_data))
    print(json.dumps({"proposal": str(output), "cell_data": str(data_path)}, indent=2))


def create_vote(args):
    proposal = read_json(args.proposal)
    choice = args.choice
    if choice not in proposal["proposal_data"]["choices"]:
        raise ValueError(f"invalid choice {choice}")
    vote_data = {
        "magic": VOTE_MAGIC,
        "version": 1,
        "choice": choice,
        "proposal_type_id": proposal["proposal_type_id"],
    }
    output = args.output
    write_json(output, {"magic": VOTE_MAGIC, "version": 1, "vote_data": vote_data, "vote_type_script": proposal["vote_type_script"]})
    data_path = output.with_suffix(".cell-data.bin")
    data_path.write_bytes(canonical_json(vote_data))
    print(json.dumps({"vote": str(output), "cell_data": str(data_path)}, indent=2))


def collect_vote_tx_evidence(rpc: RpcClient, tx):
    evidence = {}
    for tx_input in tx["inputs"]:
        previous_output = tx_input["previous_output"]
        if previous_output["tx_hash"] == "0x" + "0" * 64:
            continue
        key = outpoint_key(previous_output)
        if key not in evidence:
            evidence[key] = compact_cell_evidence(previous_cell(rpc, previous_output))
    for dep in tx.get("cell_deps", []):
        if dep.get("dep_type") != "code":
            continue
        out_point = dep["out_point"]
        key = outpoint_key(out_point)
        if key not in evidence:
            evidence[key] = compact_cell_evidence(previous_cell(rpc, out_point))
    return evidence


def build_transcript(args):
    rpc = RpcClient(args.rpc)
    proposal_artifact = read_json(args.proposal)
    proposal = proposal_artifact["proposal_data"]
    vote_type_script = proposal_artifact["vote_type_script"]
    start = args.start_block
    end = args.end_block if args.end_block is not None else start + proposal["duration"]
    if end < start:
        raise ValueError("end block is before start block")

    blocks = []
    cell_evidence = {}
    previous_hash = None
    for block_number in range(start, end + 1):
        block = rpc.get_block_by_number(block_number)
        if block is None:
            raise RuntimeError(f"missing block {block_number}")
        header = block["header"]
        if previous_hash is not None and header["parent_hash"] != previous_hash:
            raise RuntimeError(f"block parent hash mismatch at {block_number}")
        previous_hash = header["hash"]
        blocks.append(block)
        for tx in block["transactions"]:
            has_vote_output = any(output.get("type") == vote_type_script for output in tx["outputs"])
            if not has_vote_output:
                continue
            cell_evidence.update(collect_vote_tx_evidence(rpc, tx))

    transcript = {
        "magic": TRANSCRIPT_MAGIC,
        "version": 1,
        "proposal": proposal_artifact,
        "dao_type_hash": DAO_TYPE_HASH,
        "start_block": start,
        "end_block": end,
        "blocks": blocks,
        "cell_evidence": cell_evidence,
        "limitations": [
            "The SP1 guest scans every transaction output in this block window.",
            "The script-side runner adds compact transaction-root commitments before executing the guest.",
            "Raw Molecule transaction parsing inside the guest is not implemented yet.",
        ],
    }
    write_json(args.output, transcript)
    print(
        json.dumps(
            {
                "transcript": str(args.output),
                "blocks": len(blocks),
                "cell_evidence_count": len(cell_evidence),
            },
            indent=2,
            sort_keys=True,
        )
    )


def scan(args):
    rpc = RpcClient(args.rpc)
    proposal_artifact = read_json(args.proposal)
    proposal = proposal_artifact["proposal_data"]
    vote_type_script = proposal_artifact["vote_type_script"]
    start = args.start_block
    end = args.end_block if args.end_block is not None else start + proposal["duration"]
    if end < start:
        raise ValueError("end block is before start block")

    blocks_scanned = 0
    transactions_scanned = 0
    outputs_scanned = 0
    vote_outputs_seen = 0
    valid_votes = []
    invalid_votes = []
    previous_hash = None
    start_hash = None
    end_hash = None

    for block_number in range(start, end + 1):
        block = rpc.get_block_by_number(block_number)
        if block is None:
            raise RuntimeError(f"missing block {block_number}")
        header = block["header"]
        if block_number == start:
            start_hash = header["hash"]
        if previous_hash is not None and header["parent_hash"] != previous_hash:
            raise RuntimeError(f"block parent hash mismatch at {block_number}")
        previous_hash = header["hash"]
        end_hash = header["hash"]
        blocks_scanned += 1

        for tx_index, tx in enumerate(block["transactions"]):
            transactions_scanned += 1
            for output_index, output in enumerate(tx["outputs"]):
                outputs_scanned += 1
                if not script_equal(output.get("type"), vote_type_script):
                    continue
                vote_outputs_seen += 1
                vote = validate_article_vote(
                    rpc,
                    proposal,
                    tx,
                    output_index,
                    output,
                    tx["outputs_data"][output_index],
                )
                vote["block_number"] = block_number
                vote["tx_index"] = tx_index
                if vote["valid"]:
                    valid_votes.append(vote)
                else:
                    invalid_votes.append(vote)

    latest_by_voter = {}
    superseded_votes = []
    for vote in sorted(valid_votes, key=lambda item: (item["block_number"], item["tx_index"], item["output_index"])):
        key = vote["voter_key"]
        if key in latest_by_voter:
            superseded_votes.append(latest_by_voter[key])
        latest_by_voter[key] = vote
    counted_votes = sorted(latest_by_voter.values(), key=lambda item: (item["block_number"], item["tx_index"], item["output_index"]))
    weights = {choice: 0 for choice in proposal["choices"]}
    for vote in counted_votes:
        weights[vote["choice"]] += int(vote["weight_shannons"])
    participation = sum(weights.values())
    passed = weights.get("yes", 0) > weights.get("no", 0) and participation >= int(
        proposal["minimal_requirement_shannons"]
    )
    commitment = {
        "magic": REPORT_MAGIC,
        "version": 1,
        "proposal_type_id": proposal_artifact["proposal_type_id"],
        "start_block": start,
        "start_block_hash": start_hash,
        "end_block": end,
        "end_block_hash": end_hash,
        "blocks_scanned": blocks_scanned,
        "transactions_scanned": transactions_scanned,
        "outputs_scanned": outputs_scanned,
        "vote_outputs_seen": vote_outputs_seen,
        "valid_vote_count": len(valid_votes),
        "counted_vote_count": len(counted_votes),
        "superseded_vote_count": len(superseded_votes),
        "invalid_vote_count": len(invalid_votes),
        "choice_weights_shannons": {key: str(value) for key, value in weights.items()},
        "minimal_requirement_shannons": proposal["minimal_requirement_shannons"],
        "passed": passed,
    }
    report = {
        "magic": REPORT_MAGIC,
        "version": 1,
        "commitment": commitment,
        "report_root": ckb_hash(canonical_json(commitment)),
        "counted_votes": counted_votes,
        "superseded_votes": superseded_votes,
        "invalid_votes": invalid_votes,
    }
    if args.output:
        write_json(args.output, report)
    print(json.dumps(report, indent=2, sort_keys=True))


def main():
    parser = argparse.ArgumentParser(description="Article-style full-scan zkVM voting PoC utilities.")
    sub = parser.add_subparsers(dest="cmd", required=True)

    create_proposal_parser = sub.add_parser("create-proposal")
    create_proposal_parser.add_argument("--title", default="Full-scan zkVM proposal")
    create_proposal_parser.add_argument("--description", default="")
    create_proposal_parser.add_argument("--duration", type=int, default=80)
    create_proposal_parser.add_argument("--expired-time", type=int, default=0)
    create_proposal_parser.add_argument("--receiver", default="0x")
    create_proposal_parser.add_argument("--amount-shannons", default="0")
    create_proposal_parser.add_argument("--minimal-requirement-shannons", default="0")
    create_proposal_parser.add_argument("--owner-lock-blake160", default="0x" + "11" * 20)
    create_proposal_parser.add_argument("--sp1-vk-hash", default="0x" + "22" * 32)
    create_proposal_parser.add_argument("--type-id-seed")
    create_proposal_parser.add_argument("--output", type=Path, required=True)
    create_proposal_parser.set_defaults(func=create_proposal)

    create_vote_parser = sub.add_parser("create-vote")
    create_vote_parser.add_argument("--proposal", type=Path, required=True)
    create_vote_parser.add_argument("--choice", choices=["yes", "no"], required=True)
    create_vote_parser.add_argument("--output", type=Path, required=True)
    create_vote_parser.set_defaults(func=create_vote)

    scan_parser = sub.add_parser("scan")
    scan_parser.add_argument("--rpc", default="http://127.0.0.1:8114")
    scan_parser.add_argument("--proposal", type=Path, required=True)
    scan_parser.add_argument("--start-block", type=int, required=True)
    scan_parser.add_argument("--end-block", type=int)
    scan_parser.add_argument("--output", type=Path)
    scan_parser.set_defaults(func=scan)

    transcript_parser = sub.add_parser("build-transcript")
    transcript_parser.add_argument("--rpc", default="http://127.0.0.1:8114")
    transcript_parser.add_argument("--proposal", type=Path, required=True)
    transcript_parser.add_argument("--start-block", type=int, required=True)
    transcript_parser.add_argument("--end-block", type=int)
    transcript_parser.add_argument("--output", type=Path, required=True)
    transcript_parser.set_defaults(func=build_transcript)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
