// Shared code for the Metal vanity kernels: 256-bit field arithmetic on 8 little-endian 32-bit
// limbs with 64-bit products, secp256k1 points, the G table and SHA-256. The host prepends this
// to each kernel's source.

#include <metal_stdlib>
using namespace metal;

#define G_WINDOW_BITS 16
#define G_DIGITS ((1u << G_WINDOW_BITS) - 1u)
#define G_WINDOWS ((256 + G_WINDOW_BITS - 1) / G_WINDOW_BITS)

// Parameter offsets shared by both kernels; each kernel's own follow from offset 4.
#define P_START 0
#define P_COUNT 1
#define P_TARGET_VALUE 2
#define P_TARGET_BITS 3

struct Fe {
    uint v[8];
};

struct Jacobian {
    Fe x;
    Fe y;
    Fe z;
    bool infinity;
};

constant uint FIELD_P[8] = {0xFFFFFC2Fu, 0xFFFFFFFEu, 0xFFFFFFFFu, 0xFFFFFFFFu, 0xFFFFFFFFu, 0xFFFFFFFFu, 0xFFFFFFFFu, 0xFFFFFFFFu};
constant uint ORDER_N[8] = {0xD0364141u, 0xBFD25E8Cu, 0xAF48A03Bu, 0xBAAEDCE6u, 0xFFFFFFFEu, 0xFFFFFFFFu, 0xFFFFFFFFu, 0xFFFFFFFFu};

constant uint K256[64] = {
    0x428a2f98u, 0x71374491u, 0xb5c0fbcfu, 0xe9b5dba5u, 0x3956c25bu, 0x59f111f1u, 0x923f82a4u, 0xab1c5ed5u,
    0xd807aa98u, 0x12835b01u, 0x243185beu, 0x550c7dc3u, 0x72be5d74u, 0x80deb1feu, 0x9bdc06a7u, 0xc19bf174u,
    0xe49b69c1u, 0xefbe4786u, 0x0fc19dc6u, 0x240ca1ccu, 0x2de92c6fu, 0x4a7484aau, 0x5cb0a9dcu, 0x76f988dau,
    0x983e5152u, 0xa831c66du, 0xb00327c8u, 0xbf597fc7u, 0xc6e00bf3u, 0xd5a79147u, 0x06ca6351u, 0x14292967u,
    0x27b70a85u, 0x2e1b2138u, 0x4d2c6dfcu, 0x53380d13u, 0x650a7354u, 0x766a0abbu, 0x81c2c92eu, 0x92722c85u,
    0xa2bfe8a1u, 0xa81a664bu, 0xc24b8b70u, 0xc76c51a3u, 0xd192e819u, 0xd6990624u, 0xf40e3585u, 0x106aa070u,
    0x19a4c116u, 0x1e376c08u, 0x2748774cu, 0x34b0bcb5u, 0x391c0cb3u, 0x4ed8aa4au, 0x5b9cca4fu, 0x682e6ff3u,
    0x748f82eeu, 0x78a5636fu, 0x84c87814u, 0x8cc70208u, 0x90befffau, 0xa4506cebu, 0xbef9a3f7u, 0xc67178f2u,
};

// ---------------------------------------------------------------------------------------------
// 256-bit arithmetic

inline bool fe_is_zero(Fe a) {
    uint bits = 0;
    #pragma unroll
    for (int i = 0; i < 8; i++) bits |= a.v[i];
    return bits == 0;
}

// a >= b for limbs in `constant` memory (p or n).
inline bool gte_const(Fe a, constant uint *b) {
    bool result = true;
    bool decided = false;
    #pragma unroll
    for (int i = 7; i >= 0; i--) {
        if (!decided && a.v[i] != b[i]) {
            result = a.v[i] > b[i];
            decided = true;
        }
    }
    return result;
}

inline Fe sub_const(Fe a, constant uint *b) {
    Fe r;
    long borrow = 0;
    #pragma unroll
    for (int i = 0; i < 8; i++) {
        long d = (long)a.v[i] - (long)b[i] + borrow;
        r.v[i] = (uint)d;
        borrow = d >> 32;
    }
    return r;
}

// ---------------------------------------------------------------------------------------------
// The field modulo p = 2^256 - 2^32 - 977. Every function returns a fully reduced element.

