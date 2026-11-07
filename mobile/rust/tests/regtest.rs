//! Real native-facade test. Start compose.playground.yaml; this never uses existing device data.
use anzen_mobile::{Backend, RuntimeSettings, execute};
use anzen_wallet::api::{BitcoinCoreBackend, Network, RpcConfig};
use bitcoincore_rpc::{
    RpcApi,
    bitcoin::{Address, Amount},
};
use serde_json::{Value, json};
use std::{path::Path, str::FromStr};

fn rpc_url() -> String {
    std::env::var("ANZEN_NATIVE_TEST_RPC_URL").unwrap_or_else(|_| "http://127.0.0.1:18443".into())
}

fn call(root: &Path, role: &str, key: u8, command: Value) -> Value {
    try_call(root, role, key, command).unwrap()
}

fn try_call(root: &Path, role: &str, key: u8, command: Value) -> anyhow::Result<Value> {
    execute(
        root,
        role,
        [key; 32],
        &RuntimeSettings {
            dangerously_enable_mainnet: false,
            backend: Backend::Rpc {
                url: rpc_url(),
                user: "anzen".into(),
                password: "anzen".into(),
            },
        },
        &command.to_string(),
    )
}

fn enroll_native_contact(
    phone: &Path,
    phone_key: u8,
    signer: &Path,
    signer_key: u8,
) -> (tempfile::TempDir, Value) {
    let contact = tempfile::tempdir().unwrap();
    let c = |command| call(contact.path(), "wallet", 43, command);
    c(json!({"method":"initialize", "network":"regtest"}));
    let public = c(json!({"method":"generate_recovery_contact", "name":"Alice"}));
    let request = call(
        phone,
        "wallet",
        phone_key,
        json!({"method":"propose_backup_with_contact", "contact":public}),
    );
    let review = call(
        signer,
        "signer",
        signer_key,
        json!({"method":"review_backup", "message":request}),
    );
    assert_eq!(
        review["friend_fingerprints"],
        json!([public["fingerprint"]])
    );
    let approved = call(
        signer,
        "signer",
        signer_key,
        json!({"method":"approve_backup", "review_id":review["review_id"], "message":request}),
    );
    call(
        phone,
        "wallet",
        phone_key,
        json!({"method":"store_backup", "message":approved}),
    );
    (contact, public)
}

fn assert_native_contact_recovers(contact: &Path, public: &Value, backup: Value, expected: &Value) {
    assert_eq!(backup["friends"][0]["fingerprint"], public["fingerprint"]);
    let directory = tempfile::tempdir().unwrap();
    let fresh = |command| call(directory.path(), "wallet", 44, command);
    let c = |command| call(contact, "wallet", 43, command);
    fresh(json!({"method":"initialize", "network":"regtest"}));
    let request =
        fresh(json!({"method":"propose_contact_restore", "backup":backup, "contact":public}));
    let review = c(json!({"method":"review_contact_restore", "request":request}));
    let response = c(
        json!({"method":"approve_contact_restore", "review_id":review["review_id"], "request":request}),
    );
    let state = fresh(json!({"method":"store_contact_restored_key", "message":response}));
    assert_eq!(
        state["config"]["vault_descriptor"],
        expected["vault_descriptor"]
    );
    assert_eq!(
        state["config"]["phone_vault_pubkey"],
        expected["phone_vault_pubkey"]
    );
    assert_eq!(
        state["config"]["hww_vault_pubkey"],
        expected["hww_vault_pubkey"]
    );
}

