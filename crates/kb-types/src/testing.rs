//! Deterministic doubles for [`Clock`] and [`Rng`], for tests only.
//!
//! `SeededRng` is **not** cryptographically secure; it exists so tests produce the
//! same ids and keys on every run.

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use crate::platform::{Clock, Rng};

/// A clock that returns a fixed value until advanced.
#[derive(Debug)]
pub struct FixedClock(AtomicI64);

impl FixedClock {
    /// Starts at `now_ms`.
    #[must_use]
    pub const fn new(now_ms: i64) -> Self {
        Self(AtomicI64::new(now_ms))
    }

    /// Moves the clock forward.
    pub fn advance(&self, delta_ms: i64) {
        self.0.fetch_add(delta_ms, Ordering::SeqCst);
    }

    /// Sets the clock.
    pub fn set(&self, now_ms: i64) {
        self.0.store(now_ms, Ordering::SeqCst);
    }
}

impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// SplitMix64-based generator with a fixed seed. Deterministic, not secure.
#[derive(Debug)]
pub struct SeededRng(AtomicU64);

impl SeededRng {
    /// Creates a generator from `seed`.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self(AtomicU64::new(seed))
    }

    fn next_u64(&self) -> u64 {
        let state = self.0.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::SeqCst);
        let mut z = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

impl Rng for SeededRng {
    fn fill_bytes(&self, buf: &mut [u8]) {
        for chunk in buf.chunks_mut(8) {
            let bytes = self.next_u64().to_le_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::RngExt;

    #[test]
    fn seeded_rng_is_deterministic() {
        let a = SeededRng::new(42);
        let b = SeededRng::new(42);
        let first_a = a.random_array::<32>();
        let first_b = b.random_array::<32>();
        assert_eq!(first_a, first_b, "same seed, same sequence");
        assert_ne!(
            first_a,
            a.random_array::<32>(),
            "consecutive outputs differ"
        );
        let c = SeededRng::new(43);
        assert_ne!(first_a, c.random_array::<32>(), "different seeds differ");
    }

    #[test]
    fn fixed_clock_advances() {
        let c = FixedClock::new(1_000);
        c.advance(5);
        assert_eq!(c.now_ms(), 1_005);
        c.set(7);
        assert_eq!(c.now_ms(), 7);
    }
}
