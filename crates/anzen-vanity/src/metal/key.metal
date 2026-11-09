// Phone vault key grinding in Metal Shading Language: the same search as key.wgsl. Per candidate
// index: the BIP32 child of the vault xpub (HMAC-SHA512 and parent + I_L·G), BIP327 key
// aggregation with the signer key, and the BIP341 tweak. Each stage runs for the thread's batch
// and then shares one inversion to reach affine form. Large stages are `noinline`: one fully
// inlined kernel this size stalls Apple's pipeline compiler.

#define PER_INVOCATION 8
#define THREADS_PER_GROUP 64

// Parameter offsets: the same block as the wgpu key kernel.
#define P_INNER 4
#define P_OUTER 20
#define P_MESSAGE 36
#define P_HWW_X 68
#define P_HWW_Y 76
#define P_LIST_MID 84
#define P_COEF_MID 92
#define P_TWEAK_MID 100
#define P_ROOT 108
#define P_PARENT_X 116
#define P_PARENT_Y 124

// The signer's and the parent's tables in one buffer, 8-bit windows.
#define KEY_WINDOW_BITS 16
#define KEY_DIGITS ((1u << KEY_WINDOW_BITS) - 1u)
#define KEY_WINDOWS ((256 + KEY_WINDOW_BITS - 1) / KEY_WINDOW_BITS)
#define SIGNER_TABLE 0u
#define PARENT_TABLE (KEY_WINDOWS * KEY_DIGITS * 16u)

constant uint ORDER_N_COMPLEMENT[5] = {0x2FC9BEBFu, 0x402DA173u, 0x50B75FC4u, 0x45512319u, 1u};

constant ulong K512[80] = {
    0x428a2f98d728ae22ul, 0x7137449123ef65cdul, 0xb5c0fbcfec4d3b2ful, 0xe9b5dba58189dbbcul,
    0x3956c25bf348b538ul, 0x59f111f1b605d019ul, 0x923f82a4af194f9bul, 0xab1c5ed5da6d8118ul,
    0xd807aa98a3030242ul, 0x12835b0145706fbeul, 0x243185be4ee4b28cul, 0x550c7dc3d5ffb4e2ul,
    0x72be5d74f27b896ful, 0x80deb1fe3b1696b1ul, 0x9bdc06a725c71235ul, 0xc19bf174cf692694ul,
    0xe49b69c19ef14ad2ul, 0xefbe4786384f25e3ul, 0x0fc19dc68b8cd5b5ul, 0x240ca1cc77ac9c65ul,
    0x2de92c6f592b0275ul, 0x4a7484aa6ea6e483ul, 0x5cb0a9dcbd41fbd4ul, 0x76f988da831153b5ul,
    0x983e5152ee66dfabul, 0xa831c66d2db43210ul, 0xb00327c898fb213ful, 0xbf597fc7beef0ee4ul,
    0xc6e00bf33da88fc2ul, 0xd5a79147930aa725ul, 0x06ca6351e003826ful, 0x142929670a0e6e70ul,
    0x27b70a8546d22ffcul, 0x2e1b21385c26c926ul, 0x4d2c6dfc5ac42aedul, 0x53380d139d95b3dful,
    0x650a73548baf63deul, 0x766a0abb3c77b2a8ul, 0x81c2c92e47edaee6ul, 0x92722c851482353bul,
    0xa2bfe8a14cf10364ul, 0xa81a664bbc423001ul, 0xc24b8b70d0f89791ul, 0xc76c51a30654be30ul,
    0xd192e819d6ef5218ul, 0xd69906245565a910ul, 0xf40e35855771202aul, 0x106aa07032bbd1b8ul,
    0x19a4c116b8d2d0c8ul, 0x1e376c085141ab53ul, 0x2748774cdf8eeb99ul, 0x34b0bcb5e19b48a8ul,
    0x391c0cb3c5c95a63ul, 0x4ed8aa4ae3418acbul, 0x5b9cca4f7763e373ul, 0x682e6ff3d6b2b8a3ul,
    0x748f82ee5defb2fcul, 0x78a5636f43172f60ul, 0x84c87814a1f0ab72ul, 0x8cc702081a6439ecul,
    0x90befffa23631e28ul, 0xa4506cebde82bde9ul, 0xbef9a3f7b2c67915ul, 0xc67178f2e372532bul,
    0xca273eceea26619cul, 0xd186b8c721c0c207ul, 0xeada7dd6cde0eb1eul, 0xf57d4f7fee6ed178ul,
    0x06f067aa72176fbaul, 0x0a637dc5a2c898a6ul, 0x113f9804bef90daeul, 0x1b710b35131c471bul,
    0x28db77f523047d84ul, 0x32caab7b40c72493ul, 0x3c9ebe0a15c9bebcul, 0x431d67c49c100d4cul,
    0x4cc5d4becb3e42b6ul, 0x597f299cfc657e2aul, 0x5fcb6fab3ad6faecul, 0x6c44198c4a475817ul,
};

