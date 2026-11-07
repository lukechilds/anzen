//! Differential tests against rust-bitcoin and Miniscript, which the phone uses to build PSBTs
//! and finalize witnesses. Any divergence here would make one device sign a different message.

use anzen_protocol::{
    address::{self, Network},
    crypto::SoftwareCrypto,
    policy::{self, Destinations, Input, Limits, PolicyInputs},
    script::{Scripts, VaultKeys, XOnlyKey},
    sighash::SighashCache,
    spend,
    tx::{OutPoint, SpendPath, Transaction, TxOut, Txid, minimal_non_dust},
};
use bitcoin::{
    Address, Amount, ScriptBuf, Witness,
    hashes::Hash,
    key::{Keypair, Secp256k1},
    secp256k1::rand::{RngCore, thread_rng},
    sighash::{Prevouts, SighashCache as BitcoinSighashCache, TapSighashType},
    taproot::{LeafVersion, TapLeafHash},
};
use miniscript::{Descriptor, DescriptorPublicKey};
use std::str::FromStr;

const NUMS: &str = "50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0";

fn random_keys() -> VaultKeys {
    let secp = Secp256k1::new();
    let key = || {
        XOnlyKey(
            Keypair::new(&secp, &mut thread_rng())
                .x_only_public_key()
                .0
                .serialize(),
        )
    };
    VaultKeys {
        phone: key(),
        hww: key(),
    }
}

fn descriptors(
    keys: &VaultKeys,
) -> (
    Descriptor<DescriptorPublicKey>,
    Descriptor<DescriptorPublicKey>,
) {
    let phone = keys.phone.to_hex();
    let hww = keys.hww.to_hex();
    let vault = format!(
        "tr({NUMS},{{multi_a(2,{phone},{hww}),{{and_v(v:older(61200),pk({phone})),and_v(v:older(65535),pk({hww}))}}}})"
    );
    let controller = format!("tr({NUMS},{{pk({phone}),pk({hww})}})");
    (
        Descriptor::from_str(&vault).unwrap(),
        Descriptor::from_str(&controller).unwrap(),
    )
}

fn to_bitcoin(transaction: &Transaction) -> bitcoin::Transaction {
    bitcoin::consensus::deserialize(&transaction.serialize()).unwrap()
}

fn random_txid() -> Txid {
    let mut bytes = [0_u8; 32];
    thread_rng().fill_bytes(&mut bytes);
    Txid(bytes)
}

fn hot_destination() -> Vec<u8> {
    let secp = Secp256k1::new();
    let key = Keypair::new(&secp, &mut thread_rng()).x_only_public_key().0;
    ScriptBuf::new_p2tr(&secp, key, None).to_bytes()
}

/// Leaf scripts and control blocks as Miniscript satisfies them, for exact witness sizes.
fn witness_for(keys: &VaultKeys, path: SpendPath) -> Witness {
    let (vault, controller) = descriptors(keys);
    let descriptor = if path.is_controller() {
        controller
    } else {
        vault
    };
    let derived = descriptor
        .derived_descriptor(&Secp256k1::verification_only(), 0)
        .unwrap();
    let Descriptor::Tr(tr) = derived else {
        unreachable!()
    };
    let info = tr.spend_info();
    let (target_key, depth, delay) = match path {
        SpendPath::Cooperative => (None, 1, None),
        SpendPath::PhoneRecovery => (Some(keys.phone), 2, Some(61_200)),
        SpendPath::HwwRecovery => (Some(keys.hww), 2, Some(65_535)),
        SpendPath::ControllerPhone => (Some(keys.phone), 1, None),
        SpendPath::ControllerHww => (Some(keys.hww), 1, None),
    };
    let (_, script) = tr
        .iter_scripts()
        .map(|(leaf_depth, miniscript)| (leaf_depth, miniscript.encode()))
        .find(|(leaf_depth, script)| {
            *leaf_depth == depth
                && target_key
                    .is_none_or(|key| script.as_bytes().windows(32).any(|window| window == key.0))
                && delay.is_none_or(|delay: i64| {
                    let push = bitcoin::script::Builder::new()
                        .push_int(delay)
                        .into_script();
                    script
                        .as_bytes()
                        .windows(push.len())
                        .any(|window| window == push.as_bytes())
                })
        })
        .unwrap();
    let control = info
        .control_block(&(script.clone(), LeafVersion::TapScript))
        .unwrap();
    let mut witness = Witness::new();
    let signatures = if path == SpendPath::Cooperative { 2 } else { 1 };
    for _ in 0..signatures {
        witness.push([0_u8; 64]);
    }
    witness.push(script.as_bytes());
    witness.push(control.serialize());
    witness
}

