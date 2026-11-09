# GPU vanity search performance log

One kernel, through wgpu everywhere (Metal, Vulkan, DirectX natively and WebGPU in browsers):
grind the recovery leaf's long-past `after()` locktime with both keys fixed (leaf hash, Taproot
tweak, one `t·G`). A 5-character `vault` prefix needs about 33.5M candidates on average.

Earlier there was also a **key** kernel, grinding the phone vault key index (BIP32 child, MuSig2
key aggregation, Taproot tweak), and a native **Metal** backend for both. Metal was 1.15–1.3×
faster for nonce grinding and ~1.5× for key grinding, but it was a second implementation of every
kernel, it hit the iOS GPU watchdog on an iPhone 12 mini, and nonce grinding on wgpu is already
~5× faster than key grinding on Metal. Both were removed so one implementation runs everywhere;
their measurements below are kept as history, and the code is in git history.

## Method

The GPU's clocks drift by tens of percent within seconds (a run can read 1.1M or 2.3M/s), so
separate runs barely compare: an A/A test of two whole-run builds differed by 17%. `bench/ab.py`
instead compiles the baseline commit's shaders next to the working tree's in one process and
alternates single dispatches between them, over the same candidate ranges, then takes the median
of the paired ratios. A/A noise this way: 0.995–1.016 (40 pairs).

A change is kept when its paired ratio is at least 1.03 and it wins most pairs (marginal results
are re-run), or when it wins nearly every pair over 100 or more (a small gain that is plainly not
noise), and the GPU tests still match the library code. Host-side changes, which the
baseline shaders cannot run with, use `--binaries` with more rounds.

## Current speed

M4 Max (40-core GPU), cool, 5-second window of the app benchmark (`examples/benchmark.rs run`):
**~40M/s**, so `vault` takes ~0.85 s on average, after ~0.1 s of setup (compiling the kernel and
building the 128 MiB generator table on the GPU); `anzen phone init --vanity` on regtest finds one
in ~1.4 s end to end. Sustained load throttles by up to ~25% within a minute, and much further
after hours of benchmarking. Chrome's WebGPU on the same Mac runs the same kernel at **~36.4M/s**.

The third loop took the kernel from ~19.5M/s to ~40M/s (2.05×): Q folded into window 0, signed
and then varying window widths (16 additions to 13), the generator table built on the GPU, a
reordered mixed addition, Karatsuba products, shorter reductions, fused normalizations, XYZZ
coordinates, an affine first addition and safegcd inversion. Under browsers' 128 MiB per-binding limit 13 additions is the floor: 12 would need a
generator table of about 2^22.2 points.

Phones, from the app's settings benchmark (5 s, cool; measured while the key kernel still existed,
so setup then also compiled it):

| Device | Rate | `vault` on average | Batch reached |
|---|---|---|---|
| iPhone 15 Pro (A17 Pro) | 2.67M/s | ~13 s | 262,144 |
| iPhone 12 mini (A14) | 1.30M/s | ~26 s | 65,536 |

The CPU path grinds the same locktimes with the library code: **831k/s** on the M4 Max's 16
threads (`vault` in ~40 s on average) and 76k/s on one; one thread of browser WASM manages
~12k/s. Grinding phone keys instead, as it once did, ran at about 118k/s on 16 threads.

The app benchmark (`bench::Plan`) uses the ramp guard and caps each dispatch at ~0.25 s, so a
phone's GPU stays responsive.

Browser notes: WGSL requires parentheses between `<<` and `|`. naga accepted the nonce shader
without them, but Chrome rejected it, and every dispatch through the rejected pipeline then did
nothing, at an impossible rate. wgpu 30 also panics popping an empty error scope on WebGPU, so
the browser benchmark checks the kernel's output keys before timing instead.

Why the 13-bit field: WGSL has no `mulhi`, so a 32-bit field builds every product from 16-bit
halves, and the nonce kernel is faster on 20 limbs of 13 bits (limb products fit in 26 bits,
columns sum in a u32). On Apple GPUs 64-bit integer operations are emulated, so anything
accumulating in 64 bits is slow, and constant limb indexes (unrolled code) matter more than
anything else: every un-unrolled field ran 2–2.5× slower. Metal's `mulhi` was most of its lead.

History, before the removal (alternating 3-second runs, first round):

| Kernel | Start | wgpu | Metal | `vault` (33.5M), best |
|---|---|---|---|---|
| key | 710k/s | ~3.4M/s | ~5.2M/s | ~6.5 s |
| nonce | 2.64M/s | ~19.8M/s | ~23–25M/s | ~1.4 s |

