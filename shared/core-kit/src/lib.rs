//! core-kit — the foundational leaf utilities the monorepo re-implements everywhere.
//!
//! A structural-clone scan put these at the broadest end of the duplication
//! spectrum — not the largest single clones, but the most *places*: the
//! `SystemTime::now().duration_since(UNIX_EPOCH)` dance (as `now_secs`/`now_ms`/
//! `now_nanos`, copied across 8+ crates), hand-rolled hex encoders (the
//! `format!("{:02x}")` loop in netserve, share-kit, crypto-kit, kitfinder, …), and
//! the same PRNG constants over and over (the LCG multiplier `1664525` alone shows
//! up 100+ times; splitmix64's mix constants dozens more).
//!
//! core-kit is the bottom of the kit stack: pure, zero-dependency, and depended on
//! even by the other shared kits.
//!
//!   * [`time`] — `now_secs` / `now_millis` / `now_nanos` (UNIX epoch, clock-skew
//!     safe) and a monotonic [`Stopwatch`](time::Stopwatch).
//!   * [`hex`] — lowercase [`encode`](hex::encode) / [`decode`](hex::decode).
//!   * [`rng`] — [`SplitMix64`](rng::SplitMix64), a small deterministic PRNG.
//!
//! Note on `rng`: adopting it in *existing* code changes that code's random
//! sequence (any PRNG swap does), so it's offered as the canonical choice for new
//! use and for opt-in, sequence-compatibility-aware migration — not a blind
//! find-and-replace.

pub mod hex;
pub mod rng;
pub mod time;

pub use rng::SplitMix64;
pub use time::Stopwatch;
