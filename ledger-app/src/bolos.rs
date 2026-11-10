//! The secure element's hashing, curve arithmetic and key derivation, as the signer needs them.
//!
//! Every operation on a secret runs in BOLOS: derivation, the randomized scalar multiplication,
//! modular arithmetic and BIP340 signing. The vault key's bytes exist in this app's memory only
//! while one signature is produced, and are wiped after.

use anzen_protocol::{
    Error, Result,
    address::Network,
    crypto::{Crypto, TweakedKey},
    script::XOnlyKey,
    wire::CooperativeSignature,
};
use anzen_signer::{
    CooperativeSession, Keys,
    musig::{self, Arithmetic},
};
use ledger_device_sdk::{
    ecc::{CurvesId, bip32_derive},
    hash::{HashInit, sha2::Sha2_256},
    sys,
};

/// The secp256k1 group order.
const ORDER: [u8; 32] = [
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe,
    0xba, 0xae, 0xdc, 0xe6, 0xaf, 0x48, 0xa0, 0x3b, 0xbf, 0xd2, 0x5e, 0x8c, 0xd0, 0x36, 0x41, 0x41,
];
/// The secp256k1 field prime.
const PRIME: [u8; 32] = [
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe, 0xff, 0xff, 0xfc, 0x2f,
];
const GENERATOR: [u8; 33] = [
    0x02, 0x79, 0xbe, 0x66, 0x7e, 0xf9, 0xdc, 0xbb, 0xac, 0x55, 0xa0, 0x62, 0x95, 0xce, 0x87, 0x0b,
    0x07, 0x02, 0x9b, 0xfc, 0xdb, 0x2d, 0xce, 0x28, 0xd9, 0x59, 0xf2, 0x81, 0x5b, 0x16, 0xf8, 0x17,
    0x98,
];
const ONE: [u8; 32] = {
    let mut one = [0; 32];
    one[31] = 1;
    one
};

fn ok(error: sys::cx_err_t) -> Result<()> {
    if error == sys::CX_OK {
        Ok(())
    } else {
        Err(Error::InvalidKey)
    }
}

/// Overwrite a secret. Volatile, so the compiler cannot drop the writes.
pub fn wipe(bytes: &mut [u8]) {
    for byte in bytes {
        unsafe { core::ptr::write_volatile(byte, 0) };
    }
}

/// BOLOS's big-number context, which point operations need. Unlocked on drop.
struct Numbers;

impl Numbers {
    fn lock() -> Result<Self> {
        ok(unsafe { sys::cx_bn_lock(32, 0) })?;
        Ok(Self)
    }
}

impl Drop for Numbers {
    fn drop(&mut self) {
        unsafe { sys::cx_bn_unlock() };
    }
}

/// A curve point inside a [`Numbers`] context.
struct Point(sys::cx_ecpoint_t);

impl Point {
    fn alloc() -> Result<Self> {
        let mut point: sys::cx_ecpoint_t = unsafe { core::mem::zeroed() };
        ok(unsafe { sys::cx_ecpoint_alloc(&mut point, sys::CX_CURVE_SECP256K1) })?;
        Ok(Self(point))
    }

    /// A compressed point: `02` or `03`, then an x coordinate below the field prime.
    fn decompress(compressed: &[u8; 33]) -> Result<Self> {
        if !matches!(compressed[0], 0x02 | 0x03) || compressed[1..] >= PRIME[..] {
            return Err(Error::InvalidKey);
        }
        let mut point = Self::alloc()?;
        ok(unsafe {
            sys::cx_ecpoint_decompress(
                &mut point.0,
                compressed[1..].as_ptr(),
                32,
                u32::from(compressed[0] & 1),
            )
        })?;
        let mut on_curve = false;
        ok(unsafe { sys::cx_ecpoint_is_on_curve(&point.0, &mut on_curve) })?;
        if !on_curve {
            return Err(Error::InvalidKey);
        }
        Ok(point)
    }

