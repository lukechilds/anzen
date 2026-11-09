//! Throughput benchmarks on fixed public inputs, for comparing devices and backends.
//!
//! [`Plan`] paces a run without touching the GPU or a clock itself, so native hosts and browsers
//! drive the same steps: grow dispatches from 256 candidates under a guard, warm up, then count
//! candidates over a measured window. The target never matches in practice, so the rate is
//! steady-state throughput.

use crate::{NONCE_LOCKTIMES, NonceInputs, SearchInputs};
use anyhow::{Result, bail, ensure};

/// Which search to time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kernel {
    /// Key grinding: vault key indexes.
    Key,
    /// Script nonce grinding: recovery locktimes.
    Nonce,
}

impl Kernel {
    pub fn parse(name: &str) -> Result<Self> {
        match name {
            "key" => Ok(Self::Key),
            "nonce" => Ok(Self::Nonce),
            other => bail!("unknown kernel {other}"),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Key => "key",
            Self::Nonce => "nonce",
        }
    }

    /// The first candidate: key indexes start at 0, locktimes at the first past Unix time.
    fn first(self) -> u32 {
        match self {
            Self::Key => 0,
            Self::Nonce => NONCE_LOCKTIMES.start,
        }
    }

    /// The largest dispatch: the batch the desktop benchmarks were tuned at.
    fn full_batch(self) -> u32 {
        match self {
            Self::Key => 1 << 17,
            Self::Nonce => 1 << 20,
        }
    }
}

/// The first ramp step, in candidates.
const FIRST_BATCH: u32 = 256;
/// Dispatches stop growing once the next ×4 step would pass this. Short dispatches keep a
/// phone's display responsive while the GPU is saturated.
const TARGET_DISPATCH_SECONDS: f64 = 0.25;
/// The ramp's guard: a single dispatch this slow, or one costing 4× more per candidate than the
/// step before, aborts the run. Killing a process does not cancel GPU work it queued, so a
/// kernel must prove it scales before it gets a full batch.
const MAX_DISPATCH_SECONDS: f64 = 2.0;
const WARMUP_SECONDS: f64 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// One small dispatch that pays the first use's setup costs, untimed but guarded.
    Prime,
    Ramp,
    Warmup,
    Measure,
    Done,
}

/// The steps of one benchmark run. Call [`Plan::next`] for the next dispatch, run it, then
/// [`Plan::record`] how long it took, until `next` returns `None`.
#[derive(Debug, Clone)]
pub struct Plan {
    kernel: Kernel,
    phase: Phase,
    count: u32,
    offset: u32,
    /// The cheapest per-candidate cost seen while ramping.
    cheapest: Option<f64>,
    phase_seconds: f64,
    measure_seconds: f64,
    attempts: u64,
    measured_seconds: f64,
}

impl Plan {
    pub fn new(kernel: Kernel, seconds: f64) -> Self {
        Self {
            kernel,
            phase: Phase::Prime,
            count: FIRST_BATCH,
            offset: 0,
            cheapest: None,
            phase_seconds: 0.0,
            measure_seconds: seconds.max(0.1),
            attempts: 0,
            measured_seconds: 0.0,
        }
    }

    /// The next dispatch as (first candidate, count), or `None` when the run is over.
    pub fn next(&self) -> Option<(u32, u32)> {
        (self.phase != Phase::Done).then(|| (self.kernel.first() + self.offset, self.count))
    }

    /// Record how long the dispatch from [`Plan::next`] took, in seconds.
    pub fn record(&mut self, seconds: f64) -> Result<()> {
        let count = self.count;
        // Stay well inside each kernel's candidate range.
        self.offset = (self.offset + count) % (1 << 29);
        match self.phase {
            Phase::Prime => {
                ensure!(
                    seconds < MAX_DISPATCH_SECONDS,
                    "{count} candidates took {seconds:.2} s; not queuing more"
                );
                self.phase = Phase::Ramp;
            }
            Phase::Ramp => {
                ensure!(
                    seconds < MAX_DISPATCH_SECONDS,
                    "{count} candidates took {seconds:.2} s; not queuing more"
                );
                let per_candidate = seconds / f64::from(count);
                if let Some(cheapest) = self.cheapest {
                    ensure!(
                        per_candidate < cheapest * 4.0,
                        "{count} candidates cost {:.1}× more each than the last step; not queuing more",
                        per_candidate / cheapest
                    );
                }
                // Small dispatches are latency-bound, so compare against the cheapest step.
                self.cheapest = Some(
                    self.cheapest
                        .map_or(per_candidate, |c| c.min(per_candidate)),
                );
                if count >= self.kernel.full_batch() || seconds * 4.0 > TARGET_DISPATCH_SECONDS {
                    self.phase = Phase::Warmup;
                } else {
                    self.count = (count * 4).min(self.kernel.full_batch());
                }
            }
            Phase::Warmup => {
                self.phase_seconds += seconds;
                if self.phase_seconds >= WARMUP_SECONDS {
                    self.phase = Phase::Measure;
                }
            }
            Phase::Measure => {
                self.attempts += u64::from(count);
                self.measured_seconds += seconds;
                if self.measured_seconds >= self.measure_seconds {
                    self.phase = Phase::Done;
                }
            }
            Phase::Done => bail!("the benchmark is already finished"),
        }
        Ok(())
    }

