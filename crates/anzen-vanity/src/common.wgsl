// Shared code for the vanity kernels: 256-bit field and scalar arithmetic, secp256k1 points,
// precomputed point tables, SHA-256 and SHA-512, and the entry points. Each kernel file defines
// `candidate(index)`: the vault output key for one candidate, all from public data. The host
// re-derives any match on the CPU with the library code before using it.
//
// Field elements and scalars are 8 little-endian 32-bit limbs. WGSL has no 64-bit integers,
// so 32x32-bit products are built from 16-bit halves.

alias Fe = array<u32, 8>;

struct Jacobian {
    x: Fe,
    y: Fe,
    z: Fe,
    infinity: bool,
}

struct Affine {
    x: Fe,
    y: Fe,
    infinity: bool,
}

// Parameter offsets shared by every kernel; each kernel's own follow from offset 4.
const P_START: u32 = 0u;
const P_COUNT: u32 = 1u;
const P_TARGET_VALUE: u32 = 2u;
const P_TARGET_BITS: u32 = 3u;

@group(0) @binding(0) var<storage, read> params: array<u32>;
// Affine multiples of G: entry (window w, digit d) is d·2^(G_WINDOW_BITS·w)·G, for d in
// 1..2^G_WINDOW_BITS.
@group(0) @binding(1) var<storage, read> g_table: array<u32>;
@group(0) @binding(2) var<storage, read_write> found: array<atomic<u32>, 1>;
// Debug only: per candidate, a validity word then the output key's x limbs.
@group(0) @binding(3) var<storage, read_write> debug_out: array<u32>;

// Tuning. The host reads these constants from the shader source to build tables and size
// dispatches to match, so a shader change can vary them on its own.
const G_WINDOW_BITS: u32 = 16u;
const WORKGROUP_SIZE: u32 = 64u;
const G_DIGITS: u32 = (1u << G_WINDOW_BITS) - 1u;
const G_WINDOWS: u32 = (256u + G_WINDOW_BITS - 1u) / G_WINDOW_BITS;

const FIELD_P = Fe(0xFFFFFC2Fu, 0xFFFFFFFEu, 0xFFFFFFFFu, 0xFFFFFFFFu, 0xFFFFFFFFu, 0xFFFFFFFFu, 0xFFFFFFFFu, 0xFFFFFFFFu);
const ORDER_N = Fe(0xD0364141u, 0xBFD25E8Cu, 0xAF48A03Bu, 0xBAAEDCE6u, 0xFFFFFFFEu, 0xFFFFFFFFu, 0xFFFFFFFFu, 0xFFFFFFFFu);
// 2^256 - n.
const ORDER_N_COMPLEMENT = array<u32, 5>(0x2FC9BEBFu, 0x402DA173u, 0x50B75FC4u, 0x45512319u, 1u);

// ---------------------------------------------------------------------------------------------
// 256-bit arithmetic

// The full 64-bit product of two 32-bit words, as (low, high).
fn mul32(a: u32, b: u32) -> vec2<u32> {
    let a0 = a & 0xffffu;
    let a1 = a >> 16u;
    let b0 = b & 0xffffu;
    let b1 = b >> 16u;
    let p00 = a0 * b0;
    let p01 = a0 * b1;
    let p10 = a1 * b0;
    let p11 = a1 * b1;
    let middle = (p00 >> 16u) + (p01 & 0xffffu) + (p10 & 0xffffu);
    let low = a * b;
    let high = p11 + (p01 >> 16u) + (p10 >> 16u) + (middle >> 16u);
    return vec2<u32>(low, high);
}

fn is_zero(a: Fe) -> bool {
    var bits = 0u;
    for (var i = 0u; i < 8u; i++) {
        bits |= a[i];
    }
    return bits == 0u;
}

fn equal(a: Fe, b: Fe) -> bool {
    var bits = 0u;
    for (var i = 0u; i < 8u; i++) {
        bits |= a[i] ^ b[i];
    }
    return bits == 0u;
}

// a >= b as unsigned 256-bit integers.
fn gte(a: Fe, b: Fe) -> bool {
    for (var i = 7i; i >= 0i; i--) {
        if a[i] != b[i] {
            return a[i] > b[i];
        }
    }
    return true;
}