struct Affine {
    Fe x;
    Fe y;
    bool infinity;
};

// ---------------------------------------------------------------------------------------------
// Scalars modulo n

// low + high·(2^256 - n), repeated until the value fits 256 bits, then one subtraction.
inline Fe scalar_mul(Fe a, Fe b) {
    uint t[16];
    #pragma unroll
    for (int i = 0; i < 16; i++) t[i] = 0;
    #pragma unroll
    for (int i = 0; i < 8; i++) {
        ulong carry = 0;
        #pragma unroll
        for (int j = 0; j < 8; j++) {
            carry += (ulong)t[i + j] + (ulong)a.v[i] * b.v[j];
            t[i + j] = (uint)carry;
            carry >>= 32;
        }
        t[i + 8] = (uint)carry;
    }
    for (int round = 0; round < 4; round++) {
        uint r[16];
        #pragma unroll
        for (int i = 0; i < 16; i++) r[i] = i < 8 ? t[i] : 0;
        #pragma unroll
        for (int i = 0; i < 8; i++) {
            ulong carry = 0;
            #pragma unroll
            for (int j = 0; j < 5; j++) {
                carry += (ulong)r[i + j] + (ulong)t[8 + i] * ORDER_N_COMPLEMENT[j];
                r[i + j] = (uint)carry;
                carry >>= 32;
            }
            #pragma unroll
            for (int k = i + 5; k < 16; k++) {
                carry += r[k];
                r[k] = (uint)carry;
                carry >>= 32;
            }
        }
        #pragma unroll
        for (int i = 0; i < 16; i++) t[i] = r[i];
    }
    Fe out;
    #pragma unroll
    for (int i = 0; i < 8; i++) out.v[i] = t[i];
    if (gte_const(out, ORDER_N)) out = sub_const(out, ORDER_N);
    return out;
}

inline Fe scalar_neg(Fe a) {
    if (fe_is_zero(a)) return a;
    Fe r;
    long borrow = 0;
    #pragma unroll
    for (int i = 0; i < 8; i++) {
        long d = (long)ORDER_N[i] - (long)a.v[i] + borrow;
        r.v[i] = (uint)d;
        borrow = d >> 32;
    }
    return r;
}

inline Fe fe_neg(Fe a) {
    if (fe_is_zero(a)) return a;
    Fe r;
    long borrow = 0;
    #pragma unroll
    for (int i = 0; i < 8; i++) {
        long d = (long)FIELD_P[i] - (long)a.v[i] + borrow;
        r.v[i] = (uint)d;
        borrow = d >> 32;
    }
    return r;
}

inline bool fe_gte(Fe a, Fe b) {
    bool result = true;
    bool decided = false;
    #pragma unroll
    for (int i = 7; i >= 0; i--) {
        if (!decided && a.v[i] != b.v[i]) {
            result = a.v[i] > b.v[i];
            decided = true;
        }
    }
    return result;
}

inline bool fe_equal(Fe a, Fe b) {
    uint bits = 0;
    #pragma unroll
    for (int i = 0; i < 8; i++) bits |= a.v[i] ^ b.v[i];
    return bits == 0;
}

// ---------------------------------------------------------------------------------------------
// Hashing

inline ulong rotr64(ulong x, uint n) {
    return (x >> n) | (x << (64 - n));
}

