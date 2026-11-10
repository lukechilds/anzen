//! The phone library's Trezor link, through the C entry point the app calls, against the Trezor
//! emulator: connect, set a vault up, approve and decline a policy, and find the vault still
//! there after the link is closed and opened again.
//!
//! Needs the emulator and the app built for it (`trezor-app/README.md`):
//! `ANZEN_TREZOR_PORT=<port> cargo test --test trezor -- --ignored`

#[path = "../../../crates/anzen-link/tests/phone_side/mod.rs"]
#[allow(dead_code)]
mod phone_side;

use anzen_common::{
    HWW_RECOVERY_SECONDS,
    policy::VaultPolicy,
    storage::{PublicDeviceFile, VAULT_CONFIG_VERSION, VaultConfig},
};
use anzen_link::trezor::debug::{DebugLink, shows};
use anzen_mobile::anzen_mobile_trezor;
use anzen_protocol::{
    address::Network,
    policy::Limits,
    script::{VaultKeys, XOnlyKey},
    wire::PolicyApproval,
};
use bitcoin::key::{Keypair, Secp256k1};
use serde_json::{Value, json};
use std::ffi::{CStr, CString};

fn port() -> u16 {
    std::env::var("ANZEN_TREZOR_PORT")
        .ok()
        .and_then(|port| port.parse().ok())
        .unwrap_or(21324)
}

/// One call across the C boundary, as the app's native module makes it.
fn call(root: &std::path::Path, request: Value) -> Result<Value, String> {
    let root = CString::new(root.to_str().unwrap()).unwrap();
    let request = CString::new(request.to_string()).unwrap();
    // SAFETY: both strings live for the call; the answer is copied before it is freed.
    let answer = unsafe {
        let pointer = anzen_mobile_trezor(root.as_ptr(), request.as_ptr());
        let answer = CStr::from_ptr(pointer).to_str().unwrap().to_owned();
        anzen_mobile::anzen_mobile_free(pointer);
        answer
    };
    let answer: Value = serde_json::from_str(&answer).unwrap();
    match answer.get("error") {
        Some(error) => Err(error.as_str().unwrap().to_owned()),
        None => Ok(answer["ok"].clone()),
    }
}

/// Make `call` while a person at the emulator approves or rejects the screen showing `title`.
fn with_person(
    root: &std::path::Path,
    request: Value,
    title: &'static str,
    approve: bool,
) -> (Result<Value, String>, String) {
    let person = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut debug = DebugLink::connect(port()).await.unwrap();
                let shown = debug.read_review(title).await.unwrap();
                debug.press(approve).await.unwrap();
                shown
            })
    });
    let answer = call(root, request);
    (answer, person.join().unwrap())
}

#[test]
#[ignore = "requires the Trezor emulator and the Trezor app built for it"]
fn the_phone_library_drives_a_trezor_through_its_c_entry_point() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    let connect = json!({ "method": "connect", "emulator": port() });

    // Nothing is connected yet.
    let refused = call(
        root,
        json!({ "method": "request", "message": { "kind": "signer-hello", "body": {} } }),
    );
    assert!(refused.unwrap_err().contains("No Trezor is connected"));

    // The emulator pairs with no code, once the person allows it.
    let (connected, _) = with_person(root, connect.clone(), "to pair with this", true);
    assert_eq!(connected.unwrap()["name"], "Trezor emulator");
    // Connecting again reuses the open link, and asks nothing.
    assert_eq!(
        call(root, connect.clone()).unwrap()["name"],
        "Trezor emulator"
    );

    let phone = Keypair::from_seckey_slice(&Secp256k1::new(), &[0x45; 32]).unwrap();
    let phone_key = phone.x_only_public_key().0;
    let message = |kind: &str, body: Value| json!({ "method": "request", "message": { "kind": kind, "body": body } });
    let identity = call(
        root,
        message(
            "signer-hello",
            json!({ "network": "regtest", "phone": phone_key.to_string() }),
        ),
    )
    .unwrap();
    assert_eq!(identity["kind"], "signer-identity");
    let identity: PublicDeviceFile = serde_json::from_value(identity["body"].clone()).unwrap();
    let keys = VaultKeys {
        phone: XOnlyKey(phone_key.serialize()),
        hww: XOnlyKey::from_hex(&identity.vault_pubkey).unwrap(),
        recovery_locktime: 1_234_567_890,
    };
    let policy = VaultPolicy::new_for_network(
        phone_key,
        identity.parsed_vault_pubkey().unwrap(),
        keys.recovery_locktime,
        bitcoin::Network::Regtest,
    )
    .unwrap();
    let config = VaultConfig {
        version: VAULT_CONFIG_VERSION,
        network: "regtest".to_owned(),
        phone_vault_pubkey: phone_key.to_string(),
        hww_vault_pubkey: identity.vault_pubkey.clone(),
        phone_hot_external_descriptor: String::new(),
        phone_hot_internal_descriptor: String::new(),
        vault_descriptor: policy.descriptor_string(),
        vault_address: policy.address.to_string(),
        hww_recovery_seconds: HWW_RECOVERY_SECONDS,
        recovery_locktime: keys.recovery_locktime,
        monthly_limit_sats: 0,
        emergency_access_limit_sats: 0,
    };
    let setup = message("vault-setup", json!({ "config": config, "backup": null }));
    let (accepted, shown) = with_person(root, setup.clone(), "Set up vault", true);
    assert_eq!(accepted.unwrap()["kind"], "vault-accepted");
    assert!(shows(&shown, &policy.address.to_string()), "{shown}");

    // A policy the person declines comes back as declined, not as an error.
    let (request, secrets, messages) = phone_side::policy(
        &phone,
        keys,
        Network::Regtest,
        vec![phone_side::input(1, 110_000_000)],
        Limits {
            monthly_limit_sats: 10_000_000,
            emergency_access_limit_sats: 0,
        },
    );
    let proposal = message("policy-proposal", serde_json::to_value(&request).unwrap());
    let (declined, _) = with_person(root, proposal.clone(), "Vault policy", false);
    assert_eq!(declined.unwrap(), json!({ "declined": true }));

    // The link is closed and opened again, as when the app restarts. What the signer sealed is
    // on disk, so the vault is still set up: the same setup asks nothing.
    call(root, json!({ "method": "disconnect" })).unwrap();
    assert!(
        call(root, proposal.clone())
            .unwrap_err()
            .contains("No Trezor is connected")
    );
    let (connected, _) = with_person(root, connect, "to pair with this", true);
    connected.unwrap();
    assert_eq!(call(root, setup).unwrap()["kind"], "vault-accepted");

    let (approved, shown) = with_person(root, proposal, "Vault policy", true);
    let approved = approved.unwrap();
    assert!(shows(&shown, "0.1000 BTC a month"), "{shown}");
    assert_eq!(approved["kind"], "policy-approved");
    let approval: PolicyApproval = serde_json::from_value(approved["body"].clone()).unwrap();
    phone_side::complete_all(
        &phone,
        &keys,
        &messages,
        &secrets,
        &request.phone_nonces,
        &approval.signatures,
    );
    call(root, json!({ "method": "disconnect" })).unwrap();
    // What the signer sealed is what the phone keeps.
    // (A debug Trezor issues no credential, so no pairing is stored here.)
    assert!(root.join("trezor/trezor-regtest-vault.sealed").is_file());
}