// a - b mod 2^256.
fn sub_raw(a: Fe, b: Fe) -> Fe {
    var r: Fe;
    var borrow = 0u;
    for (var i = 0u; i < 8u; i++) {
        let d = a[i] - b[i];
        let b1 = select(0u, 1u, a[i] < b[i]);
        let d2 = d - borrow;
        let b2 = select(0u, 1u, d < borrow);
        r[i] = d2;
        borrow = b1 + b2;
    }
    return r;
}

// a + b + carry, as (sum, carry out); carry and the result's carry are small.
fn adc(a: u32, b: u32, carry: u32) -> vec2<u32> {
    let s1 = a + b;
    let s2 = s1 + carry;
    return vec2<u32>(s2, select(0u, 1u, s1 < a) + select(0u, 1u, s2 < s1));
}

// ---------------------------------------------------------------------------------------------
// The field modulo p = 2^256 - 2^32 - 977. Every function returns a fully reduced element.

// Reduce a 512-bit product using 2^256 ≡ 2^32 + 977 (mod p), with no data-dependent loops.
fn fe_reduce(t: array<u32, 16>) -> Fe {
    // r = low + high·977 + (high << 32), keeping the overflow above limb 7 in `top`.
    var r: Fe;
    var carry = 0u;
    var previous_high = 0u;
    var previous_word = 0u;
    for (var i = 0u; i < 8u; i++) {
        let p = mul32(t[8u + i], 977u);
        var s = adc(t[i], p.x, carry);
        let s2 = adc(s.x, previous_high, 0u);
        let s3 = adc(s2.x, previous_word, 0u);
        r[i] = s3.x;
        carry = s.y + s2.y + s3.y;
        previous_high = p.y;
        previous_word = t[8u + i];
    }
    // top < 2^34: previous_high + previous_word + carry.
    let top_sum = adc(previous_high, previous_word, carry);
    let top_low = top_sum.x;
    let top_high = top_sum.y;
    // Fold top·(2^32 + 977) back in.
    let p = mul32(top_low, 977u);
    var s = adc(r[0], p.x, 0u);
    r[0] = s.x;
    s = adc(r[1], p.y, s.y);
    let s1 = adc(s.x, top_low, 0u);
    let s1b = adc(s1.x, top_high * 977u, 0u);
    r[1] = s1b.x;
    var c = s.y + s1.y + s1b.y;
    s = adc(r[2], top_high, c);
    r[2] = s.x;
    c = s.y;
    for (var i = 3u; i < 8u; i++) {
        s = adc(r[i], 0u, c);
        r[i] = s.x;
        c = s.y;
    }
    // A final carry leaves r small, so adding 2^32 + 977 cannot carry again.
    let extra = c * 977u;
    s = adc(r[0], extra, 0u);
    r[0] = s.x;
    s = adc(r[1], c, s.y);
    r[1] = s.x;
    c = s.y;
    for (var i = 2u; i < 8u; i++) {
        s = adc(r[i], 0u, c);
        r[i] = s.x;
        c = s.y;
    }
    if gte(r, FIELD_P) {
        r = sub_raw(r, FIELD_P);
    }
    return r;
}

// The full 512-bit product.
fn mul_wide(a: Fe, b: Fe) -> array<u32, 16> {
    var t: array<u32, 16>;
    for (var i = 0u; i < 8u; i++) {
        var carry = 0u;
        for (var j = 0u; j < 8u; j++) {
            let p = mul32(a[i], b[j]);
            let s1 = t[i + j] + p.x;
            let c1 = select(0u, 1u, s1 < p.x);
            let s2 = s1 + carry;
            let c2 = select(0u, 1u, s2 < carry);
            t[i + j] = s2;
            carry = p.y + c1 + c2;
        }
        t[i + 8u] = carry;
    }
    return t;
}

// ---------------------------------------------------------------------------------------------
// Scalars modulo the group order n.

