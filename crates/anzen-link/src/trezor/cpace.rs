//! CPace over X25519, as a Trezor's code-entry pairing uses it: both sides derive a generator
//! from the six-digit code and the channel's handshake hash, and a Diffie-Hellman exchange on it
//! only agrees if both used the same code.
//!
//! Follows Trezor's own host implementation (`trezorlib.thp.cpace`). The field arithmetic for
//! the Elligator 2 map uses big integers and is not constant time: the code it protects is
//! typed by the person at this host and shown on the Trezor's screen.

use num_bigint::BigUint;
use sha2::{Digest, Sha512};

const DSI: &[u8] = b"CPace255";
const HASH_BLOCK: usize = 128;

fn prime() -> BigUint {
    (BigUint::from(1_u8) << 255) - BigUint::from(19_u8)
}

fn length_prefixed(parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for part in parts {
        // Single-byte lengths: every part here is shorter than 128 bytes.
        out.push(part.len() as u8);
        out.extend_from_slice(part);
    }
    out
}

/// The generator both sides derive from the code (`prs`) and the handshake hash (`ci`).
fn generator(prs: &[u8], ci: &[u8]) -> [u8; 32] {
    let padding = HASH_BLOCK.saturating_sub((1 + DSI.len()) + (1 + prs.len()) + 1);
    let string = length_prefixed(&[DSI, prs, &vec![0; padding], ci, b""]);
    let hash = Sha512::digest(&string);
    elligator2(hash[..32].try_into().expect("32 bytes"))
}

/// `map_to_curve_elligator2_curve25519` from RFC 9380, returning the Montgomery u coordinate.
pub fn elligator2(input: &[u8; 32]) -> [u8; 32] {
    let p = prime();
    let j = BigUint::from(486_662_u32);
    let two = BigUint::from(2_u8);
    // sqrt(-1) and (p - 5) / 8.
    let c3 = two.modpow(&((&p - 1_u8) / 4_u8), &p);
    let c4 = (&p - 5_u8) / 8_u8;
    let mul = |a: &BigUint, b: &BigUint| (a * b) % &p;

    let mut masked = *input;
    masked[31] &= 0x7f;
    let u = BigUint::from_bytes_le(&masked) % &p;
    let tv1 = (mul(&u, &u) * 2_u8) % &p;
    let xd = (&tv1 + 1_u8) % &p;
    let x1n = &p - &j;
    let tv2 = mul(&xd, &xd);
    let gxd = mul(&tv2, &xd);
    let mut gx1 = mul(&j, &tv1);
    gx1 = mul(&gx1, &x1n);
    gx1 = (gx1 + &tv2) % &p;
    gx1 = mul(&gx1, &x1n);
    let mut tv3 = mul(&gxd, &gxd);
    let mut tv2 = mul(&tv3, &tv3);
    tv3 = mul(&tv3, &gxd);
    tv3 = mul(&tv3, &gx1);
    tv2 = mul(&tv2, &tv3);
    let mut y11 = tv2.modpow(&c4, &p);
    y11 = mul(&y11, &tv3);
    let y12 = mul(&y11, &c3);
    let mut tv2 = mul(&y11, &y11);
    tv2 = mul(&tv2, &gxd);
    let y1 = if tv2 == gx1 { y11 } else { y12 };
    let x2n = mul(&x1n, &tv1);
    let mut tv2 = mul(&y1, &y1);
    tv2 = mul(&tv2, &gxd);
    let xn = if tv2 == gx1 { x1n } else { x2n };
    let x = mul(&xn, &xd.modpow(&(&p - 2_u8), &p));
    let mut out = [0_u8; 32];
    let bytes = x.to_bytes_le();
    out[..bytes.len()].copy_from_slice(&bytes);
    out
}

/// This side's public key to send, and the secret both sides share if the codes match.
pub struct Exchange {
    pub public_key: [u8; 32],
    pub shared_secret: [u8; 32],
}

pub fn exchange(
    code: &[u8],
    handshake_hash: &[u8],
    theirs: &[u8; 32],
    secret: [u8; 32],
) -> Exchange {
    let generator = generator(code, handshake_hash);
    Exchange {
        public_key: x25519_dalek::x25519(secret, generator),
        shared_secret: x25519_dalek::x25519(secret, *theirs),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(text: &str) -> [u8; 32] {
        hex::decode(text).unwrap().try_into().unwrap()
    }

    // Expected values computed with Trezor's own host implementation (`trezorlib.thp`).
    #[test]
    fn elligator2_matches_trezors_implementation() {
        for (input, output) in [
            (
                "ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb",
                "44a0aae8fb0034cc287adefd16122434b62c4f64fc34cd8799467849180fe848",
            ),
            (
                "3e23e8160039594a33894f6564e1b1348bbd7a0088d42c4acb73eeaed59c009d",
                "40f3ff09b203da46bae4e1509a2db89c14a4d08f772e826ba44969565c1c825e",
            ),
            (
                "2e7d2c03a9507ae265ecf5b5356885a53393a2029d241394997265a1a25aefc6",
                "7c54deb2c397663e611d9b54740b47ca56ce33ac4a128ab21abe4f05da1b2f0b",
            ),
            (
                "18ac3e7343f016890c510e93f935261169d9e3f565436429830faf0934f4f8e4",
                "5a6a0413a5323a05de13b867aabd2c817ccfa9fe2b56512065b62e88cf216d1a",
            ),
        ] {
            assert_eq!(hex::encode(elligator2(&unhex(input))), output);
        }
    }

    #[test]
    fn exchange_matches_trezors_implementation() {
        let hash = [7_u8; 32];
        assert_eq!(
            hex::encode(generator(b"123456", &hash)),
            "ed2eb2f6cc5e79d7412fc2c83062682a07546d9a3f4aaa5fee274f81a5d5e257"
        );
        let ours = exchange(
            b"123456",
            &hash,
            &unhex(&"22".repeat(32)),
            unhex(&"11".repeat(32)),
        );
        assert_eq!(
            hex::encode(ours.public_key),
            "7ac769526212c51361d7c9bdcb6c12df2cebca7372398e148a8442127814d513"
        );
        assert_eq!(
            hex::encode(ours.shared_secret),
            "733b02242ee5be014dc59ea39e497c837b5c53b41f33d080ade16c44a2c1561b"
        );
    }

    #[test]
    fn exchange_agrees_only_when_both_sides_use_the_same_code() {
        let hash = [7_u8; 32];
        let (a, b) = (unhex(&"11".repeat(32)), unhex(&"22".repeat(32)));
        let base = generator(b"123456", &hash);
        let (a_public, b_public) = (x25519_dalek::x25519(a, base), x25519_dalek::x25519(b, base));
        let from_a = exchange(b"123456", &hash, &b_public, a);
        let from_b = exchange(b"123456", &hash, &a_public, b);
        assert_eq!(from_a.public_key, a_public);
        assert_eq!(from_a.shared_secret, from_b.shared_secret);
        // A different code gives a different generator, and no agreement.
        let wrong = exchange(b"654321", &hash, &b_public, a);
        assert_ne!(wrong.public_key, a_public);
        assert_ne!(
            x25519_dalek::x25519(b, wrong.public_key),
            wrong.shared_secret
        );
    }
}
