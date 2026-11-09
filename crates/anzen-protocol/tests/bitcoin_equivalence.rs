//! Differential tests against rust-bitcoin and Miniscript, which the phone uses to build PSBTs
//! and finalize witnesses. Any divergence here would make one device sign a different message.

use anzen_protocol::{
    address::{self, Network},
    consts::{DEFAULT_RECOVERY_LOCKTIME, RECOVERY_LOCKTIMES},
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
    let span = RECOVERY_LOCKTIMES.end - RECOVERY_LOCKTIMES.start;
    VaultKeys {
        phone: key(),
        hww: key(),
        recovery_locktime: RECOVERY_LOCKTIMES.start + thread_rng().next_u32() % span,
    }
}

/// The BIP327 aggregate of both keys (even-y points, sorted), computed with an independent
/// MuSig2 implementation rather than the protocol's own.
fn musig_context(keys: &VaultKeys) -> musig2::KeyAggContext {
    let mut points = [keys.phone, keys.hww]
        .map(|key| musig2::secp::Point::lift_x(key.0).unwrap())
        .to_vec();
    points.sort_by_key(|point| point.serialize());
    musig2::KeyAggContext::new(points).unwrap()
}

fn aggregate_hex(keys: &VaultKeys) -> String {
    let aggregate: musig2::secp::Point = musig_context(keys).aggregated_pubkey();
    bitcoin::hex::DisplayHex::to_lower_hex_string(&aggregate.serialize_xonly()[..])
}

