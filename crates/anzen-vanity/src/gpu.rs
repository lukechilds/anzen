//! The wgpu host: shader setup, the generator table, parameter packing and batched dispatch.

use crate::{NONCE_LOCKTIMES, NonceInputs};
use anyhow::{Context, Result, anyhow, bail, ensure};
use secp256k1::{PublicKey, Scalar, Secp256k1};
use sha2::{Digest, Sha256, digest::generic_array::GenericArray};
use std::time::Duration;

const NONCE_SHADER: &str = concat!(
    include_str!("constants.wgsl"),
    include_str!("common.wgsl"),
    include_str!("field13.wgsl"),
    include_str!("nonce.wgsl")
);
const NONCE_PARAM_WORDS: usize = 60;
/// Candidates per dispatch to start with; the search adapts it towards `TARGET_BATCH_TIME`.
const FIRST_BATCH: u32 = 1 << 16;
const MAX_BATCH: u32 = 1 << 22;
const TARGET_BATCH_TIME: Duration = Duration::from_millis(250);

/// An open GPU device with the search kernel compiled.
pub struct GpuSearch {
    device: wgpu::Device,
    queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    search: wgpu::ComputePipeline,
    debug: wgpu::ComputePipeline,
    /// Each search's setup pass, which writes its window-0 table with Q added. Shaders without
    /// one (older baselines in A/B runs) start from Q instead.
    fold_q: Option<wgpu::ComputePipeline>,
    tuning: Tuning,
    g_table: wgpu::Buffer,
    adapter: String,
}

/// Constants the host reads from the shader's source so tables and dispatches match it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tuning {
    g_window_bits: u32,
    workgroup_size: u32,
    per_invocation: u32,
}

impl Tuning {
    fn from_source(source: &str) -> Result<Self> {
        let constant = |name: &str, default: Option<u32>| -> Result<u32> {
            let prefix = format!("const {name}: u32 = ");
            match source.find(&prefix) {
                Some(at) => {
                    let rest = &source[at + prefix.len()..];
                    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
                    digits
                        .parse()
                        .with_context(|| format!("shader constant {name} is not a literal"))
                }
                None => default.with_context(|| format!("shader has no {name}")),
            }
        };
        let tuning = Self {
            g_window_bits: constant("G_WINDOW_BITS", None)?,
            workgroup_size: constant("WORKGROUP_SIZE", None)?,
            per_invocation: constant("PER_INVOCATION", Some(1))?,
        };
        ensure!(
            (1..=20).contains(&tuning.g_window_bits)
                && tuning.workgroup_size > 0
                && tuning.per_invocation > 0,
            "shader tuning out of range: {tuning:?}"
        );
        Ok(tuning)
    }

    /// Workgroups for `count` candidates.
    fn workgroups(&self, count: u32) -> u32 {
        count
            .div_ceil(self.per_invocation)
            .div_ceil(self.workgroup_size)
    }
}

/// A finished search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchResult {
    /// The first matching recovery locktime found, if any.
    pub index: Option<u32>,
    /// Candidates evaluated, including the rest of the batch the match was found in.
    pub attempts: u64,
}

/// A search's parameter block and its window-0 table with Q added, built once and reused for
/// every batch.
#[doc(hidden)]
pub struct Prepared {
    params: Vec<u32>,
    q_table: wgpu::Buffer,
}

impl GpuSearch {
    /// Open the default high-performance adapter and compile the shader.
    pub async fn new() -> Result<Self> {
        Self::with_source(NONCE_SHADER).await
    }