__attribute__((noinline)) void sha512_compress(thread ulong *state, thread const ulong *block) {
    ulong w[80];
    for (int i = 0; i < 16; i++) w[i] = block[i];
    for (int i = 16; i < 80; i++) {
        ulong s0 = rotr64(w[i - 15], 1) ^ rotr64(w[i - 15], 8) ^ (w[i - 15] >> 7);
        ulong s1 = rotr64(w[i - 2], 19) ^ rotr64(w[i - 2], 61) ^ (w[i - 2] >> 6);
        w[i] = w[i - 16] + s0 + w[i - 7] + s1;
    }
    ulong a = state[0], b = state[1], c = state[2], d = state[3];
    ulong e = state[4], f = state[5], g = state[6], h = state[7];
    for (int i = 0; i < 80; i++) {
        ulong t1 = h + (rotr64(e, 14) ^ rotr64(e, 18) ^ rotr64(e, 41)) + ((e & f) ^ (~e & g)) + K512[i] + w[i];
        ulong t2 = (rotr64(a, 28) ^ rotr64(a, 34) ^ rotr64(a, 39)) + ((a & b) ^ (a & c) ^ (b & c));
        h = g;
        g = f;
        f = e;
        e = d + t1;
        d = c;
        c = b;
        b = a;
        a = t1 + t2;
    }
    state[0] += a;
    state[1] += b;
    state[2] += c;
    state[3] += d;
    state[4] += e;
    state[5] += f;
    state[6] += g;
    state[7] += h;
}

// SHA-256 of `prefix || data` (data: `length` bytes, at most 119) from a tagged midstate.
__attribute__((noinline)) void sha256_tagged(device const uint *params, int midstate, thread const uchar *data, uint length, thread uint *out) {
    #pragma unroll
    for (int i = 0; i < 8; i++) out[i] = params[midstate + i];
    uint blocks = length + 9 <= 64 ? 1 : 2;
    for (uint block_index = 0; block_index < blocks; block_index++) {
        uint block[16];
        for (uint word = 0; word < 16; word++) {
            uint value = 0;
            for (uint byte = 0; byte < 4; byte++) {
                uint position = block_index * 64 + word * 4 + byte;
                uint b = position < length ? data[position] : (position == length ? 0x80u : 0u);
                value = (value << 8) | b;
            }
            block[word] = value;
        }
        if (block_index == blocks - 1) block[15] = (64 + length) * 8;
        sha256_compress(out, block);
    }
}

inline void put_x(thread uchar *data, Fe x) {
    for (int i = 0; i < 32; i++) data[i] = (x.v[7 - i / 4] >> (24 - 8 * (i % 4))) & 0xffu;
}

inline Fe digest_to_fe(thread const uint *digest) {
    Fe r;
    #pragma unroll
    for (int i = 0; i < 8; i++) r.v[7 - i] = digest[i];
    return r;
}

// I_L of HMAC-SHA512(chain code, serP(parent) || index): the BIP32 child tweak.
__attribute__((noinline)) Fe bip32_tweak(uint index, device const uint *params) {
    ulong block[16];
    for (int i = 0; i < 16; i++) block[i] = ((ulong)params[P_MESSAGE + 2 * i] << 32) | params[P_MESSAGE + 2 * i + 1];
    // The big-endian index is bytes 33..36, inside 64-bit word 4.
    block[4] |= ((ulong)index << 24);
    ulong state[8];
    for (int i = 0; i < 8; i++) state[i] = ((ulong)params[P_INNER + 2 * i] << 32) | params[P_INNER + 2 * i + 1];
    sha512_compress(state, block);
    ulong outer[16];
    for (int i = 0; i < 8; i++) outer[i] = state[i];
    outer[8] = 0x8000000000000000ul;
    for (int i = 9; i < 15; i++) outer[i] = 0;
    outer[15] = (128 + 64) * 8;
    for (int i = 0; i < 8; i++) state[i] = ((ulong)params[P_OUTER + 2 * i] << 32) | params[P_OUTER + 2 * i + 1];
    sha512_compress(state, outer);
    Fe k;
    for (int i = 0; i < 4; i++) {
        k.v[7 - 2 * i] = (uint)(state[i] >> 32);
        k.v[6 - 2 * i] = (uint)state[i];
    }
    return k;
}

// ---------------------------------------------------------------------------------------------
// Points and stages

inline Affine key_point(device const uint *tables, uint table, uint window, uint digit) {
    uint base = table + (window * KEY_DIGITS + digit - 1) * 16;
    Affine q;
    #pragma unroll
    for (int i = 0; i < 8; i++) {
        q.x.v[i] = tables[base + i];
        q.y.v[i] = tables[base + 8 + i];
    }
    q.infinity = false;
    return q;
}

