#!/usr/bin/env python3
"""Model of f_canonical's exact u32 operations on limbs at most 2^13: no carry leaves limb 19 and
the result is the canonical value.

    crates/anzen-vanity/tools/model_canonical.py
"""
import random
P = 2**256 - 2**32 - 977
M = 0x1FFF
P13 = [(P >> (13 * i)) & M for i in range(20)]
def canonical(a):
    r = list(a)
    for _ in range(2):
        top = r[19] >> 9
        r[19] &= 0x1FF
        r[0] += top * 977
        r[2] += top * 64
        carry = 0
        for i in range(20):
            v = r[i] + carry
            r[i] = v & M
            carry = v >> 13
        assert carry == 0, "a carry out of limb 19 would be dropped"
    at_least_p = True
    for i in range(19, -1, -1):
        if r[i] != P13[i]:
            at_least_p = r[i] > P13[i]
            break
    if at_least_p:
        borrow = 0
        for i in range(20):
            v = r[i] + 8192 - P13[i] - borrow
            r[i] = v & M
            borrow = 1 - (v >> 13)
    return r
def value(l): return sum(x << (13 * i) for i, x in enumerate(l))
random.seed(5)
cases = [[M + 1] * 20, [M] * 20, [0] * 20, P13, [M + 1] * 19 + [M]]
for n in range(300000):
    cases.append([random.choice([M + 1, M, 0, random.randrange(M + 2)]) for _ in range(20)])
for a in cases:
    out = canonical(a)
    assert value(out) == value(a) % P, a
print(f"ok: {len(cases)} inputs with limbs up to 2^13 reduce to the canonical value")
