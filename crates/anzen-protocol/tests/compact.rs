//! Wire messages have two forms: JSON between phones, browsers and files, and a compact binary
//! form (postcard) for hardware signers with little memory. Both must carry the same message.

use anzen_protocol::{
    address::Network,
    policy::{Input, Limits},
    script::{VaultKeys, XOnlyKey},
    tx::{OutPoint, Txid},
    wire::{CooperativeSignature, PartialSignature, PolicyRequest, PubNonce, Signature, VERSION},
};

fn request(inputs: u8, allowances: u8) -> PolicyRequest {
    let address = "bcrt1p8wpt9v4frpf3tkn0srd97pksgsxc5hs52lafxwru9kgeephvs7rqjeprhg";
    PolicyRequest {
        version: VERSION,
        network: Network::Regtest,
        keys: VaultKeys {
            phone: XOnlyKey([0x11; 32]),
            hww: XOnlyKey([0x22; 32]),
            recovery_locktime: 500_000_000,
        },
        vault_inputs: (0..inputs)
            .map(|index| Input {
                outpoint: OutPoint::new(Txid([index; 32]), u32::from(index)),
                value_sats: 210_000_000 + u64::from(index),
            })
            .collect(),
        controller_inputs: vec![Input {
            outpoint: OutPoint::new(Txid([0xcc; 32]), 2),
            value_sats: 1_000,
        }],
        limits: Limits {
            monthly_limit_sats: 10_000_000,
            emergency_access_limit_sats: 50_000_000,
        },
        emergency_destination: Some(address.to_owned()),
        allowance_destinations: vec![address.to_owned(); usize::from(allowances)],
        previous_allowances_complete: true,
        phone_nonces: vec![PubNonce([0x02; 66]); usize::from(inputs + allowances + 2)],
    }
}

#[test]
fn compact_and_json_forms_carry_the_same_messages() {
    let request = request(12, 12);
    let compact = postcard::to_allocvec(&request).unwrap();
    assert_eq!(
        postcard::from_bytes::<PolicyRequest>(&compact).unwrap(),
        request
    );
    let json = serde_json::to_vec(&request).unwrap();
    assert_eq!(
        serde_json::from_slice::<PolicyRequest>(&json).unwrap(),
        request
    );
    // Keys, nonces and outpoints travel as raw bytes: the largest policy fits a few kilobytes.
    assert!(compact.len() < 3_500, "{} bytes", compact.len());
    assert!(
        json.len() > compact.len() * 3 / 2,
        "{} vs {} bytes",
        json.len(),
        compact.len()
    );

    let signatures = vec![
        CooperativeSignature {
            nonce: PubNonce([0x03; 66]),
            partial: PartialSignature([0x44; 32]),
        };
        26
    ];
    let compact = postcard::to_allocvec(&signatures).unwrap();
    assert_eq!(compact.len(), 1 + 26 * (1 + 66 + 1 + 32));
    assert_eq!(
        postcard::from_bytes::<Vec<CooperativeSignature>>(&compact).unwrap(),
        signatures
    );
    let signature = Signature([0x55; 64]);
    assert_eq!(
        postcard::from_bytes::<Signature>(&postcard::to_allocvec(&signature).unwrap()).unwrap(),
        signature
    );
}

#[test]
fn compact_fixed_size_fields_refuse_other_lengths() {
    let short = postcard::to_allocvec(&[0x11_u8; 31].as_slice()).unwrap();
    assert!(postcard::from_bytes::<XOnlyKey>(&short).is_err());
    let exact = postcard::to_allocvec(&[0x11_u8; 32].as_slice()).unwrap();
    assert_eq!(
        postcard::from_bytes::<XOnlyKey>(&exact).unwrap(),
        XOnlyKey([0x11; 32])
    );
    // JSON keeps its hex text, and outpoints their `txid:vout` form.
    let outpoint = OutPoint::new(Txid([0xab; 32]), 7);
    assert_eq!(
        serde_json::to_string(&outpoint).unwrap(),
        format!("\"{}:7\"", "ab".repeat(32))
    );
    let compact = postcard::to_allocvec(&outpoint).unwrap();
    assert_eq!(compact.len(), 1 + 32 + 1);
    assert_eq!(
        postcard::from_bytes::<OutPoint>(&compact).unwrap(),
        outpoint
    );
}