// Reduce a 512-bit product with 2^256 ≡ 2^32 + 977 (mod p).
inline Fe fe_reduce(thread const uint *t) {
    Fe r;
    ulong c = 0;
    #pragma unroll
    for (int i = 0; i < 8; i++) {
        c += (ulong)t[i] + (ulong)t[8 + i] * 977ul;
        if (i > 0) c += t[7 + i];
        r.v[i] = (uint)c;
        c >>= 32;
    }
    // What is left sits at 2^256: the carry plus the top word's 2^32 term.
    ulong top = c + t[15];
    c = (ulong)r.v[0] + top * 977ul;
    r.v[0] = (uint)c;
    c >>= 32;
    c += (ulong)r.v[1] + top;
    r.v[1] = (uint)c;
    c >>= 32;
    #pragma unroll
    for (int i = 2; i < 8; i++) {
        c += r.v[i];
        r.v[i] = (uint)c;
        c >>= 32;
    }
    // A final carry leaves r small, so adding 2^32 + 977 once more cannot carry far.
    c = (ulong)r.v[0] + c * 977ul + (c << 32);
    r.v[0] = (uint)c;
    c >>= 32;
    #pragma unroll
    for (int i = 1; i < 8; i++) {
        c += r.v[i];
        r.v[i] = (uint)c;
        c >>= 32;
    }
    if (gte_const(r, FIELD_P)) r = sub_const(r, FIELD_P);
    return r;
}

// Product scanning with a 96-bit column accumulator (acc + acc_hi·2^64).
inline Fe fe_mul(Fe a, Fe b) {
    uint t[16];
    ulong acc = 0;
    uint acc_hi = 0;
    #pragma unroll
    for (int k = 0; k < 15; k++) {
        #pragma unroll
        for (int i = 0; i < 8; i++) {
            int j = k - i;
            if (j >= 0 && j < 8) {
                ulong p = (ulong)a.v[i] * b.v[j];
                acc += p;
                acc_hi += acc < p;
            }
        }
        t[k] = (uint)acc;
        acc = (acc >> 32) | ((ulong)acc_hi << 32);
        acc_hi = 0;
    }
    t[15] = (uint)acc;
    return fe_reduce(t);
}

// Each cross product once, added twice: 36 products instead of 64.
inline Fe fe_sqr(Fe a) {
    uint t[16];
    ulong acc = 0;
    uint acc_hi = 0;
    #pragma unroll
    for (int k = 0; k < 15; k++) {
        #pragma unroll
        for (int i = 0; i < 8; i++) {
            int j = k - i;
            if (j > i && j < 8) {
                ulong p = (ulong)a.v[i] * a.v[j];
                acc += p;
                acc_hi += acc < p;
                acc += p;
                acc_hi += acc < p;
            }
        }
        if ((k & 1) == 0) {
            ulong p = (ulong)a.v[k / 2] * a.v[k / 2];
            acc += p;
            acc_hi += acc < p;
        }
        t[k] = (uint)acc;
        acc = (acc >> 32) | ((ulong)acc_hi << 32);
        acc_hi = 0;
    }
    t[15] = (uint)acc;
    return fe_reduce(t);
}

inline Fe fe_add(Fe a, Fe b) {
    Fe r;
    ulong c = 0;
    #pragma unroll
    for (int i = 0; i < 8; i++) {
        c += (ulong)a.v[i] + b.v[i];
        r.v[i] = (uint)c;
        c >>= 32;
    }
    if (c != 0 || gte_const(r, FIELD_P)) r = sub_const(r, FIELD_P);
    return r;
}

inline Fe fe_sub(Fe a, Fe b) {
    Fe r;
    long borrow = 0;
    #pragma unroll
    for (int i = 0; i < 8; i++) {
        long d = (long)a.v[i] - (long)b.v[i] + borrow;
        r.v[i] = (uint)d;
        borrow = d >> 32;
    }
    if (borrow != 0) {
        // Add p back.
        ulong c = 0;
        #pragma unroll
        for (int i = 0; i < 8; i++) {
            c += (ulong)r.v[i] + FIELD_P[i];
            r.v[i] = (uint)c;
            c >>= 32;
        }
    }
    return r;
}

inline Fe fe_sqr_n(Fe a, int n) {
    for (int i = 0; i < n; i++) a = fe_sqr(a);
    return a;
}

// a^(p-2) with libsecp256k1's addition chain.
inline Fe fe_inv(Fe a) {
    Fe x2 = fe_mul(fe_sqr(a), a);
    Fe x3 = fe_mul(fe_sqr(x2), a);
    Fe x6 = fe_mul(fe_sqr_n(x3, 3), x3);
    Fe x9 = fe_mul(fe_sqr_n(x6, 3), x3);
    Fe x11 = fe_mul(fe_sqr_n(x9, 2), x2);
    Fe x22 = fe_mul(fe_sqr_n(x11, 11), x11);
    Fe x44 = fe_mul(fe_sqr_n(x22, 22), x22);
    Fe x88 = fe_mul(fe_sqr_n(x44, 44), x44);
    Fe x176 = fe_mul(fe_sqr_n(x88, 88), x88);
    Fe x220 = fe_mul(fe_sqr_n(x176, 44), x44);
    Fe x223 = fe_mul(fe_sqr_n(x220, 3), x3);
    Fe t = fe_mul(fe_sqr_n(x223, 23), x22);
    t = fe_mul(fe_sqr_n(t, 5), a);
    t = fe_mul(fe_sqr_n(t, 3), x2);
    return fe_mul(fe_sqr_n(t, 2), a);
}