Chrome's WebGPU compiled the key kernel better than naga did natively (4.53M/s against 3.4M/s).

## Where the time goes

Ablating the nonce kernel at the start (8-bit windows): the 32 table additions for `t·G` were
~67% of its time, the final inversion ~32%, and its three SHA-256 compressions under 1%.

After the third loop's field changes (18-bit signed windows, Q folded in): the 14 additions are
~82% of the time (about 5.9% each) and the inversion ~9%. With XYZZ and 13 additions: the
inversion ~8.5% and the three SHA-256 compressions ~4.5%. With safegcd written out limb by limb
the inversion is ~3%.

Batch-affine additions (one inversion shared across many candidates per window, as CPU and
sequential-key GPU engines do) do not pay on a SIMD GPU: they save about 3.5 field products per
candidate-addition, roughly 0.11 SIMD issue slots across 32 lanes, while each shared inversion runs
on one lane for ~270 slots, so a batch would need more than ~2,400 candidates per inversion, and
their state traffic at that size approaches the memory bandwidth.

With 16-bit windows and 8 candidates per inversion (early in the third loop, paired against the
kernel itself): the 16 table additions are **~85%** of the time (removing them runs 6.6× faster), about
5.3% each, and the batched inversion **~8%** (skipping it runs 1.087× faster). Removing an
addition is worth more than anything else, and sharing the inversion further cannot gain much.

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
| The search split into two passes per chunk of 2^19 candidates: one thread per candidate computes X and ZZ and writes them packed to a 40 MB scratch buffer, then a second pass inverts 16 at a time. Holding a batch's state through the next candidates' point arithmetic cost ~11% (ablations: no inversion at 8 per thread 1.04, at 1 per thread 1.15); the scratch traffic is ~3 GB/s. Both passes for every chunk go in one submission. Measured in alternating whole runs (the host changed) | nonce | 1.0612 (10/10 rounds) |
| Those two held elements packed two limbs per word (limbs are at most 2^13, so 16 bits each): 20 words per candidate instead of 40, for about 30 shifts and masks | nonce | 1.0104 (59/60), 1.0100 (76/80) |
| The batch inversion keeps X·(the earlier ZZ's product) and ZZ per candidate instead of X, ZZ and the prefix product: x = A·(the prefix through this ZZ)⁻¹. The same four multiplies per candidate, 20 fewer words per candidate held through the inversion | nonce | 1.0142 (59/60), 1.0133 (80/80) |
| `f_canonical` written out limb by limb: both folds as straight carry chains, then p subtracted with a borrow chain and kept limb by limb when there is no borrow (instead of a comparison loop with an early exit) | nonce | 1.0099 (57/60), 1.0114 (78/80) |
| safegcd's batch updates written out limb by limb (constant indices), so f, g, d and e stay in registers instead of loop-indexed arrays | nonce | 1.0420 (60/60) |
| Inversion by Bernstein–Yang safegcd instead of Fermat's 255 squares: libsecp256k1's modinv32 in batches of 13 divsteps over 20 signed 13-bit limbs, so every matrix product stays within an i32 (no 64-bit emulation); 46 batches make 598 divsteps. `tools/model_safegcd.py` checks the exact i32 operations and their bounds over 20,000 inversions; the field test checks it against bignum, including 0 | nonce | 1.0403, 1.0401 (60/60 both) |
| The first addition peeled out of the loop as affine plus affine into XYZZ (both ZZ = 1, 4M + 2S: U2, S2, ZZ3 and ZZZ3 become copies), unlike the Jacobian attempt that lost | nonce | 1.0238 (60/60), 1.0238 (78/80) |
| XYZZ coordinates (x = X/ZZ, y = Y/ZZZ) for the search's accumulator: the mixed addition is madd-2008-s, 8M + 2S instead of 8M + 3S, and the output key is X·ZZ⁻¹, so the batch inversion drops a square per candidate. The extra coordinate costs less than the square | nonce | 1.1044, 1.1014 (60/60 both) |
| 13 additions instead of 14 with varying window widths that fill both per-binding limits: window 0 is 20 bits (its Q table is 2^20 + 1 points, 64 MB), and the generator table's 13 windows of 19, 19, 19 and ten 18 bits hold exactly 2^21 points (128 MiB), covering 257 bits. Setup builds the same number of points, plus a million-point Q fold per search. Measured in alternating whole runs (the host changed): 1.0738 over 10 rounds. While building it, a version computing a split multiple with point arithmetic in several branches came out of Apple's compiler wrong; the arithmetic now appears once and new tests check every table entry | nonce | 1.0738 |
| `f_reduce` ends with a tail to limb 6 and nothing more: only limbs 0..4 exceed 13 bits after its last small fold, so limb 6 takes at most one carry and ends at most 2^13, which the relaxed invariant allows (the branch that crashed Apple's compiler is not needed). The value can pass 2^260 only when limbs 7..19 are all 8191 (the model finds it only for (2^260 − 1)²); `tools/model_canonical.py` and the other models cover such inputs | nonce | 1.0686 (60/60) |
| Fused add/subtract forms in the mixed addition (4·HH, 2(S2 − Y1), R² − J − 2V and M − 2·Y1J each with one light normalization instead of two or three): five fewer normalizations per addition. `tools/model_light.py` checks every light-normalized form; it also showed the existing ones can leave one limb at exactly 2^13, which every bound allows, so the documented invariant is now limbs at most 2^13 | nonce | 1.0486 (60/60) |
| A second Karatsuba level on the low and high 5-limb halves (their sums stay below 2^14; the middle product's would not, so it stays schoolbook): 250 limb products per multiply instead of 300 | nonce | 1.0235 (60/60), 1.0229 (79/80) |
| `f_reduce` carries only the 19 high columns before folding; the low columns stay sums below 2^30.4, below 2^31 with the folded terms, and the next chain carries them (20 carry steps fewer per product, checked by the same model) | nonce | 1.0473 (60/60) |
| `f_reduce` ends with one carry chain, a fold and a tail to limb 5 instead of the full two-chain normalization: after the last small fold only limbs 0..4 exceed 13 bits and the value is below 2^260 + 2^63, so the chain carries out at most 1 and its fold cannot pass limb 5. `tools/model_reduce.py` checks the exact u32 operations over 300k random and extreme products | nonce | 1.0605 (60/60) |
| One level of Karatsuba in `f_mul` and `f_sqr` (300 limb products instead of 400, 165 instead of 210), with Z3 = 2·Z1·H in the mixed addition (a multiply instead of a square and two subtractions, freeing Z1 and Z1Z1 at once). Each alone was ~1% before the addition was reordered; together after it, far more | nonce | 1.0619, 1.0713 (60/60 both) |
| Mixed addition reordered to end values' lives early (Z3 right after HH frees Z1 and Z1Z1): the same operations, but each field element held is 20 registers and the addition spilled. Register pressure, not arithmetic, is what limits the kernel now | nonce | 1.0345 (56/60), 1.0489 (60/60) |
| Signed 18-bit windows: digits from −2^17 to 2^17 (negative ones negate y), 15 windows instead of 16, so 14 additions with Q folded in; the table is 15 × 2^17 points, 126 MB, under browsers' 128 MiB default. Built on the GPU (next row) so setup does not double | nonce | 1.0458 (60/60) |
| Setup passes batched: each thread makes 8 table (or Q-table) points with one inversion (Montgomery's trick), since the inversion costs far more than the additions; submissions of 2^17 points. Warm setup on the M4 Max 0.120 → 0.093 s (the rest is fixed: adapter, cached pipelines, the small CPU tables); the GPU passes are a larger share on phones | nonce | setup only |
| Generator table built on the GPU: the CPU computes lo·B and (hi·512)·B per window (~11,000 points) and each GPU thread adds one pair and inverts it, in 32,768-thread submissions. Setup on the M4 Max 0.45 → 0.14 s warm; in Chrome the single-thread WASM table build of ~4 s is gone. A test compares all 2M entries with the CPU's table | nonce | setup only |
| Q folded into window 0: each search's setup pass writes Q + d·G for every window-0 digit (Q at d = 0) on the GPU, one addition and inversion per thread, in milliseconds; candidates start from that entry and make 15 additions instead of 16 | nonce | 1.0575 (60/60) |
| Metal: point addition and field inversion `noinline` (smaller code, better instruction cache; matches the research) | metal key / metal nonce | 1.039 / 1.108 |
| Metal nonce: back to the 32-bit field now that it uses mulhi and 32-bit carries (from the 13-bit field) | metal nonce | 1.143 |
| Metal key: the 32-bit field with 32-bit carries and native `mulhi` for products (it used 64-bit accumulators, which Apple GPUs emulate) | metal key | 2.48 |
| wgpu key: the 32-bit field functions (mul_wide, fe_sqr, fe_add, sub_raw, gte) unrolled straight-line by `tools/gen_field32.py`, so every limb index is constant | key | 2.29 |
| Metal key: 8 candidates per thread share each inversion stage (from 4; 2 gave 0.83×, 16 gave 0.74×) | metal key | 1.126 |
| Nonce: 13-bit mixed addition without the P = ±Q checks, which cost two full reductions per addition and happen with probability ~2^-256; the host re-verifies every match on the CPU | nonce / metal nonce | 1.129 / 1.051 |
| Key: signer and parent tables with 16-bit windows (KeyAgg's table additions 32 → 16; 134 MB of tables per search) | key / metal key | 1.20 / 1.18 |
| Nonce: lighter normalization after add, sub and neg (one carry chain, fold, short tail; their limbs stay below 2^15 so the carry out is at most 3, checked by a model of the exact u32 operations) | nonce / metal nonce | 1.086 / 1.055 |
| Metal nonce: the 13-bit field instead of 32-bit limbs with 64-bit products (9.1 → 17.1M/s) | metal nonce | 1.88× |
| Nonce: point arithmetic in 20 limbs of 13 bits, fully unrolled. Products fit in 26 bits, so a column of 20 sums in a u32 with no carry handling. Loop form was 0.43×, unrolled products only 0.72×; unrolling the reduction, normalization and add/sub too gave the win. `tests/field.rs` checks every operation against num-bigint on edge and random inputs | nonce | 2.31× |
| Key: each of the three inversion stages batched across 4 candidates per invocation; 2 gave 1.17×, 8 gave 1.17× (register pressure) | key | 1.24× |
| Nonce: 8 candidates per invocation share one field inversion (Montgomery's trick); 4 gave 1.49×, 16 gave 1.56× | nonce | 1.55× |
| G table windows 8 → 16 bits: 16 mixed additions per `k·G` instead of 32, 67 MB table built on every core (12 bits gave +21%/+27%) | key / nonce | 1.38× / 1.52× |

## Window size against setup

With the table built on the GPU, wider signed windows cost setup time rather than CPU time.
On the M4 Max, against 18-bit (14 additions):

| Window | Additions | Table | Throughput | Setup (warm) | `vault` incl. setup |
|---|---|---|---|---|---|
| 18-bit | 14 | 126 MB | 1× (~21M/s) | 0.14 s | ~1.74 s |
| 19-bit | 13 | 235 MB | 1.062 (60/60) | — | — |
| 20-bit | 12 | 436 MB | 1.129 (60/60), ~24.7M/s | 0.40 s | ~1.76 s |
| 22-bit | 11 | 1.6 GB | ~26.5M/s | 2.45 s | ~3.7 s |

A table entry costs about 1.4 candidates (one addition and one inversion), so for a single
`vault` search (2^25 candidates) the extra setup cancels the faster search on the M4 Max and
loses on slower GPUs, while browsers default to a 128 MiB storage limit. 18-bit stays the
default. Longer searches (a `vault` prefix plus suffix characters, 2^30 and up) would gain from
the wider windows; the host could choose the width from the expected work and the device's
limits by patching `G_WINDOW_BITS` before compiling.

## Tried and rejected

| Idea | Kernel | Result | Why |
|---|---|---|---|
| Subtractive Karatsuba at the first level, (low − high)(high' − low') with wrapping u32 limbs, so the middle product takes a second level too: 225 limb products instead of 250 | nonce | 0.997 (20/80) | Multiplies are no longer the cost |
| The last addition peeled out of the loop computing only X3 and ZZ3 (6M + 2S) | nonce | 1.0055 (56/60) | Too small for the extra code |
| Candidates per thread 16 / 10 / 6 with the first addition peeled; 4 / 6 after safegcd made the inversion cheap | nonce | 1.001 (24/40) / 1.003 (27/40) / 0.974 (2/40); 0.984 / 0.995 | 8 stays |
| Fermat inversion with x2 and x3 recomputed late rather than held through the long squaring runs | nonce | 1.001 (31/60) | No spill to relieve; superseded by safegcd |
| Negative digits negated on the canonical words (p − y with an 8-word borrow chain) instead of `f_neg` | nonce | 1.004 (43/60) | Too small |
| The search loop pipelined, two batches in flight so the GPU starts the next while the host reads the last back (batch sizes timed between completions) | nonce search | 0.79 in alternating runs; ~0.85 on a fixed 2^28 range | Slower: the queued dispatches compete or the adaptation shrinks batches; sequential batches of up to 2^22 already leave little idle time |
| `f_mul` and `f_sqr` with the reduction written in and every column, limb and carry a scalar (no array passed to `f_reduce`) | nonce | 1.001 (31/60) | The compiler already inlines `f_reduce` and keeps the columns in registers |
| SHA-256's Ch and Maj in their shorter forms (g ^ (e & (f ^ g)), (a & b) \| (c & (a \| b))) | nonce | 1.000 (31/60) | The compiler already simplifies them |
| `fe_to_f` and `f_to_fe` written out with constant word indices and shifts | nonce | 1.000 (30/60) | The compiler already unrolls these small loops (unlike safegcd's and `f_canonical`'s) |
| safegcd's 13 divsteps per batch written out instead of looped | nonce | 0.991 (3/60) | The loop stays |
| safegcd stopping once g reaches 0 (a per-batch check; later batches only keep d congruent) | nonce | 0.991 (1/60) | The slowest of a SIMD group's lanes runs close to the bound, so the check costs more than it saves |
| One inversion shared by partner invocations in the two halves of a workgroup (separate SIMD groups on Apple GPUs; products exchanged through workgroup memory, so the second group skips the inversion) | nonce | 0.972 (0/60) | The idle group waits at the barrier, and at this kernel's register-limited occupancy nothing fills it |
| Subtractive two-level Karatsuba for squares only ((low − high)² takes a second level: 135 limb products instead of 145) | nonce | 1.002 (45/60) | Too small to matter |
| Table loads pointed at a few cached entries (ablation, not a change) | nonce | 1.015 (30/30) | Load latency is only ~1.5% of the time even at low occupancy, so prefetching cannot gain much |
| XYZZ addition reordered: Y1·PPP right after PPP (V2), R computed after ZZ3 (V3), or Q = X1·PP before ZZ3 | nonce | 0.986 (0/60) / 0.999 (29/60) / 0.990 (1/60) | The madd-2008-s order as written stays best |
| A negative digit's sign folded into the XYZZ addition (R from y2·ZZZ ± Y1 and the direction of Q − X3, chosen limb by limb with `select`) instead of negating the table point's y | nonce | 0.926 (0/60) | The per-limb selects cost far more than the branch and negation |
| Workgroup size 32 / 128 under XYZZ | nonce | 1.000 / 0.997 | 64 stays |
| Candidates per thread 16 / 12 under XYZZ | nonce | 1.000 / 0.982 | 8 stays |
| Subtractions folded into the preceding products' columns (`f_mul_sub`, `f_sqr_sub_sub_double`: four fewer normalizations per addition; the model passes) | nonce | 0.896 (0/60) | The subtrahend's 20 limbs stay live through the whole product, since Karatsuba assembles the columns at the end: 20 more registers at the product's peak, where pressure is worst. Any fused form has this cost |
| The window loop's trip count hidden from the compiler (to stop any unrolling) | nonce | 1.000 (19/40) | It was not being unrolled |
| SHA-256 written straight-line (constant indices, rolling 16-word schedule, inlined constants) | nonce | 1.004 (55/60) | The compiler already unrolls the loops |
| `f_reduce` splitting each high column into three 13-bit pieces instead of a 19-step carry chain (no dependent steps; the model passes) | nonce | 0.927 (0/60) | More operations; the kernel is throughput-bound, not latency-bound |
| `f_reduce`'s last chain replaced by a tail to limb 6 plus a rarely taken branch (limb 6 overflows about once in 10^4 products) that finishes the chain; the model passes | nonce | Did not compile | Apple's compiler service crashes (XPC_ERROR_CONNECTION_INTERRUPTED) with the branch inlined into every product. A branch-free version runs the full chain anyway |
| Schoolbook squaring (doubled cross products) instead of Karatsuba squaring, after the reduction changes | nonce | 0.981 (0/60) | Karatsuba squaring stays |
| Candidates per thread 16 / 4 after the field changes | nonce | 0.993 / 0.957 | 8 stays best |
| Only X and Z kept per candidate until the batch inversion (Y dropped from the stored point) | nonce | 0.999 (21/60) | The compiler already drops the dead Y |
| First addition affine plus affine (mmadd-2007-bl, 4M + 2S), window 1 peeled out of the loop; compiled this time | nonce | 0.979 (1/60) | Keeping the affine start alive for it costs more than the multiplies it saves |
| `f_sqr` as `f_mul(a, a)` (smaller code, more products) | nonce | 0.937 (0/40) | The specialized square pays; code size is not the limit |
| Table points read as four `vec4<u32>` loads instead of sixteen `u32` | nonce | 0.999 (18/60) | The compiler already merges them |
| Candidates per thread 16 / 4, workgroup size 32 / 128, after the signed windows | nonce | 0.988 / 0.965; 0.996 / 0.993 | 8 per thread and 64 per group stay best |
| One level of Karatsuba in `f_mul` and `f_sqr`, alone, before the addition was reordered | nonce | 1.011 / 1.013 (59/60, 60/60); multiply only 1.009 | Small alone; kept later together with Z3 = 2·Z1·H (see Kept) |
| Table coordinates read lazily inside the addition (x just before H, y after Z3) | nonce | 1.005 (38/60) | The compiler already schedules the loads |
| Z3 = 2·Z1·H alone | nonce | 1.008 (58/60) | Small alone; kept with Karatsuba |
| Y1·J computed right after J | nonce | 0.992 (3/60) on top of Z3 = 2·Z1·H | Holds J's product longer |
| Candidates per thread 4 / 16 (from 8) on the 13-bit nonce kernels | nonce / metal nonce | 0.98 / 0.95 at 4; 0.99 / 0.99 at 16 | 8 stays best after the field change |
| wgpu nonce on the unrolled 32-bit field instead of 13-bit | nonce | 0.89 | Without mulhi the 13-bit field still wins on wgpu |
| Metal kernels: candidates per thread 4 / 16 after the field change (from 8) | metal key / metal nonce | 0.93 / 0.90 at 4; 0.65 / 0.99 at 16 | 8 stays best |
| Metal key: stage functions inlined (only the hashes kept `noinline`) | metal key | 0.82 | `noinline` helps on Apple GPUs; see the kept row |
| Metal: `fe_mul` and `fe_sqr` `noinline` too | metal key / metal nonce | 0.59 / 0.58 | Call overhead dominates at this granularity |
| Metal: `point_double` and `scalar_mul` `noinline` (rarely or once called) | metal key / metal nonce | 0.99 / 0.99 | No gain; reverted |
| Metal: 32-bit point addition without the P = ±Q checks | metal key / metal nonce | 0.99 / 0.99 | Unlike the 13-bit field, its zero checks are cheap, and the doubling path is already out of line |
| Thread-group size 32 / 128 re-swept after the field changes | metal key, metal nonce, key | 0.93–1.005 | 64 stays best |
| wgpu key: 8 candidates per thread after the table change (from 4) | key | 0.97 | 4 stays best on wgpu, unlike Metal |
| Thread-group size 32 / 128 / 256 (from 64) | metal key, metal nonce, nonce | 0.96–0.99; wgpu nonce 0.95 at 256 | 64 stays best on every kernel and backend |
| Workgroup size 32 / 128 / 256 (from 64) | both | 1.01 / 0.93 / 0.83 key; 0.98 / 1.00 / 0.92 nonce | 64 is best; bigger groups lower occupancy |
| Low word of `mul32` from the 16-bit partial products instead of a fifth native multiply | both | 1.006 / 1.003 | Integer multiply rate is not the bottleneck |
| Affine + affine first addition (mmadd, 4M+2S) for `Q + t·G` | nonce | Did not compile | Apple's Metal compiler service crashes (XPC_ERROR_CONNECTION_INTERRUPTED) whenever the helper is in the module, even unused; expected only ~3% |
| Key kernel on the unrolled 13-bit field (4 or 1 candidates per invocation) | key | Hung the GPU | Pipeline creation or the first dispatch never finished, and the GPU then sat at 100% with no live client: killing a process does not cancel GPU work it queued. The key kernel inlines ~80 point additions per candidate; with 400-product unrolled multiplies that seems beyond what Apple's compiler handles. The benchmark now runs a 256-candidate dispatch first and refuses to queue more if it is slow |
| Metal key kernel on the 13-bit field, stages `noinline` | metal key | Hung the GPU again | Passed every staged check: compiled in 1.4 s, 1024 max threads per threadgroup (no register-pressure warning), 64 and 16,384 candidates in 37 and 45 ms. The first 131,072-candidate dispatch never finished and the GPU stayed at 100% with no process attached, needing a restart. Small probes do not predict full-load behaviour, so the 13-bit field stays out of the key kernel on both backends |
| Table reads replaced by a fixed point (ablation, not a change) | nonce | 1.07 | Memory is ~7% of the time; field arithmetic dominates |
