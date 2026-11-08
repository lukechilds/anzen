//! Measure GPU vanity candidates per second for one kernel and print one JSON line:
//!
//! ```console
//! cargo run --release -p anzen-vanity --example benchmark -- nonce --seconds 4 --warmup 1
//! ```
//!
//! The target never matches in practice, so the rate is steady-state throughput. Compare
//! changes with `bench/ab.py`, which alternates a baseline build with the working tree because
//! the GPU clocks down under sustained load.

use anzen_vanity::{GpuSearch, MAX_INDEX, NONCE_LOCKTIMES, NonceInputs, SearchInputs};
use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let kernel = args.next().unwrap_or_else(|| "nonce".into());
    let (mut seconds, mut warmup) = (4.0, 1.0);
    while let Some(flag) = args.next() {
        let value: f64 = args.next().expect("flag value").parse()?;
        match flag.as_str() {
            "--seconds" => seconds = value,
            "--warmup" => warmup = value,
            other => anyhow::bail!("unknown flag {other}"),
        }
    }
    let gpu = GpuSearch::open()?;
    let measure = |duration: f64| -> anyhow::Result<u64> {
        let deadline = Instant::now() + Duration::from_secs_f64(duration);
        let mut attempts = 0_u64;
        let mut start = 0_u32;
        while Instant::now() < deadline {
            let result = match kernel.as_str() {
                "key" => gpu.search_blocking(&key_inputs(), start..MAX_INDEX + 1, |_| {
                    Instant::now() < deadline
                })?,
                "nonce" => {
                    let range = NONCE_LOCKTIMES.start + start..NONCE_LOCKTIMES.end;
                    gpu.search_nonces_blocking(&nonce_inputs(), range, |_| {
                        Instant::now() < deadline
                    })?
                }
                other => anyhow::bail!("unknown kernel {other}"),
            };
            attempts += result.attempts;
            // A chance match ends the search early; carry on after it.
            start = start.wrapping_add(result.attempts as u32) % (1 << 30);
        }
        Ok(attempts)
    };
    measure(warmup)?;
    let started = Instant::now();
    let attempts = measure(seconds)?;
    let elapsed = started.elapsed().as_secs_f64();
    println!(
        "{{\"kernel\":\"{kernel}\",\"rate\":{:.0},\"attempts\":{attempts},\"seconds\":{elapsed:.3},\"adapter\":\"{}\"}}",
        attempts as f64 / elapsed,
        gpu.adapter_name()
    );
    Ok(())
}

/// A fixed, valid key search. 32 bits of target match about once in four billion candidates.
fn key_inputs() -> SearchInputs {
    let mut parent_public_key = [0x02_u8; 33];
    parent_public_key[1..].copy_from_slice(&GENERATOR_X);
    SearchInputs {
        parent_public_key,
        chain_code: [7; 32],
        signer_key: SIGNER,
        merkle_root: [9; 32],
        target_value: 0xffff_ffff,
        target_bits: 32,
    }
}

fn nonce_inputs() -> NonceInputs {
    NonceInputs {
        internal_key: GENERATOR_X,
        signer_key: SIGNER,
        target_value: 0xffff_ffff,
        target_bits: 32,
    }
}

/// G's x coordinate and 2G's: any valid x-only keys will do.
const GENERATOR_X: [u8; 32] = [
    0x79, 0xbe, 0x66, 0x7e, 0xf9, 0xdc, 0xbb, 0xac, 0x55, 0xa0, 0x62, 0x95, 0xce, 0x87, 0x0b, 0x07,
    0x02, 0x9b, 0xfc, 0xdb, 0x2d, 0xce, 0x28, 0xd9, 0x59, 0xf2, 0x81, 0x5b, 0x16, 0xf8, 0x17, 0x98,
];
const SIGNER: [u8; 32] = [
    0xc6, 0x04, 0x7f, 0x94, 0x41, 0xed, 0x7d, 0x6d, 0x30, 0x45, 0x40, 0x6e, 0x95, 0xc0, 0x7c, 0xd8,
    0x5c, 0x77, 0x8e, 0x4b, 0x8c, 0xef, 0x3c, 0xa7, 0xab, 0xac, 0x09, 0xb9, 0x5c, 0x70, 0x9e, 0xe5,
];
