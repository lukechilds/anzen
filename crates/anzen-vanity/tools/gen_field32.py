#!/usr/bin/env python3
"""Unroll the 32-bit-limb field functions in src/common.wgsl (mul_wide, fe_sqr, fe_add, sub_raw,
gte) into straight-line code, replacing each function by name.

Apple's Metal compiler keeps loop-indexed arrays in memory; with every index constant the wgpu
key kernel ran 2.29x faster. Run after changing any of these functions' logic:

    crates/anzen-vanity/tools/gen_field32.py
"""

from pathlib import Path

FILE = Path(__file__).resolve().parents[1] / "src" / "common.wgsl"
CARRY = "carry = p.y + select(0u, 1u, s1 < p.x) + select(0u, 1u, s2 < s1);"


def mul_wide() -> list[str]:
    lines = [
        "// The full 512-bit product.",
        "fn mul_wide(a: Fe, b: Fe) -> array<u32, 16> {",
        "    var t: array<u32, 16>;",
        "    var p: vec2<u32>;",
        "    var s1: u32;",
        "    var s2: u32;",
        "    var carry: u32;",
    ]
    for i in range(8):
        lines.append("    carry = 0u;")
        for j in range(8):
            lines.append(f"    p = mul32(a[{i}], b[{j}]);")
            if i == 0:
                lines.append(
                    f"    s2 = p.x + carry; t[{i + j}] = s2; carry = p.y + select(0u, 1u, s2 < carry);"
                )
            else:
                lines.append(f"    s1 = t[{i + j}] + p.x; s2 = s1 + carry; t[{i + j}] = s2; {CARRY}")
        lines.append(f"    t[{i + 8}] = carry;")
    return lines + ["    return t;", "}", ""]


def fe_sqr() -> list[str]:
    lines = [
        "// Squaring computes each cross product once and doubles it: 36 word products instead of 64.",
        "fn fe_sqr(a: Fe) -> Fe {",
        "    var t: array<u32, 16>;",
        "    var p: vec2<u32>;",
        "    var s1: u32;",
        "    var s2: u32;",
        "    var carry: u32;",
    ]
    for i in range(8):
        if i == 7:
            lines.append("    t[15] = 0u;")
            continue
        lines.append("    carry = 0u;")
        for j in range(i + 1, 8):
            lines.append(f"    p = mul32(a[{i}], a[{j}]);")
            lines.append(f"    s1 = t[{i + j}] + p.x; s2 = s1 + carry; t[{i + j}] = s2; {CARRY}")
        lines.append(f"    t[{i + 8}] = carry;")
    lines.append("    // The cross products sum to less than 2^511, so doubling cannot overflow.")
    lines += [f"    t[{i}] = (t[{i}] << 1u) | (t[{i - 1}] >> 31u);" for i in range(15, 0, -1)]
    lines += ["    t[0] = t[0] << 1u;", "    var low: vec2<u32>;", "    var high: vec2<u32>;", "    carry = 0u;"]
    for i in range(8):
        lines.append(f"    p = mul32(a[{i}], a[{i}]);")
        lines.append(
            f"    low = adc(t[{2 * i}], p.x, carry); high = adc(t[{2 * i + 1}], p.y, low.y);"
            f" t[{2 * i}] = low.x; t[{2 * i + 1}] = high.x; carry = high.y;"
        )
    return lines + ["    return fe_reduce(t);", "}", ""]


def fe_add() -> list[str]:
    lines = ["fn fe_add(a: Fe, b: Fe) -> Fe {", "    var r: Fe;", "    var s: vec2<u32>;"]
    lines.append("    s = adc(a[0], b[0], 0u); r[0] = s.x;")
    lines += [f"    s = adc(a[{i}], b[{i}], s.y); r[{i}] = s.x;" for i in range(1, 8)]
    lines += ["    if s.y != 0u || gte(r, FIELD_P) {", "        r = sub_raw(r, FIELD_P);", "    }"]
    return lines + ["    return r;", "}", ""]


def sub_raw() -> list[str]:
    lines = ["// a - b mod 2^256.", "fn sub_raw(a: Fe, b: Fe) -> Fe {", "    var r: Fe;", "    var borrow = 0u;", "    var d: u32;"]
    for i in range(8):
        lines.append(
            f"    d = a[{i}] - b[{i}]; r[{i}] = d - borrow;"
            f" borrow = select(0u, 1u, a[{i}] < b[{i}]) + select(0u, 1u, d < borrow);"
        )
    return lines + ["    return r;", "}", ""]


def gte() -> list[str]:
    lines = ["// a >= b as unsigned 256-bit integers.", "fn gte(a: Fe, b: Fe) -> bool {", "    var result = true;", "    var decided = false;"]
    for i in range(7, -1, -1):
        lines.append(f"    if !decided && a[{i}] != b[{i}] {{ result = a[{i}] > b[{i}]; decided = true; }}")
    return lines + ["    return result;", "}", ""]


def main() -> None:
    source = FILE.read_text()
    for name, generate in [("mul_wide", mul_wide), ("fe_sqr", fe_sqr), ("fe_add", fe_add),
                           ("sub_raw", sub_raw), ("gte", gte)]:
        start = source.index(f"fn {name}(")
        # Replace any comment block directly above the function too.
        while source.rfind("\n//", 0, start) == source.rfind("\n", 0, start - 1) and start > 0:
            start = source.rfind("\n", 0, start - 1) + 1
        end = source.index("\n}\n", start) + 3
        source = source[:start] + "\n".join(generate()) + source[end:]
    FILE.write_text(source)


if __name__ == "__main__":
    main()
