// Mandara patch 18: GLSL 3.3 has no findLSB intrinsic. Shard words are nonzero.
fn first_shard_bit(value: u32) -> u32 {
    var bits = value;
    var offset = 0u;
    if ((bits & 0xffffu) == 0u) { bits >>= 16u; offset += 16u; }
    if ((bits & 0xffu) == 0u) { bits >>= 8u; offset += 8u; }
    if ((bits & 0xfu) == 0u) { bits >>= 4u; offset += 4u; }
    if ((bits & 0x3u) == 0u) { bits >>= 2u; offset += 2u; }
    return offset + select(0u, 1u, (bits & 1u) == 0u);
}

