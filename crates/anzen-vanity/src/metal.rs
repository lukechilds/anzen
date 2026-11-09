//! A native Metal host for both searches on Apple GPUs, running `metal/key.metal` and
//! `metal/nonce.metal`: the same searches as the wgpu kernels, written in Metal Shading Language
//! to use what WGSL lacks.

use crate::{MAX_INDEX, NONCE_LOCKTIMES, NonceInputs, SearchInputs, SearchResult};
use anyhow::{Context, Result, anyhow, ensure};
use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_foundation::NSString;
use objc2_metal::{
    MTLBuffer, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue, MTLComputeCommandEncoder,
    MTLComputePipelineState, MTLCreateSystemDefaultDevice, MTLDevice, MTLLibrary,
    MTLResourceOptions, MTLSize,
};
use std::ptr::NonNull;

const COMMON_SOURCE: &str = include_str!("metal/common.metal");
const KEY_SOURCE: &str = include_str!("metal/key.metal");
const NONCE_SOURCE: &str = include_str!("metal/nonce.metal");
const G_WINDOW_BITS: u32 = 16;
const THREADS_PER_GROUP: usize = 64;

type Device = ProtocolObject<dyn MTLDevice>;
type Buffer = Retained<ProtocolObject<dyn MTLBuffer>>;
type Pipeline = Retained<ProtocolObject<dyn MTLComputePipelineState>>;

/// An open Metal device with both kernels compiled.
pub struct MetalSearch {
    device: Retained<Device>,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    key: Kernel,
    nonce: Kernel,
    g_table: Buffer,
    name: String,
}

struct Kernel {
    search: Pipeline,
    debug: Pipeline,
    per_invocation: u32,
}

/// A search's parameter block, and the key kernel's signer and parent tables.
#[doc(hidden)]
pub struct MetalPrepared {
    nonce: bool,
    params: Vec<u32>,
    tables: Option<Buffer>,
}

impl MetalSearch {
    pub fn open() -> Result<Self> {
        Self::with_sources(KEY_SOURCE, NONCE_SOURCE)
    }

    /// Open the GPU with other kernel sources (each without the shared code), for benchmarking
    /// changes side by side.
    #[doc(hidden)]
    pub fn with_sources(key_source: &str, nonce_source: &str) -> Result<Self> {
        Self::with_full_sources(COMMON_SOURCE, key_source, nonce_source)
    }

    #[doc(hidden)]
    pub fn with_full_sources(common: &str, key_source: &str, nonce_source: &str) -> Result<Self> {
        let device = MTLCreateSystemDefaultDevice().context("no Metal device is available")?;
        let kernel = |source: &str| -> Result<Kernel> {
            let full = format!("{common}\n{source}");
            let library = device
                .newLibraryWithSource_options_error(&NSString::from_str(&full), None)
                .map_err(|error| anyhow!("Metal shader failed to compile: {error}"))?;
            let pipeline = |name: &str| -> Result<Pipeline> {
                let function = library
                    .newFunctionWithName(&NSString::from_str(name))
                    .with_context(|| format!("Metal shader has no {name} kernel"))?;
                device
                    .newComputePipelineStateWithFunction_error(&function)
                    .map_err(|error| anyhow!("Metal pipeline failed: {error}"))
            };
            Ok(Kernel {
                search: pipeline("search")?,
                debug: pipeline("debug")?,
                per_invocation: per_invocation(source)?,
            })
        };
        let key = kernel(key_source)?;
        let nonce = kernel(nonce_source)?;
        let table = crate::gpu::point_table(crate::gpu::generator(), G_WINDOW_BITS)?;
        let g_table = buffer(&device, &table)?;
        Ok(Self {
            queue: device.newCommandQueue().context("no Metal command queue")?,
            key,
            nonce,
            g_table,
            name: device.name().to_string(),
            device,
        })
    }

    pub fn device_name(&self) -> &str {
        &self.name
    }

    #[doc(hidden)]
    pub fn prepare_nonce(&self, inputs: &NonceInputs) -> Result<MetalPrepared> {
        Ok(MetalPrepared {
            nonce: true,
            params: crate::gpu::encode_nonce_params(inputs)?,
            tables: None,
        })
    }

    #[doc(hidden)]
    pub fn prepare_key(&self, inputs: &SearchInputs) -> Result<MetalPrepared> {
        Ok(MetalPrepared {
            nonce: false,
            params: crate::gpu::encode_params(inputs)?,
            tables: Some(buffer(&self.device, &crate::gpu::key_tables(inputs)?)?),
        })
    }

    /// Run one dispatch over `start..start + count`, returning the lowest match in it.
    #[doc(hidden)]
    pub fn run_batch(
        &self,
        prepared: &MetalPrepared,
        start: u32,
        count: u32,
    ) -> Result<Option<u32>> {
        let found = self.dispatch(prepared, false, start, count, 4)?;
        Ok((found[0] != u32::MAX).then_some(found[0]))
    }

