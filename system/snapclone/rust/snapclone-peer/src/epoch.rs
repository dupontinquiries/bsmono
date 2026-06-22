//! Epoch counter — used to detect stale callbacks.
//!
//! Every time a session is reset or a new transport is adopted, the epoch
//! increments. Callbacks capture the epoch at creation time; if the epoch
//! has changed by the time they fire, they know to discard their result.
//! This eliminates the "stale timer fires on replaced session" class of bugs.

use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Debug, Default)]
pub struct Epoch(AtomicU32);

impl Epoch {
    pub fn current(&self) -> u32 { self.0.load(Ordering::Relaxed) }
    pub fn bump(&self) -> u32 {
        self.0.fetch_add(1, Ordering::Relaxed) + 1
    }
    pub fn is_current(&self, e: u32) -> bool { self.current() == e }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn increments() {
        let e = Epoch::default();
        assert_eq!(e.current(), 0);
        let n = e.bump();
        assert_eq!(n, 1);
        assert!(e.is_current(1));
        assert!(!e.is_current(0));
    }
}