    /// The measured result, once [`Plan::next`] has returned `None`.
    pub fn measurement(&self) -> Measurement {
        Measurement {
            batch: self.count,
            attempts: self.attempts,
            seconds: self.measured_seconds,
            rate: self.attempts as f64 / self.measured_seconds.max(f64::MIN_POSITIVE),
        }
    }
}

/// A finished run's throughput.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measurement {
    /// Candidates per dispatch after the ramp.
    pub batch: u32,
    /// Candidates evaluated in the measured window.
    pub attempts: u64,
    /// The measured window's GPU time, in seconds.
    pub seconds: f64,
    /// Candidates per second.
    pub rate: f64,
}

/// A finished benchmark.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    /// The GPU, as its API names it.
    pub adapter: String,
    /// Opening the GPU, compiling the kernels and building the point tables, in seconds.
    pub setup_seconds: f64,
    pub measurement: Measurement,
}

/// A fixed, valid key search. 32 bits of target match about once in four billion candidates.
pub fn key_inputs() -> SearchInputs {
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

pub fn nonce_inputs() -> NonceInputs {
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

/// Time `kernel` through wgpu for `seconds`. `now` is a clock in seconds: browsers have no
/// `std::time::Instant`, so they pass `performance.now()`.
pub async fn run_wgpu(kernel: Kernel, seconds: f64, now: impl Fn() -> f64) -> Result<Report> {
    let started = now();
    let gpu = crate::GpuSearch::new().await?;
    let prepared = match kernel {
        Kernel::Key => gpu.prepare_key(&key_inputs())?,
        Kernel::Nonce => gpu.prepare_nonce(&nonce_inputs())?,
    };
    let setup_seconds = now() - started;
    let mut plan = Plan::new(kernel, seconds);
    while let Some((start, count)) = plan.next() {
        let started = now();
        gpu.run_batch(&prepared, start, count).await?;
        plan.record(now() - started)?;
    }
    Ok(Report {
        adapter: gpu.adapter_name().into(),
        setup_seconds,
        measurement: plan.measurement(),
    })
}

/// [`run_wgpu`] on a native host, blocking.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_wgpu_blocking(kernel: Kernel, seconds: f64) -> Result<Report> {
    let epoch = std::time::Instant::now();
    pollster::block_on(run_wgpu(kernel, seconds, || epoch.elapsed().as_secs_f64()))
}

/// Time `kernel` through native Metal for `seconds`.
#[cfg(all(feature = "metal", target_vendor = "apple"))]
pub fn run_metal(kernel: Kernel, seconds: f64) -> Result<Report> {
    let epoch = std::time::Instant::now();
    let metal = crate::MetalSearch::open()?;
    let prepared = match kernel {
        Kernel::Key => metal.prepare_key(&key_inputs())?,
        Kernel::Nonce => metal.prepare_nonce(&nonce_inputs())?,
    };
    let setup_seconds = epoch.elapsed().as_secs_f64();
    let mut plan = Plan::new(kernel, seconds);
    while let Some((start, count)) = plan.next() {
        let started = std::time::Instant::now();
        metal.run_batch(&prepared, start, count)?;
        plan.record(started.elapsed().as_secs_f64())?;
    }
    Ok(Report {
        adapter: metal.device_name().into(),
        setup_seconds,
        measurement: plan.measurement(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(plan: &mut Plan, per_candidate: f64) -> Vec<u32> {
        let mut counts = vec![];
        while let Some((_, count)) = plan.next() {
            counts.push(count);
            plan.record(f64::from(count) * per_candidate).unwrap();
        }
        counts
    }

    #[test]
    fn ramps_to_the_full_batch_on_a_fast_gpu() {
        let mut plan = Plan::new(Kernel::Nonce, 2.0);
        let counts = drive(&mut plan, 1.0 / 24e6);
        assert_eq!(
            &counts[..8],
            &[256, 256, 1024, 4096, 16384, 65536, 262144, 1 << 20]
        );
        let measurement = plan.measurement();
        assert_eq!(measurement.batch, 1 << 20);
        assert!((measurement.rate - 24e6).abs() < 1.0);
        assert!(measurement.seconds >= 2.0);
    }

    #[test]
    fn keeps_dispatches_short_on_a_slow_gpu() {
        let mut plan = Plan::new(Kernel::Key, 1.0);
        let per_candidate = 1.0 / 300e3;
        drive(&mut plan, per_candidate);
        let batch = plan.measurement().batch;
        assert!(batch < Kernel::Key.full_batch());
        assert!(f64::from(batch) * per_candidate <= TARGET_DISPATCH_SECONDS);
    }

    #[test]
    fn stops_when_a_step_slows_down_sharply() {
        let mut plan = Plan::new(Kernel::Key, 1.0);
        plan.record(0.5).unwrap();
        plan.record(256.0 * 1e-6).unwrap();
        let error = plan.record(1024.0 * 5e-6).unwrap_err();
        assert!(error.to_string().contains("not queuing more"));
    }

    #[test]
    fn stops_on_a_slow_dispatch() {
        let mut plan = Plan::new(Kernel::Key, 1.0);
        assert!(plan.record(3.0).is_err());
    }
}