    /// Multiply by a public scalar.
    fn mul(&mut self, scalar: &[u8; 32]) -> Result<()> {
        ok(unsafe { sys::cx_ecpoint_scalarmul(&mut self.0, scalar.as_ptr(), 32) })
    }

    /// Multiply by a secret scalar, with BOLOS's randomized coordinates.
    fn mul_secret(&mut self, scalar: &[u8; 32]) -> Result<()> {
        ok(unsafe { sys::cx_ecpoint_rnd_scalarmul(&mut self.0, scalar.as_ptr(), 32) })
    }

    fn add(&self, other: &Self) -> Result<Self> {
        let mut sum = Self::alloc()?;
        ok(unsafe { sys::cx_ecpoint_add(&mut sum.0, &self.0, &other.0) })?;
        Ok(sum)
    }

    /// The compressed point. The point at infinity has none.
    fn compress(&self) -> Result<[u8; 33]> {
        let mut infinite = false;
        ok(unsafe { sys::cx_ecpoint_is_at_infinity(&self.0, &mut infinite) })?;
        if infinite {
            return Err(Error::InvalidKey);
        }
        let (mut x, mut y) = ([0_u8; 32], [0_u8; 32]);
        ok(unsafe { sys::cx_ecpoint_export(&self.0, x.as_mut_ptr(), 32, y.as_mut_ptr(), 32) })?;
        let mut compressed = [0x02 | (y[31] & 1); 33];
        compressed[1..].copy_from_slice(&x);
        Ok(compressed)
    }
}

/// The device's primitives.
pub struct Bolos;

impl Crypto for Bolos {
    fn sha256(&mut self, parts: &[&[u8]]) -> [u8; 32] {
        let mut hasher = Sha2_256::new();
        for part in parts {
            hasher.update(part).expect("SHA-256 update");
        }
        let mut digest = [0_u8; 32];
        hasher.finalize(&mut digest).expect("SHA-256 finalize");
        digest
    }

    fn is_valid_xonly_key(&mut self, key: &[u8; 32]) -> bool {
        let mut even = [0x02; 33];
        even[1..].copy_from_slice(key);
        Numbers::lock()
            .and_then(|_numbers| Point::decompress(&even).map(|_| ()))
            .is_ok()
    }

    fn taproot_tweak(&mut self, internal_key: &[u8; 32], tweak: &[u8; 32]) -> Result<TweakedKey> {
        if tweak >= &ORDER {
            return Err(Error::InvalidKey);
        }
        let mut internal = [0x02; 33];
        internal[1..].copy_from_slice(internal_key);
        let _numbers = Numbers::lock()?;
        let internal = Point::decompress(&internal)?;
        let mut tweak_point = Point::decompress(&GENERATOR)?;
        tweak_point.mul(tweak)?;
        let output = internal.add(&tweak_point)?.compress()?;
        let mut output_key = [0_u8; 32];
        output_key.copy_from_slice(&output[1..]);
        Ok(TweakedKey {
            output_key,
            odd_y: output[0] == 0x03,
        })
    }

    fn mul_add(&mut self, a: &[u8; 32], first: &[u8; 33], second: &[u8; 33]) -> Result<[u8; 33]> {
        if a >= &ORDER {
            return Err(Error::InvalidKey);
        }
        let _numbers = Numbers::lock()?;
        let mut product = Point::decompress(first)?;
        if a != &ONE {
            product.mul(a)?;
        }
        product.add(&Point::decompress(second)?)?.compress()
    }
}

impl Arithmetic for Bolos {
    fn reduce(&mut self, a: &[u8; 32]) -> Result<[u8; 32]> {
        let mut reduced = *a;
        ok(unsafe { sys::cx_math_modm_no_throw(reduced.as_mut_ptr(), 32, ORDER.as_ptr(), 32) })?;
        Ok(reduced)
    }

    fn add(&mut self, a: &[u8; 32], b: &[u8; 32]) -> Result<[u8; 32]> {
        let mut sum = [0_u8; 32];
        ok(unsafe {
            sys::cx_math_addm_no_throw(sum.as_mut_ptr(), a.as_ptr(), b.as_ptr(), ORDER.as_ptr(), 32)
        })?;
        Ok(sum)
    }

