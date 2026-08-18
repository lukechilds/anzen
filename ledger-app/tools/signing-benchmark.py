#!/usr/bin/env python3
"""Run Anzen's deterministic signing workload on Speculos or a physical Ledger."""

from __future__ import annotations

import argparse
import socket
import struct
import sys
import time
from dataclasses import dataclass


CLA = 0xE0
PREPARE = 0x20
KEY_DERIVATION = 0x21
GRAPH = 0x22
SIGNING = 0x23
COMPLETE = 0x24
BENCHMARK_ERRORS = {
    0x6F10: "transaction graph construction failed",
    0x6F11: "Ledger cryptography operation failed",
    0x6F13: "HWW signature creation failed",
    0x6F15: "re-derived HWW public key changed",
    0x6F16: "signature count did not match the transaction graph",
}


class Transport:
    def exchange(self, apdu: bytes) -> bytes:
        raise NotImplementedError

    def close(self) -> None:
        pass


class SpeculosTransport(Transport):
    def __init__(self, endpoint: str) -> None:
        host, port = endpoint.rsplit(":", 1)
        self.socket = socket.create_connection((host, int(port)))

    def exchange(self, apdu: bytes) -> bytes:
        self.socket.sendall(struct.pack(">I", len(apdu)) + apdu)
        length = struct.unpack(">I", self._read_exact(4))[0]
        # Speculos prefixes the response-data length, excluding the trailing
        # two-byte status word. Read both so this behaves like ledgerblue.
        response = self._read_exact(length + 2)
        return parse_response(response)

    def _read_exact(self, length: int) -> bytes:
        chunks = bytearray()
        while len(chunks) < length:
            chunk = self.socket.recv(length - len(chunks))
            if not chunk:
                raise RuntimeError("Speculos closed the APDU connection")
            chunks.extend(chunk)
        return bytes(chunks)

    def close(self) -> None:
        self.socket.close()


class LedgerCommTransport(Transport):
    def __init__(self) -> None:
        try:
            from ledgercomm import Transport as LedgerTransport
        except ImportError as error:
            raise RuntimeError(
                "ledgercomm HID support is required for a physical device; run "
                "`ledger-app/tools/run-signing-benchmark.sh`"
            ) from error
        try:
            self.transport = LedgerTransport(interface="hid")
        except Exception as error:
            raise RuntimeError(
                "could not connect to a Ledger over USB; connect and unlock it, "
                "then open Anzen"
            ) from error

    def exchange(self, apdu: bytes) -> bytes:
        try:
            status, response = self.transport.exchange_raw(apdu)
        except Exception as error:
            raise RuntimeError("Ledger USB exchange failed") from error
        return parse_response(response + status.to_bytes(2, "big"))

    def close(self) -> None:
        self.transport.close()


@dataclass(frozen=True)
class Workload:
    version: int
    rollover_inputs: int
    transactions: int
    signatures: int
    trailing: bytes

    @classmethod
    def parse(cls, response: bytes) -> "Workload":
        if len(response) != 36:
            raise RuntimeError(f"unexpected benchmark response length: {len(response)}")
        workload = cls(*response[:4], response[4:])
        if workload.version != 3:
            raise RuntimeError(f"unsupported benchmark version: {workload.version}")
        return workload

    def same_shape(self, other: "Workload") -> bool:
        return (
            self.version,
            self.rollover_inputs,
            self.transactions,
            self.signatures,
        ) == (
            other.version,
            other.rollover_inputs,
            other.transactions,
            other.signatures,
        )


def apdu(instruction: int, p1: int = 0, data: bytes = b"") -> bytes:
    if len(data) > 255:
        raise ValueError("benchmark APDU payload is too large")
    return bytes((CLA, instruction, p1, 0, len(data))) + data


def parse_response(response: bytes) -> bytes:
    if len(response) < 2:
        raise RuntimeError("short APDU response")
    status = int.from_bytes(response[-2:], "big")
    if status != 0x9000:
        detail = BENCHMARK_ERRORS.get(status)
        suffix = f": {detail}" if detail else ""
        raise RuntimeError(f"Ledger returned status 0x{status:04x}{suffix}")
    return response[:-2]


def run(transport: Transport, rollover_inputs: int) -> None:
    print("Open Anzen on the Ledger and leave it on the home screen.")
    print("Review and approve the deterministic benchmark policy when prompted.\n")
    prepare_started = time.perf_counter()
    prepared = Workload.parse(transport.exchange(apdu(PREPARE, rollover_inputs)))
    prepare_elapsed = time.perf_counter() - prepare_started

    print("✓ Benchmark policy approved and deterministic transaction graph prepared.")
    print(
        f"  {prepared.rollover_inputs} rollover inputs, "
        f"{prepared.transactions} transactions, {prepared.signatures} signatures"
    )
    def timed_phase(label: str, instruction: int) -> tuple[Workload, float]:
        print(f"\n{label}…")
        started = time.perf_counter()
        result = Workload.parse(transport.exchange(apdu(instruction)))
        elapsed = time.perf_counter() - started
        if not prepared.same_shape(result):
            raise RuntimeError("device changed the benchmark workload after approval")
        return result, elapsed

    derived, key_derivation_elapsed = timed_phase(
        "Timing BIP32 key derivation and public-key creation", KEY_DERIVATION
    )
    if derived.trailing != prepared.trailing:
        raise RuntimeError("re-derived HWW public key changed")
    _graph, graph_elapsed = timed_phase(
        "Timing transaction graph and BIP341 sighash construction", GRAPH
    )
    completed, signing_elapsed = timed_phase(
        "Timing repeated fixed-digest BIP340 signing", SIGNING
    )

    elapsed_phases = (key_derivation_elapsed, graph_elapsed, signing_elapsed)
    encoded_timings = b"".join(
        min(round(elapsed * 1000), 0xFFFFFFFF).to_bytes(4, "big")
        for elapsed in elapsed_phases
    )
    completion = transport.exchange(
        apdu(COMPLETE, data=encoded_timings)
    )
    if completion != b"\x03":
        raise RuntimeError("unexpected benchmark-completion response")

    per_signature_ms = signing_elapsed * 1000 / completed.signatures
    full_workload_elapsed = sum(elapsed_phases)
    print(f"\n✓ Completed {completed.signatures} fixed-digest HWW signatures.")
    print(f"\nKey derivation:    {key_derivation_elapsed:.3f} s")
    print(f"Transaction graph: {graph_elapsed:.3f} s")
    print(f"Signing:           {signing_elapsed:.3f} s")
    print(f"Per signature:     {per_signature_ms:.1f} ms")
    print(f"Full workload:     {full_workload_elapsed:.3f} s")
    print(f"Fixture + review:  {prepare_elapsed:.3f} s (includes human approval)")
    print(f"Last sighash:      {_graph.trailing.hex()}")
    print(f"Last signature R:  {completed.trailing.hex()}")


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Benchmark Anzen's real annual policy signing workload"
    )
    parser.add_argument(
        "--inputs",
        type=int,
        choices=(1, 2, 12),
        default=12,
        help="fake UTXOs entering the annual rollover (default: 12)",
    )
    parser.add_argument(
        "--speculos",
        metavar="HOST:PORT",
        help="use Speculos instead of a physical Ledger, e.g. 127.0.0.1:9999",
    )
    args = parser.parse_args()

    transport: Transport
    try:
        transport = (
            SpeculosTransport(args.speculos)
            if args.speculos
            else LedgerCommTransport()
        )
        try:
            run(transport, args.inputs)
        finally:
            transport.close()
    except (OSError, RuntimeError) as error:
        print(f"Error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
