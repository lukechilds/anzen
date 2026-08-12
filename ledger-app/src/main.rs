#![no_std]
#![no_main]

use anzen_cold_signer::PROTOCOL_NAME;
use ledger_device_sdk::nbgl::{NbglAction, init_comm};

ledger_device_sdk::set_panic!(ledger_device_sdk::exiting_panic);
ledger_device_sdk::define_comm!(COMM);

const APP_NAME: &str = PROTOCOL_NAME;

#[unsafe(no_mangle)]
extern "C" fn sample_main() {
    let comm = init_comm(&COMM);
    let _app_name = APP_NAME;
    let mut show_anzen = false;

    loop {
        let message = if show_anzen {
            "Hello Anzen"
        } else {
            "Hello world"
        };

        let _ = NbglAction::new()
            .message(message)
            .action_text("Continue")
            .show(comm);

        show_anzen = !show_anzen;
    }
}
