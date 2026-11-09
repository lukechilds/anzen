// Script nonce grinding in Metal Shading Language: the same search as nonce.wgsl, written for
// Apple GPUs. Point arithmetic uses the 13-bit-limb field from field13.metal, like the wgpu
// kernel: native 64-bit products were 1.8× slower, since Apple GPUs emulate 64-bit integers.
//
// One thread evaluates PER_INVOCATION consecutive recovery locktimes T and shares one field
// inversion across them. For each T: the leaf hash, the Taproot tweak t, then Q_even + t·G from
// a table of 16-bit windows, comparing the output key's top bits with the target.

#define PER_INVOCATION 8

// Parameter offsets: the same block as the wgpu nonce kernel.
#define P_LEAF_BLOCK 4
#define P_LEAF_MID 20
#define P_TWEAK_MID 28
#define P_INTERNAL_WORDS 36
#define P_INTERNAL_X 44
#define P_INTERNAL_Y 52

// ---------------------------------------------------------------------------------------------
// The candidate

// Q + t·G in Jacobian coordinates for one locktime; `valid` is false when t >= n.
inline JacF projective(uint locktime, device const uint *params, device const uint *g_table, thread bool &valid) {
    // 1. The leaf: T is pushed as four little-endian bytes at block bytes 3..7.
    uint block[16];
    #pragma unroll
    for (int i = 0; i < 16; i++) block[i] = params[P_LEAF_BLOCK + i];
    block[0] |= locktime & 0xffu;
    block[1] |= ((locktime >> 8) & 0xffu) << 24 | ((locktime >> 16) & 0xffu) << 16 | (locktime >> 24) << 8;
    uint leaf[8];
    #pragma unroll
    for (int i = 0; i < 8; i++) leaf[i] = params[P_LEAF_MID + i];
    sha256_compress(leaf, block);

    // 2. The tweak over Q || leaf, then the padding block for 128 bytes after the tag prefix.
    uint tweak_state[8];
    #pragma unroll
    for (int i = 0; i < 8; i++) {
        tweak_state[i] = params[P_TWEAK_MID + i];
        block[i] = params[P_INTERNAL_WORDS + i];
        block[8 + i] = leaf[i];
    }
    sha256_compress(tweak_state, block);
    #pragma unroll
    for (int i = 0; i < 16; i++) block[i] = 0;
    block[0] = 0x80000000u;
    block[15] = (64 + 64) * 8;
    sha256_compress(tweak_state, block);
    Fe tweak;
    #pragma unroll
    for (int i = 0; i < 8; i++) tweak.v[7 - i] = tweak_state[i];
    valid = !gte_const(tweak, ORDER_N);

    // 3. Q_even + t·G, in the 13-bit field.
    AffF internal;
    internal.x = fe_to_f(read_fe(params, P_INTERNAL_X));
    internal.y = fe_to_f(read_fe(params, P_INTERNAL_Y));
    internal.infinity = false;
    JacF p = jac_add_mul_g(jac_from_affine(internal), tweak, g_table);
    valid = valid && !p.infinity;
    return p;
}

// The batch's output keys with one field inversion (Montgomery's trick). Invalid candidates use
// Z = 1. Writes each output key's x, or marks it invalid.
inline void candidates(uint first, device const uint *params, device const uint *g_table, thread Fe *xs, thread bool *valid) {
    JacF points[PER_INVOCATION];
    F products[PER_INVOCATION];
    for (int j = 0; j < PER_INVOCATION; j++) {
        bool ok;
        points[j] = projective(first + j, params, g_table, ok);
        valid[j] = ok;
        if (!ok) points[j].z = f_one();
        products[j] = j == 0 ? points[j].z : f_mul(products[j - 1], points[j].z);
    }
    F inverse = f_inv(products[PER_INVOCATION - 1]);
    for (int j = PER_INVOCATION - 1; j >= 0; j--) {
        F z_inverse = inverse;
        if (j > 0) {
            z_inverse = f_mul(inverse, products[j - 1]);
            inverse = f_mul(inverse, points[j].z);
        }
        xs[j] = f_to_fe(f_canonical(f_mul(points[j].x, f_sqr(z_inverse))));
    }
}

kernel void search(device const uint *params [[buffer(0)]],
                   device const uint *g_table [[buffer(1)]],
                   device atomic_uint *found [[buffer(2)]],
                   uint id [[thread_position_in_grid]]) {
    uint first = id * PER_INVOCATION;
    uint count = params[P_COUNT];
    if (first >= count) return;
    Fe xs[PER_INVOCATION];
    bool valid[PER_INVOCATION];
    candidates(params[P_START] + first, params, g_table, xs, valid);
    uint bits = params[P_TARGET_BITS];
    for (int j = 0; j < PER_INVOCATION; j++) {
        if (first + j < count && valid[j] && (xs[j].v[7] >> (32 - bits)) == params[P_TARGET_VALUE]) {
            atomic_fetch_min_explicit(found, params[P_START] + first + j, memory_order_relaxed);
        }
    }
}

kernel void debug(device const uint *params [[buffer(0)]],
                  device const uint *g_table [[buffer(1)]],
                  device uint *debug_out [[buffer(2)]],
                  uint id [[thread_position_in_grid]]) {
    uint first = id * PER_INVOCATION;
    uint count = params[P_COUNT];
    if (first >= count) return;
    Fe xs[PER_INVOCATION];
    bool valid[PER_INVOCATION];
    candidates(params[P_START] + first, params, g_table, xs, valid);
    for (int j = 0; j < PER_INVOCATION; j++) {
        if (first + j < count) {
            uint at = (first + j) * 9;
            debug_out[at] = valid[j] ? 1 : 0;
            for (int i = 0; i < 8; i++) debug_out[at + 1 + i] = xs[j].v[i];
        }
    }
}
