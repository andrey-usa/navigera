//! A fast hasher for maps keyed by what Chrome or navigera numbers: node
//! indices, backend node ids, the tree's own node ids. Building a large
//! page's tree hashes such keys a few hundred thousand times, and the
//! standard SipHash was an eighth of that work. Keys a page chooses (its
//! element ids, its text) keep the standard hasher, which a page can't
//! make collide.

use std::hash::{BuildHasherDefault, Hasher};

#[derive(Default, Clone, Copy)]
pub struct FxHasher(u64);

/// The multiplier of rustc's own hasher (rustc-hash 2).
const K: u64 = 0xf135_7aea_2e62_a9c5;

impl FxHasher {
    #[inline]
    fn add(&mut self, i: u64) {
        self.0 = self.0.wrapping_add(i).wrapping_mul(K);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            self.add(u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]));
        }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            let mut b = [0u8; 8];
            b[..rest.len()].copy_from_slice(rest);
            self.add(u64::from_le_bytes(b) ^ ((rest.len() as u64) << 56));
        }
    }
    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(i as u64);
    }
    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(i as u64);
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }
    #[inline]
    fn write_i64(&mut self, i: i64) {
        self.add(i as u64);
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }
    #[inline]
    fn finish(&self) -> u64 {
        // The table picks buckets by the low bits and tags by the high
        // ones: the product's best bits are the high ones.
        self.0.rotate_left(26)
    }
}

pub type FxBuild = BuildHasherDefault<FxHasher>;
pub type FxMap<K, V> = std::collections::HashMap<K, V, FxBuild>;
pub type FxSet<K> = std::collections::HashSet<K, FxBuild>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dense_keys_spread_and_strings_differ() {
        let m: FxMap<usize, usize> = (0..100_000).map(|i| (i, i)).collect();
        assert!((0..100_000).all(|i| m[&i] == i));
        let s: FxSet<String> = (0..10_000).map(|i| i.to_string()).collect();
        assert_eq!(s.len(), 10_000);
        let short: FxSet<&str> = ["a", "a\0", "ab", "b"].into_iter().collect();
        assert_eq!(short.len(), 4);
    }
}
