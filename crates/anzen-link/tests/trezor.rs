//! The Trezor app in Trezor's emulator, driven as a phone would drive it: Trezor's encrypted
//! protocol over the emulator's socket, the app loaded over it, real requests in, the emulated
//! screen read and its buttons pressed through the debug link, and every signature checked with
//! the host's own code.
//!
//! Needs the emulator running our firmware fork with a wallet loaded, and the app built for it
//! (see `trezor-app/README.md`), so it is ignored by default:
//!
//! ```sh
//! ANZEN_TREZOR_PORT=<port> cargo test -p anzen-link --test trezor -- --ignored
//! ```
//!
//! The emulator's wallet is the recovery phrase of twelve `all`s, the one Trezor's own tests use.

mod phone_side;

use anzen_common::keys::DeviceKeys;
use anzen_link::trezor::{
    AppImage, Emulator, Host, Identity, Memory, Rejected, Slot, Trezor, TrezorSigner,
    debug::{DebugLink, shows},
};
use anzen_protocol::{
    address::Network,
    script::{VaultKeys, XOnlyKey},
};
use anzen_signer::hardware::{
    Pair,
    stateless::{MAX_INPUTS, Sealed},
};
use bitcoin::key::{Keypair, Secp256k1};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

const SEED: &str = "all all all all all all all all all all all all";
const NETWORK: Network = Network::Regtest;

/// One emulator, one screen: the tests take turns.
static EMULATOR: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn port() -> u16 {
    std::env::var("ANZEN_TREZOR_PORT")
        .ok()
        .and_then(|port| port.parse().ok())
        .unwrap_or(anzen_link::trezor::DEFAULT_PORT)
}

/// The app as `cargo app-tool build -m t3w1 -e` leaves it, or the one `ANZEN_TREZOR_APP` names.
fn image() -> AppImage {
    let path = std::env::var_os("ANZEN_TREZOR_APP")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let builds = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../trezor-app/target/artifacts/serialized/anzen.app/0.1.0.0");
            let host = if cfg!(target_os = "macos") {
                "macos-aarch64"
            } else {
                "linux-x86_64"
            };
            std::fs::read_dir(&builds)
                .unwrap_or_else(|_| panic!("build the Trezor app first: {builds:?} is missing"))
                .filter_map(|entry| Some(entry.ok()?.path()))
                .find(|path| {
                    path.extension()
                        .is_some_and(|extension| extension == "tapp")
                        && path.to_string_lossy().contains(host)
                })
                .expect("an emulator build of the Trezor app")
        });
    AppImage::from_build(&path).unwrap()
}

/// What the host carries for the signer, where a test can look at it and change it.
#[derive(Clone, Default)]
struct Carried(Arc<Mutex<HashMap<(Network, Slot), Sealed>>>);

impl Carried {
    fn get(&self, slot: Slot) -> Option<Sealed> {
        self.0.lock().unwrap().get(&(NETWORK, slot)).cloned()
    }

    fn set(&self, slot: Slot, sealed: Option<Sealed>) {
        let mut carried = self.0.lock().unwrap();
        match sealed {
            Some(sealed) => carried.insert((NETWORK, slot), sealed),
            None => carried.remove(&(NETWORK, slot)),
        };
    }
}

impl Memory for Carried {
    fn load(&self, network: Network, slot: Slot) -> anyhow::Result<Option<Sealed>> {
        Ok(self.0.lock().unwrap().get(&(network, slot)).cloned())
    }

    fn store(
        &mut self,
        network: Network,
        slot: Slot,
        sealed: Option<&Sealed>,
    ) -> anyhow::Result<()> {
        let mut carried = self.0.lock().unwrap();
        match sealed {
            Some(sealed) => carried.insert((network, slot), sealed.clone()),
            None => carried.remove(&(network, slot)),
        };
        Ok(())
    }
}

type Signer = TrezorSigner<Emulator, Carried>;

/// A channel to the emulator, paired the way debug firmware allows (no code), with its wallet
/// open.
async fn channel() -> Trezor<Emulator> {
    let mut trezor = Trezor::connect(
        Emulator::connect(port()).await.unwrap(),
        &Identity::generate(),
    )
    .await
    .expect("the Trezor emulator");
    assert!(trezor.can_skip_pairing(), "not debug firmware");
    // Even with no code to type, the Trezor asks the person to allow the pairing.
    let host = Host::default();
    let (skipped, _) = tokio::join!(trezor.skip_pairing(&host), approve("to pair with this"));
    skipped.unwrap();
    trezor.open_wallet().await.unwrap();
    trezor
}