// low + high·(2^256 - n), which is congruent mod n and shorter.
fn scalar_fold(t: array<u32, 16>) -> array<u32, 16> {
    var r: array<u32, 16>;
    for (var i = 0u; i < 8u; i++) {
        r[i] = t[i];
    }
    for (var i = 0u; i < 8u; i++) {
        var carry = 0u;
        for (var j = 0u; j < 5u; j++) {
            let p = mul32(t[8u + i], ORDER_N_COMPLEMENT[j]);
            let s1 = r[i + j] + p.x;
            let c1 = select(0u, 1u, s1 < p.x);
            let s2 = s1 + carry;
            let c2 = select(0u, 1u, s2 < carry);
            r[i + j] = s2;
            carry = p.y + c1 + c2;
        }
        for (var k = i + 5u; k < 16u; k++) {
            let s = r[k] + carry;
            carry = select(0u, 1u, s < carry);
            r[k] = s;
        }
    }
    return r;
}

// a·b mod n. Each fold shrinks the high half: to 129 bits, 3 bits, 1 bit, then nothing.
fn scalar_mul(a: Fe, b: Fe) -> Fe {
    var t = mul_wide(a, b);
    for (var round = 0u; round < 4u; round++) {
        t = scalar_fold(t);
    }
    var r: Fe;
    for (var i = 0u; i < 8u; i++) {
        r[i] = t[i];
    }
    if gte(r, ORDER_N) {
        r = sub_raw(r, ORDER_N);
    }
    return r;
}

fn scalar_neg(a: Fe) -> Fe {
    if is_zero(a) {
        return a;
    }
    return sub_raw(ORDER_N, a);
}

fn fe_mul(a: Fe, b: Fe) -> Fe {
    return fe_reduce(mul_wide(a, b));
}

// Squaring computes each cross product once and doubles it: 36 word products instead of 64.
fn fe_sqr(a: Fe) -> Fe {
    var t: array<u32, 16>;
    for (var i = 0u; i < 8u; i++) {
        var carry = 0u;
        for (var j = i + 1u; j < 8u; j++) {
            let p = mul32(a[i], a[j]);
            let s1 = t[i + j] + p.x;
            let c1 = select(0u, 1u, s1 < p.x);
            let s2 = s1 + carry;
            let c2 = select(0u, 1u, s2 < carry);
            t[i + j] = s2;
            carry = p.y + c1 + c2;
        }
        t[i + 8u] = carry;
    }
    // The cross products sum to less than 2^511, so doubling cannot overflow.
    for (var i = 15u; i > 0u; i--) {
        t[i] = (t[i] << 1u) | (t[i - 1u] >> 31u);
    }
    t[0] = t[0] << 1u;
    var carry = 0u;
    for (var i = 0u; i < 8u; i++) {
        let p = mul32(a[i], a[i]);
        let low = adc(t[2u * i], p.x, carry);
        let high = adc(t[2u * i + 1u], p.y, low.y);
        t[2u * i] = low.x;
        t[2u * i + 1u] = high.x;
        carry = high.y;
    }
    return fe_reduce(t);
}

fn fe_sqr_n(a: Fe, n: u32) -> Fe {
    var r = a;
    for (var i = 0u; i < n; i++) {
        r = fe_sqr(r);
    }
    return r;
}

fn fe_add(a: Fe, b: Fe) -> Fe {
    var r: Fe;
    var carry = 0u;
    for (var i = 0u; i < 8u; i++) {
        let s1 = a[i] + b[i];
        let c1 = select(0u, 1u, s1 < a[i]);
        let s2 = s1 + carry;
        let c2 = select(0u, 1u, s2 < carry);
        r[i] = s2;
        carry = c1 + c2;
    }
    if carry != 0u || gte(r, FIELD_P) {
        r = sub_raw(r, FIELD_P);
    }
    return r;
}

fn fe_sub(a: Fe, b: Fe) -> Fe {
    if gte(a, b) {
        return sub_raw(a, b);
    }
    // a - b + p, computed as p - (b - a).
    return sub_raw(FIELD_P, sub_raw(b, a));
}

fn fe_neg(a: Fe) -> Fe {
    if is_zero(a) {
        return a;
    }
    return sub_raw(FIELD_P, a);
}

