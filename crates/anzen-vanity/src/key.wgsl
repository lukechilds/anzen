// Phone vault key grinding: one candidate per BIP32 index.
//   1. BIP32 non-hardened derivation from the phone's vault xpub: I = HMAC-SHA512(chain code,
//      serP(parent) || index), child = parent + I_L·G.
//   2. BIP327 key aggregation of the child and the signer key (even-y points, sorted):
//      Q = a·first + second, with a from the KeyAgg hashes.
//   3. The BIP341 Taproot tweak with the signer's leaf: output = Q_even + t·G.

const P_INNER: u32 = 4u;
const P_OUTER: u32 = 20u;
const P_MESSAGE: u32 = 36u;
const P_HWW_X: u32 = 68u;
const P_HWW_Y: u32 = 76u;
const P_LIST_MID: u32 = 84u;
const P_COEF_MID: u32 = 92u;
const P_TWEAK_MID: u32 = 100u;
const P_ROOT: u32 = 108u;
const P_PARENT_X: u32 = 116u;
const P_PARENT_Y: u32 = 124u;

// The signer's and the parent's tables, 8-bit windows like G's at its default: entry (window
// w, digit d) of a point's table is d·2^(8w)·point, for d in 1..=255.
@group(0) @binding(4) var<storage, read> key_tables: array<u32>;
const SIGNER_TABLE: u32 = 0u;
const PARENT_TABLE: u32 = 130560u;

fn key_point(table: u32, window: u32, digit: u32) -> Affine {
    let base = table + (window * 255u + digit - 1u) * 16u;
    var q: Affine;
    for (var i = 0u; i < 8u; i++) {
        q.x[i] = key_tables[base + i];
        q.y[i] = key_tables[base + 8u + i];
    }
    return q;
}

// acc + k·point for the signer or parent key.
fn add_mul_key(start: Jacobian, table: u32, k: Fe) -> Jacobian {
    var acc = start;
    for (var w = 0u; w < 32u; w++) {
        let digit = (k[w / 4u] >> (8u * (w % 4u))) & 0xffu;
        if digit != 0u {
            acc = point_add_affine(acc, key_point(table, w, digit));
        }
    }
    return acc;
}

// I_L of HMAC-SHA512(chain code, serP(parent) || index): the BIP32 child tweak.
fn bip32_tweak(index: u32) -> Fe {
    // Inner hash: the message block is prepared by the host apart from the index bytes.
    var block: array<vec2<u32>, 16>;
    for (var i = 0u; i < 16u; i++) {
        block[i] = vec2<u32>(params[P_MESSAGE + 2u * i], params[P_MESSAGE + 2u * i + 1u]);
    }
    // serP is bytes 0..32; the big-endian index is bytes 33..36, inside 64-bit word 4.
    block[4] = vec2<u32>(block[4].x | (index >> 8u), block[4].y | ((index & 0xffu) << 24u));
    let inner = sha512_compress(read_state512(P_INNER), block);
    // Outer hash over the inner digest.
    var outer_block: array<vec2<u32>, 16>;
    for (var i = 0u; i < 8u; i++) {
        outer_block[i] = inner[i];
    }
    outer_block[8] = vec2<u32>(0x80000000u, 0u);
    outer_block[15] = vec2<u32>(0u, (128u + 64u) * 8u);
    let digest = sha512_compress(read_state512(P_OUTER), outer_block);
    // The first 32 bytes, big-endian, as little-endian limbs.
    var k: Fe;
    for (var i = 0u; i < 4u; i++) {
        k[7u - 2u * i] = digest[i].x;
        k[6u - 2u * i] = digest[i].y;
    }
    return k;
}

fn put_key(data: ptr<function, array<u32, 128>>, offset: u32, x: Fe) {
    (*data)[offset] = 0x02u;
    for (var i = 0u; i < 32u; i++) {
        (*data)[offset + 1u + i] = fe_byte(x, i);
    }
}

fn candidate(index: u32) -> Candidate {
    var result: Candidate;
    // 1. The child vault key.
    let tweak = bip32_tweak(index);
    if gte(tweak, ORDER_N) || is_zero(tweak) {
        return result;
    }
    var parent: Affine;
    parent.x = read_fe(P_PARENT_X);
    parent.y = read_fe(P_PARENT_Y);
    let child = to_affine(add_mul_g(from_affine(parent), tweak));
    if child.infinity {
        return result;
    }
    // Aggregation uses the even-y point of each x-only key.
    var phone = child;
    let child_odd = (child.y[0] & 1u) != 0u;
    if child_odd {
        phone.y = fe_neg(phone.y);
    }
    var signer: Affine;
    signer.x = read_fe(P_HWW_X);
    signer.y = read_fe(P_HWW_Y);
    if equal(phone.x, signer.x) {
        return result;
    }

    // 2. KeySort, then KeyAgg: the first key is hashed with the list, the second has
    //    coefficient 1.
    let phone_first = gte(signer.x, phone.x);
    var first = signer;
    var second = phone;
    if phone_first {
        first = phone;
        second = signer;
    }
    var data: array<u32, 128>;
    put_key(&data, 0u, first.x);
    put_key(&data, 33u, second.x);
    let list = sha256_tagged(read_state256(P_LIST_MID), &data, 66u);
    for (var i = 0u; i < 8u; i++) {
        for (var b = 0u; b < 4u; b++) {
            data[4u * i + b] = (list[i] >> (24u - 8u * b)) & 0xffu;
        }
    }
    put_key(&data, 32u, first.x);
    var coefficient = digest_to_fe(sha256_tagged(read_state256(P_COEF_MID), &data, 65u));
    if gte(coefficient, ORDER_N) {
        coefficient = sub_raw(coefficient, ORDER_N);
    }
    // coefficient·first from precomputed tables instead of a generic multiplication. The phone
    // key is ±(parent + tweak·G), so its multiple is s·parent + (s·tweak)·G with s = ±coefficient.
    // Both orders run the same table additions so neighbouring candidates do not diverge.
    var table = SIGNER_TABLE;
    var scalar = coefficient;
    var g_scalar: Fe;
    if phone_first {
        table = PARENT_TABLE;
        if child_odd {
            scalar = scalar_neg(scalar);
        }
        g_scalar = scalar_mul(scalar, tweak);
    }
    let scaled = add_mul_g(add_mul_key(infinity(), table, scalar), g_scalar);
    let aggregate = to_affine(point_add_affine(scaled, second));
    if aggregate.infinity {
        return result;
    }

    // 3. The Taproot tweak over the x-only internal key and the signer's leaf.
    for (var i = 0u; i < 32u; i++) {
        data[i] = fe_byte(aggregate.x, i);
    }
    for (var i = 0u; i < 8u; i++) {
        for (var b = 0u; b < 4u; b++) {
            data[32u + 4u * i + b] = (params[P_ROOT + i] >> (24u - 8u * b)) & 0xffu;
        }
    }
    let taproot = digest_to_fe(sha256_tagged(read_state256(P_TWEAK_MID), &data, 64u));
    if gte(taproot, ORDER_N) {
        return result;
    }
    var internal = aggregate;
    if (internal.y[0] & 1u) != 0u {
        internal.y = fe_neg(internal.y);
    }
    let output = to_affine(add_mul_g(from_affine(internal), taproot));
    if output.infinity {
        return result;
    }
    result.valid = true;
    result.output_x = output.x;
    return result;
}


const PER_INVOCATION: u32 = 1u;

fn candidates(first: u32) -> array<Candidate, PER_INVOCATION> {
    var batch: array<Candidate, PER_INVOCATION>;
    batch[0] = candidate(first);
    return batch;
}
