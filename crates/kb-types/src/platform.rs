//! Platform traits the core depends on and the shells implement (ADR-0005, ADR-0015).
//!
//! The core never reads the clock or the OS random source directly; both are injected
//! so every test is deterministic and the same code runs natively and in WebAssembly.

/// `Send` on native targets; no bound on wasm32, where there are no threads and JS
/// handles are not `Send`.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + ?Sized> MaybeSend for T {}
#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}
#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSend for T {}

/// `Sync` on native targets; no bound on wasm32.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSync: Sync {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Sync + ?Sized> MaybeSync for T {}
#[cfg(target_arch = "wasm32")]
pub trait MaybeSync {}
#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSync for T {}

/// Wall-clock time source.
pub trait Clock: MaybeSend + MaybeSync {
    /// Current time as Unix milliseconds. Display-only; no logic orders events by it.
    fn now_ms(&self) -> i64;

    /// How far the device's wall clock is ahead of UTC right now, in seconds (positive east
    /// of Greenwich, negative west of it; daylight saving included). Only the reminders
    /// (ADR-0024) use it: a date mention is a floating local date (ADR-0016), so "now" for a
    /// reminder is the local clock's reading, `now_ms() / 1000 + local_offset_seconds()`.
    /// Platforms that know their zone override it; the default is UTC, so a clock that does
    /// not (the web shell until it reads `Date.getTimezoneOffset`) fires reminders on UTC
    /// time, early or late by the offset, never twice.
    fn local_offset_seconds(&self) -> i32 {
        0
    }
}

/// Cryptographically secure random bytes.
pub trait Rng: MaybeSend + MaybeSync {
    /// Fills `buf` with random bytes from the operating system's CSPRNG (or a seeded
    /// generator in tests).
    fn fill_bytes(&self, buf: &mut [u8]);
}

/// Convenience methods available on every [`Rng`], including `dyn Rng`.
pub trait RngExt {
    /// A fixed-size array of random bytes.
    fn random_array<const N: usize>(&self) -> [u8; N];
}

impl<T: Rng + ?Sized> RngExt for T {
    fn random_array<const N: usize>(&self) -> [u8; N] {
        let mut out = [0u8; N];
        self.fill_bytes(&mut out);
        out
    }
}
