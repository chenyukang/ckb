#!/usr/bin/env python3
import argparse
import importlib.util
import json
from pathlib import Path


TRANSCRIPT_MAGIC = "CKB_DAO_TREASURY_ZKVM_SETTLEMENT_TRANSCRIPT_V1"


def load_vote_lib():
    path = Path(__file__).with_name("vote.py")
    spec = importlib.util.spec_from_file_location("vote_lib", path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


vote_lib = load_vote_lib()


def read_json(path: Path):
    return json.loads(path.read_text())


def write_json(path: Path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def source_path_for_snapshot(snapshot_path: Path) -> Path:
    return snapshot_path.with_name(f"{snapshot_path.stem}.source.json")


def vote_artifacts_by_id(artifacts_dir: Path):
    votes = {}
    for path in sorted(artifacts_dir.glob("vote-*.json")):
        vote = read_json(path)
        vote_id = vote.get("vote_id")
        if vote_id:
            votes[vote_id] = {
                "path": str(path),
                "artifact": vote,
            }
    return votes


def cell_data_hex(commitment) -> str:
    return "0x" + vote_lib.canonical_json(commitment).hex()


def outpoint_key(out_point) -> str:
    tx_hash = out_point["tx_hash"]
    index = out_point["index"]
    if isinstance(index, str):
        index_value = int(index, 16) if index.startswith("0x") else int(index)
    else:
        index_value = int(index)
    return f"{tx_hash}:0x{index_value:x}"


def load_transaction_witness(rpc, vote_artifact):
    tx_hash = vote_artifact["chain"]["tx_hash"]
    tx = rpc.get_transaction(tx_hash)
    if tx is None or tx.get("transaction") is None:
        raise RuntimeError(f"cannot load vote tx from rpc: {tx_hash}")
    tx_status = tx.get("tx_status")
    if tx_status is None or tx_status.get("block_number") is None:
        raise RuntimeError(f"vote tx has no committed block status: {tx_hash}")
    containing_block = rpc.get_block_by_number(int(tx_status["block_number"], 16))
    if containing_block is None:
        raise RuntimeError(f"cannot load containing block for vote tx: {tx_hash}")

    owner_input_cells = []
    referenced_block_numbers = [int(tx_status["block_number"], 16)]
    for input_index, tx_input in enumerate(tx["transaction"]["inputs"]):
        previous_output = tx_input["previous_output"]
        if previous_output["tx_hash"] == "0x" + "0" * 64:
            continue
        previous_tx = rpc.get_transaction(previous_output["tx_hash"])
        if previous_tx is None or previous_tx.get("transaction") is None:
            raise RuntimeError(f"cannot load previous tx from rpc: {previous_output['tx_hash']}")
        previous_tx_status = previous_tx.get("tx_status")
        if previous_tx_status is None or previous_tx_status.get("block_number") is None:
            raise RuntimeError(f"previous tx has no committed block status: {previous_output['tx_hash']}")
        previous_block_number = int(previous_tx_status["block_number"], 16)
        previous_containing_block = rpc.get_block_by_number(previous_block_number)
        if previous_containing_block is None:
            raise RuntimeError(f"cannot load containing block for previous tx: {previous_output['tx_hash']}")
        referenced_block_numbers.append(previous_block_number)
        previous_index = int(previous_output["index"], 16)
        owner_input_cells.append(
            {
                "input_index": input_index,
                "previous_output": previous_output,
                "previous_transaction": previous_tx,
                "previous_containing_block": previous_containing_block,
                "previous_cell": {
                    "out_point_key": outpoint_key(previous_output),
                    "output": previous_tx["transaction"]["outputs"][previous_index],
                    "output_data": previous_tx["transaction"]["outputs_data"][previous_index],
                },
            }
        )

    return {
        "tx_hash": tx_hash,
        "transaction": tx,
        "containing_block": containing_block,
        "owner_input_cells": owner_input_cells,
        "referenced_block_numbers": referenced_block_numbers,
    }


def create_transcript(args):
    rpc = vote_lib.RpcClient(args.rpc)
    proposal = read_json(args.proposal)
    snapshot = read_json(args.snapshot)
    snapshot_source = read_json(args.snapshot_source or source_path_for_snapshot(args.snapshot))
    tally = read_json(args.tally)
    artifacts_dir = args.artifacts_dir or args.proposal.parent
    votes_by_id = vote_artifacts_by_id(artifacts_dir)

    vote_witnesses = []
    referenced_block_numbers = {snapshot["snapshot_block"]["number"]}
    for compact in tally.get("valid_votes", []):
        vote_id = compact["vote_id"]
        vote = votes_by_id.get(vote_id)
        if vote is None:
            raise RuntimeError(f"vote artifact not found for valid vote {vote_id}")
        tx_witness = load_transaction_witness(rpc, vote["artifact"])
        vote_witnesses.append(
            {
                "vote_artifact_path": vote["path"],
                "vote": vote["artifact"],
                "block_number": compact["block_number"],
                "tx_index": compact["tx_index"],
                "output_index": compact["output_index"],
                "out_point": compact["out_point"],
                "expected_cell_data": cell_data_hex(vote["artifact"]["commitment"]),
                "tx_hash": tx_witness["tx_hash"],
                "transaction": tx_witness["transaction"],
                "containing_block": tx_witness["containing_block"],
                "owner_input_cells": tx_witness["owner_input_cells"],
            }
        )
        referenced_block_numbers.update(tx_witness["referenced_block_numbers"])

    header_chain_numbers = list(range(min(referenced_block_numbers), max(referenced_block_numbers) + 1))
    header_chain_witness = [rpc.get_block_by_number(number)["header"] for number in header_chain_numbers]

    vote_window = proposal["manifest"]["vote_window"]
    public_inputs = {
        "magic": "CKB_DAO_TREASURY_ZKVM_SETTLEMENT_PUBLIC_INPUT_V1",
        "version": 1,
        "proof_model": "transcript_consistency_with_anchored_ckb_block_commitments",
        "chain_inclusion_verified": False,
        "proposal_id": proposal["proposal_id"],
        "snapshot_id": snapshot["snapshot_id"],
        "snapshot_root": snapshot["roots"]["snapshot_root"],
        "record_map_root": snapshot["roots"]["record_map_root"],
        "owner_index_root": snapshot["roots"]["owner_index_root"],
        "snapshot_block_number": snapshot["snapshot_block"]["number"],
        "snapshot_block_hash": snapshot["snapshot_block"]["hash"],
        "vote_start_block": vote_window["start_block"],
        "vote_end_block": vote_window["end_block"],
        "anchor_start_block_number": header_chain_numbers[0],
        "anchor_start_block_hash": header_chain_witness[0]["hash"],
        "anchor_end_block_number": header_chain_numbers[-1],
        "anchor_end_block_hash": header_chain_witness[-1]["hash"],
        "tally_root": tally["tally_root"],
        "choice_weights_shannons": tally["choice_weights_shannons"],
    }

    transcript = {
        "magic": TRANSCRIPT_MAGIC,
        "version": 1,
        "public_inputs": public_inputs,
        "proposal": proposal,
        "snapshot": snapshot,
        "snapshot_block_witness": rpc.get_block_by_number(snapshot["snapshot_block"]["number"]),
        "header_chain_witness": header_chain_witness,
        "snapshot_source": snapshot_source,
        "votes": vote_witnesses,
        "notes": [
            "This transcript PoC verifies snapshot/tally commitment consistency.",
            "It recomputes CKB-native header and block commitments for witnessed blocks.",
            "It does not yet prove canonical CKB chain inclusion for snapshot records or vote cells.",
        ],
    }

    output = args.output
    if output is None:
        short_id = proposal["proposal_id"][2:14]
        output = artifacts_dir / f"zkvm-settlement-transcript-{short_id}.json"
    write_json(output, transcript)
    print(
        json.dumps(
            {
                "transcript_path": str(output),
                "proposal_id": proposal["proposal_id"],
                "snapshot_id": snapshot["snapshot_id"],
                "snapshot_root": snapshot["roots"]["snapshot_root"],
                "tally_root": tally["tally_root"],
                "vote_witness_count": len(vote_witnesses),
                "anchor_start_block_number": header_chain_numbers[0],
                "anchor_end_block_number": header_chain_numbers[-1],
                "chain_inclusion_verified": False,
            },
            indent=2,
            sort_keys=True,
        )
    )


def main():
    parser = argparse.ArgumentParser(description="Create a zkVM settlement transcript PoC witness.")
    parser.add_argument("--rpc", default="http://127.0.0.1:8114")
    parser.add_argument("--proposal", type=Path, required=True)
    parser.add_argument("--snapshot", type=Path, required=True)
    parser.add_argument("--snapshot-source", type=Path)
    parser.add_argument("--tally", type=Path, required=True)
    parser.add_argument("--artifacts-dir", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    create_transcript(args)


if __name__ == "__main__":
    main()