// ---------------------------------------------------------------------------------------------
// Points

inline Jacobian point_double(Jacobian p) {
    Jacobian r;
    if (p.infinity || fe_is_zero(p.y)) {
        r.infinity = true;
        return r;
    }
    Fe a = fe_sqr(p.x);
    Fe b = fe_sqr(p.y);
    Fe c = fe_sqr(b);
    Fe d = fe_sub(fe_sub(fe_sqr(fe_add(p.x, b)), a), c);
    d = fe_add(d, d);
    Fe e = fe_add(fe_add(a, a), a);
    r.x = fe_sub(fe_sqr(e), fe_add(d, d));
    Fe c8 = fe_add(c, c);
    c8 = fe_add(c8, c8);
    c8 = fe_add(c8, c8);
    r.y = fe_sub(fe_mul(e, fe_sub(d, r.x)), c8);
    Fe yz = fe_mul(p.y, p.z);
    r.z = fe_add(yz, yz);
    r.infinity = false;
    return r;
}

// Jacobian plus affine (madd-2007-bl), with the special cases.
inline Jacobian point_add_affine(Jacobian p, Fe qx, Fe qy) {
    if (p.infinity) {
        Jacobian r;
        r.x = qx;
        r.y = qy;
        r.z = Fe{{1, 0, 0, 0, 0, 0, 0, 0}};
        r.infinity = false;
        return r;
    }
    Fe z1z1 = fe_sqr(p.z);
    Fe u2 = fe_mul(qx, z1z1);
    Fe s2 = fe_mul(qy, fe_mul(p.z, z1z1));
    Fe h = fe_sub(u2, p.x);
    Fe s_diff = fe_sub(s2, p.y);
    if (fe_is_zero(h)) {
        if (fe_is_zero(s_diff)) return point_double(p);
        Jacobian r;
        r.infinity = true;
        return r;
    }
    Fe hh = fe_sqr(h);
    Fe i = fe_add(hh, hh);
    i = fe_add(i, i);
    Fe j = fe_mul(h, i);
    Fe r_ = fe_add(s_diff, s_diff);
    Fe v = fe_mul(p.x, i);
    Jacobian r;
    r.x = fe_sub(fe_sub(fe_sqr(r_), j), fe_add(v, v));
    Fe y1j = fe_mul(p.y, j);
    r.y = fe_sub(fe_mul(r_, fe_sub(v, r.x)), fe_add(y1j, y1j));
    r.z = fe_sub(fe_sub(fe_sqr(fe_add(p.z, h)), z1z1), hh);
    r.infinity = false;
    return r;
}

inline uint window_digit(Fe k, uint bit) {
    uint limb = bit / 32;
    uint shift = bit % 32;
    uint digit = k.v[limb] >> shift;
    if (shift + G_WINDOW_BITS > 32 && limb < 7) digit |= k.v[limb + 1] << (32 - shift);
    return digit & G_DIGITS;
}

// acc + k·G from the table: one mixed addition per nonzero window of k.
inline Jacobian add_mul_g(Jacobian acc, Fe k, device const uint *g_table) {
    for (uint w = 0; w < G_WINDOWS; w++) {
        uint digit = window_digit(k, w * G_WINDOW_BITS);
        if (digit != 0) {
            uint base = (w * G_DIGITS + digit - 1) * 16;
            Fe qx, qy;
            #pragma unroll
            for (int i = 0; i < 8; i++) {
                qx.v[i] = g_table[base + i];
                qy.v[i] = g_table[base + 8 + i];
            }
            acc = point_add_affine(acc, qx, qy);
        }
    }
    return acc;
}

// ---------------------------------------------------------------------------------------------
// SHA-256 from a midstate

inline uint rotr(uint x, uint n) {
    return (x >> n) | (x << (32 - n));
}

inline void sha256_compress(thread uint *state, thread const uint *block) {
    uint w[64];
    #pragma unroll
    for (int i = 0; i < 16; i++) w[i] = block[i];
    #pragma unroll
    for (int i = 16; i < 64; i++) {
        uint s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >> 3);
        uint s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16] + s0 + w[i - 7] + s1;
    }
    uint a = state[0], b = state[1], c = state[2], d = state[3];
    uint e = state[4], f = state[5], g = state[6], h = state[7];
    #pragma unroll
    for (int i = 0; i < 64; i++) {
        uint t1 = h + (rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25)) + ((e & f) ^ (~e & g)) + K256[i] + w[i];
        uint t2 = (rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22)) + ((a & b) ^ (a & c) ^ (b & c));
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

inline Fe read_fe(device const uint *params, int offset) {
    Fe r;
    #pragma unroll
    for (int i = 0; i < 8; i++) r.v[i] = params[offset + i];
    return r;
}
