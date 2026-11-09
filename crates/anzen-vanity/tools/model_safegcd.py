#!/usr/bin/env python3
"""Model of f_inv: Bernstein-Yang safegcd inversion mod p in batches of 13 divsteps over 20 signed 13-bit limbs,
simulating exact i32/u32 wrapping arithmetic, as libsecp256k1's modinv32 does with 30-bit limbs."""
import random
P = 2**256 - 2**32 - 977
B = 13                       # divsteps per batch and bits per limb
N = 20                       # limbs
MASK = (1 << B) - 1
BATCHES = 46                 # 46 * 13 = 598 >= 590, the zeta-variant bound for 256 bits
U32 = (1 << 32) - 1

def i32(x):
    x &= U32
    return x - (1 << 32) if x & (1 << 31) else x

def u32(x): return x & U32

def to_limbs(x):
    # signed limbs: limbs 0..18 in [0, 2^13), limb 19 holds the rest (signed)
    out = []
    for i in range(N - 1):
        out.append(x & MASK); x >>= B
    out.append(x)
    return out

def from_limbs(l): return sum(v << (B * i) for i, v in enumerate(l))

P_LIMBS = to_limbs(P)
P_INV = pow(P, -1, 1 << B)   # p^-1 mod 2^13

def divsteps(zeta, f0, g0):
    u, v, q, r = 1, 0, 0, 1
    f, g = u32(f0), u32(g0)
    u, v, q, r = u32(u), u32(v), u32(q), u32(r)
    zeta = i32(zeta)
    for _ in range(B):
        c1 = u32(zeta >> 31)              # all ones if zeta < 0
        c2 = u32(-(g & 1))                # all ones if g odd
        x = u32((f ^ c1) - c1)
        y = u32((u ^ c1) - c1)
        z = u32((v ^ c1) - c1)
        g = u32(g + (x & c2)); q = u32(q + (y & c2)); r = u32(r + (z & c2))
        c1 &= c2
        zeta = i32((zeta ^ i32(c1)) - 1)
        f = u32(f + (g & c1)); u = u32(u + (q & c1)); v = u32(v + (r & c1))
        g >>= 1; u = u32(u << 1); v = u32(v << 1)
    return zeta, i32(u), i32(v), i32(q), i32(r)

def update_fg(f, g, t):
    u, v, q, r = t
    cf = u * f[0] + v * g[0]
    cg = q * f[0] + r * g[0]
    assert -2**31 <= cf < 2**31 and -2**31 <= cg < 2**31
    assert cf & MASK == 0 and cg & MASK == 0
    cf >>= B; cg >>= B
    nf, ng = [0] * N, [0] * N
    for i in range(1, N):
        cf += u * f[i] + v * g[i]
        cg += q * f[i] + r * g[i]
        assert -2**31 <= cf < 2**31 and -2**31 <= cg < 2**31, (i, cf, cg)
        nf[i - 1] = cf & MASK; cf >>= B
        ng[i - 1] = cg & MASK; cg >>= B
    nf[N - 1] = cf; ng[N - 1] = cg
    return nf, ng

def update_de(d, e, t):
    u, v, q, r = t
    sd = -1 if d[N - 1] < 0 else 0
    se = -1 if e[N - 1] < 0 else 0
    md = (u & sd) + (v & se)
    me = (q & sd) + (r & se)
    cd = u * d[0] + v * e[0]
    ce = q * d[0] + r * e[0]
    md -= (P_INV * cd + md) & MASK
    me -= (P_INV * ce + me) & MASK
    cd += P_LIMBS[0] * md
    ce += P_LIMBS[0] * me
    assert cd & MASK == 0 and ce & MASK == 0
    cd >>= B; ce >>= B
    nd, ne = [0] * N, [0] * N
    for i in range(1, N):
        cd += u * d[i] + v * e[i] + P_LIMBS[i] * md
        ce += q * d[i] + r * e[i] + P_LIMBS[i] * me
        assert -2**31 <= cd < 2**31 and -2**31 <= ce < 2**31, (i, cd, ce)
        nd[i - 1] = cd & MASK; cd >>= B
        ne[i - 1] = ce & MASK; ce >>= B
    nd[N - 1] = cd; ne[N - 1] = ce
    return nd, ne

def inverse(a):
    f, g = P_LIMBS[:], to_limbs(a)
    d, e = to_limbs(0), to_limbs(1)
    zeta = -1
    for _ in range(BATCHES):
        zeta, u, v, q, r = divsteps(zeta, f[0], g[0])
        d, e = update_de(d, e, (u, v, q, r))
        f, g = update_fg(f, g, (u, v, q, r))
    assert from_limbs(g) == 0, "g must reach zero"
    fv = from_limbs(f)
    assert fv in (1, -1), fv
    dv = from_limbs(d)
    assert -2 * P < dv < P, dv
    return (dv * fv) % P

random.seed(7)
cases = [1, 2, 3, P - 1, P - 2, 2**255, 2**256 - 2**32 - 978, 7, 2**200 + 1]
cases += [random.randrange(1, P) for _ in range(20000)]
for a in cases:
    assert inverse(a) * a % P == 1, a
print(f"ok: {len(cases)} inversions with {BATCHES} batches of {B} divsteps")
