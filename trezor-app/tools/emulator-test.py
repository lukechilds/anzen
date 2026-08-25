#!/usr/bin/env python3
"""Exercise the packaged Anzen app end to end against a debug emulator."""

from __future__ import annotations

import argparse
import os
from pathlib import Path

from trezorlib import debuglink, messages
from trezorlib.debuglink import TrezorTestContext
from trezorlib.transport import get_transport

from benchmark import (
    BENCHMARK_RESULT,
    PROTOCOL_VERSION,
    ROLLOVER_INPUTS,
    RUN_BENCHMARK,
    decode_response,
)

MNEMONIC = " ".join(["all"] * 12)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--artifact",
        type=Path,
        default=Path(__file__).parents[1]
        / "target"
        / "artifacts"
        / "t3w1-emu"
        / "anzen-trezor.elf",
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    transport = get_transport(os.environ.get("TREZOR_PATH"))
    test_ctx = TrezorTestContext(transport, auto_interact=True, force_wipe=True)
    test_ctx.wipe_device()
    seedless_session = test_ctx.get_session(passphrase=None)
    debuglink.load_device(
        seedless_session,
        mnemonic=MNEMONIC,
        pin=None,
        passphrase_protection=False,
        label="Anzen benchmark test",
    )
    session = test_ctx.get_session(passphrase="")
    instance_id = debuglink.load_trezorapp(session, args.artifact)

    def approve_without_layout_wait():
        while True:
            yield
            # Optimized firmware intentionally omits most debug-layout plumbing.
            # Send the decision directly so test automation cannot deadlock while
            # waiting for a diagnostic layout response that the app does not need.
            test_ctx.debug._write(  # noqa: SLF001 - emulator-only test helper
                messages.DebugLinkDecision(button=messages.DebugButton.YES)
            )

    with test_ctx:
        test_ctx.set_input_flow(approve_without_layout_wait)
        response = session.call(
            messages.TrezorAppMessage(
                instance_id=instance_id,
                message_id=RUN_BENCHMARK,
                data=bytes((PROTOCOL_VERSION, ROLLOVER_INPUTS)),
            ),
            expect=messages.TrezorAppResponse,
        )

    if response.message_id != BENCHMARK_RESULT:
        raise AssertionError(f"unexpected response message ID: {response.message_id}")
    result = decode_response(bytes(response.data))
    assert result["rollover_inputs"] == 12
    assert result["transactions"] == 15
    assert result["signatures"] == 26
    assert result["full_workload_ms"] == (
        result["key_derivation_ms"] + result["graph_ms"] + result["signing_ms"]
    )
    assert result["last_sighash"] != "00" * 32
    assert result["last_signature_r"] != "00" * 32

    print("Standalone Anzen Trezor benchmark passed")
    print(f"Key derivation: {result['key_derivation_ms']} ms")
    print(f"Graph:          {result['graph_ms']} ms")
    print(f"Signing:        {result['signing_ms']} ms")
    print(f"Full workload:  {result['full_workload_ms']} ms")


if __name__ == "__main__":
    main()
