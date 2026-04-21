#!/usr/bin/env python3
import argparse
import json
import sys
import time
from pathlib import Path

import vote as vote_lib


TALLY_MAGIC = "CKB_GOV_TALLY_V1"
EMPTY_ROOT_TAG = b"CKB_DAO_TREASURY_EMPTY_TALLY_V1"


def shannons_to_ckb_string(shannons: int) -> str:
    whole, frac = divmod(shannons, 100_000_000)
    if frac == 0:
        return str(whole)
    return f"{whole}.{frac:08d}".rstrip("0")


def write_json(path: Path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def merkle_root(records):
    if not records:
        return vote_lib.ckb_hash(EMPTY_ROOT_TAG)

    level = [bytes.fromhex(vote_lib.ckb_hash(vote_lib.canonical_json(record))[2:]) for record in records]
    while len(level) > 1:
        if len(level) % 2 == 1:
            level.append(level[-1])
        next_level = []
        for index in range(0, len(level), 2):
            parent = vote_lib.ckb_hash(level[index] + level[index + 1])
            next_level.append(bytes.fromhex(parent[2:]))
        level = next_level
    return "0x" + level[0].hex()


def block_by_number(rpc: vote_lib.RpcClient, block_number: int):
    return rpc.call("get_block_by_number", [hex(block_number)])


def vote_order(vote):
    return (
        int(vote.get("block_number", 0)),
        int(vote.get("tx_index", 0)),
        int(vote.get("output_index", 0)),
    )


def compact_vote(vote):
    return {
        "vote_id": vote.get("vote_id"),
        "deposit_out_point_key": vote.get("deposit_out_point_key"),
        "choice": vote.get("choice"),
        "weight_shannons": vote.get("weight_shannons"),
        "voter_lock_arg": vote.get("voter_lock_arg"),
        "out_point": vote.get("out_point"),
        "block_number": vote.get("block_number"),
        "tx_index": vote.get("tx_index"),
        "output_index": vote.get("output_index"),
    }


def compact_invalid_vote(vote):
    compact = compact_vote(vote)
    compact["errors"] = vote.get("errors", [])
    return compact


def scan_range(proposal, tip_block_number: int, scan_end_block):
    vote_window = proposal["manifest"]["vote_window"]
    start_block = vote_window["start_block"]
    final_end_block = vote_window["end_block"]

    if scan_end_block is None:
        end_block = min(final_end_block, tip_block_number)
    else:
        end_block = scan_end_block
        if end_block > final_end_block:
            raise ValueError(f"scan end block {end_block} is after vote window end {final_end_block}")
        if end_block > tip_block_number:
            raise ValueError(f"scan end block {end_block} is above chain tip {tip_block_number}")

    return {
        "start_block": start_block,
        "end_block": end_block,
        "vote_end_block": final_end_block,
        "tip_block_number": tip_block_number,
        "is_final": tip_block_number >= final_end_block and end_block == final_end_block,
    }


def discover_historical_votes(rpc, proposal, snapshot, scan):
    votes = []
    prefix_hex = "0x" + vote_lib.VOTE_CELL_PREFIX.hex()
    blocks_scanned = 0
    transactions_scanned = 0
    vote_outputs_seen = 0

    if scan["end_block"] < scan["start_block"]:
        return votes, {
            "blocks_scanned": 0,
            "transactions_scanned": 0,
            "vote_outputs_seen": 0,
        }

    for block_number in range(scan["start_block"], scan["end_block"] + 1):
        block = block_by_number(rpc, block_number)
        if block is None:
            raise RuntimeError(f"block {block_number} is missing")
        blocks_scanned += 1

        for tx_index, tx in enumerate(block["transactions"]):
            transactions_scanned += 1
            outputs_data = tx["outputs_data"]
            for output_index, output_data in enumerate(outputs_data):
                if not output_data.startswith(prefix_hex):
                    continue
                vote_outputs_seen += 1
                cell = {
                    "output": tx["outputs"][output_index],
                    "output_data": output_data,
                    "out_point": {
                        "tx_hash": tx["hash"],
                        "index": hex(output_index),
                    },
                    "block_number": hex(block_number),
                    "tx_index": hex(tx_index),
                }
                try:
                    result = vote_lib.verify_vote_cell(rpc, proposal, snapshot, cell)
                except Exception as err:
                    result = {
                        "valid": False,
                        "errors": [str(err)],
                        "out_point": cell["out_point"],
                        "block_number": block_number,
                        "tx_index": tx_index,
                        "output_index": output_index,
                    }
                votes.append(result)

    votes.sort(key=vote_order)
    return votes, {
        "blocks_scanned": blocks_scanned,
        "transactions_scanned": transactions_scanned,
        "vote_outputs_seen": vote_outputs_seen,
    }


def build_report(proposal, snapshot, votes, scan, scan_stats):
    choices = vote_lib.choice_ids(proposal)
    valid_votes = [vote for vote in votes if vote.get("valid")]
    invalid_votes = [vote for vote in votes if not vote.get("valid")]

    latest_by_deposit = {}
    superseded_votes = []
    for vote in sorted(valid_votes, key=vote_order):
        deposit_key = vote_lib.normalize_outpoint_key(vote["deposit_out_point_key"])
        if deposit_key in latest_by_deposit:
            superseded_votes.append(latest_by_deposit[deposit_key])
        latest_by_deposit[deposit_key] = vote

    counted_votes = sorted(latest_by_deposit.values(), key=vote_order)
    choice_weights = {choice: 0 for choice in choices}
    for vote in counted_votes:
        choice_weights[vote["choice"]] += int(vote["weight_shannons"])

    counted_records = [compact_vote(vote) for vote in counted_votes]
    superseded_records = [compact_vote(vote) for vote in sorted(superseded_votes, key=vote_order)]
    invalid_records = [compact_invalid_vote(vote) for vote in sorted(invalid_votes, key=vote_order)]

    commitment = {
        "magic": TALLY_MAGIC,
        "version": 1,
        "proposal_id": proposal["proposal_id"],
        "snapshot_id": snapshot["snapshot_id"],
        "snapshot_root": snapshot["roots"]["snapshot_root"],
        "vote_window": proposal["manifest"]["vote_window"],
        "discovery_method": "historical_block_scan",
        "scan_range": {
            "start_block": scan["start_block"],
            "end_block": scan["end_block"],
        },
        "is_final": scan["is_final"],
        "valid_vote_count": len(valid_votes),
        "counted_vote_count": len(counted_votes),
        "superseded_vote_count": len(superseded_votes),
        "invalid_vote_count": len(invalid_votes),
        "choice_weights_shannons": {choice: str(weight) for choice, weight in choice_weights.items()},
        "counted_votes_root": merkle_root(counted_records),
        "superseded_votes_root": merkle_root(superseded_records),
        "invalid_votes_root": merkle_root(invalid_records),
    }
    tally_root = vote_lib.ckb_hash(vote_lib.canonical_json(commitment))

    return {
        "magic": TALLY_MAGIC,
        "version": 1,
        "proposal_id": proposal["proposal_id"],
        "snapshot_id": snapshot["snapshot_id"],
        "snapshot_root": snapshot["roots"]["snapshot_root"],
        "tally_root": tally_root,
        "is_final": scan["is_final"],
        "generated_at_unix": int(time.time()),
        "generated_at_tip_block": scan["tip_block_number"],
        "scan_stats": scan_stats,
        "commitment": commitment,
        "choice_weights_shannons": {choice: str(weight) for choice, weight in choice_weights.items()},
        "choice_weights_ckb": {choice: shannons_to_ckb_string(weight) for choice, weight in choice_weights.items()},
        "valid_votes": [compact_vote(vote) for vote in sorted(valid_votes, key=vote_order)],
        "counted_votes": counted_records,
        "superseded_votes": superseded_records,
        "invalid_votes": invalid_records,
    }


def summarize(report, path=None):
    return {
        "tally_path": str(path) if path is not None else None,
        "proposal_id": report["proposal_id"],
        "snapshot_id": report["snapshot_id"],
        "tally_root": report["tally_root"],
        "is_final": report["is_final"],
        "scan_range": report["commitment"]["scan_range"],
        "valid_vote_count": report["commitment"]["valid_vote_count"],
        "counted_vote_count": report["commitment"]["counted_vote_count"],
        "superseded_vote_count": report["commitment"]["superseded_vote_count"],
        "invalid_vote_count": report["commitment"]["invalid_vote_count"],
        "choice_weights_ckb": report["choice_weights_ckb"],
    }


def create_report(args):
    rpc = vote_lib.RpcClient(args.rpc)
    proposal = vote_lib.read_json(args.proposal)
    snapshot = vote_lib.read_json(args.snapshot)
    tip_block_number = rpc.get_tip_block_number()
    scan = scan_range(proposal, tip_block_number, args.scan_end_block)
    votes, scan_stats = discover_historical_votes(rpc, proposal, snapshot, scan)
    report = build_report(proposal, snapshot, votes, scan, scan_stats)

    output = args.output
    if output is None:
        short_id = proposal["proposal_id"][2:14]
        output = args.output_dir / f"tally-{short_id}-block-{scan['end_block']}.json"
    write_json(output, report)
    print(json.dumps(summarize(report, output), indent=2, sort_keys=True))


def verify_report(args):
    existing = vote_lib.read_json(args.tally)
    rpc = vote_lib.RpcClient(args.rpc)
    proposal = vote_lib.read_json(args.proposal)
    snapshot = vote_lib.read_json(args.snapshot)
    tip_block_number = rpc.get_tip_block_number()
    scan_end_block = existing["commitment"]["scan_range"]["end_block"]
    scan = scan_range(proposal, tip_block_number, scan_end_block)
    votes, scan_stats = discover_historical_votes(rpc, proposal, snapshot, scan)
    recomputed = build_report(proposal, snapshot, votes, scan, scan_stats)

    root_matches = existing["tally_root"] == recomputed["tally_root"]
    commitment_matches = existing["commitment"] == recomputed["commitment"]
    print(
        json.dumps(
            {
                "tally_path": str(args.tally),
                "root_matches": root_matches,
                "commitment_matches": commitment_matches,
                "expected_tally_root": existing["tally_root"],
                "recomputed_tally_root": recomputed["tally_root"],
                "recomputed_summary": summarize(recomputed),
            },
            indent=2,
            sort_keys=True,
        )
    )
    if not root_matches or not commitment_matches:
        raise RuntimeError("tally verification failed")


def main():
    parser = argparse.ArgumentParser(description="Create and verify DAO treasury tally MVP artifacts.")
    subparsers = parser.add_subparsers(dest="command", required=True)

    create = subparsers.add_parser("create")
    create.add_argument("--rpc", default="http://127.0.0.1:8114")
    create.add_argument("--proposal", type=Path, required=True)
    create.add_argument("--snapshot", type=Path, required=True)
    create.add_argument("--output-dir", type=Path, required=True)
    create.add_argument("--output", type=Path)
    create.add_argument("--scan-end-block", type=int)

    verify = subparsers.add_parser("verify")
    verify.add_argument("--rpc", default="http://127.0.0.1:8114")
    verify.add_argument("--proposal", type=Path, required=True)
    verify.add_argument("--snapshot", type=Path, required=True)
    verify.add_argument("--tally", type=Path, required=True)

    args = parser.parse_args()
    if args.command == "create":
        create_report(args)
    elif args.command == "verify":
        verify_report(args)


if __name__ == "__main__":
    try:
        main()
    except Exception as err:
        print(f"error: {err}", file=sys.stderr)
        sys.exit(1)
