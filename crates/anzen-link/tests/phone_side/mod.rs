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
    let script = anzen_protocol::script::p2tr_script_pubkey(&key.x_only_public_key().0.serialize());
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
        emergency_destination: (limits.emergency_access_limit_sats > 0).then(|| hot(99, network)),
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
