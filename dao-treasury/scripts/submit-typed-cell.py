#!/usr/bin/env python3
import argparse
import json
import subprocess
import sys
import tempfile
from decimal import Decimal
from pathlib import Path


SHANNONS_PER_CKB = 100_000_000
CHANGE_FEE_CKB = Decimal("0.01")
MIN_CHANGE_CKB = Decimal("61")


def run(args, *, capture=True):
    result = subprocess.run(
        args,
        check=True,
        text=True,
        stdout=subprocess.PIPE if capture else None,
        stderr=subprocess.PIPE,
    )
    return result.stdout


def read_json(path: Path):
    return json.loads(path.read_text())


def write_json(path: Path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def parse_capacity(value: str) -> int:
    amount = value.split("(", 1)[0].strip()
    return int(Decimal(amount) * SHANNONS_PER_CKB)


def ckb_to_shannons(value: str) -> int:
    return int(Decimal(value) * SHANNONS_PER_CKB)


def shannons_to_ckb(value: int) -> str:
    whole, frac = divmod(value, SHANNONS_PER_CKB)
    if frac == 0:
        return str(whole)
    return f"{whole}.{frac:08d}".rstrip("0")


def parse_out_point(value: str):
    tx_hash, index = value.split(":", 1)
    return {"tx_hash": tx_hash, "index": hex(int(index, 16) if index.startswith("0x") else int(index))}


def pick_input(ckb_cli: str, rpc: str, address: str, needed: int):
    raw = run(
        [
            ckb_cli,
            "--url",
            rpc,
            "--output-format",
            "json",
            "wallet",
            "get-live-cells",
            "--address",
            address,
            "--limit",
            "50",
            "--local-only",
        ]
    )
    live_cells = json.loads(raw)["live_cells"]
    for cell in live_cells:
        if cell.get("type_hashes") is not None:
            continue
        capacity = parse_capacity(cell["capacity"])
        if capacity >= needed:
            return cell, capacity
    raise RuntimeError(f"no ordinary live cell at {address} can cover {shannons_to_ckb(needed)} CKB")


def add_code_dep(tx, out_point):
    dep = {"out_point": out_point, "dep_type": "code"}
    if dep not in tx["transaction"]["cell_deps"]:
        tx["transaction"]["cell_deps"].append(dep)


def main():
    parser = argparse.ArgumentParser(description="Submit one typed governance output cell using ckb-cli signing.")
    parser.add_argument("--ckb-cli", default="ckb-cli")
    parser.add_argument("--rpc", default="http://127.0.0.1:8114")
    parser.add_argument("--privkey-path", type=Path, required=True)
    parser.add_argument("--from-address", required=True)
    parser.add_argument("--to-address", required=True)
    parser.add_argument("--capacity", default="5000")
    parser.add_argument("--data-path", type=Path, required=True)
    parser.add_argument("--type-code-hash", required=True)
    parser.add_argument("--type-hash-type", default="data")
    parser.add_argument("--type-args", required=True)
    parser.add_argument("--type-code-out-point", required=True, help="<tx-hash>:<index>")
    parser.add_argument("--tx-file", type=Path)
    args = parser.parse_args()

    output_capacity = ckb_to_shannons(args.capacity)
    fee = ckb_to_shannons(str(CHANGE_FEE_CKB))
    min_change = ckb_to_shannons(str(MIN_CHANGE_CKB))
    input_cell, input_capacity = pick_input(args.ckb_cli, args.rpc, args.from_address, output_capacity + fee)
    change_capacity = input_capacity - output_capacity - fee
    if 0 < change_capacity < min_change:
        fee += change_capacity
        change_capacity = 0

    if args.tx_file:
        tx_file = args.tx_file
        tx_file.parent.mkdir(parents=True, exist_ok=True)
    else:
        tmp = tempfile.NamedTemporaryFile(prefix="dao-typed-cell-", suffix=".json", delete=False)
        tx_file = Path(tmp.name)
        tmp.close()

    run([args.ckb_cli, "--url", args.rpc, "tx", "init", "--tx-file", str(tx_file)])
    run(
        [
            args.ckb_cli,
            "--url",
            args.rpc,
            "tx",
            "add-input",
            "--tx-file",
            str(tx_file),
            "--tx-hash",
            input_cell["tx_hash"],
            "--index",
            str(input_cell["output_index"]),
            "--local-only",
        ]
    )
    run(
        [
            args.ckb_cli,
            "--url",
            args.rpc,
            "tx",
            "add-output",
            "--tx-file",
            str(tx_file),
            "--to-sighash-address",
            args.to_address,
            "--capacity",
            args.capacity,
            "--to-data-path",
            str(args.data_path),
            "--local-only",
        ]
    )

    tx = read_json(tx_file)
    add_code_dep(tx, parse_out_point(args.type_code_out_point))
    tx["transaction"]["outputs"][0]["type"] = {
        "code_hash": args.type_code_hash,
        "hash_type": args.type_hash_type,
        "args": args.type_args,
    }
    if change_capacity:
        tx["transaction"]["outputs"].append(
            {
                "capacity": hex(change_capacity),
                "lock": tx["transaction"]["outputs"][0]["lock"],
                "type": None,
            }
        )
        tx["transaction"]["outputs_data"].append("0x")
    write_json(tx_file, tx)

    run(
        [
            args.ckb_cli,
            "--url",
            args.rpc,
            "tx",
            "sign-inputs",
            "--tx-file",
            str(tx_file),
            "--privkey-path",
            str(args.privkey_path),
            "--add-signatures",
            "--local-only",
        ]
    )
    raw_tx_hash = run(
        [
            args.ckb_cli,
            "--url",
            args.rpc,
            "--output-format",
            "json",
            "tx",
            "send",
            "--tx-file",
            str(tx_file),
            "--local-only",
        ]
    )
    tx_hash = json.loads(raw_tx_hash) if raw_tx_hash.strip().startswith('"') else raw_tx_hash.strip()

    print(
        json.dumps(
            {
                "tx_hash": tx_hash,
                "tx_file": str(tx_file),
                "input": {
                    "tx_hash": input_cell["tx_hash"],
                    "index": hex(input_cell["output_index"]),
                    "capacity_ckb": shannons_to_ckb(input_capacity),
                },
                "typed_output": {
                    "index": "0x0",
                    "capacity_ckb": args.capacity,
                    "type": tx["transaction"]["outputs"][0]["type"],
                },
                "change_capacity_ckb": shannons_to_ckb(change_capacity),
                "fee_ckb": shannons_to_ckb(fee),
            },
            indent=2,
            sort_keys=True,
        )
    )


if __name__ == "__main__":
    try:
        main()
    except Exception as err:
        print(f"error: {err}", file=sys.stderr)
        sys.exit(1)
