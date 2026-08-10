use anzen::{
    core::{
        EMERGENCY_ACCESS_DELAY_SECONDS, HWW_RECOVERY_BLOCKS, MONTHLY_ALLOWANCE_DELAY_SECONDS,
        PHONE_RECOVERY_BLOCKS,
        ceremony::{
            BatchTransaction, PolicyLimits, build_policy_proposal, read_psbt, validate_batch,
        },
        keys::DeviceKeys,
        policy::VaultPolicy,
        storage::VaultConfig,
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
        .map(|(input, psbt_input)| VectorInput {
            txid: input.previous_output.txid.to_string(),
            vout: input.previous_output.vout,
            value_sats: psbt_input.witness_utxo.as_ref().unwrap().value.to_sat(),
            sequence: input.sequence.to_consensus_u32(),
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
        ],
    )];
    for (index, allowance) in manifest.allowances.iter().enumerate() {
        let mut authorization_outputs = vec![format!(
            "hot-wallet-monthly-allowance:step-{}",
            allowance.step
        )];
        if index + 1 < manifest.allowance_count {
            authorization_outputs.push(format!("allowance-chain:step-{}", allowance.step + 1));
        }
        transactions.push(vector_transaction(
            &format!("allowance:step-{}:authorization", allowance.step),
            &allowance.authorization,
            &batch,
            authorization_outputs,
        ));
        transactions.push(vector_transaction(
            &format!("allowance:step-{}:revoke-chain", allowance.step),
            &allowance.revocation,
            &batch,
            vec![format!(
                "vault-revocation:step-{}-and-later",
                allowance.step
            )],
        ));
    }
    transactions.extend([
        vector_transaction(
            "emergency:trigger",
            &emergency.trigger,
            &batch,
            vec!["emergency-staging".to_owned(), "vault-change".to_owned()],
        ),
        vector_transaction(
            "emergency:withdrawal",
            &emergency.withdrawal,
            &batch,
            vec!["hot-wallet-emergency-access".to_owned()],
        ),
        vector_transaction(
            "emergency:cancellation",
            &emergency.cancellation,
            &batch,
            vec!["vault-emergency-cancellation".to_owned()],
        ),
    ]);
    let vector = VaultOutputGraphVector {
        format: "anzen-vault-output-graph",
        version: 2,
        network: manifest.network.clone(),
        scenario: VectorScenario {
            created_at: now.to_rfc3339(),
            input_sats,
        },
        vault: VectorVault {
            address: manifest.vault_address.clone(),
            descriptor: manifest.vault_descriptor.clone(),
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