async fn signer() -> (Signer, Carried) {
    let carried = Carried::default();
    let signer = TrezorSigner::open(channel().await, &image(), carried.clone())
        .await
        .unwrap();
    (signer, carried)
}

/// The person at the emulator: reads the screen that shows `title`, and approves or rejects it.
/// Returns what the screen said.
async fn decide(title: &str, approve: bool) -> String {
    let mut debug = DebugLink::connect(port()).await.unwrap();
    let screen = debug.read_review(title).await.unwrap();
    debug.press(approve).await.unwrap();
    screen
}

async fn approve(title: &str) -> String {
    decide(title, true).await
}

/// Fails if the Trezor raises a screen: for calls that must ask nothing.
async fn unasked<T>(call: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(20), call)
        .await
        .expect("the Trezor asked something it should not have")
}

fn phone() -> Keypair {
    Keypair::from_seckey_slice(&Secp256k1::new(), &[0x42; 32]).unwrap()
}

fn key_of(byte: u8) -> XOnlyKey {
    XOnlyKey(
        Keypair::from_seckey_slice(&Secp256k1::new(), &[byte; 32])
            .unwrap()
            .x_only_public_key()
            .0
            .serialize(),
    )
}

/// A signer with a vault set up for [`phone`].
async fn paired() -> (Signer, Carried, Keypair, VaultKeys) {
    let (mut signer, carried) = signer().await;
    let phone = phone();
    let keys = VaultKeys {
        phone: XOnlyKey(phone.x_only_public_key().0.serialize()),
        hww: unasked(signer.identity(NETWORK)).await.unwrap(),
        recovery_locktime: 1_234_567_890,
    };
    let pair = Pair {
        network: NETWORK,
        phone: keys.phone,
        recovery_locktime: keys.recovery_locktime,
    };
    let (paired, _) = tokio::join!(signer.pair(&pair), approve("Set up vault"));
    paired.unwrap();
    (signer, carried, phone, keys)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the Trezor emulator and the Trezor app built for it"]
async fn the_trezor_app_derives_the_vault_key_and_sets_up_a_vault() {
    let _turn = EMULATOR.lock().await;
    let (mut signer, carried) = signer().await;
    // The app derives the same vault key the phone app's signer mode would from this seed.
    let expected =
        DeviceKeys::parse_for_network(&Secp256k1::new(), SEED, bitcoin::Network::Regtest)
            .unwrap()
            .vault_pubkey;
    let identity = unasked(signer.identity(NETWORK)).await.unwrap();
    assert_eq!(identity, XOnlyKey(expected.serialize()));
    // Mainnet's key is another one, and Mutinynet shares the test networks' key.
    let mainnet = unasked(signer.identity(Network::Mainnet)).await.unwrap();
    assert_ne!(mainnet, identity);
    assert_eq!(
        mainnet,
        XOnlyKey(
            DeviceKeys::parse_for_network(&Secp256k1::new(), SEED, bitcoin::Network::Bitcoin)
                .unwrap()
                .vault_pubkey
                .serialize()
        )
    );
    assert_eq!(
        unasked(signer.identity(Network::Mutinynet)).await.unwrap(),
        identity
    );

    let pair = Pair {
        network: NETWORK,
        phone: XOnlyKey(phone().x_only_public_key().0.serialize()),
        recovery_locktime: 1_234_567_890,
    };
    let keys = VaultKeys {
        phone: pair.phone,
        hww: identity,
        recovery_locktime: 1_234_567_890,
    };
    // Nothing is set up until the person says so on the Trezor.
    assert!(signer.state(NETWORK).unwrap().is_none());
    let (rejected, _) = tokio::join!(signer.pair(&pair), decide("Set up vault", false));
    let rejected = rejected.unwrap_err();
    assert!(
        rejected.downcast_ref::<Rejected>().is_some(),
        "{rejected:#}"
    );
    assert!(signer.state(NETWORK).unwrap().is_none());

    // The Trezor shows the vault's address, computed from both keys.
    let (paired, shown) = tokio::join!(signer.pair(&pair), approve("Set up vault"));
    paired.unwrap();
    let address = {
        use anzen_protocol::{address, crypto::SoftwareCrypto, script::Scripts};
        let mut crypto = SoftwareCrypto::new();
        let scripts = Scripts::new(&mut crypto, &keys).unwrap();
        address::from_script_pubkey(&mut crypto, &scripts.vault.script_pubkey, NETWORK).unwrap()
    };
    assert!(shows(&shown, &address), "{address} not in {shown}");
    let state = signer.state(NETWORK).unwrap().expect("a vault");
    assert_eq!(state.keys, keys);
    assert_eq!(state.anchor, None);
    assert!(signer.state(Network::Mainnet).unwrap().is_none());

    // Setting the same vault up again asks nothing and changes nothing.
    let before = carried.get(Slot::Vault);
    unasked(signer.pair(&pair)).await.unwrap();
    assert_eq!(carried.get(Slot::Vault), before);

    // Another phone's vault is a new vault: the Trezor asks again, and the host then carries
    // that one.
    let other = Pair {
        phone: key_of(0x43),
        ..pair.clone()
    };
    let (paired, _) = tokio::join!(signer.pair(&other), approve("Set up vault"));
    paired.unwrap();
    assert_eq!(
        signer.state(NETWORK).unwrap().unwrap().keys.phone,
        other.phone
    );

    // A key that is not on the curve is no vault at all, and asks nothing.
    let invalid = Pair {
        phone: XOnlyKey([0x79; 32]),
        ..pair.clone()
    };
    assert!(unasked(signer.pair(&invalid)).await.is_err());

    // Forgetting is the host's doing alone.
    signer.forget_vault(NETWORK).unwrap();
    assert!(signer.state(NETWORK).unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the Trezor emulator and the Trezor app built for it"]
async fn the_trezor_app_reviews_and_signs_a_full_policy() {
    use anzen_protocol::policy::Limits;
    let _turn = EMULATOR.lock().await;
    let (mut signer, carried, phone, keys) = paired().await;
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
    let before = carried.get(Slot::Vault).unwrap();
    let (halves, shown) = tokio::join!(signer.approve_policy(&request), approve("Vault policy"));
    let halves = halves.unwrap();
    // The Trezor showed what the policy does, computed from the request it rebuilt.
    for expected in [
        "0.1000 BTC a month, 12 months funded",
        "0.5000 BTC after 1 week",
        "None: this is the first",
        "15 transactions",
    ] {
        assert!(shows(&shown, expected), "{expected:?} not in {shown}");
    }
    phone_side::complete_all(
        &phone,
        &keys,
        &messages,
        &secrets,
        &request.phone_nonces,
        &halves,
    );
    // The host now carries the policy the signer approved, under a new seal.
    let after = carried.get(Slot::Vault).unwrap();
    assert_ne!(after, before);
    let state = signer.state(NETWORK).unwrap().unwrap();
    assert_eq!(state.limits, limits);
    assert!(state.anchor.is_some());

    // A policy on other deposits would leave the first one's transactions valid beside its own,
    // so the signer refuses it, unasked.
    let (unrelated, ..) = phone_side::policy(
        &phone,
        keys,
        NETWORK,
        vec![phone_side::input(200, 110_000_000)],
        limits,
    );
    let refused = refused_policy(&mut signer, &unrelated).await;
    assert!(!refused.is_empty());
    assert_eq!(carried.get(Slot::Vault).unwrap(), after);

    // One that spends the same deposits replaces it: only one of the two can ever confirm.
    let (halves, shown) = tokio::join!(signer.approve_policy(&request), approve("Vault policy"));
    assert!(shows(&shown, "Replaced by this one"), "{shown}");
    assert_eq!(halves.unwrap().len(), messages.len());
    let after = carried.get(Slot::Vault).unwrap();

    // Forgetting the policy is the person's call, on the Trezor; then it is the first again.
    let (rejected, _) = tokio::join!(
        signer.forget_policy(NETWORK),
        decide("Forget the last policy?", false)
    );
    assert!(rejected.unwrap_err().downcast_ref::<Rejected>().is_some());
    assert_eq!(carried.get(Slot::Vault).unwrap(), after);
    let (forgotten, _) = tokio::join!(
        signer.forget_policy(NETWORK),
        approve("Forget the last policy?")
    );
    forgotten.unwrap();
    let state = signer.state(NETWORK).unwrap().unwrap();
    assert_eq!(state.anchor, None);
    assert_eq!(state.keys, keys);
    let (halves, shown) = tokio::join!(signer.approve_policy(&request), approve("Vault policy"));
    assert!(shows(&shown, "None: this is the first"), "{shown}");
    assert_eq!(halves.unwrap().len(), messages.len());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the Trezor emulator and the Trezor app built for it"]
async fn the_trezor_app_signs_sweeps_and_its_own_actions_and_honours_a_rejection() {
    use anzen_common::musig;
    use anzen_protocol::{
        crypto::SoftwareCrypto,
        script::Scripts,
        wire::{self, OfflineAction, OfflineActionRequest, SweepRequest},
    };
    use bitcoin::secp256k1::{Message, XOnlyPublicKey, schnorr};

    let _turn = EMULATOR.lock().await;
    let (mut signer, carried, phone, keys) = paired().await;
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
    // Rejecting on the device signs nothing.
    let (rejected, _) = tokio::join!(
        signer.approve_sweep(&sweep),
        decide("Move vault funds", false)
    );
    let rejected = rejected.unwrap_err();
    assert!(
        rejected.downcast_ref::<Rejected>().is_some(),
        "{rejected:#}"
    );

    let before = carried.get(Slot::Vault);
    let (halves, shown) = tokio::join!(signer.approve_sweep(&sweep), approve("Move vault funds"));
    assert!(shows(&shown, &destination), "{shown}");
    assert!(shows(&shown, "0.1199"), "{shown}");
    assert!(
        shows(&shown, &format!("{} sats", spend.fee_sats)),
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
    // A transfer is nothing the signer has to remember.
    assert_eq!(carried.get(Slot::Vault), before);

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
        let (signatures, shown) =
            tokio::join!(signer.approve_offline_action(&request), approve(title));
        assert!(shows(&shown, &destination), "{shown}");
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
    let refused = unasked(signer.approve_sweep(&other))
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("vault"), "{refused}");

    // So is one with more inputs than the device takes.
    let mut large = sweep.clone();
    large.inputs = (0..=MAX_INPUTS as u8)
        .map(|byte| phone_side::input(byte, 1_000_000))
        .collect();
    let refused = unasked(signer.approve_sweep(&large))
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("Too many inputs"), "{refused}");
}

/// Why the signer refused a policy, without asking the person anything.
async fn refused_policy(
    signer: &mut Signer,
    request: &anzen_protocol::wire::PolicyRequest,
) -> String {
    unasked(signer.approve_policy(request))
        .await
        .unwrap_err()
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the Trezor emulator and the Trezor app built for it"]
async fn the_trezor_app_takes_back_only_state_it_sealed() {
    use anzen_protocol::policy::Limits;
    let _turn = EMULATOR.lock().await;
    let (mut signer, carried, phone, keys) = paired().await;
    let limits = Limits {
        monthly_limit_sats: 10_000_000,
        emergency_access_limit_sats: 0,
    };
    let (request, ..) = phone_side::policy(
        &phone,
        keys,
        NETWORK,
        vec![phone_side::input(1, 110_000_000)],
        limits,
    );
    let sealed = carried.get(Slot::Vault).unwrap();
    // With nothing carried, there is no vault.
    carried.set(Slot::Vault, None);
    let refused = refused_policy(&mut signer, &request).await;
    assert!(refused.contains("Set up a vault"), "{refused}");

    // State altered by one bit no longer bears the seal, wherever the bit is.
    for index in [0, sealed.state.len() / 2, sealed.state.len() - 1] {
        let mut altered = sealed.clone();
        altered.state[index] ^= 1;
        carried.set(Slot::Vault, Some(altered));
        let refused = refused_policy(&mut signer, &request).await;
        assert!(refused.contains("not set up with this Trezor"), "{refused}");
    }
    let mut altered = sealed.clone();
    altered.seal[31] ^= 1;
    carried.set(Slot::Vault, Some(altered));
    let refused = refused_policy(&mut signer, &request).await;
    assert!(refused.contains("not set up with this Trezor"), "{refused}");

    // A state the host made up, for these very keys, has no seal at all.
    let invented = Sealed {
        state: sealed.state.clone(),
        seal: [0; 32],
    };
    carried.set(Slot::Vault, Some(invented));
    let refused = refused_policy(&mut signer, &request).await;
    assert!(refused.contains("not set up with this Trezor"), "{refused}");

    // The vault's own state is not a rotation's: it cannot be adopted.
    carried.set(Slot::Rotated, Some(sealed.clone()));
    let refused = unasked(signer.adopt_rotation(NETWORK))
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("not set up with this Trezor"), "{refused}");
    carried.set(Slot::Rotated, None);
    let refused = unasked(signer.adopt_rotation(NETWORK))
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("No approved rotation"), "{refused}");

    // Given back what it sealed, the signer reviews the policy.
    carried.set(Slot::Vault, Some(sealed));
    let (halves, _) = tokio::join!(signer.approve_policy(&request), approve("Vault policy"));
    assert_eq!(halves.unwrap().len(), request.phone_nonces.len());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the Trezor emulator and the Trezor app built for it"]
async fn the_trezor_app_rotates_to_a_new_phone_key_once_the_person_adopts_it() {
    use anzen_common::musig;
    use anzen_protocol::{
        crypto::SoftwareCrypto,
        policy::{Input, Limits},
        script::Scripts,
        tx::{OutPoint, SpendPath},
        wire::{self, RotationRequest},
    };

    let _turn = EMULATOR.lock().await;
    let (mut signer, carried, phone, keys) = paired().await;
    let limits = Limits {
        monthly_limit_sats: 10_000_000,
        emergency_access_limit_sats: 50_000_000,
    };
    let (first, ..) = phone_side::policy(
        &phone,
        keys,
        NETWORK,
        vec![phone_side::input(1, 200_000_000)],
        limits,
    );
    let (approved, _) = tokio::join!(signer.approve_policy(&first), approve("Vault policy"));
    approved.unwrap();

    // The phone is replaced: the old vault's live outputs move to a vault of the new phone key,
    // and the policy is renewed there.
    let mut crypto = SoftwareCrypto::new();
    let previous = first.build(&mut crypto).unwrap().1;
    let new_phone = Keypair::from_seckey_slice(&Secp256k1::new(), &[0x44; 32]).unwrap();
    let new_keys = VaultKeys {
        phone: XOnlyKey(new_phone.x_only_public_key().0.serialize()),
        hww: keys.hww,
        recovery_locktime: keys.recovery_locktime,
    };
    let sweep_inputs = vec![
        Input {
            outpoint: OutPoint::new(previous.rollover.txid, 0),
            value_sats: previous.plan.allowance_value_sats,
        },
        Input {
            outpoint: OutPoint::new(previous.rollover.txid, 1),
            value_sats: previous.plan.remainder_value_sats,
        },
    ];
    let old_scripts = Scripts::new(&mut crypto, &keys).unwrap();
    let new_scripts = Scripts::new(&mut crypto, &new_keys).unwrap();
    let sweep = anzen_protocol::spend::vault_sweep(
        &mut crypto,
        &old_scripts,
        &sweep_inputs,
        SpendPath::Cooperative,
        &new_scripts.vault.script_pubkey,
    )
    .unwrap();
    let (renewed, renewed_secrets, renewed_messages) = phone_side::policy(
        &new_phone,
        new_keys,
        NETWORK,
        vec![Input {
            outpoint: OutPoint::new(sweep.txid, 0),
            value_sats: sweep.amount_sats,
        }],
        limits,
    );
    let sweep_messages = sweep.key_path_sighashes(&mut crypto).unwrap();
    let mut sweep_secrets = Vec::new();
    let mut sweep_nonces = Vec::new();
    for message in &sweep_messages {
        let (secret, public) = musig::phone_nonce(
            &phone.secret_bytes(),
            &keys,
            &old_scripts.vault.merkle_root,
            message,
        )
        .unwrap();
        sweep_secrets.push(secret);
        sweep_nonces.push(public);
    }
    let request = RotationRequest {
        version: wire::VERSION,
        network: NETWORK,
        old_keys: keys,
        new_phone: new_keys.phone,
        new_recovery_locktime: new_keys.recovery_locktime,
        sweep_inputs,
        revoke_connectors: vec![
            Input {
                outpoint: OutPoint::new(previous.rollover.txid, 2),
                value_sats: 1_000,
            },
            Input {
                outpoint: OutPoint::new(previous.rollover.txid, 3),
                value_sats: 1_000,
            },
        ],
        renewed_policy: Some(renewed.clone()),
        previous_allowances_complete: false,
        sweep_nonces: sweep_nonces.clone(),
    };
    let vault_before = carried.get(Slot::Vault).unwrap();
    let (signatures, shown) =
        tokio::join!(signer.approve_rotation(&request), approve("Phone rotation"));
    let signatures = signatures.unwrap();
    assert!(shows(&shown, &new_keys.phone.to_hex()), "{shown}");
    assert_eq!(signatures.revocation.len(), 2);
    phone_side::complete_all(
        &phone,
        &keys,
        &sweep_messages,
        &sweep_secrets,
        &sweep_nonces,
        &signatures.sweep,
    );
    phone_side::complete_all(
        &new_phone,
        &new_keys,
        &renewed_messages,
        &renewed_secrets,
        &renewed.phone_nonces,
        &signatures.policy,
    );

    // Approving changes nothing yet: the old vault is still the signer's vault, and the new one
    // waits, sealed as a rotation's.
    assert_eq!(carried.get(Slot::Vault).unwrap(), vault_before);
    assert_eq!(signer.state(NETWORK).unwrap().unwrap().keys, keys);
    let rotated = carried.get(Slot::Rotated).expect("the rotation's state");
    // A rotation's state does not pass for the vault's.
    carried.set(Slot::Vault, Some(rotated));
    let refused = unasked(signer.approve_policy(&renewed))
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("not set up with this Trezor"), "{refused}");
    carried.set(Slot::Vault, Some(vault_before.clone()));

    // The person adopts it once the rotation has confirmed.
    let (rejected, _) = tokio::join!(
        signer.adopt_rotation(NETWORK),
        decide("Use the rotated vault?", false)
    );
    assert!(rejected.unwrap_err().downcast_ref::<Rejected>().is_some());
    assert_eq!(carried.get(Slot::Vault).unwrap(), vault_before);
    let (adopted, shown) = tokio::join!(
        signer.adopt_rotation(NETWORK),
        approve("Use the rotated vault?")
    );
    adopted.unwrap();
    let new_address = anzen_protocol::address::from_script_pubkey(
        &mut crypto,
        &new_scripts.vault.script_pubkey,
        NETWORK,
    )
    .unwrap();
    assert!(shows(&shown, &new_address), "{shown}");
    let state = signer.state(NETWORK).unwrap().unwrap();
    assert_eq!(state.keys, new_keys);
    assert_eq!(state.limits, limits);
    assert!(state.anchor.is_some());
    assert!(carried.get(Slot::Rotated).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the Trezor emulator and the Trezor app built for it"]
async fn the_trezor_answers_the_apps_own_messages() {
    use anzen_common::{
        HWW_RECOVERY_SECONDS,
        policy::VaultPolicy,
        storage::{PublicDeviceFile, VAULT_CONFIG_VERSION, VaultConfig},
    };
    use anzen_link::trezor::Message;
    use anzen_protocol::{policy::Limits, wire::PolicyApproval};
    use serde_json::json;

    let _turn = EMULATOR.lock().await;
    let (mut signer, _carried) = signer().await;
    let phone = phone();
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
    let identity = unasked(signer.answer(&hello)).await.unwrap();
    assert_eq!(identity.kind, "signer-identity");
    let identity: PublicDeviceFile = serde_json::from_value(identity.body).unwrap();
    identity.validate_kind("hww-public-key").unwrap();
    let keys = VaultKeys {
        phone: XOnlyKey(phone_key.serialize()),
        hww: XOnlyKey::from_hex(&identity.vault_pubkey).unwrap(),
        recovery_locktime: 1_234_567_890,
    };

    // The vault the phone builds from both keys. The person confirms its address on the Trezor.
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
    let (accepted, shown) = tokio::join!(signer.answer(&setup), approve("Set up vault"));
    let accepted = accepted.unwrap();
    assert!(shows(&shown, &policy.address.to_string()), "{shown}");
    assert_eq!(accepted.kind, "vault-accepted");
    assert_eq!(accepted.body["vault_address"], policy.address.to_string());
    assert!(accepted.body.get("backup").is_none());
    // Saying hello again, as the app does whenever it reconnects, asks nothing.
    unasked(signer.answer(&hello)).await.unwrap();
    unasked(signer.answer(&setup)).await.unwrap();
    // A vault whose address does not follow from its keys is refused, unasked.
    let mut forged = config.clone();
    forged.vault_address = phone_side::hot(7, NETWORK);
    let forged = message("vault-setup", json!({ "config": forged }));
    assert!(unasked(signer.answer(&forged)).await.is_err());

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
    let (declined, _) = tokio::join!(signer.answer(&proposal), decide("Vault policy", false));
    assert!(declined.unwrap_err().downcast_ref::<Rejected>().is_some());
    let (approved, _) = tokio::join!(signer.answer(&proposal), approve("Vault policy"));
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

    // What the Trezor app cannot do yet is refused by name, not left hanging.
    for kind in [
        "backup-proposal",
        "phone-rotation-proposal",
        "recovery-vault-request",
    ] {
        let refused = unasked(signer.answer(&message(kind, json!({}))))
            .await
            .unwrap_err();
        assert!(refused.to_string().contains("cannot"), "{refused}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the Trezor emulator and the Trezor app built for it"]
async fn a_phone_pairs_by_code_once_and_reconnects_without_one() {
    let _turn = EMULATOR.lock().await;
    let host = Host::default();
    let mut identity = Identity::generate();
    assert!(!identity.has_paired());

    // A wrong code ends the pairing, and leaves the phone with no credential.
    {
        let mut trezor = Trezor::connect(Emulator::connect(port()).await.unwrap(), &identity)
            .await
            .unwrap();
        assert!(!trezor.is_paired());
        let (entry, _) = tokio::join!(trezor.begin_pairing(&host), approve("to pair with this"));
        let entry = entry.unwrap();
        let (channel, handshake_hash) = trezor.pairing_reference();
        let code = DebugLink::connect(port())
            .await
            .unwrap()
            .pairing_code(channel, &handshake_hash)
            .await
            .unwrap();
        let wrong = format!("{:06}", (code.parse::<u32>().unwrap() + 1) % 1_000_000);
        let refused = trezor
            .finish_pairing(entry, &wrong, &mut identity)
            .await
            .unwrap_err();
        assert!(
            format!("{refused:#}").contains("did not match"),
            "{refused:#}"
        );
        assert!(!identity.has_paired());
    }

    // The code the Trezor shows pairs the phone, and the Trezor issues it a credential.
    {
        let mut trezor = Trezor::connect(Emulator::connect(port()).await.unwrap(), &identity)
            .await
            .unwrap();
        let (entry, _) = tokio::join!(trezor.begin_pairing(&host), approve("to pair with this"));
        let entry = entry.unwrap();
        let (channel, handshake_hash) = trezor.pairing_reference();
        let mut debug = DebugLink::connect(port()).await.unwrap();
        let code = debug.pairing_code(channel, &handshake_hash).await.unwrap();
        // The code is on the Trezor's screen for the person to read.
        let screen = debug.screen().await.unwrap();
        assert!(
            shows(&screen, &code) || shows(&screen, &format!("{} {}", &code[..3], &code[3..])),
            "{code} not in {screen}"
        );
        trezor
            .finish_pairing(entry, &code, &mut identity)
            .await
            .unwrap();
        assert!(identity.has_paired());
        trezor.open_wallet().await.unwrap();
        let mut signer = TrezorSigner::open(trezor, &image(), Carried::default())
            .await
            .unwrap();
        signer.identity(NETWORK).await.unwrap();
    }

    // Later, with the stored identity: no code, straight to work. (A Trezor that no longer
    // remembers the phone as recently paired asks the person to allow the connection, and then
    // to let the phone connect by itself from now on; the emulator always remembers, so those
    // two questions are not exercised here.)
    let stored = serde_json::to_string(&identity).unwrap();
    let mut identity: Identity = serde_json::from_str(&stored).unwrap();
    let mut trezor = Trezor::connect(Emulator::connect(port()).await.unwrap(), &identity)
        .await
        .unwrap();
    assert!(trezor.is_paired());
    unasked(trezor.finish(&mut identity)).await.unwrap();
    unasked(trezor.open_wallet()).await.unwrap();
    let mut signer = unasked(TrezorSigner::open(trezor, &image(), Carried::default()))
        .await
        .unwrap();
    unasked(signer.identity(NETWORK)).await.unwrap();

    // A phone the Trezor has never met is not paired.
    let stranger = Identity::generate();
    let trezor = Trezor::connect(Emulator::connect(port()).await.unwrap(), &stranger)
        .await
        .unwrap();
    assert!(!trezor.is_paired());
}
