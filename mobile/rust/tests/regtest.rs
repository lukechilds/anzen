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
            network: None,
            backend: Backend::Rpc {
                url: rpc_url(),
                user: "anzen".into(),
                password: "anzen".into(),
            },
        },
        &command.to_string(),
    )
}

/// The backup ceremony: the signer wraps the key to the phone's encrypted cloud backup.
fn back_up(phone: &Path, phone_key: u8, signer: &Path, signer_key: u8) {
    let request = call(
        phone,
        "wallet",
        phone_key,
        json!({"method":"propose_backup"}),
    );
    let review = call(
        signer,
        "signer",
        signer_key,
        json!({"method":"review_backup", "message":request}),
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
}

#[test]
#[ignore = "requires fresh regtest; use mobile's npm run test:native:regtest"]
fn restored_phone_recovers_hot_funds_immediately_while_savings_stay_locked() {
    let node = BitcoinCoreBackend::connect(
        &RpcConfig {
            url: rpc_url(),
            user: "anzen".into(),
            password: "anzen".into(),
        },
        Network::Regtest,
    )
    .unwrap();
    let name = format!("anzen-native-restore-hot-{}", std::process::id());
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
    let signer = tempfile::tempdir().unwrap();
    let s = |command| call(signer.path(), "signer", 52, command);
    let (config, backup, old_hot_address) = {
        let phone = tempfile::tempdir().unwrap();
        let p = |command| call(phone.path(), "wallet", 51, command);
        p(json!({"method":"initialize", "network":"regtest"}));
        let identity = s(json!({"method":"initialize", "network":"regtest"}));
        let config = p(json!({"method":"pair_signer", "signer":identity}));
        s(json!({"method":"accept_vault", "config":config}));
        back_up(phone.path(), 51, signer.path(), 52);
        let backup = p(json!({"method":"export_backup"}));
        let address = p(json!({"method":"receive_address"}))["address"]
            .as_str()
            .unwrap()
            .to_owned();
        fund(&address, 1_000_000);
        fund(config["vault_address"].as_str().unwrap(), 210_000_000);
        faucet.generate_to_address(1, &miner).unwrap();
        (config, backup, address)
        // The original phone's directory is dropped here, before any restore request.
    };
    let replacement = tempfile::tempdir().unwrap();
    let r = |command| call(replacement.path(), "wallet", 53, command);
    let identity = r(json!({"method":"initialize", "network":"regtest"}));
    r(json!({"method":"import_recovery_vault", "config":config}));
    let request = r(json!({"method":"propose_restore", "backup":backup}));
    let review = s(json!({"method":"review_restore", "request":request}));
    let response = s(json!({"method":"approve_restore", "review_id":review["review_id"]}));
    r(json!({"method":"store_restored_key", "message":response}));
    let timing = r(json!({"method":"recovery_snapshot"}));
    assert_eq!(timing["hww"]["total_sats"], 210_000_000);
    assert_eq!(timing["hww"]["mature_sats"], 0);
    assert!(timing["hww"]["seconds_remaining"].as_u64().unwrap() > 388 * 86_400);
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
        r(json!({"method":"recovery_snapshot"}))["hww"]["total_sats"],
        210_000_000
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
    back_up(phone.path(), 1, signer.path(), 2);
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
                network: None,
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
    assert_eq!(r(json!({"method":"export_backup"}))["version"], 2);
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
