#!/usr/bin/env python3
"""Model of f_reduce's exact u32 operations: checks the single final chain, fold and limb-5 tail
normalize every product of normalized elements, against the full two-chain normalization.

    crates/anzen-vanity/tools/model_reduce.py
"""
import random
P = 2**256 - 2**32 - 977
M = 0x1fff
U = 0xffffffff

def columns(a, b):
    c = [0] * 39
    for i in range(20):
        for j in range(20):
            c[i + j] += a[i] * b[j]
    assert all(x < 2**32 for x in c)
    return c

def reduce(c, short):
    # Carry only the high columns (weight 2^260 and up), which the fold multiplies; the low
    # columns stay as sums and the next chain carries them.
    l = [0] * 40
    v = c[20]; l[20] = v & M
    for k in range(21, 39):
        v = (c[k] + (v >> 13)) & U; l[k] = v & M
    l[39] = v >> 13
    r = [0] * 22
    for i in range(22):
        t = 0
        if i < 20: t += c[i] + l[20 + i] * 15632
        if i >= 2: t += l[18 + i] * 1024
        assert t < 2**32, (i, t)
        r[i] = t
    v = r[0]; r[0] = v & M
    for i in range(1, 22):
        v = r[i] + (v >> 13); assert v < 2**32; r[i] = v & M
    carry = v >> 13
    out = r[:20]
    out[0] += r[20] * 15632; out[2] += r[20] * 1024
    out[1] += r[21] * 15632; out[3] += r[21] * 1024
    out[2] += carry * 15632; out[4] += carry * 1024
    assert all(x < 2**32 for x in out)
    if not short:
        return normalize(out)
    # One chain, one fold, then a tail through limb 5.
    v = out[0]; out[0] = v & M
    for i in range(1, 20):
        v = out[i] + (v >> 13); assert v < 2**32; out[i] = v & M
    c2 = v >> 13
    assert c2 <= 1, c2
    out[0] += c2 * 15632; out[2] += c2 * 1024
    for i in range(5):
        out[i + 1] += out[i] >> 13; out[i] &= M
    return out

def normalize(r):
    r = list(r)
    for _ in range(2):
        v = r[0]; r[0] = v & M
        for i in range(1, 20):
            v = r[i] + (v >> 13); r[i] = v & M
        c = v >> 13
        r[0] += c * 15632; r[2] += c * 1024
    for i in range(3):
        r[i + 1] += r[i] >> 13; r[i] &= M
    return r

def value(l): return sum(x << (13 * i) for i, x in enumerate(l))

def check(a, b):
    out = reduce(columns(a, b), True)
    assert all(x <= M for x in out), out
    assert value(out) < 2**260
    assert value(out) % P == (value(a) * value(b)) % P
    assert out == reduce(columns(a, b), False) or value(out) % P == value(reduce(columns(a, b), False)) % P

random.seed(1)
edges = [[M] * 20, [0] * 20, [1] + [0] * 19, [M] * 19 + [0], [0] * 19 + [M]]
pl = [(P >> (13 * i)) & M for i in range(20)]
edges += [pl, [x for x in pl]]
for a in edges:
    for b in edges:
        check(a, b)
for n in range(300000):
    kind = n % 3
    if kind == 0:
        a = [random.randrange(8192) for _ in range(20)]; b = [random.randrange(8192) for _ in range(20)]
    elif kind == 1:
        a = [random.choice([0, 1, M, M - 1, random.randrange(8192)]) for _ in range(20)]; b = [random.choice([M, random.randrange(8192)]) for _ in range(20)]
    else:
        a = [M - random.randrange(3) for _ in range(20)]; b = [M - random.randrange(3) for _ in range(20)]
    check(a, b)
print("ok: 300k random and edge products reduce to normalized, congruent elements")