    /// Open the GPU with another shader source, for benchmarking shader changes side by side.
    #[doc(hidden)]
    pub async fn with_source(source: &str) -> Result<Self> {
        let tuning = Tuning::from_source(source)?;
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|error| anyhow!("no GPU adapter is available: {error}"))?;
        let info = adapter.get_info();
        let limits = adapter.limits();
        let table_bytes = table_words(tuning.g_window_bits) as u64 * 4;
        ensure!(
            limits.max_storage_buffer_binding_size >= table_bytes
                && limits.max_buffer_size >= table_bytes,
            "the GPU cannot bind the precomputed table"
        );
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("anzen vanity"),
                required_limits: wgpu::Limits {
                    max_storage_buffer_binding_size: table_bytes,
                    max_buffer_size: table_bytes,
                    max_storage_buffers_per_shader_stage: 5,
                    ..wgpu::Limits::downlevel_defaults()
                }
                .using_resolution(limits),
                ..Default::default()
            })
            .await
            .context("could not open the GPU device")?;
        // Report a rejected shader or pipeline as an error rather than a panic. Not in browsers:
        // there wgpu 30 panics popping an empty scope (WebGPU resolves it to null, which
        // wasm-bindgen does not treat as none), so callers check the shader's output instead.
        #[cfg(not(target_arch = "wasm32"))]
        let scopes = [
            wgpu::ErrorFilter::Validation,
            wgpu::ErrorFilter::OutOfMemory,
        ]
        .map(|filter| device.push_error_scope(filter));
        let entry = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vanity"),
            entries: &[
                entry(0, true),
                entry(1, true),
                entry(2, false),
                entry(3, false),
                entry(4, false),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vanity"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let g_table = upload(
            &device,
            "generator table",
            &point_table(generator(), tuning.g_window_bits)?,
        );
        // SAFETY: every array index in the shader is in bounds by construction (fixed loop
        // bounds, table digits within their windows) and every loop has a constant trip count.
        // Runtime bounds checks and loop counters would otherwise stop the compiler unrolling the
        // field arithmetic.
        let module = unsafe {
            device.create_shader_module_trusted(
                wgpu::ShaderModuleDescriptor {
                    label: Some("nonce"),
                    source: wgpu::ShaderSource::Wgsl(source.into()),
                },
                wgpu::ShaderRuntimeChecks::unchecked(),
            )
        };
        let pipeline = |entry_point| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("nonce"),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some(entry_point),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let search = pipeline("search");
        let debug = pipeline("debug");
        let fold_q = source.contains("fn fold_q(").then(|| pipeline("fold_q"));
        #[cfg(not(target_arch = "wasm32"))]
        for scope in scopes.into_iter().rev() {
            if let Some(error) = scope.pop().await {
                bail!("the GPU rejected the vanity search: {error}");
            }
        }
        Ok(Self {
            device,
            queue,
            layout,
            search,
            debug,
            fold_q,
            tuning,
            g_table,
            // Browsers do not name the GPU.
            adapter: if info.name.is_empty() {
                format!("{:?}", info.backend)
            } else {
                format!("{} ({:?})", info.name, info.backend)
            },
        })
    }

    /// The adapter in use, for display.
    pub fn adapter_name(&self) -> &str {
        &self.adapter
    }

    /// Search recovery locktimes in `range` in order until a match, reporting the candidates
    /// tested so far; `progress` returns false to stop early. A match is the lowest matching
    /// locktime of its batch.
    pub async fn search_nonces(
        &self,
        inputs: &NonceInputs,
        range: std::ops::Range<u32>,
        progress: impl FnMut(u64) -> bool,
    ) -> Result<SearchResult> {
        ensure!(
            NONCE_LOCKTIMES.start <= range.start && range.end <= NONCE_LOCKTIMES.end,
            "recovery locktimes must be past Unix times"
        );
        let prepared = self.prepare_nonce(inputs)?;
        self.search_prepared(&prepared, range, progress).await
    }

    #[doc(hidden)]
    pub fn prepare_nonce(&self, inputs: &NonceInputs) -> Result<Prepared> {
        let params = encode_nonce_params(inputs)?;
        // Q + d·G for every window-0 digit d, built on the GPU: 2^G_WINDOW_BITS threads, each
        // with one addition and one inversion, so it takes milliseconds. Batches queued after it
        // see its writes.
        let Some(fold_q) = &self.fold_q else {
            return Ok(Prepared {
                params,
                q_table: self.scratch("no window 0"),
            });
        };
        let digits = 1_u32 << self.tuning.g_window_bits;
        let q_table = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("window 0 with Q"),
            size: u64::from(digits) * 16 * 4,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        // WebGPU rejects one writable buffer bound twice, so each unused binding gets its own.
        let bind_group = self.bind_group(&[
            (0, &upload(&self.device, "params", &params)),
            (1, &self.g_table),
            (2, &self.scratch("unused found")),
            (3, &self.scratch("unused debug")),
            (4, &q_table),
        ]);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(fold_q);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(digits.div_ceil(64), 1, 1);
        }
        self.queue.submit([encoder.finish()]);
        Ok(Prepared { params, q_table })
    }

    /// A small storage buffer for bindings a pass does not use.
    fn scratch(&self, label: &str) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: 4,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        })
    }

    fn bind_group(&self, entries: &[(u32, &wgpu::Buffer)]) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vanity"),
            layout: &self.layout,
            entries: &entries
                .iter()
                .map(|(binding, buffer)| wgpu::BindGroupEntry {
                    binding: *binding,
                    resource: buffer.as_entire_binding(),
                })
                .collect::<Vec<_>>(),
        })
    }

    /// Run one dispatch over `start..start + count`, returning the lowest match in it.
    #[doc(hidden)]
    pub async fn run_batch(
        &self,
        prepared: &Prepared,
        start: u32,
        count: u32,
    ) -> Result<Option<u32>> {
        let found = self.run(prepared, false, start, count).await?;
        Ok((found[0] != u32::MAX).then_some(found[0]))
    }

    async fn search_prepared(
        &self,
        prepared: &Prepared,
        range: std::ops::Range<u32>,
        mut progress: impl FnMut(u64) -> bool,
    ) -> Result<SearchResult> {
        let mut next = range.start;
        let mut batch = FIRST_BATCH;
        let mut attempts = 0_u64;
        while next < range.end {
            let count = batch.min(range.end - next);
            let started = now();
            let found = self.run_batch(prepared, next, count).await?;
            attempts += u64::from(count);
            if let Some(index) = found {
                return Ok(SearchResult {
                    index: Some(index),
                    attempts,
                });
            }
            if !progress(attempts) {
                break;
            }
            next += count;
            if let Some(elapsed) = started.map(|started| started.elapsed()) {
                batch = if elapsed < TARGET_BATCH_TIME / 2 {
                    (batch * 2).min(MAX_BATCH)
                } else if elapsed > TARGET_BATCH_TIME * 2 {
                    (batch / 2).max(1024)
                } else {
                    batch
                };
            } else {
                // No clock (in some browsers' workers): grow to a moderate fixed batch.
                batch = (batch * 2).min(1 << 18);
            }
        }
        Ok(SearchResult {
            index: None,
            attempts,
        })
    }

    /// The vault output key the shader computes for each locktime in `start..start + count`, or
    /// `None` where it rejects the candidate. For checking the shader against the CPU.
    pub async fn nonce_output_keys(
        &self,
        inputs: &NonceInputs,
        start: u32,
        count: u32,
    ) -> Result<Vec<Option<[u8; 32]>>> {
        let prepared = self.prepare_nonce(inputs)?;
        self.debug_keys(&prepared, start, count).await
    }

    /// The output keys for a prepared search, as [`Self::nonce_output_keys`].
    #[doc(hidden)]
    pub async fn debug_keys(
        &self,
        prepared: &Prepared,
        start: u32,
        count: u32,
    ) -> Result<Vec<Option<[u8; 32]>>> {
        let words = self.run(prepared, true, start, count).await?;
        Ok(words
            .chunks(9)
            .map(|chunk| {
                (chunk[0] == 1).then(|| {
                    let mut key = [0_u8; 32];
                    for (index, limb) in chunk[1..].iter().rev().enumerate() {
                        key[index * 4..index * 4 + 4].copy_from_slice(&limb.to_be_bytes());
                    }
                    key
                })
            })
            .collect())
    }

    /// Dispatch one kernel over `count` candidates from `start` and read back the found word, or
    /// with `debug` each candidate's validity and output key.
    async fn run(
        &self,
        prepared: &Prepared,
        debug: bool,
        start: u32,
        count: u32,
    ) -> Result<Vec<u32>> {
        let mut params = prepared.params.clone();
        params[0] = start;
        params[1] = count;
        let read_bytes = if debug { count as u64 * 36 } else { 4 };
        let params = upload(&self.device, "params", &params);
        let found = {
            use wgpu::util::DeviceExt;
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("found"),
                    contents: bytemuck::cast_slice(&[u32::MAX]),
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                })
        };
        let debug_out = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("debug"),
            size: if debug { read_bytes } else { 4 },
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: read_bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = self.bind_group(&[
            (0, &params),
            (1, &self.g_table),
            (2, &found),
            (3, &debug_out),
            (4, &prepared.q_table),
        ]);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(if debug { &self.debug } else { &self.search });
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(self.tuning.workgroups(count), 1, 1);
        }
        let source = if debug { &debug_out } else { &found };
        encoder.copy_buffer_to_buffer(source, 0, &readback, 0, read_bytes);
        self.queue.submit([encoder.finish()]);
        let (sender, receiver) = futures_channel::oneshot::channel();
        readback.map_async(wgpu::MapMode::Read, .., move |result| {
            let _ = sender.send(result);
        });
        #[cfg(not(target_arch = "wasm32"))]
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .context("GPU search failed")?;
        receiver
            .await
            .context("GPU readback was cancelled")?
            .context("GPU readback failed")?;
        let words = {
            let view = readback
                .get_mapped_range(..)
                .map_err(|error| anyhow!("GPU readback failed: {error:?}"))?;
            bytemuck::cast_slice(&view).to_vec()
        };
        readback.unmap();
        Ok(words)
    }
}

