//! Measure GPU vanity candidates per second and print one JSON line:
//!
//! ```console
//! cargo run --release -p anzen-vanity --example benchmark -- --seconds 4 --warmup 1
//! ```
//!
//! The target never matches in practice, so the rate is steady-state throughput.
//!
//! `run [seconds]` runs the app's benchmark instead, ramp guard included.
//!
//! `pair --base-shaders DIR` compiles the baseline shaders in DIR next to this build's and
//! alternates fixed-size dispatches between them, reporting the median of the paired ratios. The
//! GPU's clocks drift by tens of percent within seconds, so only dispatches milliseconds apart
//! compare fairly. `bench/ab.py` sets this up from a git revision.

use anzen_vanity::bench::nonce_inputs;
use anzen_vanity::{GpuSearch, NONCE_LOCKTIMES};
use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1).peekable();
    if args.peek().map(String::as_str) == Some("pair") {
        args.next();
        return pair(args.collect());
    }
    if args.peek().map(String::as_str) == Some("run") {
        args.next();
        let seconds = args.next().map_or(Ok(5.0), |value| value.parse())?;
        println!("{:?}", anzen_vanity::bench::run_wgpu_blocking(seconds)?);
        return Ok(());
    }
    // Builds from before the key kernel was removed took the kernel name first; accept it so
    // `bench/ab.py --binaries` can run old and new builds alike.
    if args.peek().map(String::as_str) == Some("nonce") {
        args.next();
    }
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
            let range = NONCE_LOCKTIMES.start + start..NONCE_LOCKTIMES.end;
            let result =
                gpu.search_nonces_blocking(&nonce_inputs(), range, |_| Instant::now() < deadline)?;
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
        "{{\"rate\":{:.0},\"attempts\":{attempts},\"seconds\":{elapsed:.3},\"adapter\":\"{}\"}}",
        attempts as f64 / elapsed,
        gpu.adapter_name()
    );
    Ok(())
}

fn pair(args: Vec<String>) -> anyhow::Result<()> {
    let (mut base_dir, mut pairs, mut batch_log2) = (None, 40_usize, 19_u32);
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        let value = rest.next().expect("flag value");
        match flag.as_str() {
            "--base-shaders" => base_dir = Some(std::path::PathBuf::from(value)),
            "--pairs" => pairs = value.parse()?,
            "--batch-log2" => batch_log2 = value.parse()?,
            other => anyhow::bail!("unknown flag {other}"),
        }
    }
    let dir = base_dir.expect("--base-shaders DIR");
    let read = |name: &str| std::fs::read_to_string(dir.join(name));
    let base = GpuSearch::open_with_source(
        &(read("constants.wgsl")?
            + &read("common.wgsl")?
            + &read("field13.wgsl")?
            + &read("nonce.wgsl")?),
    )?;
    let candidate = GpuSearch::open()?;
    let batch = 1 << batch_log2;
    let first = NONCE_LOCKTIMES.start;
    let builds = [
        (&base, base.prepare_nonce(&nonce_inputs())?),
        (&candidate, candidate.prepare_nonce(&nonce_inputs())?),
    ];
    let time = |index: usize, start: u32| -> anyhow::Result<f64> {
        let (gpu, prepared) = &builds[index];
        let started = Instant::now();
        gpu.run_batch_blocking(prepared, start, batch)?;
        Ok(f64::from(batch) / started.elapsed().as_secs_f64())
    };
    for (index, (gpu, prepared)) in builds.iter().enumerate() {
        ramp(&format!("build {index}"), batch, |count| {
            gpu.run_batch_blocking(prepared, first, count)?;
            Ok(())
        })?;
    }
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
        "{{\"base\":{:.0},\"candidate\":{:.0},\"ratio\":{:.4},\"wins\":\"{wins}/{pairs}\"}}",
        median(&mut base_rates),
        median(&mut candidate_rates),
        median(&mut ratios)
    );
    Ok(())
}

/// Grow dispatches from 256 candidates to `full` by 4× steps, refusing to go on if a step takes
/// over 2 s or costs over 4× more per candidate than the previous one. Killing a process does
/// not cancel GPU work it queued, and two kernels have hung the GPU at full load after small
/// dispatches ran fine, so a kernel must prove it scales before it gets a full batch.
fn ramp(
    name: &str,
    full: u32,
    mut run: impl FnMut(u32) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let mut count = 256_u32;
    let mut previous: Option<f64> = None;
    loop {
        let started = Instant::now();
        run(count)?;
        let elapsed = started.elapsed();
        let per_candidate = elapsed.as_secs_f64() / f64::from(count);
        anyhow::ensure!(
            elapsed < Duration::from_secs(2),
            "{name}: {count} candidates took {elapsed:?}; not queuing more"
        );
        if let Some(previous) = previous {
            anyhow::ensure!(
                per_candidate < previous * 4.0,
                "{name}: {count} candidates cost {:.1}x more each than the last step; not queuing more",
                per_candidate / previous
            );
        }
        if count >= full {
            return Ok(());
        }
        // Small dispatches are latency-bound, so compare against the cheaper of the two.
        previous = Some(previous.map_or(per_candidate, |p: f64| p.min(per_candidate)));
        count = (count * 4).min(full);
    }
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}
