//! Measure GPU vanity candidates per second for one kernel and print one JSON line:
//!
//! ```console
//! cargo run --release -p anzen-vanity --example benchmark -- nonce --seconds 4 --warmup 1
//! ```
//!
//! The target never matches in practice, so the rate is steady-state throughput.
//!
//! `pair <kernel> --base-shaders DIR` compiles the baseline shaders in DIR next to this build's
//! and alternates fixed-size dispatches between them, reporting the median of the paired
//! ratios. The GPU's clocks drift by tens of percent within seconds, so only dispatches
//! milliseconds apart compare fairly. `bench/ab.py` sets this up from a git revision.

use anzen_vanity::{GpuSearch, MAX_INDEX, NONCE_LOCKTIMES, NonceInputs, SearchInputs};
use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1).peekable();
    if args.peek().map(String::as_str) == Some("pair") {
        args.next();
        return pair(args.collect());
    }
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

fn pair(args: Vec<String>) -> anyhow::Result<()> {
    let kernel = args.first().cloned().unwrap_or_else(|| "nonce".into());
    let (mut base_dir, mut pairs, mut batch_log2) = (None, 40_usize, None);
    let mut rest = args.iter().skip(1);
    while let Some(flag) = rest.next() {
        let value = rest.next().expect("flag value");
        match flag.as_str() {
            "--base-shaders" => base_dir = Some(std::path::PathBuf::from(value)),
            "--pairs" => pairs = value.parse()?,
            "--batch-log2" => batch_log2 = Some(value.parse::<u32>()?),
            other => anyhow::bail!("unknown flag {other}"),
        }
    }
    let dir = base_dir.expect("--base-shaders DIR");
    let read = |name: &str| std::fs::read_to_string(dir.join(name));
    let common = read("constants.wgsl")? + &read("common.wgsl")?;
    let base = GpuSearch::open_with_sources(
        &(common.clone() + &read("key.wgsl")?),
        &(common + &read("nonce.wgsl")?),
    )?;
    let candidate = GpuSearch::open()?;
    let (prepare, batch): (
        fn(&GpuSearch) -> anyhow::Result<anzen_vanity::Prepared>,
        u32,
    ) = match kernel.as_str() {
        "key" => (
            |gpu| gpu.prepare_key(&key_inputs()),
            1 << batch_log2.unwrap_or(17),
        ),
        "nonce" => (
            |gpu| gpu.prepare_nonce(&nonce_inputs()),
            1 << batch_log2.unwrap_or(19),
        ),
        other => anyhow::bail!("unknown kernel {other}"),
    };
    let first = if kernel == "nonce" {
        NONCE_LOCKTIMES.start
    } else {
        0
    };
    let builds = [(&base, prepare(&base)?), (&candidate, prepare(&candidate)?)];
    let time = |index: usize, start: u32| -> anyhow::Result<f64> {
        let (gpu, prepared) = &builds[index];
        let started = Instant::now();
        gpu.run_batch_blocking(prepared, start, batch)?;
        Ok(f64::from(batch) / started.elapsed().as_secs_f64())
    };
    for round in 0..3 {
        time(0, first + round * batch)?;
        time(1, first + round * batch)?;
    }
    let (mut base_rates, mut candidate_rates, mut ratios) = (vec![], vec![], vec![]);
    for round in 0..pairs {
        let start = first + (round as u32 % 512) * batch;
        let (base_rate, candidate_rate) = if round % 2 == 0 {
            let base_rate = time(0, start)?;
            (base_rate, time(1, start)?)
        } else {
            let candidate_rate = time(1, start)?;
            (time(0, start)?, candidate_rate)
        };
        base_rates.push(base_rate);
        candidate_rates.push(candidate_rate);
        ratios.push(candidate_rate / base_rate);
    }
    let wins = ratios.iter().filter(|ratio| **ratio > 1.0).count();
    println!(
        "{{\"kernel\":\"{kernel}\",\"base\":{:.0},\"candidate\":{:.0},\"ratio\":{:.4},\"wins\":\"{wins}/{pairs}\"}}",
        median(&mut base_rates),
        median(&mut candidate_rates),
        median(&mut ratios)
    );
    Ok(())
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
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
