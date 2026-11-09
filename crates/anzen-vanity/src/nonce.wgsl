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
    let window0 = signed_digit(tweak, 0u, 0u);
    result.point = jac_add_mul_g(jac_from_affine(q_point(window0)), tweak, window0.carry);
    result.valid = !result.point.infinity;
    return result;
}

fn q_point(digit: Digit) -> AffF {
    let index = select(G_DIGITS + digit.magnitude, G_DIGITS - digit.magnitude, digit.negative);
    let base = index * 16u;
    var x: Fe;
    var y: Fe;
    for (var i = 0u; i < 8u; i++) {
        x[i] = q_table[base + i];
        y[i] = q_table[base + 8u + i];
    }
    return AffF(fe_to_f(x), fe_to_f(y), false);
}

// The setup pass: q_table[G_DIGITS + d] = Q + d·G for d in −G_DIGITS..=G_DIGITS, each made
// affine with its own inversion. Q = ±d·G happens with probability about 2^-237; the host
// re-derives any match.
@compute @workgroup_size(64)
fn fold_q(@builtin(global_invocation_id) id: vec3<u32>) {
    // The host runs this in chunks and passes each chunk's first index in params[0].
    let index = params[0] + id.x;
    if index > 2u * G_DIGITS {
        return;
    }
    var x = read_fe(P_INTERNAL_X);
    var y = read_fe(P_INTERNAL_Y);
    if index != G_DIGITS {
        let negative = index < G_DIGITS;
        let digit = Digit(select(index - G_DIGITS, G_DIGITS - index, negative), negative, 0u);
        let q = AffF(fe_to_f(x), fe_to_f(y), false);
        let sum = jac_add_affine(jac_from_affine(q), g_point_signed(0u, digit));
        let z_inverse = f_inv(sum.z);
        let z_inverse2 = f_sqr(z_inverse);
        x = f_to_fe(f_canonical(f_mul(sum.x, z_inverse2)));
        y = f_to_fe(f_canonical(f_mul(sum.y, f_mul(z_inverse2, z_inverse))));
    }
    for (var i = 0u; i < 8u; i++) {
        q_table[index * 16u + i] = x[i];
        q_table[index * 16u + 8u + i] = y[i];
    }
}

// The generator table's setup pass. The host sends, per window, lo·B for lo in 1..512 then
// (hi·512)·B for hi in 1..=G_DIGITS/512, where B = 2^(G_WINDOW_BITS·w)·G; each thread writes one
// entry d·B = (hi·512)·B + lo·B with one addition and one inversion. hi·512 = ±lo never holds,
// so the addition needs no special cases. The host runs it in chunks, passing each chunk's first
// entry in params[0].
const SPLIT_BITS: u32 = 9u;
const LO_POINTS: u32 = (1u << SPLIT_BITS) - 1u;
const HI_POINTS: u32 = G_DIGITS >> SPLIT_BITS;

fn small_point(index: u32) -> array<u32, 16> {
    var words: array<u32, 16>;
    for (var i = 0u; i < 16u; i++) {
        words[i] = params[4u + index * 16u + i];
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

@compute @workgroup_size(64)
fn build_table(@builtin(global_invocation_id) id: vec3<u32>) {
    let entry = params[0] + id.x;
    if entry >= G_WINDOWS * G_DIGITS {
        return;
    }
    let window = (entry / G_DIGITS) * (LO_POINTS + HI_POINTS);
    let digit = entry % G_DIGITS + 1u;
    let hi = digit >> SPLIT_BITS;
    let lo = digit & LO_POINTS;
    var words: array<u32, 16>;
    if hi == 0u {
        words = small_point(window + lo - 1u);
    } else if lo == 0u {
        words = small_point(window + LO_POINTS + hi - 1u);
    } else {
        let low = words_to_affine(small_point(window + lo - 1u));
        let high = words_to_affine(small_point(window + LO_POINTS + hi - 1u));
        let sum = jac_add_affine(jac_from_affine(high), low);
        let z_inverse = f_inv(sum.z);
        let z_inverse2 = f_sqr(z_inverse);
        let x = f_to_fe(f_canonical(f_mul(sum.x, z_inverse2)));
        let y = f_to_fe(f_canonical(f_mul(sum.y, f_mul(z_inverse2, z_inverse))));
        for (var i = 0u; i < 8u; i++) {
            words[i] = x[i];
            words[8u + i] = y[i];
        }
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
