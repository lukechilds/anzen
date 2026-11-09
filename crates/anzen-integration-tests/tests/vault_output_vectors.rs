use anzen_common::DEFAULT_FEE_RATE_SAT_VB;
use anzen_common::EMERGENCY_ACCESS_DELAY_SECONDS;
use anzen_common::HWW_RECOVERY_SECONDS;
use anzen_common::MONTHLY_ALLOWANCE_DELAY_SECONDS;
use anzen_common::keys::DeviceKeys;
use anzen_common::policy::{ControllerPolicy, VaultPolicy};
use anzen_common::protocol::{self, SoftwareCrypto};
use anzen_common::storage::VaultConfig;
use anzen_common::types::VaultUtxo;
use anzen_protocol::{policy::GraphTransaction, spend, tx::SpendPath};
use anzen_wallet::HotWallet;
use anzen_wallet::ceremony::{self, PolicyLimits};
use bitcoin::{Address, Amount, Network, OutPoint, TxOut, Txid, hashes::Hash, key::Secp256k1};
use chrono::{TimeZone, Utc};
use serde::Serialize;

const PHONE_MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const HWW_MNEMONIC: &str =
    "legal winner thank year wave sausage worth useful legal winner thank yellow";

#[derive(Serialize)]
struct VaultOutputGraphVector {
    format: &'static str,
    version: u8,
    network: String,
    scenario: VectorScenario,
    vault: VectorVault,
    controller: VectorController,
    policy: VectorPolicy,
    transactions: Vec<VectorTransaction>,
}

#[derive(Serialize)]
struct VectorScenario {
    created_at: String,
    input_sats: u64,
}

#[derive(Serialize)]
struct VectorVault {
    address: String,
    descriptor: String,
}

#[derive(Serialize)]
struct VectorController {
    address: String,
    descriptor: String,
    value_sats: u64,
    behavior: &'static str,
}

#[derive(Serialize)]
struct VectorPolicy {
    monthly_limit_sats: u64,
    monthly_allowance_delay_seconds: u32,
    emergency_access_limit_sats: u64,
    emergency_access_delay_seconds: u32,
    fee_rate_sat_vb: u64,
}

#[derive(Serialize)]
struct VectorTransaction {
    name: String,
    txid: String,
    version: i32,
    lock_time: u32,
    fee_sats: u64,
    inputs: Vec<VectorInput>,
    outputs: Vec<VectorOutput>,
}

#[derive(Serialize)]
struct VectorInput {
    txid: String,
    vout: u32,
    value_sats: u64,
    sequence: u32,
    role: String,
}

#[derive(Serialize)]
struct VectorOutput {
    vout: u32,
    purpose: String,
    value_sats: u64,
    address: String,
    script_pubkey: String,
}

fn fixed_config(phone: &DeviceKeys, hww: &DeviceKeys) -> VaultConfig {
    let secp = Secp256k1::new();
    let policy = VaultPolicy::new(phone.vault_pubkey, hww.vault_pubkey).unwrap();
    let (hot_external, hot_internal) = phone.hot_descriptors(&secp).unwrap();
    VaultConfig {
        version: anzen_common::storage::VAULT_CONFIG_VERSION,
        network: "regtest".to_owned(),
        phone_vault_pubkey: phone.vault_pubkey.to_string(),
        hww_vault_pubkey: hww.vault_pubkey.to_string(),
        phone_hot_external_descriptor: hot_external,
        phone_hot_internal_descriptor: hot_internal,
        vault_descriptor: policy.descriptor_string(),
        vault_address: policy.address.to_string(),
        hww_recovery_seconds: HWW_RECOVERY_SECONDS,
        recovery_locktime: anzen_common::DEFAULT_RECOVERY_LOCKTIME,
        monthly_limit_sats: 0,
        emergency_access_limit_sats: 0,
    }
}

fn fake_utxo(config: &VaultConfig, sats: u64) -> VaultUtxo {
    VaultUtxo {
        outpoint: OutPoint::new(Txid::all_zeros(), 0),
        txout: TxOut {
            value: Amount::from_sat(sats),
            script_pubkey: VaultPolicy::from_descriptor(&config.vault_descriptor)
                .unwrap()
                .address
                .script_pubkey(),
        },
        confirmation_height: 1,
    }
}

fn vector_transaction(
    name: &str,
    transaction: &GraphTransaction,
    purposes: Vec<String>,
) -> VectorTransaction {
    let unsigned = protocol::transaction(&transaction.transaction).unwrap();
    assert_eq!(purposes.len(), unsigned.output.len());
    let inputs = unsigned
        .input
        .iter()
        .zip(&transaction.prevouts)
        .zip(&transaction.spends)
        .map(|((input, prevout), spend)| VectorInput {
            txid: input.previous_output.txid.to_string(),
            vout: input.previous_output.vout,
            value_sats: prevout.value,
            sequence: input.sequence.to_consensus_u32(),
            role: if spend.is_controller() {
                "controller".to_owned()
            } else {
                "vault".to_owned()
            },
        })
        .collect::<Vec<_>>();
    let outputs = unsigned
        .output
        .iter()
        .zip(purposes)
        .enumerate()
        .map(|(vout, (output, purpose))| VectorOutput {
            vout: vout as u32,
            purpose,
            value_sats: output.value.to_sat(),
            address: Address::from_script(&output.script_pubkey, Network::Regtest)
                .unwrap()
                .to_string(),
            script_pubkey: output.script_pubkey.to_hex_string(),
        })
        .collect::<Vec<_>>();
    let input_sats = inputs.iter().map(|input| input.value_sats).sum::<u64>();
    let output_sats = outputs.iter().map(|output| output.value_sats).sum::<u64>();
    let fee_sats = input_sats.checked_sub(output_sats).unwrap();
    assert_eq!(fee_sats, transaction.fee_sats);

    VectorTransaction {
        name: name.to_owned(),
        txid: unsigned.compute_txid().to_string(),
        version: unsigned.version.0,
        lock_time: unsigned.lock_time.to_consensus_u32(),
        fee_sats,
        inputs,
        outputs,
    }
}

