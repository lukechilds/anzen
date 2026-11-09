// Script nonce grinding: one candidate per recovery locktime T, with both device keys fixed.
// The vault is tr(Q, and_v(v:after(T),and_v(v:older(4259839),pk(hww)))), where Q is the MuSig2
// aggregate of the two keys. T is a Unix time long past, so its lock is always satisfied; it only
// varies the address.
//   1. The leaf hash: TaggedHash("TapLeaf", 0xc0 || 47 || script), where only T's four bytes vary.
//   2. The Taproot tweak t = TaggedHash("TapTweak", Q || leaf), the leaf being the only one.
//   3. output = Q_even + t·G: window 0 of t from the search's table of Q + d·G, then one addition
//      per generator-table window in XYZZ coordinates (the first affine plus affine), and the
//      output key x = X·ZZ⁻¹ with one safegcd inversion per PER_INVOCATION candidates.

// The leaf's message block with T's bytes zero: 0xc0, 47, then the script
// <T> OP_CLTV OP_VERIFY <4259839> OP_CSV OP_VERIFY <hww> OP_CHECKSIG, then SHA-256 padding.
const P_LEAF_BLOCK: u32 = 4u;
const P_LEAF_MID: u32 = 20u;
const P_TWEAK_MID: u32 = 28u;
// Q's x coordinate as big-endian words for hashing, then Q_even's affine limbs.
const P_INTERNAL_WORDS: u32 = 36u;
const P_INTERNAL_X: u32 = 44u;
const P_INTERNAL_Y: u32 = 52u;

const PER_INVOCATION: u32 = 16u;
const WORKGROUP_SIZE: u32 = 64u;

// X and ZZ of Q + t·G in XYZZ coordinates for one locktime (the output key's x is X/ZZ); `valid`
// is false when t >= n.
struct Projective {
    x: F,
    zz: F,
    valid: bool,
}

fn projective(locktime: u32) -> Projective {
    var result: Projective;
    // 1. The leaf: T is pushed as four little-endian bytes at block bytes 3..7.
    var block: array<u32, 16>;
    for (var i = 0u; i < 16u; i++) {
        block[i] = params[P_LEAF_BLOCK + i];
    }
    block[0] |= locktime & 0xffu;
    // WGSL requires parentheses between << and |; naga accepts them without, browsers do not.
    block[1] |= (((locktime >> 8u) & 0xffu) << 24u)
        | (((locktime >> 16u) & 0xffu) << 16u)
        | ((locktime >> 24u) << 8u);
    let leaf = sha256_compress(read_state256(P_LEAF_MID), block);

    // 2. The tweak over Q || leaf, then the padding block for 128 bytes after the tag prefix.
    var first: array<u32, 16>;
    for (var i = 0u; i < 8u; i++) {
        first[i] = params[P_INTERNAL_WORDS + i];
        first[8u + i] = leaf[i];
    }
    var padding: array<u32, 16>;
    padding[0] = 0x80000000u;
    padding[15] = (64u + 64u) * 8u;
    let tweak = digest_to_fe(sha256_compress(sha256_compress(read_state256(P_TWEAK_MID), first), padding));
    if gte(tweak, ORDER_N) {
        return result;
    }

    // 3. output = Q_even + t·G: window 0 with Q already added, then the other windows.
    let window0 = signed_digit(tweak, 0u, Q_BITS, 0u);
    let point = xyzz_add_mul_g(q_point(window0), tweak, window0.carry);
    result.x = point.x;
    result.zz = point.zz;
    result.valid = true;
    return result;
}

fn q_point(digit: Digit) -> AffF {
    let index = select(Q_DIGITS + digit.magnitude, Q_DIGITS - digit.magnitude, digit.negative);
    let base = index * 16u;
    var x: Fe;
    var y: Fe;
    for (var i = 0u; i < 8u; i++) {
        x[i] = q_table[base + i];
        y[i] = q_table[base + 8u + i];
    }
    return AffF(fe_to_f(x), fe_to_f(y), false);
}

