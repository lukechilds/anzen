//! The wgpu host: shader setup, point tables, parameter packing and batched dispatch.

use crate::{MAX_INDEX, NONCE_LOCKTIMES, NonceInputs, SearchInputs};
use anyhow::{Context, Result, anyhow, bail, ensure};
use secp256k1::{PublicKey, Scalar, Secp256k1};
use sha2::{Digest, Sha256, digest::generic_array::GenericArray};
use std::time::Duration;

const KEY_SHADER: &str = concat!(
    include_str!("constants.wgsl"),
    include_str!("common.wgsl"),
    include_str!("key.wgsl")
);
const NONCE_SHADER: &str = concat!(
    include_str!("constants.wgsl"),
    include_str!("common.wgsl"),
    include_str!("nonce.wgsl")
);
const PARAM_WORDS: usize = 132;
const NONCE_PARAM_WORDS: usize = 60;
/// The key kernel's signer and parent tables use 8-bit windows.
const KEY_TABLE_BITS: u32 = 8;
/// Candidates per dispatch to start with; the search adapts it towards `TARGET_BATCH_TIME`.
const FIRST_BATCH: u32 = 1 << 16;
const MAX_BATCH: u32 = 1 << 22;
const TARGET_BATCH_TIME: Duration = Duration::from_millis(250);

/// An open GPU device with both search kernels compiled.
pub struct GpuSearch {
    device: wgpu::Device,
    queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    key: Kernel,
    nonce: Kernel,
    /// Bound in place of the key tables for the nonce kernel, which has none.
    empty: wgpu::Buffer,
    adapter: String,
}

/// One kernel's entry points and the G table its tuning asks for.
struct Kernel {
    search: wgpu::ComputePipeline,
    debug: wgpu::ComputePipeline,
    tuning: Tuning,
    g_table: wgpu::Buffer,
}

/// Constants the host reads from a shader's source so tables and dispatches match it.
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
    /// The first matching index found, if any.
    pub index: Option<u32>,
    /// Candidates evaluated, including the rest of the batch the match was found in.
    pub attempts: u64,
}

/// A search's parameter block and point tables, built once and reused for every batch.
#[doc(hidden)]
pub struct Prepared {
    nonce: bool,
    params: Vec<u32>,
    /// The key kernel's signer and parent tables.
    key_tables: Option<wgpu::Buffer>,
}

impl GpuSearch {
    /// Open the default high-performance adapter and compile the shaders.
    pub async fn new() -> Result<Self> {
        Self::with_sources(KEY_SHADER, NONCE_SHADER).await
    }