fn descriptors(
    keys: &VaultKeys,
) -> (
    Descriptor<DescriptorPublicKey>,
    Descriptor<DescriptorPublicKey>,
) {
    let phone = keys.phone.to_hex();
    let hww = keys.hww.to_hex();
    let aggregate = aggregate_hex(keys);
    let locktime = keys.recovery_locktime;
    let vault =
        format!("tr({aggregate},and_v(v:after({locktime}),and_v(v:older(4259839),pk({hww}))))");
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

/// Leaf scripts and control blocks as Miniscript satisfies them, for exact witness sizes. The
/// cooperative key path is a single signature.
fn witness_for(keys: &VaultKeys, path: SpendPath) -> Witness {
    if path == SpendPath::Cooperative {
        let mut witness = Witness::new();
        witness.push([0_u8; 64]);
        return witness;
    }
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
        SpendPath::Cooperative => unreachable!("handled above"),
        SpendPath::HwwRecovery => (Some(keys.hww), 0, Some(4_259_839)),
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
    witness.push([0_u8; 64]);
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
                    sequence: [u32::MAX, 0xffff_fffd, 4_259_839, 4_199_367][index % 4],
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
            lock_time: [0, keys.recovery_locktime][input_count % 2],
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
            if *spend == SpendPath::Cooperative {
                let expected = bitcoin_cache
                    .taproot_key_spend_signature_hash(
                        index,
                        &Prevouts::All(&bitcoin_prevouts),
                        TapSighashType::Default,
                    )
                    .unwrap();
                assert_eq!(cache.key_path(&mut crypto, index), expected.to_byte_array());
                continue;
            }
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
            .taproot_key_spend_signature_hash(
                job.input,
                &Prevouts::All(&prevouts),
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
    for path in [SpendPath::Cooperative, SpendPath::HwwRecovery] {
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

#[test]
fn recovery_sweeps_lock_to_the_leaf_locktime_and_joint_sweeps_do_not() {
    let mut crypto = SoftwareCrypto::new();
    let keys = random_keys();
    let scripts = Scripts::new(&mut crypto, &keys).unwrap();
    let inputs = [Input {
        outpoint: OutPoint::new(random_txid(), 0),
        value_sats: 1_000_000,
    }];
    let destination = hot_destination();
    let mut sweep = |path| spend::vault_sweep(&mut crypto, &scripts, &inputs, path, &destination);
    let recovery = to_bitcoin(&sweep(SpendPath::HwwRecovery).unwrap().transaction);
    // CHECKLOCKTIMEVERIFY passes only with a time-based lock time at or past the leaf's, and an
    // input that is not final, which the relative delay already requires.
    assert_eq!(
        recovery.lock_time.to_consensus_u32(),
        keys.recovery_locktime
    );
    assert!(recovery.lock_time.is_block_time());
    assert!(
        recovery
            .input
            .iter()
            .all(|input| input.sequence.enables_absolute_lock_time())
    );
    let joint = to_bitcoin(&sweep(SpendPath::Cooperative).unwrap().transaction);
    assert_eq!(joint.lock_time.to_consensus_u32(), 0);
}

#[test]
fn recovery_locktimes_outside_the_range_are_rejected() {
    let mut crypto = SoftwareCrypto::new();
    let mut keys = random_keys();
    for locktime in [
        0,
        1,
        RECOVERY_LOCKTIMES.start - 1,
        RECOVERY_LOCKTIMES.end,
        u32::MAX,
    ] {
        keys.recovery_locktime = locktime;
        assert_eq!(
            Scripts::new(&mut crypto, &keys).unwrap_err(),
            anzen_protocol::Error::InvalidRecoveryLocktime
        );
    }
    keys.recovery_locktime = DEFAULT_RECOVERY_LOCKTIME;
    let scripts = Scripts::new(&mut crypto, &keys).unwrap();
    let witness = witness_for(&keys, SpendPath::HwwRecovery);
    assert_eq!(witness.nth(witness.len() - 2).unwrap().len(), 47);
    assert_eq!(
        scripts.vault.hww_recovery_leaf,
        TapLeafHash::from_script(
            &ScriptBuf::from_bytes(witness.nth(witness.len() - 2).unwrap().to_vec()),
            LeafVersion::TapScript
        )
        .to_byte_array()
    );
}

#[test]
fn the_internal_key_is_the_bip327_aggregate_and_joint_signatures_verify() {
    use musig2::secp::{Point, Scalar};
    let mut crypto = SoftwareCrypto::new();
    let secp = Secp256k1::new();
    for round in 0..16_u8 {
        // BIP327 aggregates the even-y point of each x-only key, so sign with that point's secret.
        let secret = || {
            let keypair = Keypair::new(&secp, &mut thread_rng());
            let scalar = Scalar::from_slice(&keypair.secret_bytes()).unwrap();
            if scalar.base_point_mul().has_even_y() {
                scalar
            } else {
                -scalar
            }
        };
        let (phone, hww) = (secret(), secret());
        let keys = VaultKeys {
            phone: XOnlyKey(phone.base_point_mul().serialize_xonly()),
            hww: XOnlyKey(hww.base_point_mul().serialize_xonly()),
            recovery_locktime: DEFAULT_RECOVERY_LOCKTIME,
        };
        let scripts = Scripts::new(&mut crypto, &keys).unwrap();
        let untweaked: Point = musig_context(&keys).aggregated_pubkey();
        assert_eq!(scripts.vault.internal_key, untweaked.serialize_xonly());
        let context = musig_context(&keys)
            .with_taproot_tweak(&scripts.vault.merkle_root)
            .unwrap();
        let output: Point = context.aggregated_pubkey();
        assert_eq!(scripts.vault.output.output_key, output.serialize_xonly());

        // A full two-round signature over a key-path message verifies under rust-bitcoin.
        let message = [round; 32];
        let nonces = [phone, hww].map(|key| {
            let mut seed = [0_u8; 32];
            thread_rng().fill_bytes(&mut seed);
            musig2::SecNonceBuilder::new(seed)
                .with_seckey(key)
                .with_message(&message)
                .build()
        });
        let aggregate_nonce =
            musig2::AggNonce::sum(nonces.iter().map(|nonce| nonce.public_nonce()));
        let partials: Vec<musig2::PartialSignature> = [phone, hww]
            .into_iter()
            .zip(nonces)
            .map(|(key, nonce)| {
                musig2::sign_partial(&context, key, nonce, &aggregate_nonce, message).unwrap()
            })
            .collect();
        let signature: musig2::LiftedSignature =
            musig2::aggregate_partial_signatures(&context, &aggregate_nonce, partials, message)
                .unwrap();
        let output_key =
            bitcoin::XOnlyPublicKey::from_slice(&scripts.vault.output.output_key).unwrap();
        secp.verify_schnorr(
            &bitcoin::secp256k1::schnorr::Signature::from_slice(&signature.serialize()).unwrap(),
            &bitcoin::secp256k1::Message::from_digest(message),
            &output_key,
        )
        .unwrap();
    }
}
