use anzen_cold_signer::benchmark::{
    BenchmarkConfig, BenchmarkError as GraphError, FIXED_SIGNING_DIGEST, PolicyCommitment, Sha256,
    VisitError, WorkloadSummary, normalize_bip340_public_key,
};
use ledger_device_sdk::{
    ecc::{ECPrivateKey, Secp256k1, SeedDerive, make_bip32_path},
    hash::{HashInit, sha2::Sha2_256},
    sys,
};

const BENCHMARK_PATH: [u32; 5] = make_bip32_path(b"m/86'/1'/100'/0/2147483647");
const FIXED_PHONE_XONLY_PUBLIC_KEY: [u8; 32] = [
    0x79, 0xbe, 0x66, 0x7e, 0xf9, 0xdc, 0xbb, 0xac, 0x55, 0xa0, 0x62, 0x95, 0xce, 0x87, 0x0b, 0x07,
    0x02, 0x9b, 0xfc, 0xdb, 0x2d, 0xce, 0x28, 0xd9, 0x59, 0xf2, 0x81, 0x5b, 0x16, 0xf8, 0x17, 0x98,
];
const SECP256K1_GENERATOR: [u8; 65] = [
    0x04, 0x79, 0xbe, 0x66, 0x7e, 0xf9, 0xdc, 0xbb, 0xac, 0x55, 0xa0, 0x62, 0x95, 0xce, 0x87, 0x0b,
    0x07, 0x02, 0x9b, 0xfc, 0xdb, 0x2d, 0xce, 0x28, 0xd9, 0x59, 0xf2, 0x81, 0x5b, 0x16, 0xf8, 0x17,
    0x98, 0x48, 0x3a, 0xda, 0x77, 0x26, 0xa3, 0xc4, 0x65, 0x5d, 0xa4, 0xfb, 0xfc, 0x0e, 0x11, 0x08,
    0xa8, 0xfd, 0x17, 0xb4, 0x48, 0xa6, 0x85, 0x54, 0x19, 0x9c, 0x47, 0xd0, 0x8f, 0xfb, 0x10, 0xd4,
    0xb8,
];
const BIP341_NUMS_POINT: [u8; 65] = [
    0x04, 0x50, 0x92, 0x9b, 0x74, 0xc1, 0xa0, 0x49, 0x54, 0xb7, 0x8b, 0x4b, 0x60, 0x35, 0xe9, 0x7a,
    0x5e, 0x07, 0x8a, 0x5a, 0x0f, 0x28, 0xec, 0x96, 0xd5, 0x47, 0xbf, 0xee, 0x9a, 0xce, 0x80, 0x3a,
    0xc0, 0x31, 0xd3, 0xc6, 0x86, 0x39, 0x73, 0x92, 0x6e, 0x04, 0x9e, 0x63, 0x7c, 0xb1, 0xb5, 0xf4,
    0x0a, 0x36, 0xda, 0xc2, 0x8a, 0xf1, 0x76, 0x69, 0x68, 0xc3, 0x0c, 0x23, 0x13, 0xf3, 0xa3, 0x89,
    0x04,
];
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchmarkError {
    Graph(GraphError),
    Crypto,
    SignatureCreation,
    PublicKeyMismatch,
    SignatureCountMismatch,
}

impl From<GraphError> for BenchmarkError {
    fn from(value: GraphError) -> Self {
        Self::Graph(value)
    }
}

impl From<VisitError<BenchmarkError>> for BenchmarkError {
    fn from(value: VisitError<BenchmarkError>) -> Self {
        match value {
            VisitError::Graph(error) => Self::Graph(error),
            VisitError::Callback(error) => error,
        }
    }
}

pub struct LedgerSha256;

impl Sha256 for LedgerSha256 {
    fn hash(&mut self, parts: &[&[u8]]) -> [u8; 32] {
        let mut hasher = Sha2_256::new();
        for part in parts {
            hasher.update(part).expect("Ledger SHA-256 update failed");
        }
        let mut digest = [0_u8; 32];
        hasher
            .finalize(&mut digest)
            .expect("Ledger SHA-256 finalize failed");
        digest
    }
}

pub struct BenchmarkContext {
    config: BenchmarkConfig,
    summary: WorkloadSummary,
    hww_private_key: ECPrivateKey<32, 'W'>,
    hww_public_key: [u8; 65],
}

impl BenchmarkContext {
    pub fn prepare(rollover_inputs: u8) -> Result<Self, BenchmarkError> {
        let (hww_private_key, hww_public_key) = derive_benchmark_key()?;
        let hww_xonly = xonly(hww_public_key)?;

        let mut hasher = LedgerSha256;
        let policy = PolicyCommitment::new(&mut hasher, FIXED_PHONE_XONLY_PUBLIC_KEY, hww_xonly);
        let vault_output_key = taproot_output_key(policy.output_key_tweak)?;
        let config = BenchmarkConfig::deterministic(
            &mut hasher,
            rollover_inputs,
            vault_output_key,
            policy.cooperative_leaf_hash,
        )?;
        let summary = config.summary();
        Ok(Self {
            config,
            summary,
            hww_private_key,
            hww_public_key,
        })
    }

