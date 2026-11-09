//! Host MuSig2 (BIP327) for the vault's joint key path.
//!
//! A cooperative vault spend carries one signature from both devices. The phone signs first and
//! the signer last: [`phone_nonce`] starts the phone's half, the signer answers with
//! [`sign_last`], and [`complete`] checks that answer, adds the phone's partial signature and
//! aggregates. Key aggregation itself is [`anzen_protocol::musig`]; the two must agree, and the
//! tests check they do.
//!
//! Each x-only vault key enters aggregation as its even-y point, so each device signs with the
//! secret of that point ([`even_secret`]).

use anyhow::{Context, Result, anyhow, ensure};
use anzen_protocol::{
    musig::{plain_key, sorted_keys},
    script::VaultKeys,
    wire::{CooperativeSignature, PartialSignature, PubNonce},
};
use musig2::{
    AggNonce, BinaryEncoding, KeyAggContext, LiftedSignature, SecNonce,
    secp::{MaybeScalar, Point, Scalar},
};
use rand::RngCore;
use sha2::{Digest, Sha256};

/// Key aggregation with the vault's Taproot tweak: the context of every joint signature.
pub fn context(keys: &VaultKeys, merkle_root: &[u8; 32]) -> Result<KeyAggContext> {
    let points = sorted_keys(keys)
        .iter()
        .map(|key| Point::from_slice(key).map_err(|_| anyhow!("invalid vault key")))
        .collect::<Result<Vec<_>>>()?;
    KeyAggContext::new(points)
        .map_err(|error| anyhow!("{error}"))?
        .with_taproot_tweak(merkle_root)
        .map_err(|error| anyhow!("{error}"))
}

/// The secret of the even-y point BIP327 aggregates for this key's x-only public key.
pub fn even_secret(secret: &[u8; 32]) -> Result<Scalar> {
    let scalar = Scalar::from_slice(secret).map_err(|_| anyhow!("invalid secret key"))?;
    Ok(if scalar.base_point_mul().has_even_y() {
        scalar
    } else {
        -scalar
    })
}

/// The signer's half of a joint signature, as the last signer (BIP327 `DeterministicSign`
/// without auxiliary randomness).
///
/// The nonce is derived from the secret key, the phone's nonce, the tweaked aggregate key and
/// the message, used once and dropped. Nothing is stored, so there is nothing to reuse: a repeated
/// request repeats the same answer, and any changed input gives a different nonce. This is safe
/// only because the phone's nonce is fixed before the signer derives its own, which is why the
/// signer always signs last.
pub fn sign_last(
    secret: &[u8; 32],
    keys: &VaultKeys,
    merkle_root: &[u8; 32],
    message: &[u8; 32],
    phone_nonce: &PubNonce,
) -> Result<CooperativeSignature> {
    let (nonce, partial) = deterministic_sign(
        even_secret(secret)?,
        &phone_nonce.0,
        &context(keys, merkle_root)?,
        message,
        None,
    )?;
    Ok(CooperativeSignature {
        nonce: PubNonce(nonce.to_bytes()),
        partial: PartialSignature(partial.serialize()),
    })
}

/// BIP327 `DeterministicSign`, exactly as specified: the public nonce and partial signature of
/// the last signer, whose nonce is derived from its secret key (optionally blinded by `rand`),
/// the other signers' aggregate nonce, the tweaked aggregate key and the message. Checked against
/// the official BIP327 test vectors.
pub fn deterministic_sign(
    secret: Scalar,
    aggregate_other_nonce: &[u8; 66],
    context: &KeyAggContext,
    message: &[u8],
    rand: Option<&[u8; 32]>,
) -> Result<(musig2::PubNonce, musig2::PartialSignature)> {
    let other = musig2::PubNonce::from_bytes(aggregate_other_nonce)
        .context("invalid aggregate nonce of the other signers")?;
    let mut blinded = secret.serialize();
    if let Some(rand) = rand {
        for (byte, mask) in blinded.iter_mut().zip(tagged(b"MuSig/aux", &[rand])) {
            *byte ^= mask;
        }
    }
    let aggregate: Point = context.aggregated_pubkey();
    let derive = |index: u8| -> Result<Scalar> {
        let digest = tagged(
            b"MuSig/deterministic/nonce",
            &[
                &blinded,
                aggregate_other_nonce,
                &aggregate.serialize_xonly(),
                &(message.len() as u64).to_be_bytes(),
                message,
                &[index],
            ],
        );
        MaybeScalar::reduce_from(&digest)
            .into_option()
            .context("derived nonce is zero")
    };
    let secnonce = SecNonce::new(derive(0)?, derive(1)?);
    let own = secnonce.public_nonce();
    let aggregate_nonce = AggNonce::sum([own.clone(), other]);
    let partial = musig2::sign_partial(context, secret, secnonce, &aggregate_nonce, message)
        .map_err(|error| anyhow!("{error}"))?;
    Ok((own, partial))
}

