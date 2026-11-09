//! The 13-bit-limb field arithmetic against arbitrary-precision integers, on edge cases (values
//! near p, 2^256 and 2^260, and non-canonical forms the normalizer can produce) and random ones.

use anzen_vanity::GpuSearch;
use num_bigint::BigUint;

fn p() -> BigUint {
    (BigUint::from(1_u8) << 256) - (BigUint::from(1_u8) << 32) - BigUint::from(977_u32)
}

/// The standard 13-bit decomposition of a value below 2^260.
fn limbs(value: &BigUint) -> [u32; 20] {
    let mask = BigUint::from(0x1fff_u32);
    std::array::from_fn(|i| {
        let limb: BigUint = (value >> (13 * i)) & &mask;
        limb.to_u32_digits().first().copied().unwrap_or(0)
    })
}

fn from_words(words: &[u32]) -> BigUint {
    BigUint::from_slice(words)
}

#[test]
fn field_operations_match_bignum_arithmetic() {
    let gpu = match GpuSearch::open() {
        Ok(gpu) => gpu,
        // Only a machine without a GPU skips; a rejected shader or pipeline must fail.
        Err(error) if error.to_string().contains("no GPU adapter") => {
            eprintln!("skipping: {error:#}");
            return;
        }
        Err(error) => panic!("{error:#}"),
    };
    let p = p();
    let one = BigUint::from(1_u8);
    let two_256: BigUint = &one << 256;
    let two_260: BigUint = &one << 260;
    let mut values = vec![
        BigUint::from(0_u8),
        one.clone(),
        BigUint::from(2_u8),
        &p - 1_u8,
        p.clone(),
        &p + 1_u8,
        &p * 2_u8,
        &p * 15_u8,
        &two_256 - 1_u8,
        two_256.clone(),
        &two_256 + 977_u32,
        &two_260 - 1_u8,
        &two_260 - &p,
        (&one << 255) - 1_u8,
        BigUint::from(0x1fff_u32),
    ];
    // Deterministic pseudorandom values below 2^260.
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    for _ in 0..400 {
        let mut words = [0_u32; 9];
        for word in &mut words {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *word = state as u32;
        }
        words[8] &= 0xf;
        values.push(BigUint::from_slice(&words));
    }
    let mut cases = Vec::new();
    for (index, a) in values.iter().enumerate() {
        for b in [
            &values[(index * 7 + 3) % values.len()],
            &values[index % 15],
            a,
        ] {
            cases.push((a.clone(), b.clone()));
        }
    }
    let mut inputs = vec![cases.len() as u32];
    for (case, (a, b)) in cases.iter().enumerate() {
        let mut a_limbs = limbs(a);
        // Normalized outputs can carry limb 3 at exactly 2^13; feed that form too.
        if case % 5 == 0 && a_limbs[3] == 0 && a_limbs[4] > 0 {
            a_limbs[3] = 0x2000;
            a_limbs[4] -= 1;
        }
        inputs.extend(a_limbs);
        inputs.extend(limbs(b));
    }
    let output = gpu.field_test_blocking(&inputs).unwrap();
    for (case, (a, b)) in cases.iter().enumerate() {
        let results = &output[case * 56..case * 56 + 56];
        let result = |op: usize| from_words(&results[op * 8..op * 8 + 8]);
        let (a, b) = (a % &p, b % &p);
        assert_eq!(result(0), (&a * &b) % &p, "mul {a} {b}");
        assert_eq!(result(1), (&a * &a) % &p, "sqr {a}");
        assert_eq!(result(2), (&a + &b) % &p, "add {a} {b}");
        assert_eq!(result(3), (&a + &p - &b) % &p, "sub {a} {b}");
        assert_eq!(result(4), (&p - &a) % &p, "neg {a}");
        if a != BigUint::from(0_u8) {
            assert_eq!(result(5), a.modpow(&(&p - 2_u8), &p), "inv {a}");
        }
        assert_eq!(result(6), a, "canonical {a}");
    }
}
