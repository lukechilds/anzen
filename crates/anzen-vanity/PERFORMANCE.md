# GPU vanity search performance log

Two kernels, both measured on an Apple M4 Max (40-core GPU) through wgpu → Metal:

- **key**: grind the phone vault key index (BIP32 child, MuSig2 key aggregation, Taproot tweak).
- **nonce**: grind the recovery leaf's past `after()` locktime with both keys fixed (leaf hash,
  Taproot tweak, one `t·G`).

A 5-character `vault` prefix needs about 33.5M candidates on average.

## Method

`bench/ab.py` builds the benchmark at a baseline commit and from the working tree and alternates
them (ABBA), because the GPU clocks down under sustained load: runs minutes apart are not
comparable, adjacent runs are. A change is kept when its median beats the baseline's beyond the
noise measured by an A/A run, and the GPU tests still match the library code.

## Current speed

| Kernel | Candidates/s | `vault` (33.5M) |
|---|---|---|
| key | 710k (cool) | 47 s |
| nonce | 2.64M (cool) | 13 s |

## Kept

| Change | Kernel | Speedup |
|---|---|---|

## Tried and rejected

| Idea | Kernel | Result | Why |
|---|---|---|---|
