use anzen::{
    core::{
        DEFAULT_FEE_RATE_SAT_VB, EMERGENCY_ACCESS_DELAY_SECONDS, HWW_RECOVERY_BLOCKS,
        MONTHLY_ALLOWANCE_DELAY_SECONDS, PHONE_RECOVERY_BLOCKS,
        ceremony::{
            BatchTransaction, PolicyLimits, build_policy_proposal, read_psbt, validate_batch,
        },
        keys::DeviceKeys,
        policy::{ControllerPolicy, VaultPolicy},
        storage::VaultConfig,
        transactions::build_controller_revocation_psbt,
        types::VaultUtxo,
    },
    hot_wallet::HotWallet,
};
use bitcoin::{Address, Amount, Network, OutPoint, TxOut, Txid, hashes::Hash, key::Secp256k1};
use chrono::{TimeZone, Utc};
use serde::Serialize;
use std::path::Path;

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
        version: 1,
        network: "regtest".to_owned(),
        phone_vault_pubkey: phone.vault_pubkey.to_string(),
        hww_vault_pubkey: hww.vault_pubkey.to_string(),
        phone_hot_external_descriptor: hot_external,
        phone_hot_internal_descriptor: hot_internal,
        vault_descriptor: policy.descriptor_string(),
        vault_address: policy.address.to_string(),
        phone_recovery_blocks: PHONE_RECOVERY_BLOCKS,
        hww_recovery_blocks: HWW_RECOVERY_BLOCKS,
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
    transaction: &BatchTransaction,
    batch_dir: &Path,
    purposes: Vec<String>,
) -> VectorTransaction {
    let psbt = read_psbt(&batch_dir.join(&transaction.psbt_file)).unwrap();
    let unsigned = &psbt.unsigned_tx;
    assert_eq!(purposes.len(), unsigned.output.len());

    let inputs = unsigned
        .input
        .iter()
        .zip(&psbt.inputs)
        .enumerate()
        .map(|(index, (input, psbt_input))| VectorInput {
            txid: input.previous_output.txid.to_string(),
            vout: input.previous_output.vout,
            value_sats: psbt_input.witness_utxo.as_ref().unwrap().value.to_sat(),
            sequence: input.sequence.to_consensus_u32(),
            role: if transaction.vault_input_indexes.contains(&(index as u32)) {
                "vault".to_owned()
            } else if transaction
                .controller_input_indexes
                .contains(&(index as u32))
            {
                "controller".to_owned()
            } else {
                panic!("test transaction input has no role")
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
    let dir = tempfile::tempdir().unwrap();
    let secp = Secp256k1::new();
    let phone = DeviceKeys::parse(&secp, PHONE_MNEMONIC).unwrap();
    let hww = DeviceKeys::parse(&secp, HWW_MNEMONIC).unwrap();
    let config = fixed_config(&phone, &hww);
    let mut hot = HotWallet::ephemeral(&phone).unwrap();
    let batch = dir.path().join("batch");
    let now = Utc.with_ymd_and_hms(2026, 8, 3, 12, 0, 0).unwrap();
    let input_sats = 90_000_000;
    let manifest = build_policy_proposal(
        &config,
        &[fake_utxo(&config, input_sats)],
        now,
        PolicyLimits {
            monthly_limit_sats: 10_000_000,
            emergency_access_limit_sats: 50_000_000,
        },
        &batch,
        &phone,
        &mut hot,
    )
    .unwrap();
    assert_eq!(manifest.allowance_count, 3);
    validate_batch(&config, &manifest, &batch).unwrap();

    let emergency = manifest.emergency_access.as_ref().unwrap();
    assert_eq!(emergency.delay_seconds, EMERGENCY_ACCESS_DELAY_SECONDS);
    let mut transactions = vec![vector_transaction(
        "rollover",
        &manifest.rollover,
        &batch,
        vec![
            "allowance-chain:step-1".to_owned(),
            "vault-remainder".to_owned(),
            "monthly-controller:state-1".to_owned(),
            "emergency-controller:trigger".to_owned(),
        ],
    )];
    for (index, allowance) in manifest.allowances.iter().enumerate() {
        let mut authorization_outputs = vec![format!(
            "hot-wallet-monthly-allowance:step-{}",
            allowance.step
        )];
        if index + 1 < manifest.allowance_count {
            authorization_outputs.push(format!("allowance-chain:step-{}", allowance.step + 1));
            authorization_outputs.push(format!("monthly-controller:state-{}", allowance.step + 1));
        }
        transactions.push(vector_transaction(
            &format!("allowance:step-{}:authorization", allowance.step),
            &allowance.authorization,
            &batch,
            authorization_outputs,
        ));
    }
    transactions.extend([
        vector_transaction(
            "emergency:trigger",
            &emergency.trigger,
            &batch,
            vec![
                "emergency-staging".to_owned(),
                "vault-change".to_owned(),
                "emergency-controller:withdrawal".to_owned(),
            ],
        ),
        vector_transaction(
            "emergency:withdrawal",
            &emergency.withdrawal,
            &batch,
            vec!["hot-wallet-emergency-access".to_owned()],
        ),
    ]);

    let controller = ControllerPolicy::new(phone.vault_pubkey, hww.vault_pubkey).unwrap();
    let monthly_connector = VaultUtxo {
        outpoint: manifest.allowances[0].connector.outpoint,
        txout: TxOut {
            value: Amount::from_sat(manifest.connector_value_sats),
            script_pubkey: controller.address.script_pubkey(),
        },
        confirmation_height: 1,
    };
    let monthly_revoke_destination = hot.next_change_address().unwrap().script_pubkey();
    let (monthly_revoke, monthly_revoke_fee) = build_controller_revocation_psbt(
        &[monthly_connector],
        monthly_revoke_destination,
        DEFAULT_FEE_RATE_SAT_VB,
        &controller,
    )
    .unwrap();
    let monthly_revoke_meta = BatchTransaction {
        psbt_file: "dynamic-monthly-revocation.psbt".to_owned(),
        unsigned_txid: monthly_revoke.unsigned_tx.compute_txid().to_string(),
        fee_sats: monthly_revoke_fee,
        vault_input_indexes: vec![],
        controller_input_indexes: vec![0],
    };
    anzen::core::ceremony::write_psbt(&batch.join(&monthly_revoke_meta.psbt_file), &monthly_revoke)
        .unwrap();
    transactions.push(vector_transaction(
        "dynamic-revocation:monthly-state-1",
        &monthly_revoke_meta,
        &batch,
        vec!["phone-wallet-change".to_owned()],
    ));
    let vector = VaultOutputGraphVector {
        format: "anzen-vault-output-graph",
        version: 3,
        network: manifest.network.clone(),
        scenario: VectorScenario {
            created_at: now.to_rfc3339(),
            input_sats,
        },
        vault: VectorVault {
            address: manifest.vault_address.clone(),
            descriptor: manifest.vault_descriptor.clone(),
        },
        controller: VectorController {
            address: manifest.controller_address.clone(),
            descriptor: manifest.controller_descriptor.clone(),
            value_sats: manifest.connector_value_sats,
            behavior: "either device may spend; execution rolls state forward; revocation sends change to a normal wallet address and creates no controller output",
        },
        policy: VectorPolicy {
            monthly_limit_sats: manifest.monthly_limit_sats,
            monthly_allowance_delay_seconds: MONTHLY_ALLOWANCE_DELAY_SECONDS,
            emergency_access_limit_sats: manifest.emergency_access_limit_sats,
            emergency_access_delay_seconds: emergency.delay_seconds,
            fee_rate_sat_vb: manifest.fee_rate_sat_vb,
        },
        transactions,
    };
    let actual = format!("{}\n", serde_json::to_string_pretty(&vector).unwrap());
    let expected = include_str!("../test-vectors/vault-output-graph.json");
    assert_eq!(
        actual, expected,
        "vault output graph changed; inspect the diff and intentionally update the JSON vector"
    );
}
