"""Give the Trezor emulator a wallet, so the Anzen app has a key to sign with.

A fresh emulator has no seed, and setting one up by hand means tapping through the device's own
screens. This wipes it and loads a recovery phrase through the debug link, as Trezor's device
tests do. Emulator and debug firmware only: a real Trezor has no debug link.

    TREZOR_PATH=udp:127.0.0.1:<port> python tools/seed-emulator.py ["recovery phrase"]

With `--check <app.tapp>` it also loads the Anzen app and asks for its version and its regtest
vault key, a quick check that the firmware, the app and the key patch agree.
"""

import os
import sys
from pathlib import Path

from trezorlib import debuglink, messages
from trezorlib.debuglink import TrezorTestContext
from trezorlib.transport import get_transport

# The phrase Trezor's own tests use. It is public: never fund it.
DEFAULT_PHRASE = " ".join(["all"] * 12)


def main() -> None:
    arguments = sys.argv[1:]
    check = None
    if "--check" in arguments:
        at = arguments.index("--check")
        check = Path(arguments[at + 1])
        del arguments[at : at + 2]
    phrase = arguments[0] if arguments else DEFAULT_PHRASE

    context = TrezorTestContext(
        get_transport(os.environ["TREZOR_PATH"]), auto_interact=True, force_wipe=True
    )
    context.wipe_device()
    session = context.get_session(passphrase=None)
    debuglink.load_device(
        session,
        mnemonic=phrase,
        pin=None,
        passphrase_protection=False,
        label="Anzen emulator",
    )
    session.close()
    print("The emulator has a wallet.")

    if check is not None:
        session = context.get_session(passphrase="")
        instance = debuglink.load_extapp(session, check)
        version = session.call(
            messages.ExtAppMessage(instance_id=instance, message_id=1, data=b""),
            expect=messages.ExtAppResponse,
        )
        key = session.call(
            messages.ExtAppMessage(instance_id=instance, message_id=2, data=b"\x02"),
            expect=messages.ExtAppResponse,
        )
        print(f"Anzen app protocol version {version.data[0]}")
        print(f"Regtest vault key {key.data.hex()}")


if __name__ == "__main__":
    main()