// The setup passes take small tables from the host and split each digit d into
// hi·2^SPLIT_BITS + lo: for a window with base B, lo·B for lo in 1..2^SPLIT_BITS, then
// (hi·2^SPLIT_BITS)·B for hi in 1..=2^(width − 1 − SPLIT_BITS). hi·2^SPLIT_BITS = ±lo never
// holds, so adding a pair needs no special cases. They run in chunks, with each chunk's first
// thread index in params[0].
const SPLIT_BITS: u32 = 9u;
const LO_POINTS: u32 = (1u << SPLIT_BITS) - 1u;

fn small_point(offset: u32, index: u32) -> array<u32, 16> {
    var words: array<u32, 16>;
    for (var i = 0u; i < 16u; i++) {
        words[i] = params[offset + index * 16u + i];
    }
    return words;
}

fn words_to_affine(words: array<u32, 16>) -> AffF {
    var x: Fe;
    var y: Fe;
    for (var i = 0u; i < 8u; i++) {
        x[i] = words[i];
        y[i] = words[8u + i];
    }
    return AffF(fe_to_f(x), fe_to_f(y), false);
}

// Setup passes run as two dispatches per chunk, like the search: a `_points` pass makes one
// Jacobian point per thread and writes it packed to debug_out (bound to the search's scratch
// buffer during setup), SETUP_WORDS per point from the chunk's start, then an `_affine` pass makes
// SETUP_PER of them affine per thread with one inversion (Montgomery's trick). Holding the points
// in registers through the inversion instead cost more than the inversions it saved. Each chunk's
// first point and point count are in params[0] and params[1].
const SETUP_PER: u32 = 16u;
// x, y and z, then the product of the earlier points' z.
const SETUP_WORDS: u32 = 40u;

fn store_setup_point(index: u32, p: JacF) {
    let at = index * SETUP_WORDS;
    let x = f_pack(p.x);
    let y = f_pack(p.y);
    let z = f_pack(p.z);
    for (var i = 0u; i < 10u; i++) {
        debug_out[at + i] = x[i];
        debug_out[at + 10u + i] = y[i];
        debug_out[at + 20u + i] = z[i];
    }
}

fn setup_element(at: u32) -> F {
    var packed: Packed;
    for (var i = 0u; i < 10u; i++) {
        packed[i] = debug_out[at + i];
    }
    return f_unpack(packed);
}

// Makes the chunk's points first..first + SETUP_PER (those below its count) affine and writes them
// at their table index, to q_table or to table_out.
fn setup_affine(first: u32, to_q: bool) {
    let count = params[1];
    var product = f_one();
    for (var j = 0u; j < SETUP_PER; j++) {
        if first + j < count {
            let at = (first + j) * SETUP_WORDS;
            let earlier = f_pack(product);
            for (var i = 0u; i < 10u; i++) {
                debug_out[at + 30u + i] = earlier[i];
            }
            product = f_mul(product, setup_element(at + 20u));
        }
    }
    var inverse = f_inv(product);
    for (var j = SETUP_PER - 1u; j < SETUP_PER; j--) {
        if first + j < count {
            let at = (first + j) * SETUP_WORDS;
            let z_inverse = f_mul(inverse, setup_element(at + 30u));
            inverse = f_mul(inverse, setup_element(at + 20u));
            let z_inverse2 = f_sqr(z_inverse);
            let x = f_to_fe(f_canonical(f_mul(setup_element(at), z_inverse2)));
            let y = f_to_fe(f_canonical(f_mul(setup_element(at + 10u), f_mul(z_inverse2, z_inverse))));
            let index = params[0] + first + j;
            for (var i = 0u; i < 8u; i++) {
                if to_q {
                    q_table[index * 16u + i] = x[i];
                    q_table[index * 16u + 8u + i] = y[i];
                } else {
                    table_out[index * 16u + i] = x[i];
                    table_out[index * 16u + 8u + i] = y[i];
                }
            }
        }
    }
}

// Window 0's small tables for G follow the search parameters.
const P_FOLD_POINTS: u32 = 64u;

