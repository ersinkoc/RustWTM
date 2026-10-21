//! A small folded-multiply hasher for the maps filled on every event or
//! sample. Their keys are PIDs, thread IDs and kernel pointers handed out by
//! the system, not chosen by an attacker, so the DoS resistance of the
//! default SipHash buys nothing there and costs most of each lookup.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

/// Odd constants with well-spread bits (digits of pi).
const SEED: u64 = 0x243f_6a88_85a3_08d3;
const K: u64 = 0x1319_8a2e_0370_7345;

#[derive(Debug, Clone, Copy)]
pub(crate) struct FastHasher {
    hash: u64,
}

impl Default for FastHasher {
    fn default() -> Self {
        Self { hash: SEED }
    }
}

impl FastHasher {
    /// Mixes in one word: the full 128-bit product folded onto itself, so
    /// every input bit reaches the low bits the table indexes by (a plain
    /// multiply would leave the zero low bits of aligned pointers zero).
    #[inline]
    fn add(&mut self, word: u64) {
        let product = u128::from(self.hash ^ word) * u128::from(K);
        self.hash = (product as u64) ^ ((product >> 64) as u64);
    }
}

impl Hasher for FastHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let (chunks, rest) = bytes.as_chunks::<8>();
        for c in chunks {
            self.add(u64::from_le_bytes(*c));
        }
        if !rest.is_empty() {
            let mut last = [0u8; 8];
            last[..rest.len()].copy_from_slice(rest);
            self.add(u64::from_le_bytes(last) ^ (rest.len() as u64) << 59);
        }
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(u64::from(i));
    }

    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.add(u64::from(i));
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(u64::from(i));
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }
}

pub(crate) type FastMap<K, V> = HashMap<K, V, BuildHasherDefault<FastHasher>>;
pub(crate) type FastSet<K> = HashSet<K, BuildHasherDefault<FastHasher>>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::{BuildHasher, Hash};

    fn hash<T: Hash>(v: T) -> u64 {
        BuildHasherDefault::<FastHasher>::default().hash_one(v)
    }

    #[test]
    fn spreads_aligned_pointers_over_the_low_bits() {
        // File objects are 16-byte aligned kernel addresses.
        let buckets: FastSet<u64> = (0..1024u64)
            .map(|i| hash(0xffff_a000_0000_0000u64 + i * 16) & 1023)
            .collect();
        assert!(buckets.len() > 600, "{} of 1024 buckets", buckets.len());
    }

    #[test]
    fn distinguishes_byte_strings_of_different_lengths() {
        assert_ne!(hash("a"), hash("a\0"));
        assert_ne!(hash([1u8, 2, 3].as_slice()), hash([1u8, 2].as_slice()));
        assert_eq!(hash((7u32, 9u64)), hash((7u32, 9u64)));
    }

    /// What one disk or network event costs in map updates, SipHash against
    /// this hasher. Run with `--release -- --ignored --nocapture`.
    #[test]
    #[ignore = "timing harness"]
    fn event_map_updates_cost_less_than_with_siphash() {
        fn run<S: BuildHasher + Default>() -> f64 {
            let mut bytes: HashMap<(u32, u64), (u64, u64), S> = HashMap::default();
            let mut threads: HashMap<u32, u32, S> = HashMap::default();
            for t in 0..2000u32 {
                threads.insert(t * 4, t % 300);
            }
            let events = 2_000_000u64;
            let start = std::time::Instant::now();
            for i in 0..events {
                let thread = ((i * 7919) % 2000) as u32 * 4;
                let pid = threads[&thread];
                let file = 0xffff_a000_0000_0000u64 + ((i * 104_729) % 5000) * 16;
                bytes.entry((pid, file)).or_default().0 += 4096;
            }
            std::hint::black_box(&bytes);
            start.elapsed().as_secs_f64() * 1e9 / events as f64
        }
        let sip = run::<std::hash::RandomState>();
        let fast = run::<BuildHasherDefault<FastHasher>>();
        println!("per event: SipHash {sip:.1} ns, FastHasher {fast:.1} ns");
        assert!(fast < sip);
    }

    #[test]
    fn works_as_a_map() {
        let mut m: FastMap<(u32, u64), u32> = FastMap::default();
        for i in 0..10_000u32 {
            m.insert((i, u64::from(i) << 4), i);
        }
        assert!((0..10_000u32).all(|i| m[&(i, u64::from(i) << 4)] == i));
    }
}