#[test]
fn scripts_and_addresses_match_miniscript_descriptors() {
    let mut crypto = SoftwareCrypto::new();
    for _ in 0..32 {
        let keys = random_keys();
        let scripts = Scripts::new(&mut crypto, &keys).unwrap();
        let (vault, controller) = descriptors(&keys);
        let secp = Secp256k1::verification_only();
        for (descriptor, script) in [
            (vault, &scripts.vault.script_pubkey),
            (controller, &scripts.controller.script_pubkey),
        ] {
            let derived = descriptor.derived_descriptor(&secp, 0).unwrap();
            assert_eq!(derived.script_pubkey().as_bytes(), script.as_slice());
            for (network, bitcoin_network) in [
                (Network::Mainnet, bitcoin::Network::Bitcoin),
                (Network::Mutinynet, bitcoin::Network::Signet),
                (Network::Regtest, bitcoin::Network::Regtest),
            ] {
                assert_eq!(
                    address::from_script_pubkey(&mut crypto, script, network).unwrap(),
                    derived.address(bitcoin_network).unwrap().to_string()
                );
            }
        }
    }
}

#[test]
fn every_leaf_hash_matches_rust_bitcoin() {
    let mut crypto = SoftwareCrypto::new();
    let keys = random_keys();
    let scripts = Scripts::new(&mut crypto, &keys).unwrap();
    for (path, leaf) in [
        (SpendPath::Cooperative, scripts.vault.cooperative_leaf),
        (SpendPath::PhoneRecovery, scripts.vault.phone_recovery_leaf),
        (SpendPath::HwwRecovery, scripts.vault.hww_recovery_leaf),
        (SpendPath::ControllerPhone, scripts.controller.phone_leaf),
        (SpendPath::ControllerHww, scripts.controller.hww_leaf),
    ] {
        let witness = witness_for(&keys, path);
        let script = ScriptBuf::from_bytes(witness.nth(witness.len() - 2).unwrap().to_vec());
        assert_eq!(
            TapLeafHash::from_script(&script, LeafVersion::TapScript).to_byte_array(),
            leaf,
            "{path:?}"
        );
    }
}

#[test]
fn serialization_vsize_and_sighashes_match_rust_bitcoin() {
    let mut crypto = SoftwareCrypto::new();
    let paths = [
        SpendPath::Cooperative,
        SpendPath::PhoneRecovery,
        SpendPath::HwwRecovery,
        SpendPath::ControllerPhone,
        SpendPath::ControllerHww,
    ];
    for input_count in [1_usize, 2, 3, 7, 253, 260] {
        let keys = random_keys();
        let scripts = Scripts::new(&mut crypto, &keys).unwrap();
        let spends = (0..input_count)
            .map(|index| paths[index % paths.len()])
            .collect::<Vec<_>>();
        let transaction = Transaction {
            input: spends
                .iter()
                .enumerate()
                .map(|(index, _)| anzen_protocol::tx::TxIn {
                    previous_output: OutPoint::new(random_txid(), index as u32),
                    sequence: [u32::MAX, 0xffff_fffd, 61_200, 4_199_367][index % 4],
                })
                .collect(),
            output: vec![
                TxOut {
                    value: 12_345,
                    script_pubkey: hot_destination(),
                },
                TxOut {
                    value: 1_000,
                    script_pubkey: scripts.controller.script_pubkey.clone(),
                },
            ],
        };
        let prevouts = spends
            .iter()
            .enumerate()
            .map(|(index, spend)| TxOut {
                value: 10_000 + index as u64,
                script_pubkey: if spend.is_controller() {
                    scripts.controller.script_pubkey.clone()
                } else {
                    scripts.vault.script_pubkey.clone()
                },
            })
            .collect::<Vec<_>>();

        let mut bitcoin_tx = to_bitcoin(&transaction);
        assert_eq!(
            bitcoin::consensus::serialize(&bitcoin_tx),
            transaction.serialize()
        );
        assert_eq!(
            bitcoin_tx.compute_txid().to_byte_array(),
            transaction.txid(&mut crypto).0
        );

        let bitcoin_prevouts = prevouts
            .iter()
            .map(|prevout| bitcoin::TxOut {
                value: Amount::from_sat(prevout.value),
                script_pubkey: ScriptBuf::from_bytes(prevout.script_pubkey.clone()),
            })
            .collect::<Vec<_>>();
        let cache = SighashCache::new(&mut crypto, &transaction, &prevouts).unwrap();
        let mut bitcoin_cache = BitcoinSighashCache::new(&bitcoin_tx);
        for (index, spend) in spends.iter().enumerate() {
            let witness = witness_for(&keys, *spend);
            let script = ScriptBuf::from_bytes(witness.nth(witness.len() - 2).unwrap().to_vec());
            let leaf = TapLeafHash::from_script(&script, LeafVersion::TapScript);
            let expected = bitcoin_cache
                .taproot_script_spend_signature_hash(
                    index,
                    &Prevouts::All(&bitcoin_prevouts),
                    leaf,
                    TapSighashType::Default,
                )
                .unwrap();
            assert_eq!(
                cache.script_path(&mut crypto, index, &leaf.to_byte_array()),
                expected.to_byte_array()
            );
        }

        for (input, spend) in bitcoin_tx.input.iter_mut().zip(&spends) {
            input.witness = witness_for(&keys, *spend);
        }
        assert_eq!(
            transaction.estimated_vsize(&spends).unwrap(),
            bitcoin_tx.vsize() as u64
        );
    }
}

