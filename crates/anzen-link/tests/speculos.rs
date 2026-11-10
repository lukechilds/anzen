//! The Ledger app in the Speculos emulator, driven as a phone would drive it: real requests in,
//! taps on the emulated screen, and every signature checked with the host's own code.
//!
//! Needs the emulator (`ledger-app/tools/speculos.sh` in Ledger's dev container, with its APDU
//! port 9999 and API port 5001 published), so it is ignored by default:
//! `cargo test -p anzen-link --test speculos -- --ignored`
//!
//! The same tests run against a real device, with a person doing the tapping: start
//! `cargo run -p anzen-link --example ledger-proxy`, open the Anzen app on the Ledger, and set
//! `ANZEN_LEDGER=127.0.0.1:9998 ANZEN_LEDGER_HUMAN=1` (with `--test-threads=1 --nocapture`).
//! They use the device's regtest vault and a fixed test phone key.

use anzen_common::keys::DeviceKeys;
use anzen_link::ledger::{HardwareSigner, Speculos};
use anzen_protocol::{
    address::Network,
    script::{VaultKeys, XOnlyKey},
};
use anzen_signer::hardware::Pair;
use bitcoin::key::{Keypair, Secp256k1};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

const SEED: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const NETWORK: Network = Network::Regtest;

/// Where the app answers APDUs: the emulator, or a proxy to a real device.
fn address() -> String {
    std::env::var("ANZEN_LEDGER").unwrap_or_else(|_| "127.0.0.1:9999".to_owned())
}

/// A person holds the device and approves on its screen; nothing is automated or read back.
fn human() -> bool {
    std::env::var_os("ANZEN_LEDGER_HUMAN").is_some()
}

/// The emulator's automation API.
async fn api(method: &str, path: &str, body: &str) -> Vec<u8> {
    let mut stream = TcpStream::connect("127.0.0.1:5001")
        .await
        .expect("Speculos API");
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    response.split_off(split + 4)
}