#[test]
#[ignore = "requires fresh regtest; use mobile's npm run test:native:regtest"]
fn contact_recovers_hot_funds_immediately_without_either_original_device() {
    let node = BitcoinCoreBackend::connect(
        &RpcConfig {
            url: rpc_url(),
            user: "anzen".into(),
            password: "anzen".into(),
        },
        Network::Regtest,
    )
    .unwrap();
    let name = format!("anzen-native-contact-hot-{}", std::process::id());
    node.client
        .create_wallet(&name, None, None, None, None)
        .unwrap();
    let faucet = bitcoincore_rpc::Client::new(
        &format!("{}/wallet/{name}", rpc_url()),
        bitcoincore_rpc::Auth::UserPass("anzen".into(), "anzen".into()),
    )
    .unwrap();
    let miner = faucet
        .get_new_address(None, None)
        .unwrap()
        .require_network(Network::Regtest)
        .unwrap();
    faucet.generate_to_address(102, &miner).unwrap();
    let fund = |destination: &str, sats| {
        let address = Address::from_str(destination)
            .unwrap()
            .require_network(Network::Regtest)
            .unwrap();
        faucet
            .send_to_address(
                &address,
                Amount::from_sat(sats),
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap();
    };
    let (contact, public, backup, old_hot_address) = {
        let phone = tempfile::tempdir().unwrap();
        let signer = tempfile::tempdir().unwrap();
        let p = |command| call(phone.path(), "wallet", 51, command);
        let s = |command| call(signer.path(), "signer", 52, command);
        p(json!({"method":"initialize", "network":"regtest"}));
        let identity = s(json!({"method":"initialize", "network":"regtest"}));
        let config = p(json!({"method":"pair_signer", "signer":identity}));
        s(json!({"method":"accept_vault", "config":config}));
        let (contact, public) = enroll_native_contact(phone.path(), 51, signer.path(), 52);
        let backup = p(json!({"method":"export_backup"}));
        let address = p(json!({"method":"receive_address"}))["address"]
            .as_str()
            .unwrap()
            .to_owned();
        fund(&address, 1_000_000);
        fund(config["vault_address"].as_str().unwrap(), 210_000_000);
        faucet.generate_to_address(1, &miner).unwrap();
        (contact, public, backup, address)
        // Both original device directories are dropped here, before any restore request.
    };
    let replacement = tempfile::tempdir().unwrap();
    let r = |command| call(replacement.path(), "wallet", 53, command);
    let c = |command| call(contact.path(), "wallet", 43, command);
    let identity = r(json!({"method":"initialize", "network":"regtest"}));
    let request = r(json!({"method":"propose_contact_restore", "backup":backup, "contact":public}));
    let review = c(json!({"method":"review_contact_restore", "request":request}));
    let response = c(
        json!({"method":"approve_contact_restore", "review_id":review["review_id"], "request":request}),
    );
    r(json!({"method":"store_contact_restored_key", "message":response}));
    let timing = r(json!({"method":"contact_recovery_status"}));
    assert_eq!(timing["balance_sats"], 210_000_000);
    assert!(timing["blocks_remaining"].as_u64().unwrap() > 61_000);
    let cancelled = r(json!({"method":"review_recovered_wallet"}));
    r(json!({"method":"reject_recovered_wallet", "review_id":cancelled["review_id"]}));
    assert!(
        try_call(
            replacement.path(),
            "wallet",
            53,
            json!({"method":"confirm_recovered_wallet", "review_id":cancelled["review_id"]})
        )
        .is_err()
    );
    let mut received = 0;
    for sats in [1_000_000, 200_000] {
        if sats == 200_000 {
            fund(&old_hot_address, sats);
            faucet.generate_to_address(1, &miner).unwrap();
        }
        let review = r(json!({"method":"review_recovered_wallet"}));
        assert_eq!(
            review["amount_sats"].as_u64().unwrap() + review["fee_sats"].as_u64().unwrap(),
            sats
        );
        received += review["amount_sats"].as_u64().unwrap();
        let result =
            r(json!({"method":"confirm_recovered_wallet", "review_id":review["review_id"]}));
        let signed = replacement
            .path()
            .join("wallet/phone/recovered-wallet")
            .join(review["review_id"].as_str().unwrap())
            .join("signed.json");
        assert!(
            std::fs::read_to_string(signed)
                .unwrap()
                .contains("anzen_native_storage")
        );
        faucet.generate_to_address(1, &miner).unwrap();
        // Simulate the local receipt being unavailable after Core accepted and confirmed
        // the transaction. The retained signed copy must also retry against a real node.
        std::fs::remove_file(
            replacement
                .path()
                .join("wallet/phone/recovered-wallet")
                .join(review["review_id"].as_str().unwrap())
                .join("broadcast.json"),
        )
        .unwrap();
        assert_eq!(
            result,
            r(json!({"method":"confirm_recovered_wallet", "review_id":review["review_id"]}))
        );
        assert!(
            try_call(
                replacement.path(),
                "wallet",
                53,
                json!({"method":"reject_recovered_wallet", "review_id":review["review_id"]})
            )
            .is_err()
        );
        assert_eq!(
            r(json!({"method":"wallet_snapshot"}))["confirmed_sats"],
            received
        );
        assert_eq!(r(json!({"method":"identity"})), identity);
    }
    assert!(
        try_call(
            replacement.path(),
            "wallet",
            53,
            json!({"method":"review_recovered_wallet"})
        )
        .unwrap_err()
        .to_string()
        .contains("no spendable funds")
    );
    assert_eq!(
        r(json!({"method":"contact_recovery_status"}))["balance_sats"],
        210_000_000
    );
    node.client.unload_wallet(Some(&name)).unwrap();
}

#[test]
#[ignore = "requires fresh regtest and mines 61,200 blocks; use npm run test:native:regtest"]
fn surviving_native_phone_replaces_lost_signer_after_real_csv_delay() {
    use anzen_wallet::api::Blockchain;
    let node = BitcoinCoreBackend::connect(
        &RpcConfig {
            url: rpc_url(),
            user: "anzen".into(),
            password: "anzen".into(),
        },
        Network::Regtest,
    )
    .unwrap();
    let name = format!("anzen-native-replacement-{}", std::process::id());
    node.client
        .create_wallet(&name, None, None, None, None)
        .unwrap();
    let faucet = bitcoincore_rpc::Client::new(
        &format!("{}/wallet/{name}", rpc_url()),
        bitcoincore_rpc::Auth::UserPass("anzen".into(), "anzen".into()),
    )
    .unwrap();
    let miner = faucet
        .get_new_address(None, None)
        .unwrap()
        .require_network(Network::Regtest)
        .unwrap();
    faucet.generate_to_address(102, &miner).unwrap();
    let phone = tempfile::tempdir().unwrap();
    let old_signer = tempfile::tempdir().unwrap();
    let new_signer = tempfile::tempdir().unwrap();
    let p = |command| call(phone.path(), "wallet", 11, command);
    let old = |command| call(old_signer.path(), "signer", 12, command);
    let new = |command| call(new_signer.path(), "signer", 13, command);
    let phone_identity = p(json!({"method":"initialize", "network":"regtest"}));
    let old_identity = old(json!({"method":"initialize", "network":"regtest"}));
    let config = p(json!({"method":"pair_signer", "signer":old_identity}));
    old(json!({"method":"accept_vault", "config":config}));
    let address = Address::from_str(config["vault_address"].as_str().unwrap())
        .unwrap()
        .require_network(Network::Regtest)
        .unwrap();
    faucet
        .send_to_address(
            &address,
            Amount::from_sat(210_000_000),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
    faucet.generate_to_address(1, &miner).unwrap();
    let policy = p(
        json!({"method":"propose_policy", "monthly_limit_sats":8_000_000, "emergency_access_limit_sats":25_000_000}),
    );
    let review = old(json!({"method":"review_policy", "request":policy}));
    let signed = old(json!({"method":"approve_policy", "review_id":review["review_id"]}));
    p(json!({"method":"activate_policy", "approval":signed}));
    faucet.generate_to_address(1, &miner).unwrap();
    let (contact, public_contact) = enroll_native_contact(phone.path(), 11, old_signer.path(), 12);
    // A later deposit must not postpone the older savings' recovery window.
    faucet.generate_to_address(5, &miner).unwrap();
    faucet
        .send_to_address(
            &address,
            Amount::from_sat(100_000),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
    faucet.generate_to_address(1, &miner).unwrap();
    std::fs::remove_file(old_signer.path().join("signer/hww/device.json")).unwrap();
    let identity = new(json!({"method":"initialize", "network":"regtest"}));
    let candidate = p(json!({"method":"inspect_signer_replacement", "signer":identity}));
    assert_eq!(
        candidate["total_sats"].as_u64().unwrap() - candidate["selected_sats"].as_u64().unwrap(),
        100_000
    );
    let request = p(json!({"method":"propose_signer_replacement", "signer":identity}));
    let review = new(json!({"method":"review_signer_replacement", "request":request}));
    assert_eq!(
        review["friend_fingerprints"],
        json!([public_contact["fingerprint"]])
    );
    let approval = new(
        json!({"method":"approve_signer_replacement", "review_id":review["review_id"], "request":request}),
    );
    let rejected = try_call(
        phone.path(),
        "wallet",
        11,
        json!({"method":"activate_signer_replacement", "approval":approval}),
    )
    .unwrap_err();
    assert!(
        rejected
            .to_string()
            .contains("phone recovery is not mature")
    );
    let remaining = candidate["recovery_blocks_remaining"].as_u64().unwrap();
    // Mine to a provably unspendable test script, not a tracked wallet: no 61k wallet writes.
    let discard = Address::p2wsh(
        &bitcoincore_rpc::bitcoin::ScriptBuf::from_bytes(vec![0x6a]),
        Network::Regtest,
    );
    let mut mined = 0;
    while mined < remaining - 1 {
        let count = (remaining - 1 - mined).min(500);
        let tip = node.chain_tip().unwrap();
        let header = node
            .client
            .get_block_header_info(&tip.best_block_hash)
            .unwrap();
        node.set_mock_time(u64::try_from(header.time).unwrap() + count + 1)
            .unwrap();
        node.mine(count, &discard).unwrap();
        mined += count;
        if mined % 5000 == 0 {
            eprintln!("Native signer replacement: mined {mined}/{remaining} real recovery blocks");
        }
    }
    assert!(
        try_call(
            phone.path(),
            "wallet",
            11,
            json!({"method":"activate_signer_replacement", "approval":approval})
        )
        .unwrap_err()
        .to_string()
        .contains("phone recovery is not mature")
    );
    node.mine(1, &discard).unwrap();
    let activated = p(json!({"method":"activate_signer_replacement", "approval":approval}));
    node.mine(1, &discard).unwrap();
    assert_eq!(
        activated,
        p(json!({"method":"activate_signer_replacement", "approval":approval}))
    );
    assert_eq!(p(json!({"method":"identity"})), phone_identity);
    let snapshot = p(json!({"method":"vault_snapshot"}));
    assert_eq!(snapshot["config"]["monthly_limit_sats"], 8_000_000);
    assert_eq!(
        snapshot["config"]["emergency_access_limit_sats"],
        25_000_000
    );
    let old_config = serde_json::from_value(config).unwrap();
    let leftovers = node.scan_vault(&old_config).unwrap();
    assert_eq!(leftovers.len(), 1);
    assert_eq!(leftovers[0].txout.value.to_sat(), 100_000);
    let tracked = p(json!({"method":"previous_vaults"}));
    let retired = &tracked[0];
    assert_eq!(retired["phone"]["total_sats"], 100_000);
    let archived = std::fs::read(
        phone
            .path()
            .join("wallet/phone/previous-vaults")
            .join(format!("{}.json", retired["vault_id"].as_str().unwrap())),
    )
    .unwrap();
    assert!(!String::from_utf8_lossy(&archived).contains(address.to_string().as_str()));
    let delayed =
        p(json!({"method":"review_previous_vault_recovery", "vault_id":retired["vault_id"]}));
    assert_eq!(delayed["destination"], snapshot["config"]["vault_address"]);
    assert!(
        try_call(
            phone.path(),
            "wallet",
            11,
            json!({"method":"confirm_previous_vault_recovery", "review_id":delayed["review_id"]})
        )
        .unwrap_err()
        .to_string()
        .contains("not mature")
    );
    let later_remaining = delayed["blocks_remaining"].as_u64().unwrap();
    assert!(later_remaining > 1);
    node.mine(later_remaining - 1, &discard).unwrap();
    assert!(
        try_call(
            phone.path(),
            "wallet",
            11,
            json!({"method":"confirm_previous_vault_recovery", "review_id":delayed["review_id"]})
        )
        .unwrap_err()
        .to_string()
        .contains("not mature")
    );
    node.mine(1, &discard).unwrap();
    let recovered =
        p(json!({"method":"confirm_previous_vault_recovery", "review_id":delayed["review_id"]}));
    assert_eq!(
        recovered,
        p(json!({"method":"confirm_previous_vault_recovery", "review_id":delayed["review_id"]}))
    );
    node.mine(1, &discard).unwrap();
    assert!(node.scan_vault(&old_config).unwrap().is_empty());
    assert_eq!(
        p(json!({"method":"previous_vaults"}))[0]["phone"]["total_sats"],
        0
    );
    new(json!({"method":"adopt_signer_replacement", "request_id":activated["request_id"]}));
    assert_eq!(
        new(json!({"method":"export_recovery_bundle"}))["config"],
        snapshot["config"]
    );
    assert_native_contact_recovers(
        contact.path(),
        &public_contact,
        p(json!({"method":"export_backup"})),
        &snapshot["config"],
    );
    // The fresh backup really decrypts under the new signer, without the lost signer key.
    let third_phone = tempfile::tempdir().unwrap();
    let restored = |command| call(third_phone.path(), "wallet", 14, command);
    restored(json!({"method":"initialize", "network":"regtest"}));
    restored(
        json!({"method":"import_recovery_bundle", "bundle":new(json!({"method":"export_recovery_bundle"}))}),
    );
    let restore = restored(
        json!({"method":"propose_restore", "backup":p(json!({"method":"export_backup"}))}),
    );
    let review = new(json!({"method":"review_restore", "request":restore}));
    let response = new(json!({"method":"approve_restore", "review_id":review["review_id"]}));
    assert_eq!(
        restored(json!({"method":"store_restored_key", "message":response}))["old_phone_public_key"],
        phone_identity["vault_pubkey"]
    );
    node.client.unload_wallet(Some(&name)).unwrap();
}

#[test]
#[ignore = "requires fresh regtest; use mobile's npm run test:native:regtest"]
fn encrypted_native_devices_complete_real_policy_and_payment_reviews() {
    let node = BitcoinCoreBackend::connect(
        &RpcConfig {
            url: rpc_url(),
            user: "anzen".into(),
            password: "anzen".into(),
        },
        Network::Regtest,
    )
    .unwrap();
    let wallet_name = format!("anzen-native-test-{}", std::process::id());
    node.client
        .create_wallet(&wallet_name, None, None, None, None)
        .unwrap();
    let faucet = bitcoincore_rpc::Client::new(
        &format!("{}/wallet/{wallet_name}", rpc_url()),
        bitcoincore_rpc::Auth::UserPass("anzen".into(), "anzen".into()),
    )
    .unwrap();
    let miner = faucet
        .get_new_address(None, None)
        .unwrap()
        .require_network(Network::Regtest)
        .unwrap();
    faucet.generate_to_address(102, &miner).unwrap();
    let phone = tempfile::tempdir().unwrap();
    let signer = tempfile::tempdir().unwrap();
    let p = |command| call(phone.path(), "wallet", 1, command);
    let s = |command| call(signer.path(), "signer", 2, command);
    p(json!({"method":"initialize", "network":"regtest"}));
    let identity = s(json!({"method":"initialize", "network":"regtest"}));
    let config = p(json!({"method":"pair_signer", "signer":identity}));
    s(json!({"method":"accept_vault", "config":config}));
    let (contact, public_contact) = enroll_native_contact(phone.path(), 1, signer.path(), 2);
    let cloud_backup = p(json!({"method":"export_backup"}));
    assert_eq!(cloud_backup["kind"], "vault-cloud-recovery");
    let vault_address = Address::from_str(config["vault_address"].as_str().unwrap())
        .unwrap()
        .require_network(Network::Regtest)
        .unwrap();
    faucet
        .send_to_address(
            &vault_address,
            Amount::from_sat(210_000_000),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
    faucet.generate_to_address(1, &miner).unwrap();
    let package = p(
        json!({"method":"propose_policy", "monthly_limit_sats":10_000_000, "emergency_access_limit_sats":50_000_000}),
    );
    let review = s(json!({"method":"review_policy", "request":package}));
    let approved = s(json!({"method":"approve_policy", "review_id":review["review_id"]}));
    p(json!({"method":"activate_policy", "approval":approved}));
    faucet.generate_to_address(1, &miner).unwrap();
    assert_eq!(
        p(json!({"method":"vault_snapshot"}))["config"]["monthly_limit_sats"],
        10_000_000
    );

    let receive = p(json!({"method":"receive_address"}));
    let address = Address::from_str(receive["address"].as_str().unwrap())
        .unwrap()
        .require_network(Network::Regtest)
        .unwrap();
    faucet
        .send_to_address(
            &address,
            Amount::from_sat(1_000_000),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
    faucet.generate_to_address(1, &miner).unwrap();
    let payment = p(json!({"method":"review_payment", "destination":miner, "amount_sats":50_000}));
    assert_eq!(payment["amount_sats"], 50_000);
    assert!(payment["fee_sats"].as_u64().unwrap() > 0);
    let pending_path = phone.path().join(format!(
        "wallet/phone/payment-reviews/{}.json",
        payment["review_id"].as_str().unwrap()
    ));
    assert!(
        std::fs::read_to_string(pending_path)
            .unwrap()
            .contains("anzen_native_storage")
    );
    let result = p(json!({"method":"confirm_payment", "review_id":payment["review_id"]}));
    faucet.generate_to_address(1, &miner).unwrap();
    assert_eq!(
        result,
        p(json!({"method":"confirm_payment", "review_id":payment["review_id"]}))
    ); // Idempotent, no second payment.
    let cancelled =
        p(json!({"method":"review_payment", "destination":miner, "amount_sats":10_000}));
    p(json!({"method":"reject_payment", "review_id":cancelled["review_id"]}));
    assert!(
        execute(
            phone.path(),
            "wallet",
            [1; 32],
            &RuntimeSettings {
                dangerously_enable_mainnet: false,
                backend: Backend::Rpc {
                    url: rpc_url(),
                    user: "anzen".into(),
                    password: "anzen".into()
                },
            },
            &json!({"method":"confirm_payment", "review_id":cancelled["review_id"]}).to_string()
        )
        .is_err()
    );
    // The replacement phone coordinates revocation with public data only. Its key is unrelated
    // to the original phone; neither signer nor replacement can read the original phone root.
    let replacement = tempfile::tempdir().unwrap();
    let r = |command| call(replacement.path(), "wallet", 3, command);
    let replacement_identity = r(json!({"method":"initialize", "network":"regtest"}));
    let recovery_config = s(json!({"method":"export_vault"}));
    r(json!({"method":"import_recovery_vault", "config":recovery_config}));
    let restore = r(json!({"method":"propose_restore", "backup":cloud_backup}));
    let review = s(json!({"method":"review_restore", "request":restore}));
    assert_eq!(
        review["recipient_public_key"],
        replacement_identity["vault_pubkey"]
    );
    let unlocked = s(json!({"method":"approve_restore", "review_id":review["review_id"]}));
    let restored = r(json!({"method":"store_restored_key", "message":unlocked}));
    assert_eq!(
        restored["old_phone_public_key"],
        config["phone_vault_pubkey"]
    );
    assert_eq!(restored["requires_rotation"], true);
    assert_eq!(r(json!({"method":"identity"})), replacement_identity);
    assert_eq!(r(json!({"method":"restored_phone"})), restored);
    let destination = r(json!({"method":"receive_address"}))["address"].clone();
    let revoke = r(
        json!({"method":"propose_offline_action", "action":"revoke_actions", "destination":destination}),
    );
    let review = s(json!({"method":"review_offline_action", "request":revoke}));
    assert_eq!(review["input_count"], 2);
    assert_eq!(
        review["amount_sats"].as_u64().unwrap() + review["fee_sats"].as_u64().unwrap(),
        2000
    );
    let approved = s(json!({"method":"approve_offline_action", "review_id":review["review_id"]}));
    let revoked = r(json!({"method":"activate_offline_action", "approval":approved}));
    faucet.generate_to_address(1, &miner).unwrap();
    assert_eq!(
        revoked,
        r(json!({"method":"activate_offline_action", "approval":approved}))
    );
    assert_eq!(
        r(json!({"method":"wallet_snapshot"}))["confirmed_sats"],
        review["amount_sats"]
    );
    assert!(
        node.scan_connectors(&serde_json::from_value(config).unwrap())
            .unwrap()
            .is_empty()
    );
    let bundle = s(json!({"method":"export_recovery_bundle"}));
    r(json!({"method":"import_recovery_bundle", "bundle":bundle}));
    let rotation = r(json!({"method":"propose_restored_rotation"}));
    let review = s(json!({"method":"review_phone_rotation", "request":rotation}));
    assert_eq!(
        review["friend_fingerprints"],
        json!([public_contact["fingerprint"]])
    );
    assert_eq!(review["monthly_limit_sats"], 10_000_000);
    assert_eq!(review["emergency_access_limit_sats"], 50_000_000);
    let approval = s(
        json!({"method":"approve_phone_rotation", "review_id":review["review_id"], "request":rotation}),
    );
    let rotated = r(json!({"method":"activate_rotation", "approval":approval}));
    faucet.generate_to_address(1, &miner).unwrap();
    assert_eq!(
        rotated,
        r(json!({"method":"activate_rotation", "approval":approval}))
    );
    assert_eq!(r(json!({"method":"identity"})), replacement_identity);
    let snapshot = r(json!({"method":"vault_snapshot"}));
    assert_eq!(snapshot["config"]["monthly_limit_sats"], 10_000_000);
    assert_eq!(
        snapshot["config"]["emergency_access_limit_sats"],
        50_000_000
    );
    assert!(snapshot["balance_sats"].as_u64().unwrap() > 200_000_000);
    assert_eq!(snapshot["recovery_backup_stored"], true);
    assert_native_contact_recovers(
        contact.path(),
        &public_contact,
        r(json!({"method":"export_backup"})),
        &snapshot["config"],
    );
    assert!(
        node.scan_vault(&serde_json::from_value(bundle["config"].clone()).unwrap())
            .unwrap()
            .is_empty()
    );
    s(json!({"method":"adopt_phone_rotation", "request_id":rotated["request_id"]}));
    assert_eq!(
        s(json!({"method":"export_recovery_bundle"}))["config"],
        snapshot["config"]
    );
    let transfer = r(json!({"method":"review_recovered_wallet"}));
    assert!(transfer["amount_sats"].as_u64().unwrap() > 900_000);
    let recovered =
        r(json!({"method":"confirm_recovered_wallet", "review_id":transfer["review_id"]}));
    faucet.generate_to_address(1, &miner).unwrap();
    assert_eq!(
        recovered,
        r(json!({"method":"confirm_recovered_wallet", "review_id":transfer["review_id"]}))
    );
    assert_eq!(p(json!({"method":"wallet_snapshot"}))["confirmed_sats"], 0);
    assert!(
        r(json!({"method":"wallet_snapshot"}))["confirmed_sats"]
            .as_u64()
            .unwrap()
            > 900_000
    );
    node.client.unload_wallet(Some(&wallet_name)).unwrap();
}