#[test]
fn addresses_round_trip_through_rust_bitcoin_for_every_standard_type() {
    let mut crypto = SoftwareCrypto::new();
    let secp = Secp256k1::new();
    let keypair = Keypair::new(&secp, &mut thread_rng());
    let public = bitcoin::PublicKey::new(keypair.public_key());
    let compressed = bitcoin::CompressedPublicKey(keypair.public_key());
    let script = ScriptBuf::from_bytes(vec![0x51]);
    for (network, bitcoin_network) in [
        (Network::Mainnet, bitcoin::Network::Bitcoin),
        (Network::Mutinynet, bitcoin::Network::Signet),
        (Network::Regtest, bitcoin::Network::Regtest),
    ] {
        for expected in [
            Address::p2pkh(public, bitcoin_network),
            Address::p2sh(&script, bitcoin_network).unwrap(),
            Address::p2wpkh(&compressed, bitcoin_network),
            Address::p2wsh(&script, bitcoin_network),
            Address::p2tr(&secp, keypair.x_only_public_key().0, None, bitcoin_network),
        ] {
            let text = expected.to_string();
            let decoded = address::script_pubkey(&mut crypto, &text, network).unwrap();
            assert_eq!(decoded, expected.script_pubkey().to_bytes(), "{text}");
            assert_eq!(
                address::from_script_pubkey(&mut crypto, &decoded, network).unwrap(),
                text
            );
            assert_eq!(
                minimal_non_dust(&decoded),
                expected.script_pubkey().minimal_non_dust().to_sat()
            );
        }
        let other = match network {
            Network::Mainnet => Network::Regtest,
            _ => Network::Mainnet,
        };
        let taproot =
            Address::p2tr(&secp, keypair.x_only_public_key().0, None, bitcoin_network).to_string();
        assert!(address::script_pubkey(&mut crypto, &taproot, other).is_err());
    }
}

#[test]
fn policy_signing_jobs_cover_every_vault_input_and_match_rust_bitcoin() {
    let mut crypto = SoftwareCrypto::new();
    let keys = random_keys();
    let scripts = Scripts::new(&mut crypto, &keys).unwrap();
    let vault = (0..4)
        .map(|index| Input {
            outpoint: OutPoint::new(random_txid(), index),
            value_sats: 60_000_000,
        })
        .collect::<Vec<_>>();
    let controllers = [Input {
        outpoint: OutPoint::new(random_txid(), 7),
        value_sats: 1_000,
    }];
    let inputs = PolicyInputs {
        scripts: &scripts,
        vault: &vault,
        controllers: &controllers,
        limits: Limits {
            monthly_limit_sats: 10_000_000,
            emergency_access_limit_sats: 50_000_000,
        },
    };
    let plan = policy::plan(&inputs).unwrap();
    assert_eq!(plan.allowance_count, 12);
    let graph = policy::build(
        &mut crypto,
        &inputs,
        &Destinations {
            emergency: Some(hot_destination()),
            allowances: (0..12).map(|_| hot_destination()).collect(),
        },
    )
    .unwrap();
    let jobs = graph.cooperative_signing_jobs(&mut crypto).unwrap();
    assert_eq!(jobs.len(), 4 + 12 + 2);
    let transactions = graph.transactions();
    for job in jobs {
        let transaction = transactions[job.transaction];
        let bitcoin_tx = to_bitcoin(&transaction.transaction);
        let prevouts = transaction
            .prevouts
            .iter()
            .map(|prevout| bitcoin::TxOut {
                value: Amount::from_sat(prevout.value),
                script_pubkey: ScriptBuf::from_bytes(prevout.script_pubkey.clone()),
            })
            .collect::<Vec<_>>();
        let expected = BitcoinSighashCache::new(&bitcoin_tx)
            .taproot_script_spend_signature_hash(
                job.input,
                &Prevouts::All(&prevouts),
                TapLeafHash::from_byte_array(scripts.vault.cooperative_leaf),
                TapSighashType::Default,
            )
            .unwrap();
        assert_eq!(job.sighash, expected.to_byte_array());
    }
}

#[test]
fn sweeps_pay_exactly_the_fixed_fee_rate() {
    let mut crypto = SoftwareCrypto::new();
    let keys = random_keys();
    let scripts = Scripts::new(&mut crypto, &keys).unwrap();
    let inputs = (0..3)
        .map(|index| Input {
            outpoint: OutPoint::new(random_txid(), index),
            value_sats: 1_000_000,
        })
        .collect::<Vec<_>>();
    for path in [
        SpendPath::Cooperative,
        SpendPath::PhoneRecovery,
        SpendPath::HwwRecovery,
    ] {
        let destination = hot_destination();
        let sweep = spend::vault_sweep(&mut crypto, &scripts, &inputs, path, &destination).unwrap();
        let mut bitcoin_tx = to_bitcoin(&sweep.transaction);
        for input in &mut bitcoin_tx.input {
            input.witness = witness_for(&keys, path);
        }
        assert_eq!(sweep.fee_sats, bitcoin_tx.vsize() as u64);
        assert_eq!(sweep.amount_sats + sweep.fee_sats, 3_000_000);
    }
}