// a^(p-2) with libsecp256k1's addition chain: 255 squarings and 15 multiplications.
fn fe_inv(a: Fe) -> Fe {
    let x2 = fe_mul(fe_sqr(a), a);
    let x3 = fe_mul(fe_sqr(x2), a);
    let x6 = fe_mul(fe_sqr_n(x3, 3u), x3);
    let x9 = fe_mul(fe_sqr_n(x6, 3u), x3);
    let x11 = fe_mul(fe_sqr_n(x9, 2u), x2);
    let x22 = fe_mul(fe_sqr_n(x11, 11u), x11);
    let x44 = fe_mul(fe_sqr_n(x22, 22u), x22);
    let x88 = fe_mul(fe_sqr_n(x44, 44u), x44);
    let x176 = fe_mul(fe_sqr_n(x88, 88u), x88);
    let x220 = fe_mul(fe_sqr_n(x176, 44u), x44);
    let x223 = fe_mul(fe_sqr_n(x220, 3u), x3);
    var t = fe_mul(fe_sqr_n(x223, 23u), x22);
    t = fe_mul(fe_sqr_n(t, 5u), a);
    t = fe_mul(fe_sqr_n(t, 3u), x2);
    return fe_mul(fe_sqr_n(t, 2u), a);
}

// ---------------------------------------------------------------------------------------------
// Points on y^2 = x^3 + 7

fn infinity() -> Jacobian {
    var p: Jacobian;
    p.infinity = true;
    return p;
}

fn from_affine(q: Affine) -> Jacobian {
    var one: Fe;
    one[0] = 1u;
    return Jacobian(q.x, q.y, one, q.infinity);
}

// dbl-2009-l.
fn point_double(p: Jacobian) -> Jacobian {
    if p.infinity || is_zero(p.y) {
        return infinity();
    }
    let a = fe_sqr(p.x);
    let b = fe_sqr(p.y);
    let c = fe_sqr(b);
    let xb = fe_add(p.x, b);
    var d = fe_sub(fe_sub(fe_sqr(xb), a), c);
    d = fe_add(d, d);
    let e = fe_add(fe_add(a, a), a);
    let f = fe_sqr(e);
    let x3 = fe_sub(f, fe_add(d, d));
    var c8 = fe_add(c, c);
    c8 = fe_add(c8, c8);
    c8 = fe_add(c8, c8);
    let y3 = fe_sub(fe_mul(e, fe_sub(d, x3)), c8);
    let yz = fe_mul(p.y, p.z);
    let z3 = fe_add(yz, yz);
    return Jacobian(x3, y3, z3, false);
}

// Jacobian plus affine (madd-2007-bl), with the special cases.
fn point_add_affine(p: Jacobian, q: Affine) -> Jacobian {
    if q.infinity {
        return p;
    }
    if p.infinity {
        return from_affine(q);
    }
    let z1z1 = fe_sqr(p.z);
    let u2 = fe_mul(q.x, z1z1);
    let s2 = fe_mul(q.y, fe_mul(p.z, z1z1));
    let h = fe_sub(u2, p.x);
    let s_diff = fe_sub(s2, p.y);
    if is_zero(h) {
        if is_zero(s_diff) {
            return point_double(p);
        }
        return infinity();
    }
    let hh = fe_sqr(h);
    var i = fe_add(hh, hh);
    i = fe_add(i, i);
    let j = fe_mul(h, i);
    let r = fe_add(s_diff, s_diff);
    let v = fe_mul(p.x, i);
    let x3 = fe_sub(fe_sub(fe_sqr(r), j), fe_add(v, v));
    let y1j = fe_mul(p.y, j);
    let y3 = fe_sub(fe_mul(r, fe_sub(v, x3)), fe_add(y1j, y1j));
    let z3 = fe_sub(fe_sub(fe_sqr(fe_add(p.z, h)), z1z1), hh);
    return Jacobian(x3, y3, z3, false);
}

fn to_affine(p: Jacobian) -> Affine {
    if p.infinity {
        var q: Affine;
        q.infinity = true;
        return q;
    }
    let zi = fe_inv(p.z);
    let zi2 = fe_sqr(zi);
    return Affine(fe_mul(p.x, zi2), fe_mul(p.y, fe_mul(zi2, zi)), false);
}

fn g_point(window: u32, digit: u32) -> Affine {
    let base = (window * G_DIGITS + digit - 1u) * 16u;
    var q: Affine;
    for (var i = 0u; i < 8u; i++) {
        q.x[i] = g_table[base + i];
        q.y[i] = g_table[base + 8u + i];
    }
    return q;
}