/// BIP340 tagged hash: `SHA256(SHA256(tag) || SHA256(tag) || parts…)`.
fn tagged(tag: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let tag = Sha256::digest(tag);
    let mut hasher = Sha256::new().chain_update(tag).chain_update(tag);
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// A fresh phone nonce for one joint signature: the secret half (`k1 || k2`) and its public
/// half for the request.
///
/// The secret half must be used for exactly one signature. Callers keep it only until
/// [`complete`], and must destroy it as that signature is produced; see
/// `anzen_wallet::cosigning` for how the phone does this.
pub fn phone_nonce(
    secret: &[u8; 32],
    keys: &VaultKeys,
    merkle_root: &[u8; 32],
    message: &[u8; 32],
) -> Result<([u8; 64], PubNonce)> {
    let context = context(keys, merkle_root)?;
    let aggregate: Point = context.aggregated_pubkey();
    // BIP327 NonceGen: fresh randomness, with the key, aggregate key and message mixed in so a
    // weak random source alone cannot repeat a nonce across different signatures.
    let mut seed = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut seed);
    let secnonce = SecNonce::build(seed)
        .with_seckey(even_secret(secret)?)
        .with_aggregated_pubkey(aggregate)
        .with_message(message)
        .build();
    Ok((
        secnonce.to_bytes(),
        PubNonce(secnonce.public_nonce().to_bytes()),
    ))
}

/// Check that `signer` is the signer's valid half of the joint signature over `message` with the
/// phone's `phone_nonce`. Only the holder of the signer's key can produce one.
pub fn verify_signer_half(
    keys: &VaultKeys,
    merkle_root: &[u8; 32],
    message: &[u8; 32],
    phone_nonce: &PubNonce,
    signer: &CooperativeSignature,
) -> Result<()> {
    let context = context(keys, merkle_root)?;
    let (aggregate_nonce, signer_public, signer_partial) = session(phone_nonce, signer)?;
    verify(
        &context,
        &aggregate_nonce,
        &signer_public,
        signer_partial,
        keys,
        message,
    )
}

fn session(
    phone_nonce: &PubNonce,
    signer: &CooperativeSignature,
) -> Result<(AggNonce, musig2::PubNonce, MaybeScalar)> {
    let phone_public =
        musig2::PubNonce::from_bytes(&phone_nonce.0).context("invalid phone nonce")?;
    let signer_public =
        musig2::PubNonce::from_bytes(&signer.nonce.0).context("invalid signer nonce")?;
    let signer_partial = MaybeScalar::from_slice(&signer.partial.0)
        .map_err(|_| anyhow!("invalid signer partial signature"))?;
    Ok((
        AggNonce::sum([phone_public, signer_public.clone()]),
        signer_public,
        signer_partial,
    ))
}

fn verify(
    context: &KeyAggContext,
    aggregate_nonce: &AggNonce,
    signer_public: &musig2::PubNonce,
    signer_partial: MaybeScalar,
    keys: &VaultKeys,
    message: &[u8; 32],
) -> Result<()> {
    let signer_key =
        Point::from_slice(&plain_key(&keys.hww)).map_err(|_| anyhow!("invalid key"))?;
    musig2::verify_partial(
        context,
        signer_partial,
        aggregate_nonce,
        signer_key,
        signer_public,
        message,
    )
    .map_err(|_| anyhow!("signer's partial signature is invalid"))
}

