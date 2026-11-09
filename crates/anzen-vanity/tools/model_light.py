#!/usr/bin/env python3
"""Model of the light normalization's exact u32 operations (one carry chain, one fold, a tail to
limb 3) on every limb-wise form that uses it, with inputs whose limbs are at most 2^13: the
results must keep limbs at most 2^13, a value below 2^260 and the right residue.

    crates/anzen-vanity/tools/model_light.py
"""
import random

P = 2**256 - 2**32 - 977
M = 0x1FFF
# 32p with every limb at least 2^13, as in field13.wgsl.
P32 = [17888, 24570, 22525] + [24573] * 16 + [16381]
assert sum(x << (13 * i) for i, x in enumerate(P32)) == 32 * P


def light(a):
    r = list(a)
    v = r[0]
    r[0] = v & M
    for i in range(1, 20):
        v = r[i] + (v >> 13)
        assert v < 2**32
        r[i] = v & M
    c = v >> 13
    r[0] += c * 15632
    r[2] += c * 1024
    for i in range(3):
        r[i + 1] += r[i] >> 13
        r[i] &= M
    return r


def value(limbs):
    return sum(x << (13 * i) for i, x in enumerate(limbs))


FORMS = {
    "f_add": (lambda a, b, c: [a[i] + b[i] for i in range(20)], lambda a, b, c: a + b),
    "f_sub": (lambda a, b, c: [a[i] + P32[i] - b[i] for i in range(20)], lambda a, b, c: a - b),
    "f_neg": (lambda a, b, c: [P32[i] - a[i] for i in range(20)], lambda a, b, c: -a),
    "f_times4": (lambda a, b, c: [4 * a[i] for i in range(20)], lambda a, b, c: 4 * a),
    "f_double_sub": (lambda a, b, c: [2 * (a[i] + P32[i] - b[i]) for i in range(20)], lambda a, b, c: 2 * (a - b)),
    "f_sub_double": (lambda a, b, c: [a[i] + 2 * P32[i] - 2 * b[i] for i in range(20)], lambda a, b, c: a - 2 * b),
    "f_sub_sub_double": (
        lambda a, b, c: [a[i] + 2 * P32[i] - b[i] - 2 * c[i] for i in range(20)],
        lambda a, b, c: a - b - 2 * c,
    ),
}


def element(n):
    kind = n % 4
    if kind == 0:
        return [random.randrange(M + 2) for _ in range(20)]
    if kind == 1:
        return [random.choice([0, 1, M, M + 1, random.randrange(M + 2)]) for _ in range(20)]
    if kind == 2:
        return [M + 1 - random.randrange(3) for _ in range(20)]
    return [random.randrange(3) for _ in range(20)]


random.seed(4)
for name, (form, expected) in FORMS.items():
    for n in range(150000):
        a, b, c = element(n), element(n + 1), element(n + 2)
        limbs = form(a, b, c)
        assert all(0 <= x < 2**17 for x in limbs), name
        out = light(limbs)
        assert all(x <= M + 1 for x in out), (name, out)
        assert value(out) < 2**260, name
        assert (value(out) - expected(value(a), value(b), value(c))) % P == 0, name
print(f"ok: {len(FORMS)} forms x 150k inputs keep limbs at most 2^13")
