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
    block[1] |= ((locktime >> 8u) & 0xffu) << 24u
        | ((locktime >> 16u) & 0xffu) << 16u
        | (locktime >> 24u) << 8u;
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

    // 3. output = Q_even + t·G.
    var internal: AffF;
    internal.x = fe_to_f(read_fe(P_INTERNAL_X));
    internal.y = fe_to_f(read_fe(P_INTERNAL_Y));
    result.point = jac_add_mul_g(jac_from_affine(internal), tweak);
    result.valid = !result.point.infinity;
    return result;
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