/// The phone's completion of one joint signature: verify the signer's half, add the phone's
/// partial signature with its stored nonce, aggregate and verify the result under the vault's
/// output key. Returns the final BIP340 signature.
#[allow(clippy::too_many_arguments)]
pub fn complete(
    secret: &[u8; 32],
    secnonce: &[u8; 64],
    keys: &VaultKeys,
    merkle_root: &[u8; 32],
    message: &[u8; 32],
    phone_nonce: &PubNonce,
    signer: &CooperativeSignature,
) -> Result<[u8; 64]> {
    let context = context(keys, merkle_root)?;
    let (aggregate_nonce, signer_public, signer_partial) = session(phone_nonce, signer)?;
    // Only the real signer can produce a valid half, so nothing else ever gets the phone to sign.
    verify(
        &context,
        &aggregate_nonce,
        &signer_public,
        signer_partial,
        keys,
        message,
    )?;
    let secnonce = SecNonce::from_bytes(secnonce).context("invalid stored nonce")?;
    ensure!(
        secnonce.public_nonce().to_bytes() == phone_nonce.0,
        "stored nonce does not belong to this request"
    );
    let own: musig2::PartialSignature = musig2::sign_partial(
        &context,
        even_secret(secret)?,
        secnonce,
        &aggregate_nonce,
        message,
    )
    .map_err(|error| anyhow!("{error}"))?;
    let signature: LiftedSignature = musig2::aggregate_partial_signatures(
        &context,
        &aggregate_nonce,
        [own, signer_partial],
        message,
    )
    .map_err(|error| anyhow!("{error}"))?;
    let output: Point = context.aggregated_pubkey();
    musig2::verify_single(output, signature, message)
        .map_err(|_| anyhow!("joint signature does not verify"))?;
    Ok(signature.serialize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anzen_protocol::{crypto::SoftwareCrypto, script::Scripts};

    fn keys_with_secrets() -> (VaultKeys, [u8; 32], [u8; 32]) {
        let secp = bitcoin::key::Secp256k1::new();
        let secret = || bitcoin::secp256k1::Keypair::new(&secp, &mut rand::thread_rng());
        let (phone, hww) = (secret(), secret());
        let key = |pair: &bitcoin::secp256k1::Keypair| {
            anzen_protocol::script::XOnlyKey(pair.x_only_public_key().0.serialize())
        };
        (
            VaultKeys {
                phone: key(&phone),
                hww: key(&hww),
                recovery_locktime: crate::DEFAULT_RECOVERY_LOCKTIME,
            },
            phone.secret_bytes(),
            hww.secret_bytes(),
        )
    }

    #[test]
    fn a_joint_signature_verifies_under_the_protocol_output_key() {
        for _ in 0..8 {
            let (keys, phone, hww) = keys_with_secrets();
            let scripts = Scripts::new(&mut SoftwareCrypto::new(), &keys).unwrap();
            let root = scripts.vault.merkle_root;
            let message = [9_u8; 32];
            let (secnonce, public) = phone_nonce(&phone, &keys, &root, &message).unwrap();
            let signer = sign_last(&hww, &keys, &root, &message, &public).unwrap();
            // The signer is deterministic: the same request gives the same answer.
            assert_eq!(
                signer,
                sign_last(&hww, &keys, &root, &message, &public).unwrap()
            );
            let signature =
                complete(&phone, &secnonce, &keys, &root, &message, &public, &signer).unwrap();
            let secp = bitcoin::key::Secp256k1::verification_only();
            secp.verify_schnorr(
                &bitcoin::secp256k1::schnorr::Signature::from_slice(&signature).unwrap(),
                &bitcoin::secp256k1::Message::from_digest(message),
                &bitcoin::XOnlyPublicKey::from_slice(&scripts.vault.output.output_key).unwrap(),
            )
            .unwrap();
        }
    }

    #[test]
    fn the_phone_refuses_a_forged_or_mismatched_signer_half() {
        let (keys, phone, hww) = keys_with_secrets();
        let root = Scripts::new(&mut SoftwareCrypto::new(), &keys)
            .unwrap()
            .vault
            .merkle_root;
        let message = [3_u8; 32];
        let (secnonce, public) = phone_nonce(&phone, &keys, &root, &message).unwrap();
        let mut forged = sign_last(&hww, &keys, &root, &message, &public).unwrap();
        forged.partial.0[31] ^= 1;
        assert!(complete(&phone, &secnonce, &keys, &root, &message, &public, &forged).is_err());
        // An answer for a different message, or from someone else's key, is refused too.
        let other = sign_last(&hww, &keys, &root, &[4; 32], &public).unwrap();
        assert!(complete(&phone, &secnonce, &keys, &root, &message, &public, &other).is_err());
        let (_, _, stranger) = keys_with_secrets();
        let stranger = sign_last(&stranger, &keys, &root, &message, &public);
        assert!(
            stranger.is_err()
                || complete(
                    &phone,
                    &secnonce,
                    &keys,
                    &root,
                    &message,
                    &public,
                    &stranger.unwrap()
                )
                .is_err()
        );
        // A different stored nonce cannot complete this request's public nonce.
        let (wrong, _) = phone_nonce(&phone, &keys, &root, &message).unwrap();
        let signer = sign_last(&hww, &keys, &root, &message, &public).unwrap();
        assert!(complete(&phone, &wrong, &keys, &root, &message, &public, &signer).is_err());
    }

    #[test]
    fn deterministic_sign_matches_the_official_bip327_vectors() {
        // https://github.com/bitcoin/bips/blob/master/bip-0327/vectors/det_sign_vectors.json
        let vectors: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/vectors/bip327_det_sign_vectors.json"
        ))
        .unwrap();
        let bytes = |value: &serde_json::Value| {
            let text = value.as_str().unwrap();
            (0..text.len())
                .step_by(2)
                .map(|index| u8::from_str_radix(&text[index..index + 2], 16).unwrap())
                .collect::<Vec<u8>>()
        };
        let secret = Scalar::from_slice(&bytes(&vectors["sk"])).unwrap();
        let pubkeys = vectors["pubkeys"].as_array().unwrap();
        let messages = vectors["msgs"].as_array().unwrap();
        // Everything a case asks of the signer, or the reason the implementation refuses it.
        let run = |case: &serde_json::Value| -> Result<(Vec<u8>, Vec<u8>)> {
            let keys = case["key_indices"]
                .as_array()
                .unwrap()
                .iter()
                .map(|index| {
                    Point::from_slice(&bytes(&pubkeys[index.as_u64().unwrap() as usize]))
                        .map_err(|_| anyhow!("invalid public key"))
                })
                .collect::<Result<Vec<_>>>()?;
            let mut context = KeyAggContext::new(keys).map_err(|error| anyhow!("{error}"))?;
            for (tweak, xonly) in case["tweaks"]
                .as_array()
                .unwrap()
                .iter()
                .zip(case["is_xonly"].as_array().unwrap())
            {
                let tweak =
                    Scalar::from_slice(&bytes(tweak)).map_err(|_| anyhow!("tweak exceeds n"))?;
                context = context
                    .with_tweak(tweak, xonly.as_bool().unwrap())
                    .map_err(|error| anyhow!("{error}"))?;
            }
            let other: [u8; 66] = bytes(&case["aggothernonce"]).try_into().unwrap();
            let rand: Option<[u8; 32]> =
                (!case["rand"].is_null()).then(|| bytes(&case["rand"]).try_into().unwrap());
            let message = bytes(&messages[case["msg_index"].as_u64().unwrap() as usize]);
            let (nonce, partial) =
                deterministic_sign(secret, &other, &context, &message, rand.as_ref())?;
            Ok((nonce.to_bytes().to_vec(), partial.serialize().to_vec()))
        };
        let valid = vectors["valid_test_cases"].as_array().unwrap();
        assert_eq!(valid.len(), 4);
        for case in valid {
            let (nonce, partial) = run(case).unwrap();
            assert_eq!(nonce, bytes(&case["expected"][0]), "{case}");
            assert_eq!(partial, bytes(&case["expected"][1]), "{case}");
        }
        let errors = vectors["error_test_cases"].as_array().unwrap();
        assert_eq!(errors.len(), 5);
        for case in errors {
            assert!(run(case).is_err(), "{case}");
        }
    }
}