// The setup pass for each search: q_table[Q_DIGITS + d] = Q + d·G for d in
// −Q_DIGITS..=Q_DIGITS. Q = ±d·G happens with probability about 2^-235; the host re-derives any
// match.
@compute @workgroup_size(64)
fn fold_q_points(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params[1] {
        return;
    }
    let index = params[0] + id.x;
    let q = AffF(fe_to_f(read_fe(P_INTERNAL_X)), fe_to_f(read_fe(P_INTERNAL_Y)), false);
    let negative = index < Q_DIGITS;
    let digit = select(index - Q_DIGITS, Q_DIGITS - index, negative);
    let hi = digit >> SPLIT_BITS;
    let lo = digit & LO_POINTS;
    // The branches only choose which small-table points to load; the point arithmetic appears
    // once. With it repeated in several branches, Apple's compiler produced wrong results. A digit
    // with one term marks the second point as infinity, which the addition skips; digit 0
    // computes Q + G and is replaced by Q in the affine pass.
    let first = select(select(LO_POINTS + hi - 1u, lo - 1u, hi == 0u), 0u, digit == 0u);
    let second = select(0u, lo - 1u, hi != 0u && lo != 0u);
    var low = words_to_affine(small_point(P_FOLD_POINTS, second));
    low.infinity = hi == 0u || lo == 0u;
    var multiple = jac_add_affine(jac_from_affine(words_to_affine(small_point(P_FOLD_POINTS, first))), low);
    let negated = f_neg(multiple.y);
    for (var i = 0u; i < 20u; i++) {
        multiple.y[i] = select(multiple.y[i], negated[i], negative);
    }
    store_setup_point(id.x, jac_add_affine(multiple, q));
}

@compute @workgroup_size(64)
fn fold_q_affine(@builtin(global_invocation_id) id: vec3<u32>) {
    let first = id.x * SETUP_PER;
    if first >= params[1] {
        return;
    }
    setup_affine(first, true);
    // Q itself, straight from the parameters.
    if params[0] + first <= Q_DIGITS && Q_DIGITS < params[0] + first + SETUP_PER {
        for (var i = 0u; i < 8u; i++) {
            q_table[Q_DIGITS * 16u + i] = params[P_INTERNAL_X + i];
            q_table[Q_DIGITS * 16u + 8u + i] = params[P_INTERNAL_Y + i];
        }
    }
}

// The generator table's setup pass.
const SMALL_BASES = array<u32, 13>(0u, 1023u, 2046u, 3069u, 3836u, 4603u, 5370u, 6137u, 6904u, 7671u, 8438u, 9205u, 9972u);

@compute @workgroup_size(64)
fn build_table_points(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params[1] {
        return;
    }
    let entry = params[0] + id.x;
    var window = 0u;
    for (var w = 1u; w < G_WINDOWS; w++) {
        if entry >= G_BASES[w] {
            window = w;
        }
    }
    let digit = entry - G_BASES[window] + 1u;
    let offset = 4u + SMALL_BASES[window] * 16u;
    let hi = digit >> SPLIT_BITS;
    let lo = digit & LO_POINTS;
    // As in fold_q_points: the branches only choose points, and a one-term digit marks the second
    // as infinity.
    let first = select(LO_POINTS + hi - 1u, lo - 1u, hi == 0u);
    let second = select(0u, lo - 1u, hi != 0u && lo != 0u);
    var low = words_to_affine(small_point(offset, second));
    low.infinity = hi == 0u || lo == 0u;
    store_setup_point(id.x, jac_add_affine(jac_from_affine(words_to_affine(small_point(offset, first))), low));
}

@compute @workgroup_size(64)
fn build_table_affine(@builtin(global_invocation_id) id: vec3<u32>) {
    let first = id.x * SETUP_PER;
    if first >= params[1] {
        return;
    }
    setup_affine(first, false);
}

// A field element held in the search's scratch buffer, two limbs (each at most 2^13) per word.
alias Packed = array<u32, 10>;

fn f_pack(a: F) -> Packed {
    var r: Packed;
    for (var i = 0u; i < 10u; i++) {
        r[i] = a[2u * i] | (a[2u * i + 1u] << 16u);
    }
    return r;
}

fn f_unpack(a: Packed) -> F {
    var r: F;
    for (var i = 0u; i < 10u; i++) {
        r[2u * i] = a[i] & 0xffffu;
        r[2u * i + 1u] = a[i] >> 16u;
    }
    return r;
}