// Bits `bit..bit + width` of k, for width < 32.
fn window_digit(k: Fe, bit: u32, width: u32) -> u32 {
    let limb = bit / 32u;
    let shift = bit % 32u;
    var digit = k[limb] >> shift;
    if shift + width > 32u && limb < 7u {
        digit |= k[limb + 1u] << (32u - shift);
    }
    return digit & ((1u << width) - 1u);
}

// acc + k·G from the table: one mixed addition per nonzero window of k, no doublings.
fn add_mul_g(start: Jacobian, k: Fe) -> Jacobian {
    var acc = start;
    for (var w = 0u; w < G_WINDOWS; w++) {
        let digit = window_digit(k, w * G_WINDOW_BITS, G_WINDOW_BITS);
        if digit != 0u {
            acc = point_add_affine(acc, g_point(w, digit));
        }
    }
    return acc;
}

// ---------------------------------------------------------------------------------------------
// SHA-256 (from a midstate)

fn rotr(x: u32, n: u32) -> u32 {
    return (x >> n) | (x << (32u - n));
}

fn sha256_compress(state: array<u32, 8>, block: array<u32, 16>) -> array<u32, 8> {
    var w: array<u32, 64>;
    for (var i = 0u; i < 16u; i++) {
        w[i] = block[i];
    }
    for (var i = 16u; i < 64u; i++) {
        let s0 = rotr(w[i - 15u], 7u) ^ rotr(w[i - 15u], 18u) ^ (w[i - 15u] >> 3u);
        let s1 = rotr(w[i - 2u], 17u) ^ rotr(w[i - 2u], 19u) ^ (w[i - 2u] >> 10u);
        w[i] = w[i - 16u] + s0 + w[i - 7u] + s1;
    }
    var a = state[0];
    var b = state[1];
    var c = state[2];
    var d = state[3];
    var e = state[4];
    var f = state[5];
    var g = state[6];
    var h = state[7];
    for (var i = 0u; i < 64u; i++) {
        let s1 = rotr(e, 6u) ^ rotr(e, 11u) ^ rotr(e, 25u);
        let ch = (e & f) ^ (~e & g);
        let t1 = h + s1 + ch + K256[i] + w[i];
        let s0 = rotr(a, 2u) ^ rotr(a, 13u) ^ rotr(a, 22u);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0 + maj;
        h = g;
        g = f;
        f = e;
        e = d + t1;
        d = c;
        c = b;
        b = a;
        a = t1 + t2;
    }
    return array<u32, 8>(state[0] + a, state[1] + b, state[2] + c, state[3] + d,
        state[4] + e, state[5] + f, state[6] + g, state[7] + h);
}

// SHA-256 of `prefix || data`, where the 64-byte prefix (the tag hashes) is already compressed
// into `midstate`. `data` holds one byte per element; at most 119 bytes.
fn sha256_tagged(midstate: array<u32, 8>, data: ptr<function, array<u32, 128>>, length: u32) -> array<u32, 8> {
    let total_bits = (64u + length) * 8u;
    let blocks = select(2u, 1u, length + 9u <= 64u);
    var state = midstate;
    for (var block_index = 0u; block_index < blocks; block_index++) {
        var block: array<u32, 16>;
        for (var word = 0u; word < 16u; word++) {
            var value = 0u;
            for (var byte = 0u; byte < 4u; byte++) {
                let position = block_index * 64u + word * 4u + byte;
                var b = 0u;
                if position < length {
                    b = (*data)[position];
                } else if position == length {
                    b = 0x80u;
                }
                value = (value << 8u) | b;
            }
            block[word] = value;
        }
        if block_index == blocks - 1u {
            block[15] = total_bits;
        }
        state = sha256_compress(state, block);
    }
    return state;
}

// ---------------------------------------------------------------------------------------------
// SHA-512 with 64-bit words as (high, low) pairs

fn add64(a: vec2<u32>, b: vec2<u32>) -> vec2<u32> {
    let low = a.y + b.y;
    let carry = select(0u, 1u, low < a.y);
    return vec2<u32>(a.x + b.x + carry, low);
}

fn rotr64(x: vec2<u32>, n: u32) -> vec2<u32> {
    if n == 32u {
        return vec2<u32>(x.y, x.x);
    }
    if n < 32u {
        return vec2<u32>((x.x >> n) | (x.y << (32u - n)), (x.y >> n) | (x.x << (32u - n)));
    }
    let m = n - 32u;
    return vec2<u32>((x.y >> m) | (x.x << (32u - m)), (x.x >> m) | (x.y << (32u - m)));
}