    /// Search key indexes in `range` in order until a match.
    pub fn search(
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
        self.search_prepared(&prepared, range, 1 << 19, progress)
    }

    /// The output key the key kernel computes for each index in `start..start + count`.
    pub fn output_keys(
        &self,
        inputs: &SearchInputs,
        start: u32,
        count: u32,
    ) -> Result<Vec<Option<[u8; 32]>>> {
        let prepared = self.prepare_key(inputs)?;
        self.debug_keys(&prepared, start, count)
    }

    /// Search recovery locktimes in `range` in order until a match; `progress` returns false to
    /// stop early.
    pub fn search_nonces(
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
        self.search_prepared(&prepared, range, 1 << 21, progress)
    }

    fn search_prepared(
        &self,
        prepared: &MetalPrepared,
        range: std::ops::Range<u32>,
        batch: u32,
        mut progress: impl FnMut(u64) -> bool,
    ) -> Result<SearchResult> {
        let (mut next, mut attempts) = (range.start, 0_u64);
        while next < range.end {
            let count = batch.min(range.end - next);
            let found = self.run_batch(prepared, next, count)?;
            attempts += u64::from(count);
            if found.is_some() {
                return Ok(SearchResult {
                    index: found,
                    attempts,
                });
            }
            if !progress(attempts) {
                break;
            }
            next += count;
        }
        Ok(SearchResult {
            index: None,
            attempts,
        })
    }

    /// The output key the kernel computes for each locktime in `start..start + count`.
    pub fn nonce_output_keys(
        &self,
        inputs: &NonceInputs,
        start: u32,
        count: u32,
    ) -> Result<Vec<Option<[u8; 32]>>> {
        let prepared = self.prepare_nonce(inputs)?;
        self.debug_keys(&prepared, start, count)
    }

    fn debug_keys(
        &self,
        prepared: &MetalPrepared,
        start: u32,
        count: u32,
    ) -> Result<Vec<Option<[u8; 32]>>> {
        let words = self.dispatch(prepared, true, start, count, count as usize * 36)?;
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

    fn dispatch(
        &self,
        prepared: &MetalPrepared,
        debug: bool,
        start: u32,
        count: u32,
        output_bytes: usize,
    ) -> Result<Vec<u32>> {
        let kernel = if prepared.nonce {
            &self.nonce
        } else {
            &self.key
        };
        let pipeline = if debug { &kernel.debug } else { &kernel.search };
        let mut params = prepared.params.clone();
        params[0] = start;
        params[1] = count;
        let params = buffer(&self.device, &params)?;
        let output = buffer(&self.device, &vec![u32::MAX; output_bytes / 4])?;
        let commands = self
            .queue
            .commandBuffer()
            .context("no Metal command buffer")?;
        let encoder = commands
            .computeCommandEncoder()
            .context("no Metal compute encoder")?;
        encoder.setComputePipelineState(pipeline);
        // SAFETY: the buffers outlive the command buffer, which completes before they drop.
        unsafe {
            encoder.setBuffer_offset_atIndex(Some(&params), 0, 0);
            encoder.setBuffer_offset_atIndex(Some(&self.g_table), 0, 1);
            encoder.setBuffer_offset_atIndex(Some(&output), 0, 2);
            if let Some(tables) = &prepared.tables {
                encoder.setBuffer_offset_atIndex(Some(tables), 0, 3);
            }
        }
        let threads = count.div_ceil(kernel.per_invocation) as usize;
        encoder.dispatchThreadgroups_threadsPerThreadgroup(
            MTLSize {
                width: threads.div_ceil(THREADS_PER_GROUP),
                height: 1,
                depth: 1,
            },
            MTLSize {
                width: THREADS_PER_GROUP,
                height: 1,
                depth: 1,
            },
        );
        encoder.endEncoding();
        commands.commit();
        commands.waitUntilCompleted();
        if let Some(error) = commands.error() {
            anyhow::bail!("Metal dispatch failed: {error}");
        }
        // SAFETY: shared-storage buffer, and the GPU has finished with it.
        let words = unsafe {
            std::slice::from_raw_parts(output.contents().as_ptr().cast::<u32>(), output_bytes / 4)
        };
        Ok(words.to_vec())
    }
}

fn buffer(device: &Device, words: &[u32]) -> Result<Buffer> {
    // SAFETY: copies `words` into a new shared buffer of the same length.
    unsafe {
        device.newBufferWithBytes_length_options(
            NonNull::new(words.as_ptr().cast_mut().cast()).context("empty buffer")?,
            std::mem::size_of_val(words),
            MTLResourceOptions::StorageModeShared,
        )
    }
    .context("Metal buffer allocation failed")
}

fn per_invocation(source: &str) -> Result<u32> {
    let prefix = "#define PER_INVOCATION ";
    let at = source
        .find(prefix)
        .context("kernel has no PER_INVOCATION")?;
    let digits: String = source[at + prefix.len()..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().context("PER_INVOCATION is not a literal")
}
