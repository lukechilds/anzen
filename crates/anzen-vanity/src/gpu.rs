//! The wgpu host: shader setup, point tables, parameter packing and batched dispatch.

use crate::{MAX_INDEX, SearchInputs};
use anyhow::{Context, Result, anyhow, bail, ensure};
use secp256k1::{PublicKey, Scalar, Secp256k1};
use sha2::{Digest, Sha256, digest::generic_array::GenericArray};
use std::time::Duration;

const SHADER: &str = concat!(include_str!("constants.wgsl"), include_str!("vanity.wgsl"));
const WORKGROUP: u32 = 64;
const PARAM_WORDS: usize = 132;
/// One point's table: `d·2^(8w)·point` for 32 byte windows `w` and digits `d` in 1..=255.
const TABLE_WORDS: usize = 32 * 255 * 16;
/// Candidates per dispatch to start with; the search adapts it towards `TARGET_BATCH_TIME`.
const FIRST_BATCH: u32 = 1 << 16;
const MAX_BATCH: u32 = 1 << 22;
const TARGET_BATCH_TIME: Duration = Duration::from_millis(250);

/// An open GPU device with the search shader compiled.
pub struct GpuSearch {
    device: wgpu::Device,
    queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    search: wgpu::ComputePipeline,
    debug: wgpu::ComputePipeline,
    /// The table for G; each search appends the signer's and the parent's.
    generator: Vec<u32>,
    adapter: String,
}

/// A finished search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchResult {
    /// The first matching index found, if any.
    pub index: Option<u32>,
    /// Candidates evaluated, including the rest of the batch the match was found in.
    pub attempts: u64,
}

