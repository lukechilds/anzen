use anzen_cold_signer::benchmark::{
    BIP341_NUMS_XONLY, BenchmarkConfig, ControllerCommitment, FIXED_SIGNING_DIGEST,
    MAX_ROLLOVER_INPUTS, MAX_SIGNATURE_JOBS, PolicyCommitment, Sha256, WorkloadSummary,
};
use trezor_app_sdk::{
    Error, Result,
    crypto::{self, Hasher as TrezorHasher, sha2},
    util,
};

const HARDENED: u32 = 1 << 31;
const BENCHMARK_PATH: [u32; 5] = [86 | HARDENED, HARDENED, 100 | HARDENED, 0, 2_147_483_647];
const FIXED_PHONE_XONLY_PUBLIC_KEY: [u8; 32] = [
    0x79, 0xbe, 0x66, 0x7e, 0xf9, 0xdc, 0xbb, 0xac, 0x55, 0xa0, 0x62, 0x95, 0xce, 0x87, 0x0b, 0x07,
    0x02, 0x9b, 0xfc, 0xdb, 0x2d, 0xce, 0x28, 0xd9, 0x59, 0xf2, 0x81, 0x5b, 0x16, 0xf8, 0x17, 0x98,
];

pub struct PhaseResult {
    pub elapsed_us: u64,
    pub commitment: [u8; 32],
}

pub struct GraphResult {
    pub elapsed_us: u64,
    pub last_sighash: [u8; 32],
    pub summary: WorkloadSummary,
}

pub struct TrezorSha256;

impl Sha256 for TrezorSha256 {
    fn hash(&mut self, parts: &[&[u8]]) -> [u8; 32] {
        let mut hasher = sha2::Sha256::new(None);
        for part in parts {
            TrezorHasher::update(&mut hasher, part);
        }
        hasher.digest()
    }
}

pub fn benchmark_key_derivation() -> Result<PhaseResult> {
    let started_us = util::monotonic_micros();
    let public_key = crypto::get_public_key(&BENCHMARK_PATH, true)?;
    let elapsed_us = util::monotonic_micros().wrapping_sub(started_us);
    let commitment = public_key
        .as_slice()
        .try_into()
        .map_err(|_| Error::DataError("Invalid BIP340 public key length"))?;
    Ok(PhaseResult {
        elapsed_us,
        commitment,
    })
}

pub fn benchmark_graph(hww_xonly_public_key: [u8; 32]) -> Result<GraphResult> {
    let started_us = util::monotonic_micros();
    let mut hasher = TrezorSha256;
    let policy = PolicyCommitment::new(
        &mut hasher,
        FIXED_PHONE_XONLY_PUBLIC_KEY,
        hww_xonly_public_key,
    );
    let vault_output_key =
        crypto::bip340::tweak_public_key(&BIP341_NUMS_XONLY, &policy.merkle_root)?;
    let controller = ControllerCommitment::new(
        &mut hasher,
        FIXED_PHONE_XONLY_PUBLIC_KEY,
        hww_xonly_public_key,
    );
    let controller_output_key =
        crypto::bip340::tweak_public_key(&BIP341_NUMS_XONLY, &controller.merkle_root)?;
    let config = BenchmarkConfig::deterministic(
        &mut hasher,
        MAX_ROLLOVER_INPUTS as u8,
        vault_output_key,
        controller_output_key,
        policy.cooperative_leaf_hash,
    )
    .map_err(|_| Error::DataError("Failed to construct benchmark policy"))?;

    let mut last_sighash = [0_u8; 32];
    let summary = config
        .for_each_signature_job(&mut hasher, |job| {
            last_sighash = job.sighash;
            Ok::<(), ()>(())
        })
        .map_err(|_| Error::DataError("Failed to construct benchmark graph"))?;
    if summary.signature_jobs as usize != MAX_SIGNATURE_JOBS {
        return Err(Error::DataError("Unexpected benchmark signature count"));
    }

    Ok(GraphResult {
        elapsed_us: util::monotonic_micros().wrapping_sub(started_us),
        last_sighash,
        summary,
    })
}

pub fn benchmark_fixed_digest_signing(signature_count: u8) -> Result<PhaseResult> {
    if signature_count as usize > MAX_SIGNATURE_JOBS {
        return Err(Error::DataError("Too many benchmark signatures"));
    }
    let digests = [FIXED_SIGNING_DIGEST; MAX_SIGNATURE_JOBS];
    let started_us = util::monotonic_micros();
    let signatures =
        crypto::sign_bip340_digests(&BENCHMARK_PATH, &digests[..signature_count as usize])?;
    let elapsed_us = util::monotonic_micros().wrapping_sub(started_us);
    let last_signature = signatures
        .get(signatures.len().saturating_sub(64)..)
        .ok_or(Error::DataError("Missing benchmark signature"))?;
    let mut commitment = [0_u8; 32];
    commitment.copy_from_slice(&last_signature[..32]);
    Ok(PhaseResult {
        elapsed_us,
        commitment,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn benchmark_path_matches_the_declared_app_scope() {
        assert_eq!(BENCHMARK_PATH[0], 86 | HARDENED);
        assert_eq!(BENCHMARK_PATH[1], HARDENED);
        assert_eq!(BENCHMARK_PATH[2], 100 | HARDENED);
        assert_eq!(BENCHMARK_PATH[4], 2_147_483_647);
        assert!(
            include_str!("../Cargo.toml").contains("m/86'/coin_type'/100'/0/[0-2147483647]/**")
        );
    }

    #[test]
    fn sha256_adapter_matches_the_shared_fixed_digest() {
        let mut hasher = TrezorSha256;
        assert_eq!(
            hasher.hash(&[b"Anzen benchmark fixed BIP340 digest v1"]),
            FIXED_SIGNING_DIGEST
        );
    }
}
