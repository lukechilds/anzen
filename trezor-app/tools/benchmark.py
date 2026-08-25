#!/usr/bin/env python3
"""Load the Anzen Trezor app and run its deterministic signing benchmark."""

from __future__ import annotations

import argparse
import struct
from pathlib import Path

from trezorlib import messages, trezorapp
from trezorlib.cli import TrezorConnection

PROTOCOL_VERSION = 1
ROLLOVER_INPUTS = 12
RUN_BENCHMARK = 1
BENCHMARK_RESULT = 2
RESPONSE_SIZE = 84


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--artifacts",
        type=Path,
        default=Path(__file__).parents[1] / "target" / "artifacts" / "t3w1",
        help="directory containing the built app, proof, and root packet",
    )
    return parser.parse_args()


def load_bundle(session, artifacts: Path) -> int:
    binary_path = artifacts / "anzen-trezor.elf"
    proof_path = artifacts / "anzen-trezor.proof"
    root_packet_path = artifacts / "rootpacket_12-timestamped-signed.tmr"
    for path in (binary_path, proof_path, root_packet_path):
        if not path.is_file():
            raise SystemExit(f"missing build artifact: {path}")

    return trezorapp.load(
        session,
        binary_path.read_bytes(),
        proof_path.read_bytes(),
        root_packet_path.read_bytes(),
        min_version=None,
        force_reload=True,
    )


def decode_response(data: bytes) -> dict[str, int | str]:
    if len(data) != RESPONSE_SIZE:
        raise ValueError(f"unexpected benchmark response length: {len(data)}")
    version, inputs, transactions, signatures, key_ms, graph_ms, signing_ms, total_ms = (
        struct.unpack("<BBBBIIII", data[:20])
    )
    if version != PROTOCOL_VERSION or inputs != ROLLOVER_INPUTS:
        raise ValueError(f"unexpected benchmark response header: {data[:4].hex()}")
    return {
        "rollover_inputs": inputs,
        "transactions": transactions,
        "signatures": signatures,
        "key_derivation_ms": key_ms,
        "graph_ms": graph_ms,
        "signing_ms": signing_ms,
        "full_workload_ms": total_ms,
        "last_sighash": data[20:52].hex(),
        "last_signature_r": data[52:84].hex(),
    }


def main() -> None:
    args = parse_args()
    connection = TrezorConnection(app_name="trezorctl")
    with connection.session_context() as session:
        instance_id = load_bundle(session, args.artifacts)
        print(f"Anzen app ready: instance {instance_id}")
        print("Approve the annual vault policy on the Trezor to start the benchmark.")
        response = session.call(
            messages.TrezorAppMessage(
                instance_id=instance_id,
                message_id=RUN_BENCHMARK,
                data=bytes((PROTOCOL_VERSION, ROLLOVER_INPUTS)),
            ),
            expect=messages.TrezorAppResponse,
        )
    if response.message_id != BENCHMARK_RESULT:
        raise ValueError(f"unexpected response message ID: {response.message_id}")

    result = decode_response(bytes(response.data))
    print()
    print(f"Rollover inputs: {result['rollover_inputs']}")
    print(f"Transactions:     {result['transactions']}")
    print(f"Signatures:       {result['signatures']}")
    print(f"Key derivation:   {result['key_derivation_ms']} ms")
    print(f"Graph:            {result['graph_ms']} ms")
    print(f"Signing:          {result['signing_ms']} ms")
    print(f"Full workload:    {result['full_workload_ms']} ms")
    print(f"Last sighash:     {result['last_sighash']}")
    print(f"Last signature R: {result['last_signature_r']}")


if __name__ == "__main__":
    main()
