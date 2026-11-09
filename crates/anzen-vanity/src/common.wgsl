// The nonce kernel's shared parts: bindings, the generator table, SHA-256 and the entry points.
// The kernel defines `candidates(first)`: the vault output keys for a run of candidates, all from
// public data. The host re-derives any match on the CPU with the library code before using it.
//
// Scalars and table entries are 8 little-endian 32-bit limbs; field13.wgsl does the field
// arithmetic in 13-bit limbs.

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

// Parameter offsets; the kernel's own follow from offset 4.
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
// Q + d·G for every digit d of window 0 (Q itself at d = 0), as affine 8-limb x then y. One
// search's setup pass writes it, so candidates start from it and skip one addition.
@group(0) @binding(4) var<storage, read_write> q_table: array<u32>;

// Tuning. The host reads these constants from the shader source to build tables and size
// dispatches to match, so a shader change can vary them on its own. The kernel file defines
// WORKGROUP_SIZE and PER_INVOCATION.
const G_WINDOW_BITS: u32 = 16u;
const G_DIGITS: u32 = (1u << G_WINDOW_BITS) - 1u;
const G_WINDOWS: u32 = (256u + G_WINDOW_BITS - 1u) / G_WINDOW_BITS;

const ORDER_N = Fe(0xD0364141u, 0xBFD25E8Cu, 0xAF48A03Bu, 0xBAAEDCE6u, 0xFFFFFFFEu, 0xFFFFFFFFu, 0xFFFFFFFFu, 0xFFFFFFFFu);

// ---------------------------------------------------------------------------------------------
// 256-bit arithmetic

// a >= b as unsigned 256-bit integers.
fn gte(a: Fe, b: Fe) -> bool {
    var result = true;
    var decided = false;
    if !decided && a[7] != b[7] { result = a[7] > b[7]; decided = true; }
    if !decided && a[6] != b[6] { result = a[6] > b[6]; decided = true; }
    if !decided && a[5] != b[5] { result = a[5] > b[5]; decided = true; }
    if !decided && a[4] != b[4] { result = a[4] > b[4]; decided = true; }
    if !decided && a[3] != b[3] { result = a[3] > b[3]; decided = true; }
    if !decided && a[2] != b[2] { result = a[2] > b[2]; decided = true; }
    if !decided && a[1] != b[1] { result = a[1] > b[1]; decided = true; }
    if !decided && a[0] != b[0] { result = a[0] > b[0]; decided = true; }
    return result;
}

// ---------------------------------------------------------------------------------------------
// Points on y^2 = x^3 + 7

fn infinity() -> Jacobian {
    var p: Jacobian;
    p.infinity = true;
    return p;
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

// ---------------------------------------------------------------------------------------------
// Parameter block readers

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

// Each invocation evaluates PER_INVOCATION consecutive candidates, which lets the kernel share
// work between them (one field inversion for the batch).
@compute @workgroup_size(WORKGROUP_SIZE)
fn search(@builtin(global_invocation_id) id: vec3<u32>) {
    let first = id.x * PER_INVOCATION;
    let count = params[P_COUNT];
    if first >= count {
        return;
    }
    let batch = candidates(params[P_START] + first);
    let bits = params[P_TARGET_BITS];
    for (var j = 0u; j < PER_INVOCATION; j++) {
        let c = batch[j];
        if first + j < count && c.valid && (c.output_x[7] >> (32u - bits)) == params[P_TARGET_VALUE] {
            atomicMin(&found[0], params[P_START] + first + j);
        }
    }
}

@compute @workgroup_size(WORKGROUP_SIZE)
fn debug(@builtin(global_invocation_id) id: vec3<u32>) {
    let first = id.x * PER_INVOCATION;
    let count = params[P_COUNT];
    if first >= count {
        return;
    }
    let batch = candidates(params[P_START] + first);
    for (var j = 0u; j < PER_INVOCATION; j++) {
        if first + j < count {
            let at = (first + j) * 9u;
            debug_out[at] = select(0u, 1u, batch[j].valid);
            for (var i = 0u; i < 8u; i++) {
                debug_out[at + 1u + i] = batch[j].output_x[i];
            }
        }
    }
}
