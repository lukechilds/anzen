# GPU vanity search performance log

Two kernels, both measured on an Apple M4 Max (40-core GPU) through wgpu → Metal:

- **key**: grind the phone vault key index (BIP32 child, MuSig2 key aggregation, Taproot tweak).
- **nonce**: grind the recovery leaf's past `after()` locktime with both keys fixed (leaf hash,
  Taproot tweak, one `t·G`).

A 5-character `vault` prefix needs about 33.5M candidates on average.

## Method

The GPU's clocks drift by tens of percent within seconds (a run can read 1.1M or 2.3M/s), so
separate runs barely compare: an A/A test of two whole-run builds differed by 17%. `bench/ab.py`
instead compiles the baseline commit's shaders next to the working tree's in one process and
alternates single dispatches between them, over the same candidate ranges, then takes the median
of the paired ratios. A/A noise this way: key 0.995, nonce 1.016 (40 pairs).

A change is kept when its paired ratio is at least 1.03 and it wins most pairs (marginal results
are re-run), and the GPU tests still match the library code. Host-side changes, which the
baseline shaders cannot run with, use `--binaries` with more rounds.

## Current speed

Measured on a cool GPU, alternating short runs (sustained load throttles all by up to ~2×):

| Kernel | Start | wgpu now | Metal | `vault` (33.5M), best |
|---|---|---|---|---|
| key | 710k/s | ~1.25M/s | **~1.51M/s** | ~22 s |
| nonce | 2.64M/s | ~16.0M/s | **~17.1M/s** | ~2.0 s |

The Metal kernels (`--features metal`) carry the same optimizations as the wgpu ones (16-bit G
windows, a shared inversion per 4 or 8 candidates). The key kernel keeps 8×32-bit limbs with
native 64-bit products, which beats wgpu's 32-bit limbs built from 16-bit halves. The nonce
kernel first did the same and ran at 9.1M/s, 1.8× slower than the 13-bit wgpu kernel: Apple GPUs
emulate 64-bit integer operations with several 32-bit instructions. Ported to the same unrolled
13-bit field (generated from the same script as the WGSL), it runs at ~17.1M/s, about 7% ahead
of wgpu in alternating runs.

## Where the time goes

Ablating the nonce kernel at the start (8-bit windows): the 32 table additions for `t·G` were
~67% of its time, the final inversion ~32%, and its three SHA-256 compressions under 1%.

## Research

What other secp256k1 GPU engines do, and whether it applies here:

- **Batched inversion** (VanitySearch, MasterKey: ~17.6× for them). Applied: one inversion per
  4 (key) or 8 (nonce) candidates.
- **Sequential keys** (k, k+1, …: one affine point addition per candidate, with the ±iG trick
  sharing denominators). This is where those engines get ~10× over random keys (about 1B/s versus
  100M/s on an RTX 5090). It does not apply here: every candidate goes through hashes (MuSig2
  KeyAgg coefficients, BIP32 HMAC, TapTweak), so neighbouring candidates share no linear structure.
- **Wide precomputed tables** for fixed-base multiplication. Applied: 16-bit windows.
- **Midstates** (Bitcoin mining). Applied: every tagged hash starts from a precomputed midstate;
  the nonce kernel's hashing is under 1% of its time anyway.
- **Oversized fully inlined kernels stall Apple's pipeline compiler** (llvm-metal #17: 100 MB in
  one function never finished in 600 s; the same split into 62 functions took 5.5 s). This
  explains the key kernel hang. In Metal, `noinline` on the field functions should keep
  compilation sane, and is reported as +22% elsewhere (smaller code, better instruction cache).
- **16-bit types reduce register pressure on Apple GPUs.** Candidate for the Metal loop: 13-bit
  limbs stored as `ushort`.
- **Apple GPU ALUs are 32-bit**: 64-bit integer ops are several instructions, and `mulhi` is how
  Metal gets a 32×32→64 product. The Metal kernel keeps 8×32-bit limbs with `ulong` products.

## Kept

| Change | Kernel | Speedup |
|---|---|---|
| Metal nonce: the 13-bit field instead of 32-bit limbs with 64-bit products (9.1 → 17.1M/s) | metal nonce | 1.88× |
| Nonce: point arithmetic in 20 limbs of 13 bits, fully unrolled. Products fit in 26 bits, so a column of 20 sums in a u32 with no carry handling. Loop form was 0.43×, unrolled products only 0.72×; unrolling the reduction, normalization and add/sub too gave the win. `tests/field.rs` checks every operation against num-bigint on edge and random inputs | nonce | 2.31× |
| Key: each of the three inversion stages batched across 4 candidates per invocation; 2 gave 1.17×, 8 gave 1.17× (register pressure) | key | 1.24× |
| Nonce: 8 candidates per invocation share one field inversion (Montgomery's trick); 4 gave 1.49×, 16 gave 1.56× | nonce | 1.55× |
| G table windows 8 → 16 bits: 16 mixed additions per `k·G` instead of 32, 67 MB table built on every core (12 bits gave +21%/+27%) | key / nonce | 1.38× / 1.52× |

## Tried and rejected

| Idea | Kernel | Result | Why |
|---|---|---|---|
| Workgroup size 32 / 128 / 256 (from 64) | both | 1.01 / 0.93 / 0.83 key; 0.98 / 1.00 / 0.92 nonce | 64 is best; bigger groups lower occupancy |
| Low word of `mul32` from the 16-bit partial products instead of a fifth native multiply | both | 1.006 / 1.003 | Integer multiply rate is not the bottleneck |
| Affine + affine first addition (mmadd, 4M+2S) for `Q + t·G` | nonce | Did not compile | Apple's Metal compiler service crashes (XPC_ERROR_CONNECTION_INTERRUPTED) whenever the helper is in the module, even unused; expected only ~3% |
| Key kernel on the unrolled 13-bit field (4 or 1 candidates per invocation) | key | Hung the GPU | Pipeline creation or the first dispatch never finished, and the GPU then sat at 100% with no live client: killing a process does not cancel GPU work it queued. The key kernel inlines ~80 point additions per candidate; with 400-product unrolled multiplies that seems beyond what Apple's compiler handles. The benchmark now runs a 256-candidate dispatch first and refuses to queue more if it is slow |
| Table reads replaced by a fixed point (ablation, not a change) | nonce | 1.07 | Memory is ~7% of the time; field arithmetic dominates |
