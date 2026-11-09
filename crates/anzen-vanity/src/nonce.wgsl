// Script nonce grinding: one candidate per recovery locktime T, with both device keys fixed.
// The vault is tr(Q, and_v(v:after(T),and_v(v:older(4259839),pk(hww)))), where Q is the MuSig2
// aggregate of the two keys. T is a Unix time long past, so its lock is always satisfied; it only
// varies the address.
//   1. The leaf hash: TaggedHash("TapLeaf", 0xc0 || 47 || script), where only T's four bytes vary.
//   2. The Taproot tweak t = TaggedHash("TapTweak", Q || leaf), the leaf being the only one.
//   3. output = Q_even + t·G.

// The leaf's message block with T's bytes zero: 0xc0, 47, then the script
// <T> OP_CLTV OP_VERIFY <4259839> OP_CSV OP_VERIFY <hww> OP_CHECKSIG, then SHA-256 padding.
const P_LEAF_BLOCK: u32 = 4u;
const P_LEAF_MID: u32 = 20u;
const P_TWEAK_MID: u32 = 28u;
// Q's x coordinate as big-endian words for hashing, then Q_even's affine limbs.
const P_INTERNAL_WORDS: u32 = 36u;
const P_INTERNAL_X: u32 = 44u;
const P_INTERNAL_Y: u32 = 52u;

const PER_INVOCATION: u32 = 8u;
const WORKGROUP_SIZE: u32 = 64u;

// Q + t·G in Jacobian coordinates for one locktime; `valid` is false when t >= n.
struct Projective {
    point: JacF,
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
    result.point = jac_add_mul_g(jac_from_affine(q_point(window0)), tweak, window0.carry);
    result.valid = !result.point.infinity;
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


// The affine x and y limbs of a Jacobian point, with its own inversion.
fn affine_words(point: JacF) -> array<u32, 16> {
    let z_inverse = f_inv(point.z);
    let z_inverse2 = f_sqr(z_inverse);
    let x = f_to_fe(f_canonical(f_mul(point.x, z_inverse2)));
    let y = f_to_fe(f_canonical(f_mul(point.y, f_mul(z_inverse2, z_inverse))));
    var words: array<u32, 16>;
    for (var i = 0u; i < 8u; i++) {
        words[i] = x[i];
        words[8u + i] = y[i];
    }
    return words;
}

// Window 0's small tables for G follow the search parameters.
const P_FOLD_POINTS: u32 = 64u;

// The setup pass for each search: q_table[Q_DIGITS + d] = Q + d·G for d in
// −Q_DIGITS..=Q_DIGITS, each made affine with its own inversion. Q = ±d·G happens with
// probability about 2^-235; the host re-derives any match.
@compute @workgroup_size(64)
fn fold_q(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = params[0] + id.x;
    if index > 2u * Q_DIGITS {
        return;
    }
    let q = AffF(fe_to_f(read_fe(P_INTERNAL_X)), fe_to_f(read_fe(P_INTERNAL_Y)), false);
    let negative = index < Q_DIGITS;
    let digit = select(index - Q_DIGITS, Q_DIGITS - index, negative);
    let hi = digit >> SPLIT_BITS;
    let lo = digit & LO_POINTS;
    // The branches only choose which small-table points to load; the point arithmetic appears
    // once. With it repeated in several branches, Apple's compiler produced wrong results. A
    // digit with one term marks the second point as infinity, which the addition skips.
    let first = select(LO_POINTS + hi - 1u, lo - 1u, hi == 0u);
    let second = select(0u, lo - 1u, hi != 0u && lo != 0u);
    var low = words_to_affine(small_point(P_FOLD_POINTS, second));
    low.infinity = hi == 0u || lo == 0u;
    var multiple = jac_add_affine(jac_from_affine(words_to_affine(small_point(P_FOLD_POINTS, first))), low);
    let negated = f_neg(multiple.y);
    for (var i = 0u; i < 20u; i++) {
        multiple.y[i] = select(multiple.y[i], negated[i], negative);
    }
    var words = affine_words(jac_add_affine(multiple, q));
    if digit == 0u {
        // Q itself, straight from the parameters.
        for (var i = 0u; i < 8u; i++) {
            words[i] = params[P_INTERNAL_X + i];
            words[8u + i] = params[P_INTERNAL_Y + i];
        }
    }
    for (var i = 0u; i < 16u; i++) {
        q_table[index * 16u + i] = words[i];
    }
}


// (hi·2^SPLIT_BITS)·B + lo·B from a window's small tables at `start`; hi and lo both nonzero.
// Callers handle a zero hi or lo themselves: a version branching on them inside, assigning the
// point on each path, came out of Apple's compiler computing wrong sums.
fn split_sum(start: u32, hi: u32, lo: u32) -> JacF {
    let low = words_to_affine(small_point(start, lo - 1u));
    let high = words_to_affine(small_point(start, LO_POINTS + hi - 1u));
    return jac_add_affine(jac_from_affine(high), low);
}

// The generator table's setup pass: one entry per thread.
const SMALL_BASES = array<u32, 13>(0u, 1023u, 2046u, 3069u, 3836u, 4603u, 5370u, 6137u, 6904u, 7671u, 8438u, 9205u, 9972u);

@compute @workgroup_size(64)
fn build_table(@builtin(global_invocation_id) id: vec3<u32>) {
    let entry = params[0] + id.x;
    if entry >= G_ENTRIES {
        return;
    }
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
    var words: array<u32, 16>;
    if hi == 0u {
        words = small_point(offset, lo - 1u);
    } else if lo == 0u {
        words = small_point(offset, LO_POINTS + hi - 1u);
    } else {
        words = affine_words(split_sum(offset, hi, lo));
    }
    for (var i = 0u; i < 16u; i++) {
        table_out[entry * 16u + i] = words[i];
    }
}

// The batch's output keys with one field inversion (Montgomery's trick): invert the product of
// every Z, then peel each Z's inverse off it. Invalid candidates use Z = 1.
fn candidates(first: u32) -> array<Candidate, PER_INVOCATION> {
    var points: array<Projective, PER_INVOCATION>;
    var products: array<F, PER_INVOCATION>;
    let one = f_one();
    for (var j = 0u; j < PER_INVOCATION; j++) {
        points[j] = projective(first + j);
        if !points[j].valid {
            points[j].point.z = one;
        }
        if j == 0u {
            products[j] = points[j].point.z;
        } else {
            products[j] = f_mul(products[j - 1u], points[j].point.z);
        }
    }
    var inverse = f_inv(products[PER_INVOCATION - 1u]);
    var batch: array<Candidate, PER_INVOCATION>;
    for (var j = PER_INVOCATION - 1u; j < PER_INVOCATION; j--) {
        var z_inverse = inverse;
        if j > 0u {
            z_inverse = f_mul(inverse, products[j - 1u]);
            inverse = f_mul(inverse, points[j].point.z);
        }
        batch[j].valid = points[j].valid;
        batch[j].output_x = f_to_fe(f_canonical(f_mul(points[j].point.x, f_sqr(z_inverse))));
    }
    return batch;
}