fn upload(device: &wgpu::Device, label: &str, words: &[u32]) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(words),
        usage: wgpu::BufferUsages::STORAGE,
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn now() -> Option<std::time::Instant> {
    Some(std::time::Instant::now())
}

#[cfg(target_arch = "wasm32")]
fn now() -> Option<std::time::Instant> {
    None
}

/// Blocking wrappers for native hosts.
#[cfg(not(target_arch = "wasm32"))]
impl GpuSearch {
    pub fn open() -> Result<Self> {
        pollster::block_on(Self::new())
    }

    #[doc(hidden)]
    pub fn open_with_source(source: &str) -> Result<Self> {
        pollster::block_on(Self::with_source(source))
    }

    /// Run the nonce shader's field self-test: `inputs` is a case count then 40 limbs per case;
    /// returns 56 words per case.
    #[doc(hidden)]
    pub fn field_test_blocking(&self, inputs: &[u32]) -> Result<Vec<u32>> {
        let cases = inputs[0];
        // SAFETY: as for the search kernels.
        let module = unsafe {
            self.device.create_shader_module_trusted(
                wgpu::ShaderModuleDescriptor {
                    label: Some("field test"),
                    source: wgpu::ShaderSource::Wgsl(NONCE_SHADER.into()),
                },
                wgpu::ShaderRuntimeChecks::unchecked(),
            )
        };
        let pipeline_layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("field test"),
                bind_group_layouts: &[Some(&self.layout)],
                immediate_size: 0,
            });
        let pipeline = self
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("field test"),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some("field_test"),
                compilation_options: Default::default(),
                cache: None,
            });
        let bytes = u64::from(cases) * 56 * 4;
        let params = upload(&self.device, "field inputs", inputs);
        let out = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("field outputs"),
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("field readback"),
            size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let unused = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("field test found"),
            size: 4,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let no_q_table = self.scratch("field test window 0");
        let bind_group = self.bind_group(&[
            (0, &params),
            (1, &self.g_table),
            (2, &unused),
            (3, &out),
            (4, &no_q_table),
        ]);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(cases.div_ceil(64), 1, 1);
        }
        encoder.copy_buffer_to_buffer(&out, 0, &readback, 0, bytes);
        self.queue.submit([encoder.finish()]);
        readback.map_async(wgpu::MapMode::Read, .., |_| {});
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .context("field test failed")?;
        let words = bytemuck::cast_slice(
            &readback
                .get_mapped_range(..)
                .map_err(|error| anyhow!("field test readback failed: {error:?}"))?,
        )
        .to_vec();
        Ok(words)
    }

    #[doc(hidden)]
    pub fn run_batch_blocking(
        &self,
        prepared: &Prepared,
        start: u32,
        count: u32,
    ) -> Result<Option<u32>> {
        pollster::block_on(self.run_batch(prepared, start, count))
    }

    pub fn search_nonces_blocking(
        &self,
        inputs: &NonceInputs,
        range: std::ops::Range<u32>,
        progress: impl FnMut(u64) -> bool,
    ) -> Result<SearchResult> {
        pollster::block_on(self.search_nonces(inputs, range, progress))
    }

    pub fn nonce_output_keys_blocking(
        &self,
        inputs: &NonceInputs,
        start: u32,
        count: u32,
    ) -> Result<Vec<Option<[u8; 32]>>> {
        pollster::block_on(self.nonce_output_keys(inputs, start, count))
    }
}