impl GpuSearch {
    /// Open the default high-performance adapter and compile the shader.
    pub async fn new() -> Result<Self> {
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
        ensure!(
            limits.max_storage_buffer_binding_size as usize >= 3 * TABLE_WORDS * 4,
            "the GPU cannot bind the precomputed table"
        );
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("anzen vanity"),
                required_limits: wgpu::Limits {
                    max_storage_buffer_binding_size: (3 * TABLE_WORDS * 4) as u64,
                    ..wgpu::Limits::downlevel_defaults()
                }
                .using_resolution(limits),
                ..Default::default()
            })
            .await
            .context("could not open the GPU device")?;
        // SAFETY: every array index in the shader is in bounds by construction (fixed loop bounds,
        // table digits 1..=255) and every loop has a constant trip count. Runtime bounds checks
        // and loop counters would otherwise stop the compiler unrolling the field arithmetic.
        let module = unsafe {
            device.create_shader_module_trusted(
                wgpu::ShaderModuleDescriptor {
                    label: Some("vanity"),
                    source: wgpu::ShaderSource::Wgsl(SHADER.into()),
                },
                wgpu::ShaderRuntimeChecks::unchecked(),
            )
        };
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
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vanity"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry_point| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry_point),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some(entry_point),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let search = pipeline("search");
        let debug = pipeline("debug");
        let mut generator = [0x02_u8; 33];
        generator[1..].copy_from_slice(&secp256k1::constants::GENERATOR_X);
        let generator = point_table(PublicKey::from_slice(&generator)?)?;
        Ok(Self {
            device,
            queue,
            layout,
            search,
            debug,
            generator,
            adapter: format!("{} ({:?})", info.name, info.backend),
        })
    }

    /// The adapter in use, for display.
    pub fn adapter_name(&self) -> &str {
        &self.adapter
    }

    /// Search indexes in `range` in order until a match, reporting the candidates tested so far.
    /// A match is the lowest matching index of its batch.
    pub async fn search(
        &self,
        inputs: &SearchInputs,
        range: std::ops::Range<u32>,
        mut progress: impl FnMut(u64),
    ) -> Result<SearchResult> {
        ensure!(
            range.end <= MAX_INDEX + 1,
            "vault key indexes must be below 2^31"
        );
        let mut params = encode_params(inputs)?;
        let tables = self.tables(inputs)?;
        let mut next = range.start;
        let mut batch = FIRST_BATCH;
        let mut attempts = 0_u64;
        while next < range.end {
            let count = batch.min(range.end - next);
            params[0] = next;
            params[1] = count;
            let started = now();
            let found = self.run(&self.search, &params, &tables, count, 4).await?;
            attempts += u64::from(count);
            progress(attempts);
            let index = found[0];
            if index != u32::MAX {
                return Ok(SearchResult {
                    index: Some(index),
                    attempts,
                });
            }
            next += count;
            if let Some(elapsed) = started.map(|started| started.elapsed()) {
                batch = if elapsed < TARGET_BATCH_TIME / 2 {
                    (batch * 2).min(MAX_BATCH)
                } else if elapsed > TARGET_BATCH_TIME * 2 {
                    (batch / 2).max(WORKGROUP)
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
        let mut params = encode_params(inputs)?;
        params[0] = start;
        params[1] = count;
        let tables = self.tables(inputs)?;
        let words = self
            .run(&self.debug, &params, &tables, count, count as usize * 36)
            .await?;
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

    /// The tables for G, the signer key (even y) and the parent key, uploaded for one search.
    fn tables(&self, inputs: &SearchInputs) -> Result<wgpu::Buffer> {
        use wgpu::util::DeviceExt;
        let mut words = Vec::with_capacity(3 * TABLE_WORDS);
        words.extend_from_slice(&self.generator);
        words.extend(point_table(signer_point(&inputs.signer_key)?)?);
        words.extend(point_table(
            PublicKey::from_slice(&inputs.parent_public_key).context("invalid parent key")?,
        )?);
        Ok(self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("point tables"),
                contents: bytemuck::cast_slice(&words),
                usage: wgpu::BufferUsages::STORAGE,
            }))
    }

    /// Dispatch `pipeline` over `count` candidates and read back the found word (search) or the
    /// debug output, `read_bytes` long.
    async fn run(
        &self,
        pipeline: &wgpu::ComputePipeline,
        params: &[u32],
        tables: &wgpu::Buffer,
        count: u32,
        read_bytes: usize,
    ) -> Result<Vec<u32>> {
        use wgpu::util::DeviceExt;
        let storage = |label, contents: &[u8], usage| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents,
                    usage,
                })
        };
        let params = storage(
            "params",
            bytemuck::cast_slice(params),
            wgpu::BufferUsages::STORAGE,
        );
        let found = storage(
            "found",
            bytemuck::cast_slice(&[u32::MAX]),
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let debug_bytes = if std::ptr::eq(pipeline, &self.debug) {
            read_bytes.max(4)
        } else {
            4
        };
        let debug = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("debug"),
            size: debug_bytes as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: read_bytes as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vanity"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: tables.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: found.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: debug.as_entire_binding(),
                },
            ],
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(count.div_ceil(WORKGROUP), 1, 1);
        }
        let source = if debug_bytes > 4 { &debug } else { &found };
        encoder.copy_buffer_to_buffer(source, 0, &readback, 0, read_bytes as u64);
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

    pub fn search_blocking(
        &self,
        inputs: &SearchInputs,
        range: std::ops::Range<u32>,
        progress: impl FnMut(u64),
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

/// The even-y point of an x-only key, as BIP327 key aggregation uses it.
fn signer_point(key: &[u8; 32]) -> Result<PublicKey> {
    let mut even = [0x02_u8; 33];
    even[1..].copy_from_slice(key);
    PublicKey::from_slice(&even).context("invalid signer key")
}

/// `d·2^(8w)·point` for every window `w` and digit `d` in 1..=255, as affine limbs.
fn point_table(point: PublicKey) -> Result<Vec<u32>> {
    let secp = Secp256k1::new();
    let mut base = point;
    let mut table = Vec::with_capacity(TABLE_WORDS);
    for _ in 0..32 {
        let mut multiple = base;
        for digit in 1..=255 {
            if digit > 1 {
                multiple = multiple.combine(&base)?;
            }
            let (x, y) = affine(&multiple);
            table.extend(x);
            table.extend(y);
        }
        let mut shift = [0_u8; 32];
        shift[30] = 1; // 256
        base = base.mul_tweak(&secp, &Scalar::from_be_bytes(shift)?)?;
    }
    if table.len() != TABLE_WORDS {
        bail!("point table size changed");
    }
    Ok(table)
}
