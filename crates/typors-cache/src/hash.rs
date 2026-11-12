//! Stable hashing used for on-disk cache names and entry keys.
//!
//! `std::collections::hash_map::DefaultHasher` is explicitly not stable across
//! Rust releases, so a cache directory named from it would be orphaned by a
//! compiler upgrade. Everything persisted therefore uses FNV-1a 64, whose
//! vectors are pinned by tests below.

/// FNV-1a 64-bit offset basis.
pub const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a 64-bit prime.
pub const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a over `bytes`, seeded so two independent 64-bit values can be produced
/// from the same input.
pub fn fnv1a64_seeded(seed: u64, bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET ^ seed;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// FNV-1a 64-bit of `bytes` with the standard basis.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    fnv1a64_seeded(0, bytes)
}

/// Two independent 64-bit hashes of the same input (seeds 0 and `0x9e37...`).
pub fn dual(bytes: &[u8]) -> (u64, u64) {
    (fnv1a64_seeded(0, bytes), fnv1a64_seeded(0x9e37_79b9_7f4a_7c15, bytes))
}

/// Lower-case 16-character hex of one 64-bit hash.
pub fn hex16(value: u64) -> String {
    format!("{value:016x}")
}

/// 32-character hex of the dual hash, used for one-shot keys.
pub fn hex32(bytes: &[u8]) -> String {
    let (a, b) = dual(bytes);
    format!("{a:016x}{b:016x}")
}

/// Hash a path's platform bytes. On Windows the canonical path string is used
/// unchanged, so case differences stay distinct.
pub fn hash_path(path: &std::path::Path) -> String {
    let text = path.to_string_lossy();
    hex16(fnv1a64(text.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_vectors_are_pinned() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn dual_hashes_differ_and_are_stable() {
        let (a, b) = dual(b"notes.md");
        assert_ne!(a, b);
        assert_eq!(hex32(b"notes.md").len(), 32);
        assert_eq!(hex16(0), "0000000000000000");
    }

    #[test]
    fn seeded_hash_is_not_the_plain_hash() {
        assert_ne!(fnv1a64_seeded(1, b"x"), fnv1a64(b"x"));
    }
}
