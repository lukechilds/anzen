//! BIP327 (MuSig2) key aggregation for the vault's two keys.
//!
//! Every implementation must derive the same internal key, so aggregation lives here with the
//! scripts. Nonces and partial signatures do not: the phone and the signer produce those in
//! their own code, because only they hold secrets.

use crate::{
    Error, Result,
    crypto::{Crypto, tagged_hash},
    script::{VaultKeys, XOnlyKey},
};

/// The secp256k1 group order.
const ORDER: [u8; 32] = [
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe,
    0xba, 0xae, 0xdc, 0xe6, 0xaf, 0x48, 0xa0, 0x3b, 0xbf, 0xd2, 0x5e, 0x8c, 0xd0, 0x36, 0x41, 0x41,
];

/// The compressed key BIP327 aggregates for an x-only key: the point with an even y.
pub fn plain_key(key: &XOnlyKey) -> [u8; 33] {
    let mut plain = [0x02; 33];
    plain[1..].copy_from_slice(&key.0);
    plain
}

/// Both keys in BIP327 `KeySort` order, the order they are aggregated in.
pub fn sorted_keys(keys: &VaultKeys) -> [[u8; 33]; 2] {
    let mut sorted = [plain_key(&keys.phone), plain_key(&keys.hww)];
    sorted.sort();
    sorted
}

/// The x-only aggregate of both keys, before the Taproot tweak: the vault's internal key.
pub fn aggregate_key<C: Crypto + ?Sized>(crypto: &mut C, keys: &VaultKeys) -> Result<[u8; 32]> {
    let [first, second] = sorted_keys(keys);
    if first == second {
        return Err(Error::InvalidKey);
    }
    // KeyAgg: the second distinct key has coefficient 1; the first is hashed with the key list.
    let list_tag = crypto.sha256(&[b"KeyAgg list"]);
    let list = tagged_hash(crypto, &list_tag, &[&first, &second]);
    let coefficient_tag = crypto.sha256(&[b"KeyAgg coefficient"]);
    let coefficient = reduce(tagged_hash(crypto, &coefficient_tag, &[&list, &first]));
    let aggregate = crypto.mul_add(&coefficient, &first, &second)?;
    let mut xonly = [0_u8; 32];
    xonly.copy_from_slice(&aggregate[1..]);
    Ok(xonly)
}

/// A 32-byte hash reduced modulo the group order. Values are below 2^256 < 2n, so at most one
/// subtraction is needed.
fn reduce(mut value: [u8; 32]) -> [u8; 32] {
    if value >= ORDER {
        let mut borrow = 0_i16;
        for index in (0..32).rev() {
            let difference = i16::from(value[index]) - i16::from(ORDER[index]) - borrow;
            borrow = i16::from(difference < 0);
            value[index] = (difference + (borrow << 8)) as u8;
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduction_subtracts_the_order_once() {
        assert_eq!(reduce([0; 32]), [0; 32]);
        let mut above = ORDER;
        above[31] += 5;
        let mut five = [0; 32];
        five[31] = 5;
        assert_eq!(reduce(above), five);
        assert_eq!(reduce(ORDER), [0; 32]);
        assert_eq!(reduce([0xff; 32])[..15], [0; 15]);
    }
}
