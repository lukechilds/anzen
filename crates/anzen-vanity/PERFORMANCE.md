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

| Kernel | Candidates/s | `vault` (33.5M) |
|---|---|---|
| key | 710k (cool) | 47 s |
| nonce | 2.64M (cool) | 13 s |

## Where the time goes

Ablating the nonce kernel at the start (8-bit windows): the 32 table additions for `t·G` were
~67% of its time, the final inversion ~32%, and its three SHA-256 compressions under 1%.

## Kept

| Change | Kernel | Speedup |
|---|---|---|
| Nonce: 8 candidates per invocation share one field inversion (Montgomery's trick); 4 gave 1.49×, 16 gave 1.56× | nonce | 1.55× |
| G table windows 8 → 16 bits: 16 mixed additions per `k·G` instead of 32, 67 MB table built on every core (12 bits gave +21%/+27%) | key / nonce | 1.38× / 1.52× |

## Tried and rejected

| Idea | Kernel | Result | Why |
|---|---|---|---|