fn shr64(x: vec2<u32>, n: u32) -> vec2<u32> {
    return vec2<u32>(x.x >> n, (x.y >> n) | (x.x << (32u - n)));
}

fn sha512_compress(state: array<vec2<u32>, 8>, block: array<vec2<u32>, 16>) -> array<vec2<u32>, 8> {
    var w: array<vec2<u32>, 80>;
    for (var i = 0u; i < 16u; i++) {
        w[i] = block[i];
    }
    for (var i = 16u; i < 80u; i++) {
        let s0 = rotr64(w[i - 15u], 1u) ^ rotr64(w[i - 15u], 8u) ^ shr64(w[i - 15u], 7u);
        let s1 = rotr64(w[i - 2u], 19u) ^ rotr64(w[i - 2u], 61u) ^ shr64(w[i - 2u], 6u);
        w[i] = add64(add64(w[i - 16u], s0), add64(w[i - 7u], s1));
    }
    var a = state[0];
    var b = state[1];
    var c = state[2];
    var d = state[3];
    var e = state[4];
    var f = state[5];
    var g = state[6];
    var h = state[7];
    for (var i = 0u; i < 80u; i++) {
        let s1 = rotr64(e, 14u) ^ rotr64(e, 18u) ^ rotr64(e, 41u);
        let ch = (e & f) ^ (~e & g);
        let k = vec2<u32>(K512[2u * i], K512[2u * i + 1u]);
        let t1 = add64(add64(add64(h, s1), add64(ch, k)), w[i]);
        let s0 = rotr64(a, 28u) ^ rotr64(a, 34u) ^ rotr64(a, 39u);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = add64(s0, maj);
        h = g;
        g = f;
        f = e;
        e = add64(d, t1);
        d = c;
        c = b;
        b = a;
        a = add64(t1, t2);
    }
    return array<vec2<u32>, 8>(add64(state[0], a), add64(state[1], b), add64(state[2], c),
        add64(state[3], d), add64(state[4], e), add64(state[5], f), add64(state[6], g),
        add64(state[7], h));
}

fn read_state512(offset: u32) -> array<vec2<u32>, 8> {
    var s: array<vec2<u32>, 8>;
    for (var i = 0u; i < 8u; i++) {
        s[i] = vec2<u32>(params[offset + 2u * i], params[offset + 2u * i + 1u]);
    }
    return s;
}

fn read_state256(offset: u32) -> array<u32, 8> {
    var s: array<u32, 8>;
    for (var i = 0u; i < 8u; i++) {
        s[i] = params[offset + i];
    }
    return s;
}

fn read_fe(offset: u32) -> Fe {
    var r: Fe;
    for (var i = 0u; i < 8u; i++) {
        r[i] = params[offset + i];
    }
    return r;
}

// Byte `i` (big-endian) of a field element.
fn fe_byte(a: Fe, i: u32) -> u32 {
    return (a[7u - i / 4u] >> (24u - 8u * (i % 4u))) & 0xffu;
}

fn digest_to_fe(digest: array<u32, 8>) -> Fe {
    var r: Fe;
    for (var i = 0u; i < 8u; i++) {
        r[7u - i] = digest[i];
    }
    return r;
}

struct Candidate {
    valid: bool,
    output_x: Fe,
}

@compute @workgroup_size(WORKGROUP_SIZE)
fn search(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params[P_COUNT] {
        return;
    }
    let index = params[P_START] + id.x;
    let c = candidate(index);
    let bits = params[P_TARGET_BITS];
    if c.valid && (c.output_x[7] >> (32u - bits)) == params[P_TARGET_VALUE] {
        atomicMin(&found[0], index);
    }
}

@compute @workgroup_size(WORKGROUP_SIZE)
fn debug(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params[P_COUNT] {
        return;
    }
    let c = candidate(params[P_START] + id.x);
    debug_out[id.x * 9u] = select(0u, 1u, c.valid);
    for (var i = 0u; i < 8u; i++) {
        debug_out[id.x * 9u + 1u + i] = c.output_x[i];
    }
}