/// Field element bytes (big-endian) as little-endian 32-bit limbs.
fn limbs(bytes: &[u8]) -> [u32; 8] {
    let mut limbs = [0_u32; 8];
    for (index, chunk) in bytes.chunks(4).enumerate() {
        limbs[7 - index] = u32::from_be_bytes(chunk.try_into().expect("four bytes"));
    }
    limbs
}

fn affine(point: &PublicKey) -> ([u32; 8], [u32; 8]) {
    let uncompressed = point.serialize_uncompressed();
    (limbs(&uncompressed[1..33]), limbs(&uncompressed[33..65]))
}

const SHA256_IV: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];
/// The SHA-256 state after `tag || tag` for a BIP340 tagged hash.
fn tag_midstate(tag: &[u8]) -> [u32; 8] {
    let hash = Sha256::digest(tag);
    let mut block = [0_u8; 64];
    block[..32].copy_from_slice(&hash);
    block[32..].copy_from_slice(&hash);
    let mut state = SHA256_IV;
    sha2::compress256(&mut state, &[GenericArray::clone_from_slice(&block)]);
    state
}

/// The nonce kernel's parameter block, in its `P_*` order.
pub(crate) fn encode_nonce_params(inputs: &NonceInputs) -> Result<Vec<u32>> {
    ensure!(
        (1..=32).contains(&inputs.target_bits),
        "target must be between 1 and 32 bits"
    );
    let internal = signer_point(&inputs.internal_key).context("invalid internal key")?;
    signer_point(&inputs.signer_key)?;
    // 0xc0 || 47 || <T> OP_CLTV OP_VERIFY <4259839> OP_CSV OP_VERIFY <hww> OP_CHECKSIG, with T's
    // four little-endian bytes (always four for past Unix times) left zero for the shader.
    let mut block = [0_u8; 64];
    block[..16].copy_from_slice(&[
        0xc0, 47, 0x04, 0, 0, 0, 0, 0xb1, 0x69, 0x03, 0xff, 0xff, 0x40, 0xb2, 0x69, 32,
    ]);
    block[16..48].copy_from_slice(&inputs.signer_key);
    block[48] = 0xac;
    block[49] = 0x80;
    block[56..].copy_from_slice(&((64_u64 + 49) * 8).to_be_bytes());
    let words = |bytes: &[u8]| {
        bytes
            .chunks(4)
            .map(|chunk| u32::from_be_bytes(chunk.try_into().expect("four bytes")))
            .collect::<Vec<_>>()
    };
    let mut params = Vec::with_capacity(NONCE_PARAM_WORDS);
    params.extend([0, 0, inputs.target_value, inputs.target_bits]);
    params.extend(words(&block));
    params.extend(tag_midstate(b"TapLeaf"));
    params.extend(tag_midstate(b"TapTweak"));
    params.extend(words(&inputs.internal_key));
    let (internal_x, internal_y) = affine(&internal);
    params.extend(internal_x);
    params.extend(internal_y);
    ensure!(
        params.len() == NONCE_PARAM_WORDS,
        "parameter block size changed"
    );
    Ok(params)
}