__attribute__((noinline)) Jacobian add_mul_key(Jacobian acc, device const uint *tables, uint table, Fe k) {
    for (uint w = 0; w < KEY_WINDOWS; w++) {
        uint bit = w * KEY_WINDOW_BITS;
        uint limb = bit / 32;
        uint shift = bit % 32;
        uint digit = k.v[limb] >> shift;
        if (shift + KEY_WINDOW_BITS > 32 && limb < 7) digit |= k.v[limb + 1] << (32 - shift);
        digit &= KEY_DIGITS;
        if (digit != 0) {
            Affine q = key_point(tables, table, w, digit);
            acc = point_add_affine(acc, q.x, q.y);
        }
    }
    return acc;
}

__attribute__((noinline)) Jacobian add_mul_g_call(Jacobian acc, Fe k, device const uint *g_table) {
    return add_mul_g(acc, k, g_table);
}

inline Jacobian jacobian_from(Affine a) {
    Jacobian j;
    j.x = a.x;
    j.y = a.y;
    j.z = Fe{{1, 0, 0, 0, 0, 0, 0, 0}};
    j.infinity = a.infinity;
    return j;
}

// Stage 2: KeySort and KeyAgg of the child with the signer key, Q = a·first + second.
__attribute__((noinline)) Jacobian aggregate_key(Affine child, Fe tweak, device const uint *params, device const uint *g_table, device const uint *tables, thread bool &valid) {
    Affine phone = child;
    bool child_odd = (child.y.v[0] & 1u) != 0;
    if (child_odd) phone.y = fe_neg(phone.y);
    Affine signer;
    signer.x = read_fe(params, P_HWW_X);
    signer.y = read_fe(params, P_HWW_Y);
    signer.infinity = false;
    if (fe_equal(phone.x, signer.x)) valid = false;
    bool phone_first = fe_gte(signer.x, phone.x);
    Affine first = phone_first ? phone : signer;
    Affine second = phone_first ? signer : phone;
    uchar data[66];
    data[0] = 0x02;
    put_x(data + 1, first.x);
    data[33] = 0x02;
    put_x(data + 34, second.x);
    uint list[8];
    sha256_tagged(params, P_LIST_MID, data, 66, list);
    for (int i = 0; i < 8; i++) {
        for (int b = 0; b < 4; b++) data[4 * i + b] = (list[i] >> (24 - 8 * b)) & 0xffu;
    }
    data[32] = 0x02;
    put_x(data + 33, first.x);
    uint digest[8];
    sha256_tagged(params, P_COEF_MID, data, 65, digest);
    Fe coefficient = digest_to_fe(digest);
    if (gte_const(coefficient, ORDER_N)) coefficient = sub_const(coefficient, ORDER_N);
    // coefficient·first from precomputed tables: the phone key is ±(parent + tweak·G), so its
    // multiple is s·parent + (s·tweak)·G with s = ±coefficient.
    uint table = SIGNER_TABLE;
    Fe scalar = coefficient;
    Fe g_scalar = Fe{{0, 0, 0, 0, 0, 0, 0, 0}};
    if (phone_first) {
        table = PARENT_TABLE;
        if (child_odd) scalar = scalar_neg(scalar);
        g_scalar = scalar_mul(scalar, tweak);
    }
    Jacobian scaled;
    scaled.infinity = true;
    scaled = add_mul_key(scaled, tables, table, scalar);
    scaled = add_mul_g_call(scaled, g_scalar, g_table);
    return point_add_affine(scaled, second.x, second.y);
}

// Stage 3: the Taproot tweak over the x-only internal key and the signer's leaf.
__attribute__((noinline)) Jacobian output_key(Affine aggregate, device const uint *params, device const uint *g_table, thread bool &valid) {
    uchar data[64];
    put_x(data, aggregate.x);
    for (int i = 0; i < 8; i++) {
        for (int b = 0; b < 4; b++) data[32 + 4 * i + b] = (params[P_ROOT + i] >> (24 - 8 * b)) & 0xffu;
    }
    uint digest[8];
    sha256_tagged(params, P_TWEAK_MID, data, 64, digest);
    Fe taproot = digest_to_fe(digest);
    if (gte_const(taproot, ORDER_N)) valid = false;
    Affine internal = aggregate;
    if ((internal.y.v[0] & 1u) != 0) internal.y = fe_neg(internal.y);
    return add_mul_g_call(jacobian_from(internal), taproot, g_table);
}