#[test]
fn vault_output_graph_matches_checked_in_json_vector() {
    let secp = Secp256k1::new();
    let phone = DeviceKeys::parse(&secp, PHONE_MNEMONIC).unwrap();
    let hww = DeviceKeys::parse(&secp, HWW_MNEMONIC).unwrap();
    let config = fixed_config(&phone, &hww);
    let mut hot = HotWallet::ephemeral(&phone).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 8, 3, 12, 0, 0).unwrap();
    let input_sats = 90_000_000;
    let request = ceremony::propose(
        &config,
        &[fake_utxo(&config, input_sats)],
        &[],
        PolicyLimits {
            monthly_limit_sats: 10_000_000,
            emergency_access_limit_sats: 50_000_000,
        },
        &mut hot,
        false,
        anzen_wallet::cosigning::Phone {
            data_dir: tempfile::tempdir().unwrap().path(),
            keys: &phone,
        },
    )
    .unwrap();
    let (scripts, graph) = ceremony::graph(&request).unwrap();
    assert_eq!(graph.allowances.len(), 3);

    let emergency = graph.emergency.as_ref().unwrap();
    let mut transactions = vec![vector_transaction(
        "rollover",
        &graph.rollover,
        vec![
            "allowance-chain:step-1".to_owned(),
            "vault-remainder".to_owned(),
            "monthly-controller:state-1".to_owned(),
            "emergency-controller:trigger".to_owned(),
        ],
    )];
    for (index, allowance) in graph.allowances.iter().enumerate() {
        let mut authorization_outputs = vec![format!(
            "hot-wallet-monthly-allowance:step-{}",
            allowance.step
        )];
        if index + 1 < graph.allowances.len() {
            authorization_outputs.push(format!("allowance-chain:step-{}", allowance.step + 1));
            authorization_outputs.push(format!("monthly-controller:state-{}", allowance.step + 1));
        }
        transactions.push(vector_transaction(
            &format!("allowance:step-{}:authorization", allowance.step),
            &allowance.authorization,
            authorization_outputs,
        ));
    }
    transactions.extend([
        vector_transaction(
            "emergency:trigger",
            &emergency.trigger,
            vec![
                "emergency-staging".to_owned(),
                "vault-change".to_owned(),
                "emergency-controller:withdrawal".to_owned(),
            ],
        ),
        vector_transaction(
            "emergency:withdrawal",
            &emergency.withdrawal,
            vec!["hot-wallet-emergency-access".to_owned()],
        ),
    ]);

    let controller = ControllerPolicy::new(phone.vault_pubkey, hww.vault_pubkey).unwrap();
    let monthly_revoke_destination = hot.next_change_address().unwrap().script_pubkey();
    let revocation = spend::controller_revocation(
        &mut SoftwareCrypto::new(),
        &scripts,
        &[anzen_protocol::policy::Input {
            outpoint: graph.allowances[0].connector,
            value_sats: anzen_protocol::consts::CONNECTOR_VALUE_SATS,
        }],
        SpendPath::ControllerPhone,
        monthly_revoke_destination.as_bytes(),
    )
    .unwrap();
    transactions.push(vector_transaction(
        "dynamic-revocation:monthly-state-1",
        &GraphTransaction {
            kind: anzen_protocol::policy::TransactionKind::Rollover,
            transaction: revocation.transaction,
            txid: revocation.txid,
            prevouts: revocation.prevouts,
            spends: revocation.spends,
            fee_sats: revocation.fee_sats,
        },
        vec!["phone-wallet-change".to_owned()],
    ));
    let vector = VaultOutputGraphVector {
        format: "anzen-vault-output-graph",
        version: 3,
        network: config.network.clone(),
        scenario: VectorScenario {
            created_at: now.to_rfc3339(),
            input_sats,
        },
        vault: VectorVault {
            address: config.vault_address.clone(),
            descriptor: config.vault_descriptor.clone(),
        },
        controller: VectorController {
            address: controller.address.to_string(),
            descriptor: controller.descriptor_string(),
            value_sats: anzen_protocol::consts::CONNECTOR_VALUE_SATS,
            behavior: "either device may spend; execution rolls state forward; revocation sends change to a normal wallet address and creates no controller output",
        },
        policy: VectorPolicy {
            monthly_limit_sats: request.limits.monthly_limit_sats,
            monthly_allowance_delay_seconds: MONTHLY_ALLOWANCE_DELAY_SECONDS,
            emergency_access_limit_sats: request.limits.emergency_access_limit_sats,
            emergency_access_delay_seconds: EMERGENCY_ACCESS_DELAY_SECONDS,
            fee_rate_sat_vb: DEFAULT_FEE_RATE_SAT_VB,
        },
        transactions,
    };
    let actual = format!("{}\n", serde_json::to_string_pretty(&vector).unwrap());
    // After an intentional policy change, `ANZEN_UPDATE_VECTORS=1` rewrites the vector.
    if std::env::var_os("ANZEN_UPDATE_VECTORS").is_some() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test-vectors/vault-output-graph.json"
        );
        std::fs::write(path, &actual).unwrap();
        return;
    }
    let expected = include_str!("../../../test-vectors/vault-output-graph.json");
    assert_eq!(
        actual, expected,
        "vault output graph changed; inspect the diff and intentionally update the JSON vector"
    );
}
