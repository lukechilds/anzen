//! Measure GPU vanity candidates per second: `cargo run --release -p anzen-vanity --example benchmark`.

use anzen_vanity::{GpuSearch, SearchInputs};
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let gpu = GpuSearch::open()?;
    println!("adapter: {}", gpu.adapter_name());
    let mut parent_public_key = [0_u8; 33];
    parent_public_key[0] = 0x02;
    // The generator's x coordinate: any valid point will do.
    parent_public_key[1..].copy_from_slice(&hex(
        b"79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
    ));
    let inputs = SearchInputs {
        parent_public_key,
        chain_code: [7; 32],
        signer_key: hex(b"c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5"),
        merkle_root: [9; 32],
        // 32 bits never matches in the range searched, so this measures steady throughput.
        target_value: 0xffff_ffff,
        target_bits: 32,
    };
    // Warm up so pipeline compilation and batch sizing are not measured.
    gpu.search_blocking(&inputs, 0..1 << 18, |_| {})?;
    // Per-batch rates show the GPU clocking down under sustained load.
    let started = Instant::now();
    let mut last = (0_u64, Instant::now());
    let result = gpu.search_blocking(&inputs, 1 << 18..1 << 18 | 1 << 24, |attempts| {
        let now = Instant::now();
        let batch = attempts - last.0;
        let took = now.duration_since(last.1).as_secs_f64();
        last = (attempts, now);
        eprintln!(
            "{:6.1}s batch {batch:>8} in {:6.1}ms: {:>7.0}/s",
            started.elapsed().as_secs_f64(),
            took * 1000.0,
            batch as f64 / took
        );
    })?;
    let elapsed = started.elapsed().as_secs_f64();
    println!(
        "{} candidates in {elapsed:.2}s: {:.0} per second (match: {:?})",
        result.attempts,
        result.attempts as f64 / elapsed,
        result.index
    );
    Ok(())
}

fn hex(text: &[u8]) -> [u8; 32] {
    let mut bytes = [0_u8; 32];
    for (index, pair) in text.chunks(2).enumerate() {
        bytes[index] = u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap();
    }
    bytes
}
