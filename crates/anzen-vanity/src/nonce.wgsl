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

fn candidate(locktime: u32) -> Candidate {
    var result: Candidate;
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
    var internal: Affine;
    internal.x = read_fe(P_INTERNAL_X);
    internal.y = read_fe(P_INTERNAL_Y);
    let output = to_affine(add_mul_table(from_affine(internal), G_TABLE, tweak));
    if output.infinity {
        return result;
    }
    result.valid = true;
    result.output_x = output.x;
    return result;
}