// One inversion takes the batch to affine (Montgomery's trick); invalid points come back invalid.
__attribute__((noinline)) void batch_affine(thread Jacobian *points, thread bool *valid, thread Affine *out) {
    Fe products[PER_INVOCATION];
    Fe zs[PER_INVOCATION];
    const Fe one = Fe{{1, 0, 0, 0, 0, 0, 0, 0}};
    for (int j = 0; j < PER_INVOCATION; j++) {
        valid[j] = valid[j] && !points[j].infinity;
        zs[j] = valid[j] ? points[j].z : one;
        products[j] = j == 0 ? zs[j] : fe_mul(products[j - 1], zs[j]);
    }
    Fe inverse = fe_inv(products[PER_INVOCATION - 1]);
    for (int j = PER_INVOCATION - 1; j >= 0; j--) {
        Fe z_inverse = inverse;
        if (j > 0) {
            z_inverse = fe_mul(inverse, products[j - 1]);
            inverse = fe_mul(inverse, zs[j]);
        }
        Fe zi2 = fe_sqr(z_inverse);
        out[j].x = fe_mul(points[j].x, zi2);
        out[j].y = fe_mul(points[j].y, fe_mul(zi2, z_inverse));
        out[j].infinity = !valid[j];
    }
}

inline void candidates(uint first, device const uint *params, device const uint *g_table, device const uint *tables, thread Fe *xs, thread bool *valid) {
    Fe tweaks[PER_INVOCATION];
    Jacobian points[PER_INVOCATION];
    Affine affine[PER_INVOCATION];
    Affine parent;
    parent.x = read_fe(params, P_PARENT_X);
    parent.y = read_fe(params, P_PARENT_Y);
    parent.infinity = false;
    for (int j = 0; j < PER_INVOCATION; j++) {
        tweaks[j] = bip32_tweak(first + j, params);
        valid[j] = !gte_const(tweaks[j], ORDER_N) && !fe_is_zero(tweaks[j]);
        points[j] = add_mul_g_call(jacobian_from(parent), tweaks[j], g_table);
    }
    batch_affine(points, valid, affine);
    for (int j = 0; j < PER_INVOCATION; j++) {
        bool ok = valid[j];
        points[j] = aggregate_key(affine[j], tweaks[j], params, g_table, tables, ok);
        valid[j] = ok;
    }
    batch_affine(points, valid, affine);
    for (int j = 0; j < PER_INVOCATION; j++) {
        bool ok = valid[j];
        points[j] = output_key(affine[j], params, g_table, ok);
        valid[j] = ok;
    }
    batch_affine(points, valid, affine);
    for (int j = 0; j < PER_INVOCATION; j++) xs[j] = affine[j].x;
}

kernel void search(device const uint *params [[buffer(0)]],
                   device const uint *g_table [[buffer(1)]],
                   device atomic_uint *found [[buffer(2)]],
                   device const uint *tables [[buffer(3)]],
                   uint id [[thread_position_in_grid]]) {
    uint first = id * PER_INVOCATION;
    uint count = params[P_COUNT];
    if (first >= count) return;
    Fe xs[PER_INVOCATION];
    bool valid[PER_INVOCATION];
    candidates(params[P_START] + first, params, g_table, tables, xs, valid);
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
                  device const uint *tables [[buffer(3)]],
                  uint id [[thread_position_in_grid]]) {
    uint first = id * PER_INVOCATION;
    uint count = params[P_COUNT];
    if (first >= count) return;
    Fe xs[PER_INVOCATION];
    bool valid[PER_INVOCATION];
    candidates(params[P_START] + first, params, g_table, tables, xs, valid);
    for (int j = 0; j < PER_INVOCATION; j++) {
        if (first + j < count) {
            uint at = (first + j) * 9;
            debug_out[at] = valid[j] ? 1 : 0;
            for (int i = 0; i < 8; i++) debug_out[at + 1 + i] = xs[j].v[i];
        }
    }
}
