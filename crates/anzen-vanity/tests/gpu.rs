//! The shader must agree with the library code for every candidate. Skipped where no GPU adapter
//! is available.

use anzen_common::keys::DeviceKeys;
use anzen_vanity::{GpuSearch, NonceInputs};
use bitcoin::{
    Network,
    secp256k1::{Secp256k1, XOnlyPublicKey},
};

/// The nonce kernel's reference: the vault address Miniscript derives for locktime `locktime`.
fn library_nonce_output_key(
    internal: &XOnlyPublicKey,
    signer: &XOnlyPublicKey,
    locktime: u32,
) -> [u8; 32] {
    use std::str::FromStr;
    let descriptor = miniscript::Descriptor::<XOnlyPublicKey>::from_str(&format!(
        "tr({internal},and_v(v:after({locktime}),and_v(v:older(4259839),pk({signer}))))"
    ))
    .unwrap();
    descriptor.script_pubkey().as_bytes()[2..34]
        .try_into()
        .unwrap()
}

fn nonce_setup() -> Option<(GpuSearch, XOnlyPublicKey, XOnlyPublicKey, NonceInputs)> {
    let gpu = match GpuSearch::open() {
        Ok(gpu) => gpu,
        // Only a machine without a GPU skips; a rejected shader or pipeline must fail.
        Err(error) if error.to_string().contains("no GPU adapter") => {
            eprintln!("skipping: {error:#}");
            return None;
        }
        Err(error) => panic!("{error:#}"),
    };
    let secp = Secp256k1::new();
    let phone = DeviceKeys::generate_for_network(&secp, Network::Regtest).unwrap();
    let signer = DeviceKeys::generate_for_network(&secp, Network::Regtest).unwrap();
    let internal =
        anzen_common::policy::internal_key(phone.vault_pubkey, signer.vault_pubkey).unwrap();
    let inputs = NonceInputs {
        internal_key: internal.serialize(),
        signer_key: signer.vault_pubkey.serialize(),
        target_value: 0x5a,
        target_bits: 8,
    };
    Some((gpu, internal, signer.vault_pubkey, inputs))
}

#[test]
fn nonce_shader_output_keys_match_miniscript() {
    let Some((gpu, internal, signer, inputs)) = nonce_setup() else {
        return;
    };
    let range = anzen_vanity::NONCE_LOCKTIMES;
    for start in [range.start, 1_234_567_890, range.end - 256] {
        let keys = gpu.nonce_output_keys_blocking(&inputs, start, 256).unwrap();
        assert_eq!(keys.len(), 256);
        for (offset, key) in keys.into_iter().enumerate() {
            let locktime = start + offset as u32;
            assert_eq!(
                key,
                Some(library_nonce_output_key(&internal, &signer, locktime)),
                "locktime {locktime}"
            );
        }
    }
}

/// A batch runs in chunks of SEARCH_CHUNK candidates, each inverted PER_INVOCATION at a time:
/// check candidates on both sides of a chunk boundary and a partial batch at the end.
#[test]
fn nonce_output_keys_match_across_chunks() {
    let Some((gpu, internal, signer, inputs)) = nonce_setup() else {
        return;
    };
    let start = 1_500_000_000;
    let count = anzen_vanity::SEARCH_CHUNK + 37;
    let keys = gpu
        .nonce_output_keys_blocking(&inputs, start, count)
        .unwrap();
    assert_eq!(keys.len(), count as usize);
    let boundary = anzen_vanity::SEARCH_CHUNK;
    for offset in (0..40).chain(boundary - 40..count) {
        let locktime = start + offset;
        assert_eq!(
            keys[offset as usize],
            Some(library_nonce_output_key(&internal, &signer, locktime)),
            "locktime {locktime}"
        );
    }
}

#[test]
fn nonce_search_finds_the_first_match_miniscript_agrees_with() {
    let Some((gpu, internal, signer, inputs)) = nonce_setup() else {
        return;
    };
    let range = anzen_vanity::NONCE_LOCKTIMES;
    let result = gpu
        .search_nonces_blocking(&inputs, range.clone(), |_| true)
        .unwrap();
    let locktime = result.index.expect("an 8-bit target matches quickly");
    assert_eq!(
        library_nonce_output_key(&internal, &signer, locktime)[0],
        0x5a
    );
    for earlier in range.start..locktime {
        assert_ne!(
            library_nonce_output_key(&internal, &signer, earlier)[0],
            0x5a
        );
    }
}