    pub fn summary(&self) -> WorkloadSummary {
        self.summary
    }

    pub fn hww_xonly_public_key(&self) -> [u8; 32] {
        xonly(self.hww_public_key).expect("prepared benchmark public key is valid")
    }

    /// Repeat the complete BIP32 derivation and public-key construction.
    pub fn benchmark_key_derivation(&self) -> Result<[u8; 32], BenchmarkError> {
        let (_hww_private_key, hww_public_key) = derive_benchmark_key()?;
        if hww_public_key != self.hww_public_key {
            return Err(BenchmarkError::PublicKeyMismatch);
        }
        xonly(hww_public_key)
    }

    /// Construct every transaction and BIP341 signature message without signing it.
    pub fn benchmark_graph(&self) -> Result<[u8; 32], BenchmarkError> {
        let config = &self.config;
        let mut hasher = LedgerSha256;
        let mut last_sighash = [0_u8; 32];
        let mut generated = 0_u8;
        config.for_each_signature_job(&mut hasher, |job| {
            last_sighash = job.sighash;
            generated += 1;
            Ok::<(), BenchmarkError>(())
        })?;
        if generated != self.summary.signature_jobs {
            return Err(BenchmarkError::SignatureCountMismatch);
        }
        Ok(last_sighash)
    }

    /// Sign one fixed digest once per graph signature job.
    ///
    /// Keeping graph construction out of this loop makes this phase directly
    /// comparable with other hardware implementations.
    pub fn benchmark_fixed_digest_signing(&self) -> Result<[u8; 32], BenchmarkError> {
        let mut last_signature = [0_u8; 64];
        for _ in 0..self.summary.signature_jobs {
            last_signature = schnorr_sign_derived(&self.hww_private_key, FIXED_SIGNING_DIGEST)
                .map_err(|_| BenchmarkError::SignatureCreation)?;
        }
        let mut signature_commitment = [0_u8; 32];
        signature_commitment.copy_from_slice(&last_signature[..32]);
        Ok(signature_commitment)
    }
}

fn derive_benchmark_key() -> Result<(ECPrivateKey<32, 'W'>, [u8; 65]), BenchmarkError> {
    let (private_key, _) = Secp256k1::derive_from(&BENCHMARK_PATH);
    let public_key = private_key
        .public_key()
        .map_err(|_| BenchmarkError::Crypto)?;
    let public_key: [u8; 65] = public_key.into();
    Ok((private_key, normalize_bip340_public_key(public_key)?))
}

fn xonly(public_key: [u8; 65]) -> Result<[u8; 32], BenchmarkError> {
    if public_key[0] != 0x04 {
        return Err(BenchmarkError::Crypto);
    }
    let mut xonly = [0_u8; 32];
    xonly.copy_from_slice(&public_key[1..33]);
    Ok(xonly)
}

fn taproot_output_key(tweak: [u8; 32]) -> Result<[u8; 32], BenchmarkError> {
    let mut tweak_point = SECP256K1_GENERATOR;
    cx_ok(unsafe {
        sys::cx_ecfp_scalar_mult_no_throw(
            sys::CX_CURVE_SECP256K1,
            tweak_point.as_mut_ptr(),
            tweak.as_ptr(),
            tweak.len(),
        )
    })?;
    let mut output_point = [0_u8; 65];
    cx_ok(unsafe {
        sys::cx_ecfp_add_point_no_throw(
            sys::CX_CURVE_SECP256K1,
            output_point.as_mut_ptr(),
            BIP341_NUMS_POINT.as_ptr(),
            tweak_point.as_ptr(),
        )
    })?;
    xonly(output_point)
}

fn schnorr_sign_derived(
    private_key: &ECPrivateKey<32, 'W'>,
    message: [u8; 32],
) -> Result<[u8; 64], BenchmarkError> {
    let raw = private_key as *const ECPrivateKey<32, 'W'> as *const sys::cx_ecfp_private_key_t;
    schnorr_sign_raw(unsafe { &*raw }, message)
}

fn schnorr_sign_raw(
    private_key: &sys::cx_ecfp_private_key_t,
    message: [u8; 32],
) -> Result<[u8; 64], BenchmarkError> {
    let mut signature = [0_u8; 64];
    let mut signature_len = signature.len();
    let error = unsafe {
        sys::cx_ecschnorr_sign_no_throw(
            private_key,
            sys::CX_ECSCHNORR_BIP0340 | sys::CX_RND_PROVIDED,
            sys::CX_SHA256,
            message.as_ptr(),
            message.len(),
            signature.as_mut_ptr(),
            &mut signature_len,
        )
    };
    cx_ok(error)?;
    if signature_len != signature.len() {
        return Err(BenchmarkError::Crypto);
    }
    Ok(signature)
}

fn cx_ok(error: sys::cx_err_t) -> Result<(), BenchmarkError> {
    if error == sys::CX_OK {
        Ok(())
    } else {
        Err(BenchmarkError::Crypto)
    }
}