/// The even-y point of an x-only key, as BIP327 key aggregation uses it.
fn signer_point(key: &[u8; 32]) -> Result<PublicKey> {
    let mut even = [0x02_u8; 33];
    even[1..].copy_from_slice(key);
    PublicKey::from_slice(&even).context("invalid signer key")
}

/// Words in a table of `bits`-wide windows: 16 per affine point.
fn table_words(bits: u32) -> usize {
    (256_usize).div_ceil(bits as usize) * ((1_usize << bits) - 1) * 16
}

pub(crate) fn generator() -> PublicKey {
    let mut generator = [0x02_u8; 33];
    generator[1..].copy_from_slice(&secp256k1::constants::GENERATOR_X);
    PublicKey::from_slice(&generator).expect("the generator is a valid point")
}

/// `d·2^(bits·w)·point` for every window `w` and digit `d` in `1..2^bits`, as affine limbs.
/// Windows are independent, so wide tables are built on every core.
pub(crate) fn point_table(point: PublicKey, bits: u32) -> Result<Vec<u32>> {
    let secp = Secp256k1::new();
    let mut shift = [0_u8; 32];
    shift[31 - bits as usize / 8] = 1 << (bits % 8);
    let shift = Scalar::from_be_bytes(shift)?;
    let mut bases = vec![point];
    for _ in 1..256_u32.div_ceil(bits) {
        let next = bases.last().expect("one base").mul_tweak(&secp, &shift)?;
        bases.push(next);
    }
    let window = |base: &PublicKey| -> Result<Vec<u32>> {
        let mut words = Vec::with_capacity(((1 << bits) - 1) * 16);
        let mut multiple = *base;
        for digit in 1..(1_u32 << bits) {
            if digit > 1 {
                multiple = multiple.combine(base)?;
            }
            let (x, y) = affine(&multiple);
            words.extend(x);
            words.extend(y);
        }
        Ok(words)
    };
    #[cfg(not(target_arch = "wasm32"))]
    let windows = std::thread::scope(|scope| {
        let handles = bases
            .iter()
            .map(|base| scope.spawn(move || window(base)))
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("table thread panicked"))
            .collect::<Result<Vec<_>>>()
    })?;
    #[cfg(target_arch = "wasm32")]
    let windows = bases.iter().map(window).collect::<Result<Vec<_>>>()?;
    let table = windows.concat();
    if table.len() != table_words(bits) {
        bail!("point table size changed");
    }
    Ok(table)
}