    fn mul(&mut self, a: &[u8; 32], b: &[u8; 32]) -> Result<[u8; 32]> {
        let mut product = [0_u8; 32];
        ok(unsafe {
            sys::cx_math_multm_no_throw(
                product.as_mut_ptr(),
                a.as_ptr(),
                b.as_ptr(),
                ORDER.as_ptr(),
                32,
            )
        })?;
        Ok(product)
    }

    fn base_mul(&mut self, k: &[u8; 32]) -> Result<[u8; 33]> {
        if k == &[0; 32] || k >= &ORDER {
            return Err(Error::InvalidKey);
        }
        let _numbers = Numbers::lock()?;
        let mut point = Point::decompress(&GENERATOR)?;
        point.mul_secret(k)?;
        point.compress()
    }
}

const HARDENED: u32 = 0x8000_0000;

/// The signer's vault key for one network: `m/86'/coin'/100'/0/0`, where the phone app's own
/// signer mode keeps its key too.
pub struct VaultKey {
    path: [u32; 5],
}

impl VaultKey {
    pub fn new(network: Network) -> Self {
        let coin = match network {
            Network::Mainnet => 0,
            Network::Mutinynet | Network::Regtest => 1,
        };
        Self {
            path: [HARDENED | 86, HARDENED | coin, HARDENED | 100, 0, 0],
        }
    }

    /// Run `use_secret` with the key's bytes, then wipe them.
    fn with_secret<T>(&self, use_secret: impl FnOnce(&[u8; 32]) -> Result<T>) -> Result<T> {
        // BOLOS writes the key into the first half of a 64-byte buffer.
        let mut raw = [0_u8; 64];
        let derived = bip32_derive(CurvesId::Secp256k1, &self.path, &mut raw, None);
        let mut secret = [0_u8; 32];
        secret.copy_from_slice(&raw[..32]);
        wipe(&mut raw);
        let result = match derived {
            Ok(()) => use_secret(&secret),
            Err(_) => Err(Error::SigningFailed),
        };
        wipe(&mut secret);
        result
    }
}

impl Keys for VaultKey {
    fn vault_key(&mut self) -> Result<XOnlyKey> {
        let point = self.with_secret(|secret| Bolos.base_mul(secret))?;
        let mut key = [0_u8; 32];
        key.copy_from_slice(&point[1..]);
        Ok(XOnlyKey(key))
    }

    fn sign(&mut self, message: &[u8; 32]) -> Result<[u8; 64]> {
        self.with_secret(|secret| {
            let mut key: sys::cx_ecfp_256_private_key_t = unsafe { core::mem::zeroed() };
            let key_pointer = (&raw mut key).cast::<sys::cx_ecfp_private_key_t>();
            let signing = (|| {
                ok(unsafe {
                    sys::cx_ecfp_init_private_key_no_throw(
                        sys::CX_CURVE_SECP256K1,
                        secret.as_ptr(),
                        32,
                        key_pointer,
                    )
                })?;
                let mut signature = [0_u8; 64];
                let mut length = signature.len();
                // BIP340 with fresh auxiliary randomness from the device.
                ok(unsafe {
                    sys::cx_ecschnorr_sign_no_throw(
                        key_pointer,
                        sys::CX_ECSCHNORR_BIP0340 | sys::CX_RND_TRNG,
                        sys::CX_SHA256,
                        message.as_ptr(),
                        message.len(),
                        signature.as_mut_ptr(),
                        &mut length,
                    )
                })?;
                if length != signature.len() {
                    return Err(Error::SigningFailed);
                }
                Ok(signature)
            })();
            wipe(&mut key.d);
            signing.map_err(|_| Error::SigningFailed)
        })
    }

    fn sign_cooperative(
        &mut self,
        session: &CooperativeSession<'_>,
    ) -> Result<CooperativeSignature> {
        self.with_secret(|secret| musig::sign_last(&mut Bolos, secret, session))
    }
}