/// A locktime whose tweak has a zero window-0 digit starts from Q itself, not a table entry with
/// a multiple of G added: window 0 is 20 bits, so about one candidate in a million; find one on the
/// CPU and check it.
#[test]
fn a_zero_first_window_digit_starts_from_q() {
    use bitcoin::{
        hashes::Hash,
        taproot::{LeafVersion, TapLeafHash, TapNodeHash, TapTweakHash},
    };
    let Some((gpu, internal, signer, inputs)) = nonce_setup() else {
        return;
    };
    let leaf = |locktime: u32| {
        let script = bitcoin::script::Builder::new()
            .push_int(i64::from(locktime))
            .push_opcode(bitcoin::opcodes::all::OP_CLTV)
            .push_opcode(bitcoin::opcodes::all::OP_VERIFY)
            .push_int(4_259_839)
            .push_opcode(bitcoin::opcodes::all::OP_CSV)
            .push_opcode(bitcoin::opcodes::all::OP_VERIFY)
            .push_x_only_key(&signer)
            .push_opcode(bitcoin::opcodes::all::OP_CHECKSIG)
            .into_script();
        TapLeafHash::from_script(&script, LeafVersion::TapScript)
    };
    // The tweak is a big-endian scalar, so window 0 is its low 20 bits.
    let locktime = (anzen_vanity::NONCE_LOCKTIMES.start..)
        .find(|&locktime| {
            let root = TapNodeHash::from(leaf(locktime));
            let tweak = TapTweakHash::from_key_and_tweak(internal, Some(root)).to_byte_array();
            tweak[29] & 0x0f == 0 && tweak[30] == 0 && tweak[31] == 0
        })
        .unwrap();
    let keys = gpu
        .nonce_output_keys_blocking(&inputs, locktime, 1)
        .unwrap();
    assert_eq!(
        keys[0],
        Some(library_nonce_output_key(&internal, &signer, locktime))
    );
}

/// The GPU builds the generator table from a small CPU one; every entry must match the table
/// the CPU builds directly.
#[test]
fn the_gpu_builds_the_same_generator_table_as_the_cpu() {
    let Some((gpu, ..)) = nonce_setup() else {
        return;
    };
    let (on_gpu, on_cpu) = gpu.generator_tables_blocking().unwrap();
    assert_eq!(on_gpu.len(), on_cpu.len());
    if let Some(word) = on_gpu.iter().zip(&on_cpu).position(|(a, b)| a != b) {
        panic!("table entry {} differs", word / 16);
    }
}

/// Each search's window-0 table must hold Q + d·G at index 2^19 + d: every edge of the small
/// tables' split, and a random sample. Apple's compiler once produced wrong entries here from
/// correct-looking code.
#[test]
fn the_window_zero_table_holds_q_plus_each_digit() {
    use bitcoin::secp256k1::{Parity, PublicKey, SecretKey};
    let Some((gpu, internal, _, inputs)) = nonce_setup() else {
        return;
    };
    let table = gpu
        .q_table_blocking(&gpu.prepare_nonce(&inputs).unwrap())
        .unwrap();
    let secp = Secp256k1::new();
    let q = PublicKey::from_x_only_public_key(internal, Parity::Even);
    let half: i64 = 1 << 19;
    let mut digits = vec![0, 1, 2, 511, 512, 513, 1023, 1024, 1025, half - 1, half];
    let mut state = 0x9e37_79b9_u64;
    for _ in 0..500 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        digits.push((state >> 40) as i64 % (half + 1));
    }
    for digit in digits.iter().flat_map(|digit| [*digit, -digit]) {
        let expected = if digit == 0 {
            q
        } else {
            let mut scalar = [0_u8; 32];
            scalar[24..].copy_from_slice(&digit.unsigned_abs().to_be_bytes());
            let multiple =
                PublicKey::from_secret_key(&secp, &SecretKey::from_slice(&scalar).unwrap());
            let multiple = if digit < 0 {
                multiple.negate(&secp)
            } else {
                multiple
            };
            q.combine(&multiple).unwrap()
        };
        let point = expected.serialize_uncompressed();
        // Little-endian 32-bit words of x, then of y.
        let words: Vec<u32> = [&point[1..33], &point[33..65]]
            .iter()
            .flat_map(|half| {
                (0..8).map(|i| u32::from_be_bytes(half[28 - 4 * i..32 - 4 * i].try_into().unwrap()))
            })
            .collect();
        let index = (half + digit) as usize;
        assert_eq!(
            table[index * 16..index * 16 + 16],
            words[..],
            "digit {digit}"
        );
    }
}