/// The text on the emulated screen.
async fn screen() -> String {
    let events: serde_json::Value =
        serde_json::from_slice(&api("GET", "/events?currentscreenonly=true", "").await).unwrap();
    events["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|event| event["text"].as_str())
        .collect::<Vec<_>>()
        .join(" | ")
}

async fn tap(x: u32, y: u32) {
    api(
        "POST",
        "/finger",
        &format!(r#"{{"action":"press-and-release","x":{x},"y":{y}}}"#),
    )
    .await;
}

async fn hold(x: u32, y: u32) {
    api(
        "POST",
        "/finger",
        &format!(r#"{{"action":"press","x":{x},"y":{y}}}"#),
    )
    .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    api(
        "POST",
        "/finger",
        &format!(r#"{{"action":"release","x":{x},"y":{y}}}"#),
    )
    .await;
}

async fn wait_for(text: &str) -> String {
    for _ in 0..150 {
        let screen = screen().await;
        if screen.contains(text) {
            return screen;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("the screen never showed {text:?}: {}", screen().await);
}

/// Read a review to its last page and hold the approve button, as a person would on a Flex.
/// Returns everything the review showed.
async fn approve_on_screen(first: &str) -> String {
    if human() {
        eprintln!("\n>>> On the Ledger: review \"{first}\" and hold to approve.\n");
        return String::new();
    }
    let mut shown = wait_for(first).await;
    for _ in 0..8 {
        if shown.contains("Hold to approve") {
            hold(400, 430).await;
            return shown;
        }
        tap(430, 550).await;
        tokio::time::sleep(Duration::from_millis(600)).await;
        shown.push_str(" | ");
        shown.push_str(&screen().await);
    }
    panic!("the review never reached its approval page: {shown}");
}

fn phone() -> Keypair {
    Keypair::from_seckey_slice(&Secp256k1::new(), &[0x42; 32]).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the Speculos emulator running the Ledger app"]
async fn the_ledger_app_pairs_and_remembers_its_vault() {
    let mut signer = HardwareSigner::open(Speculos::connect(&address()).await.unwrap())
        .await
        .unwrap();
    // The device derives the same vault key the phone app's signer mode would from this seed.
    let expected =
        DeviceKeys::parse_for_network(&Secp256k1::new(), SEED, bitcoin::Network::Regtest)
            .unwrap()
            .vault_pubkey;
    let identity = signer.identity(NETWORK).await.unwrap();
    if !human() {
        assert_eq!(identity, XOnlyKey(expected.serialize()));
    }
    // Mainnet's key is another one.
    assert_ne!(signer.identity(Network::Mainnet).await.unwrap(), identity);

    let pair = Pair {
        network: NETWORK,
        phone: XOnlyKey(phone().x_only_public_key().0.serialize()),
        recovery_locktime: 1_234_567_890,
    };
    signer.pair(&pair).await.unwrap();
    // Repeating setup is fine; another phone is refused until this vault is forgotten.
    signer.pair(&pair).await.unwrap();
    let state = signer.state(NETWORK).await.unwrap().expect("a vault");
    assert_eq!(
        state.keys,
        VaultKeys {
            phone: pair.phone,
            hww: identity,
            recovery_locktime: 1_234_567_890
        }
    );
    assert!(signer.state(Network::Mainnet).await.unwrap().is_none());
    let other = Pair {
        phone: XOnlyKey(
            Keypair::from_seckey_slice(&Secp256k1::new(), &[0x43; 32])
                .unwrap()
                .x_only_public_key()
                .0
                .serialize(),
        ),
        ..pair.clone()
    };
    let refused = signer.pair(&other).await.unwrap_err().to_string();
    assert!(refused.contains("already has a vault"), "{refused}");
    // A key that is not on the curve is no vault at all.
    let invalid = Pair {
        phone: XOnlyKey([0x79; 32]),
        ..pair.clone()
    };
    assert!(signer.pair(&invalid).await.is_err());
}

mod phone_side {
    //! What the phone does around a signer's approval: build the request with its nonces, then
    //! check and complete every joint signature.

    use anzen_common::musig;
    use anzen_protocol::{
        address::{self, Network},
        crypto::SoftwareCrypto,
        policy::{Input, Limits, PolicyInputs},
        script::{Scripts, VaultKeys},
        tx::{OutPoint, Txid},
        wire::{self, CooperativeSignature, PolicyRequest, PubNonce},
    };
    use bitcoin::{
        key::{Keypair, Secp256k1},
        secp256k1::{Message, XOnlyPublicKey, schnorr},
    };

    pub fn hot(byte: u8, network: Network) -> String {
        let key = Keypair::from_seckey_slice(&Secp256k1::new(), &[byte; 32]).unwrap();
        let script =
            anzen_protocol::script::p2tr_script_pubkey(&key.x_only_public_key().0.serialize());
        address::from_script_pubkey(&mut SoftwareCrypto::new(), &script, network).unwrap()
    }

    pub fn input(byte: u8, sats: u64) -> Input {
        Input {
            outpoint: OutPoint::new(Txid([byte; 32]), 0),
            value_sats: sats,
        }
    }

    /// A policy request and the phone's secret nonces for it.
    pub fn policy(
        phone: &Keypair,
        keys: VaultKeys,
        network: Network,
        vault: Vec<Input>,
        limits: Limits,
    ) -> (PolicyRequest, Vec<[u8; 64]>, Vec<[u8; 32]>) {
        let mut crypto = SoftwareCrypto::new();
        let scripts = Scripts::new(&mut crypto, &keys).unwrap();
        let plan = anzen_protocol::policy::plan(&PolicyInputs {
            scripts: &scripts,
            vault: &vault,
            controllers: &[],
            limits,
        })
        .unwrap();
        let mut request = PolicyRequest {
            version: wire::VERSION,
            network,
            keys,
            vault_inputs: vault,
            controller_inputs: Vec::new(),
            limits,
            emergency_destination: (limits.emergency_access_limit_sats > 0)
                .then(|| hot(99, network)),
            allowance_destinations: (0..plan.allowance_count)
                .map(|index| hot(index as u8 + 1, network))
                .collect(),
            previous_allowances_complete: false,
            phone_nonces: Vec::new(),
        };
        let graph = request.build(&mut crypto).unwrap().1;
        let messages = graph
            .cooperative_signing_jobs(&mut crypto)
            .unwrap()
            .iter()
            .map(|job| job.sighash)
            .collect::<Vec<_>>();
        let mut secrets = Vec::new();
        for message in &messages {
            let (secret, public) = musig::phone_nonce(
                &phone.secret_bytes(),
                &keys,
                &scripts.vault.merkle_root,
                message,
            )
            .unwrap();
            secrets.push(secret);
            request.phone_nonces.push(public);
        }
        (request, secrets, messages)
    }

    /// Complete every joint signature with the phone's half, and verify it as Bitcoin would:
    /// a BIP340 signature under the vault's output key.
    pub fn complete_all(
        phone: &Keypair,
        keys: &VaultKeys,
        messages: &[[u8; 32]],
        secrets: &[[u8; 64]],
        nonces: &[PubNonce],
        halves: &[CooperativeSignature],
    ) {
        let scripts = Scripts::new(&mut SoftwareCrypto::new(), keys).unwrap();
        assert_eq!(messages.len(), halves.len());
        let output = XOnlyPublicKey::from_slice(&scripts.vault.output.output_key).unwrap();
        for (index, message) in messages.iter().enumerate() {
            let signature = musig::complete(
                &phone.secret_bytes(),
                &secrets[index],
                keys,
                &scripts.vault.merkle_root,
                message,
                &nonces[index],
                &halves[index],
            )
            .unwrap();
            Secp256k1::verification_only()
                .verify_schnorr(
                    &schnorr::Signature::from_slice(&signature).unwrap(),
                    &Message::from_digest(*message),
                    &output,
                )
                .unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the Speculos emulator running the Ledger app"]
async fn the_ledger_app_reviews_and_signs_a_full_policy() {
    use anzen_protocol::policy::Limits;
    let mut signer = HardwareSigner::open(Speculos::connect(&address()).await.unwrap())
        .await
        .unwrap();
    let phone = phone();
    let keys = VaultKeys {
        phone: XOnlyKey(phone.x_only_public_key().0.serialize()),
        hww: signer.identity(NETWORK).await.unwrap(),
        recovery_locktime: 1_234_567_890,
    };
    signer
        .pair(&Pair {
            network: NETWORK,
            phone: keys.phone,
            recovery_locktime: keys.recovery_locktime,
        })
        .await
        .unwrap();
    let limits = Limits {
        monthly_limit_sats: 10_000_000,
        emergency_access_limit_sats: 50_000_000,
    };
    let inputs = std::env::var("ANZEN_TEST_INPUTS")
        .ok()
        .and_then(|count| count.parse().ok())
        .unwrap_or(2_u8);
    let vault = (1..=inputs)
        .map(|byte| phone_side::input(byte, 110_000_000))
        .collect();
    let (request, secrets, messages) = phone_side::policy(&phone, keys, NETWORK, vault, limits);
    let (halves, shown) = tokio::join!(
        signer.approve_policy(&request),
        approve_on_screen("Vault balance")
    );
    let halves = halves.unwrap();
    // The device showed what the policy does, computed from the request it rebuilt. The screen
    // wraps long values over several lines.
    let shown = shown.replace(" | ", " ").replace("  ", " ");
    for expected in [
        "0.1000 BTC a month, 12 months funded",
        "0.5000 BTC after 1 week",
        "15 transactions",
    ] {
        assert!(
            human() || shown.contains(expected),
            "{expected:?} not in {shown}"
        );
    }
    phone_side::complete_all(
        &phone,
        &keys,
        &messages,
        &secrets,
        &request.phone_nonces,
        &halves,
    );
}

/// A paired signer and the vault's keys.
async fn paired() -> (HardwareSigner<Speculos>, Keypair, VaultKeys) {
    let mut signer = HardwareSigner::open(Speculos::connect(&address()).await.unwrap())
        .await
        .unwrap();
    let phone = phone();
    let keys = VaultKeys {
        phone: XOnlyKey(phone.x_only_public_key().0.serialize()),
        hww: signer.identity(NETWORK).await.unwrap(),
        recovery_locktime: 1_234_567_890,
    };
    signer
        .pair(&Pair {
            network: NETWORK,
            phone: keys.phone,
            recovery_locktime: keys.recovery_locktime,
        })
        .await
        .unwrap();
    (signer, phone, keys)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the Speculos emulator running the Ledger app"]
async fn the_ledger_app_signs_sweeps_and_its_own_actions_and_honours_a_rejection() {
    use anzen_common::musig;
    use anzen_link::ledger::Rejected;
    use anzen_protocol::{
        crypto::SoftwareCrypto,
        script::Scripts,
        wire::{self, OfflineAction, OfflineActionRequest, SweepRequest},
    };
    use bitcoin::secp256k1::{Message, XOnlyPublicKey, schnorr};

    let (mut signer, phone, keys) = paired().await;
    let mut crypto = SoftwareCrypto::new();
    let scripts = Scripts::new(&mut crypto, &keys).unwrap();
    let destination = phone_side::hot(50, NETWORK);
    let inputs = vec![
        phone_side::input(1, 5_000_000),
        phone_side::input(2, 7_000_000),
    ];

    // Moving all vault funds: a joint signature per input.
    let mut sweep = SweepRequest {
        version: wire::VERSION,
        network: NETWORK,
        keys,
        inputs: inputs.clone(),
        destination: destination.clone(),
        phone_nonces: Vec::new(),
    };
    let (_, spend) = sweep.build(&mut crypto).unwrap();
    let messages = spend.key_path_sighashes(&mut crypto).unwrap();
    let mut secrets = Vec::new();
    for message in &messages {
        let (secret, public) = musig::phone_nonce(
            &phone.secret_bytes(),
            &keys,
            &scripts.vault.merkle_root,
            message,
        )
        .unwrap();
        secrets.push(secret);
        sweep.phone_nonces.push(public);
    }
    // Rejecting on the device signs nothing. A person's taps are not scripted, so this part runs
    // only against the emulator.
    if !human() {
        let (rejected, ()) = tokio::join!(signer.approve_sweep(&sweep), async {
            wait_for("Move vault funds").await;
            tap(100, 550).await;
            // The device asks to confirm the rejection.
            tokio::time::sleep(Duration::from_millis(800)).await;
            let asked = screen().await;
            if asked.contains("Yes") {
                tap(240, 400).await;
            }
        });
        let rejected = rejected.unwrap_err();
        assert!(
            rejected.downcast_ref::<Rejected>().is_some(),
            "{rejected:#}"
        );
    }

    let (halves, shown) = tokio::join!(
        signer.approve_sweep(&sweep),
        approve_on_screen("Move vault funds")
    );
    let shown = shown.replace(" | ", "");
    assert!(human() || shown.contains(&destination), "{shown}");
    assert!(
        human() || shown.contains(&format!("{} sats", spend.fee_sats)),
        "{shown}"
    );
    phone_side::complete_all(
        &phone,
        &keys,
        &messages,
        &secrets,
        &sweep.phone_nonces,
        &halves.unwrap(),
    );

    // The signer's own actions: plain BIP340 signatures through its leaves.
    let hww = XOnlyPublicKey::from_slice(&keys.hww.0).unwrap();
    for (action, title, leaf, inputs) in [
        (
            OfflineAction::HwwRecovery,
            "Recover vault funds",
            scripts.vault.hww_recovery_leaf,
            inputs.clone(),
        ),
        (
            OfflineAction::RevokeActions,
            "Revoke vault actions",
            scripts.controller.hww_leaf,
            vec![phone_side::input(3, 1_000), phone_side::input(4, 1_000)],
        ),
    ] {
        let request = OfflineActionRequest {
            version: wire::VERSION,
            network: NETWORK,
            keys,
            action,
            inputs,
            destination: destination.clone(),
        };
        let (_, spend, _) = request.build(&mut crypto).unwrap();
        let (signatures, _) = tokio::join!(
            signer.approve_offline_action(&request),
            approve_on_screen(title)
        );
        let signatures = signatures.unwrap();
        let messages = spend.sighashes(&mut crypto, &leaf).unwrap();
        assert_eq!(signatures.len(), messages.len());
        for (signature, message) in signatures.iter().zip(messages) {
            Secp256k1::verification_only()
                .verify_schnorr(
                    &schnorr::Signature::from_slice(&signature.0).unwrap(),
                    &Message::from_digest(message),
                    &hww,
                )
                .unwrap();
        }
    }

    // A request for another vault is refused before any review.
    let mut other = sweep.clone();
    other.keys.recovery_locktime += 1;
    let refused = signer.approve_sweep(&other).await.unwrap_err().to_string();
    assert!(refused.contains("vault"), "{refused}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the Speculos emulator running the Ledger app"]
async fn the_ledger_answers_the_apps_own_messages() {
    use anzen_common::{
        HWW_RECOVERY_SECONDS,
        policy::VaultPolicy,
        storage::{PublicDeviceFile, VAULT_CONFIG_VERSION, VaultConfig},
    };
    use anzen_link::ledger::Message;
    use anzen_protocol::{policy::Limits, wire::PolicyApproval};
    use serde_json::json;

    let (mut signer, phone, keys) = paired().await;
    let phone_key = phone.x_only_public_key().0;
    let message = |kind: &str, body: serde_json::Value| Message {
        kind: kind.to_owned(),
        body,
    };

    // The phone says hello, in the app's network names; the signer answers with its identity.
    let hello = message(
        "signer-hello",
        json!({ "network": "regtest", "phone": phone_key.to_string() }),
    );
    let identity = signer.answer(&hello).await.unwrap();
    assert_eq!(identity.kind, "signer-identity");
    let identity: PublicDeviceFile = serde_json::from_value(identity.body).unwrap();
    identity.validate_kind("hww-public-key").unwrap();
    assert_eq!(identity.vault_pubkey, keys.hww.to_hex());
    // Another phone is told the Ledger is taken, before it builds a vault.
    let stranger = message(
        "signer-hello",
        json!({ "network": "regtest", "phone": keys.hww.to_hex() }),
    );
    let refused = signer.answer(&stranger).await.unwrap_err().to_string();
    assert!(
        refused.contains("already protects another vault"),
        "{refused}"
    );

    // The vault the phone builds from both keys.
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
    let accepted = signer.answer(&setup).await.unwrap();
    assert_eq!(accepted.kind, "vault-accepted");
    assert_eq!(accepted.body["vault_address"], policy.address.to_string());
    assert!(accepted.body.get("backup").is_none());
    // A vault whose address does not follow from its keys is refused.
    let mut forged = config.clone();
    forged.vault_address = phone_side::hot(7, NETWORK);
    let forged = message("vault-setup", json!({ "config": forged }));
    assert!(signer.answer(&forged).await.is_err());

    // A policy proposal comes back as the approval the phone activates.
    let limits = Limits {
        monthly_limit_sats: 10_000_000,
        emergency_access_limit_sats: 0,
    };
    let (request, secrets, messages) = phone_side::policy(
        &phone,
        keys,
        NETWORK,
        vec![phone_side::input(1, 110_000_000)],
        limits,
    );
    let proposal = message("policy-proposal", serde_json::to_value(&request).unwrap());
    let (approved, _) = tokio::join!(signer.answer(&proposal), approve_on_screen("Vault balance"));
    let approved = approved.unwrap();
    assert_eq!(approved.kind, "policy-approved");
    let approval: PolicyApproval = serde_json::from_value(approved.body).unwrap();
    assert_eq!(approval.request, request);
    phone_side::complete_all(
        &phone,
        &keys,
        &messages,
        &secrets,
        &request.phone_nonces,
        &approval.signatures,
    );

    // What the Ledger app cannot do yet is refused by name, not left hanging.
    for kind in [
        "backup-proposal",
        "phone-rotation-proposal",
        "recovery-vault-request",
    ] {
        let refused = signer.answer(&message(kind, json!({}))).await.unwrap_err();
        assert!(refused.to_string().contains("cannot"), "{refused}");
    }
}