    /// Open the GPU with other shader sources, for benchmarking shader changes side by side.
    #[doc(hidden)]
    pub async fn with_sources(key_shader: &str, nonce_shader: &str) -> Result<Self> {
        let tunings = [
            Tuning::from_source(key_shader)?,
            Tuning::from_source(nonce_shader)?,
        ];
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
        let largest_table = tunings
            .iter()
            .map(|tuning| table_words(tuning.g_window_bits))
            .chain([2 * table_words(KEY_TABLE_BITS)])
            .max()
            .expect("tables")
            * 4;
        ensure!(
            limits.max_storage_buffer_binding_size >= largest_table as u64
                && limits.max_buffer_size >= largest_table as u64,
            "the GPU cannot bind the precomputed tables"
        );
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("anzen vanity"),
                required_limits: wgpu::Limits {
                    max_storage_buffer_binding_size: largest_table as u64,
                    max_buffer_size: largest_table as u64,
                    max_storage_buffers_per_shader_stage: 5,
                    ..wgpu::Limits::downlevel_defaults()
                }
                .using_resolution(limits),
                ..Default::default()
            })
            .await
            .context("could not open the GPU device")?;
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
                entry(4, true),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vanity"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let key_table = upload(
            &device,
            "generator table",
            &point_table(generator(), tunings[0].g_window_bits)?,
        );
        let nonce_table = if tunings[1].g_window_bits == tunings[0].g_window_bits {
            key_table.clone()
        } else {
            upload(
                &device,
                "generator table",
                &point_table(generator(), tunings[1].g_window_bits)?,
            )
        };
        let kernel = |label, source: &str, tuning: Tuning, g_table| -> Result<Kernel> {
            // SAFETY: every array index in the shaders is in bounds by construction (fixed loop
            // bounds, table digits within their windows) and every loop has a constant trip
            // count. Runtime bounds checks and loop counters would otherwise stop the compiler
            // unrolling the field arithmetic.
            let module = unsafe {
                device.create_shader_module_trusted(
                    wgpu::ShaderModuleDescriptor {
                        label: Some(label),
                        source: wgpu::ShaderSource::Wgsl(source.into()),
                    },
                    wgpu::ShaderRuntimeChecks::unchecked(),
                )
            };
            let pipeline = |entry_point| {
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(label),
                    layout: Some(&pipeline_layout),
                    module: &module,
                    entry_point: Some(entry_point),
                    compilation_options: Default::default(),
                    cache: None,
                })
            };
            Ok(Kernel {
                search: pipeline("search"),
                debug: pipeline("debug"),
                tuning,
                g_table,
            })
        };
        let key = kernel("key", key_shader, tunings[0], key_table)?;
        let nonce = kernel("nonce", nonce_shader, tunings[1], nonce_table)?;
        let empty = upload(&device, "no key tables", &[0; 4]);
        Ok(Self {
            device,
            queue,
            layout,
            key,
            nonce,
            empty,
            adapter: format!("{} ({:?})", info.name, info.backend),
        })
    }

    /// The adapter in use, for display.
    pub fn adapter_name(&self) -> &str {
        &self.adapter
    }

    /// Search key indexes in `range` in order until a match, reporting the candidates tested so
    /// far; `progress` returns false to stop early. A match is the lowest matching index of its
    /// batch.
    pub async fn search(
        &self,
        inputs: &SearchInputs,
        range: std::ops::Range<u32>,
        progress: impl FnMut(u64) -> bool,
    ) -> Result<SearchResult> {
        ensure!(
            range.end <= MAX_INDEX + 1,
            "vault key indexes must be below 2^31"
        );
        let prepared = self.prepare_key(inputs)?;
        self.search_prepared(&prepared, range, progress).await
    }

    /// Search recovery locktimes in `range` in order until a match, like [`Self::search`].
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
    pub fn prepare_key(&self, inputs: &SearchInputs) -> Result<Prepared> {
        let mut tables = point_table(signer_point(&inputs.signer_key)?, KEY_TABLE_BITS)?;
        tables.extend(point_table(
            PublicKey::from_slice(&inputs.parent_public_key).context("invalid parent key")?,
            KEY_TABLE_BITS,
        )?);
        Ok(Prepared {
            nonce: false,
            params: encode_params(inputs)?,
            key_tables: Some(upload(&self.device, "key tables", &tables)),
        })
    }

    #[doc(hidden)]
    pub fn prepare_nonce(&self, inputs: &NonceInputs) -> Result<Prepared> {
        Ok(Prepared {
            nonce: true,
            params: encode_nonce_params(inputs)?,
            key_tables: None,
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

    /// The vault output key the shader computes for each index in `start..start + count`, or
    /// `None` where it rejects the candidate. For checking the shader against the CPU.
    pub async fn output_keys(
        &self,
        inputs: &SearchInputs,
        start: u32,
        count: u32,
    ) -> Result<Vec<Option<[u8; 32]>>> {
        let prepared = self.prepare_key(inputs)?;
        self.debug_keys(&prepared, start, count).await
    }

    /// The output key the nonce shader computes for each locktime in `start..start + count`.
    pub async fn nonce_output_keys(
        &self,
        inputs: &NonceInputs,
        start: u32,
        count: u32,
    ) -> Result<Vec<Option<[u8; 32]>>> {
        let prepared = self.prepare_nonce(inputs)?;
        self.debug_keys(&prepared, start, count).await
    }

    async fn debug_keys(
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
        let kernel = if prepared.nonce {
            &self.nonce
        } else {
            &self.key
        };
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
        let key_tables = prepared.key_tables.as_ref().unwrap_or(&self.empty);
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vanity"),
            layout: &self.layout,
            entries: &[
                (0, &params),
                (1, &kernel.g_table),
                (2, &found),
                (3, &debug_out),
                (4, key_tables),
            ]
            .map(|(binding, buffer)| wgpu::BindGroupEntry {
                binding,
                resource: buffer.as_entire_binding(),
            }),
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(if debug { &kernel.debug } else { &kernel.search });
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(kernel.tuning.workgroups(count), 1, 1);
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
    pub fn open_with_sources(key_shader: &str, nonce_shader: &str) -> Result<Self> {
        pollster::block_on(Self::with_sources(key_shader, nonce_shader))
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

    pub fn search_blocking(
        &self,
        inputs: &SearchInputs,
        range: std::ops::Range<u32>,
        progress: impl FnMut(u64) -> bool,
    ) -> Result<SearchResult> {
        pollster::block_on(self.search(inputs, range, progress))
    }

    pub fn output_keys_blocking(
        &self,
        inputs: &SearchInputs,
        start: u32,
        count: u32,
    ) -> Result<Vec<Option<[u8; 32]>>> {
        pollster::block_on(self.output_keys(inputs, start, count))
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
const SHA512_IV: [u64; 8] = [
    0x6a09e667f3bcc908,
    0xbb67ae8584caa73b,
    0x3c6ef372fe94f82b,
    0xa54ff53a5f1d36f1,
    0x510e527fade682d1,
    0x9b05688c2b3e6c1f,
    0x1f83d9abfb41bd6b,
    0x5be0cd19137e2179,
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

/// The SHA-512 state after the HMAC key block padded with `pad`.
fn hmac_midstate(chain_code: &[u8; 32], pad: u8) -> [u32; 16] {
    let mut block = [pad; 128];
    for (byte, key) in block.iter_mut().zip(chain_code) {
        *byte ^= key;
    }
    let mut state = SHA512_IV;
    sha2::compress512(&mut state, &[GenericArray::clone_from_slice(&block)]);
    let mut words = [0_u32; 16];
    for (index, word) in state.iter().enumerate() {
        words[2 * index] = (word >> 32) as u32;
        words[2 * index + 1] = *word as u32;
    }
    words
}

/// The parameter block the shader reads, in its `P_*` order.
fn encode_params(inputs: &SearchInputs) -> Result<Vec<u32>> {
    ensure!(
        (1..=32).contains(&inputs.target_bits),
        "target must be between 1 and 32 bits"
    );
    let parent = PublicKey::from_slice(&inputs.parent_public_key).context("invalid parent key")?;
    let signer = signer_point(&inputs.signer_key)?;
    let mut params = Vec::with_capacity(PARAM_WORDS);
    params.extend([0, 0, inputs.target_value, inputs.target_bits]);
    params.extend(hmac_midstate(&inputs.chain_code, 0x36));
    params.extend(hmac_midstate(&inputs.chain_code, 0x5c));
    // The inner message: serP(parent) || index || padding, length (128 + 37) bytes.
    let mut message = [0_u8; 128];
    message[..33].copy_from_slice(&inputs.parent_public_key);
    message[37] = 0x80;
    message[120..].copy_from_slice(&((128_u64 + 37) * 8).to_be_bytes());
    params.extend(
        message
            .chunks(4)
            .map(|chunk| u32::from_be_bytes(chunk.try_into().expect("four bytes"))),
    );
    let (signer_x, signer_y) = affine(&signer);
    params.extend(signer_x);
    params.extend(signer_y);
    params.extend(tag_midstate(b"KeyAgg list"));
    params.extend(tag_midstate(b"KeyAgg coefficient"));
    params.extend(tag_midstate(b"TapTweak"));
    params.extend(
        inputs
            .merkle_root
            .chunks(4)
            .map(|chunk| u32::from_be_bytes(chunk.try_into().expect("four bytes"))),
    );
    let (parent_x, parent_y) = affine(&parent);
    params.extend(parent_x);
    params.extend(parent_y);
    ensure!(params.len() == PARAM_WORDS, "parameter block size changed");
    Ok(params)
}

/// The nonce kernel's parameter block, in its `P_*` order.
fn encode_nonce_params(inputs: &NonceInputs) -> Result<Vec<u32>> {
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

fn generator() -> PublicKey {
    let mut generator = [0x02_u8; 33];
    generator[1..].copy_from_slice(&secp256k1::constants::GENERATOR_X);
    PublicKey::from_slice(&generator).expect("the generator is a valid point")
}

/// `d·2^(bits·w)·point` for every window `w` and digit `d` in `1..2^bits`, as affine limbs.
/// Windows are independent, so wide tables are built on every core.
fn point_table(point: PublicKey, bits: u32) -> Result<Vec<u32>> {
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
