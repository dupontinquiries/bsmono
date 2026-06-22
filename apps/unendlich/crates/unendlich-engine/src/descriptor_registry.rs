//! descriptor_registry.rs — pure descriptor-slot registry (R9/R3).
//!
//! Tracks which `(set, binding)` slots are in use so the free-slot UBO/descriptor
//! scheme can reserve and auto-allocate bindings without collisions. Pairs with
//! `spirv_reflect_check` (which catches shader/Rust drift); this is the Rust-side
//! source of truth. Pure, deterministic, panic-free (R1/R11/R21).

/// Per-set ceiling for auto-allocation via [`Registry::alloc`]. Explicit
/// [`Registry::reserve`] is not capped (a binding number can be any `u32`); this
/// only bounds the "next free binding" search so a full set is detectable.
pub const MAX_AUTO_BINDINGS_PER_SET: u32 = 16;

#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Registry {
    /// Occupied `(set, binding)` slots. Kept sorted+deduped for determinism.
    used: Vec<(u32, u32)>,
}

impl Registry {
    pub fn new() -> Self {
        Registry { used: Vec::new() }
    }

    /// All occupied slots, in deterministic sorted order.
    pub fn slots(&self) -> &[(u32, u32)] {
        &self.used
    }

    /// Whether `(set, binding)` is currently unoccupied.
    pub fn is_free(&self, set: u32, binding: u32) -> bool {
        self.used.binary_search(&(set, binding)).is_err()
    }

    /// Reserve `(set, binding)`. `Ok(())` if it was free; `Err((set, binding))`
    /// on a collision (already reserved).
    pub fn reserve(&mut self, set: u32, binding: u32) -> Result<(), (u32, u32)> {
        match self.used.binary_search(&(set, binding)) {
            Ok(_) => Err((set, binding)),
            Err(pos) => {
                self.used.insert(pos, (set, binding));
                Ok(())
            }
        }
    }

    /// Reserve the smallest free binding in `set` (searching
    /// `0..MAX_AUTO_BINDINGS_PER_SET`). Returns the binding, or `None` if the set
    /// is full within that range.
    pub fn alloc(&mut self, set: u32) -> Option<u32> {
        for b in 0..MAX_AUTO_BINDINGS_PER_SET {
            if self.is_free(set, b) {
                // reserve cannot fail here (we just checked it's free)
                if self.reserve(set, b).is_ok() {
                    return Some(b);
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserve_then_not_free() {
        let mut r = Registry::new();
        assert!(r.is_free(0, 1));
        assert_eq!(r.reserve(0, 1), Ok(()));
        assert!(!r.is_free(0, 1));
    }

    #[test]
    fn alloc_returns_next_free() {
        let mut r = Registry::new();
        assert_eq!(r.alloc(0), Some(0));
        assert_eq!(r.alloc(0), Some(1));
        assert_eq!(r.reserve(0, 2), Ok(())); // occupy 2 manually
        assert_eq!(r.alloc(0), Some(3)); // alloc skips the taken slot
    }

    #[test]
    fn collision_detected() {
        let mut r = Registry::new();
        assert_eq!(r.reserve(0, 5), Ok(()));
        assert_eq!(r.reserve(0, 5), Err((0, 5)));
    }

    #[test]
    fn sets_are_independent() {
        let mut r = Registry::new();
        assert_eq!(r.reserve(0, 1), Ok(()));
        assert!(r.is_free(1, 1), "binding 1 in set 1 should be free");
        assert_eq!(r.alloc(1), Some(0), "set 1 allocates from 0");
        assert!(!r.is_free(0, 1));
        assert!(!r.is_free(1, 0));
    }

    #[test]
    fn full_set_handling() {
        let mut r = Registry::new();
        let mut got = Vec::new();
        for _ in 0..MAX_AUTO_BINDINGS_PER_SET {
            match r.alloc(3) {
                Some(b) => got.push(b),
                None => assert!(false, "alloc should succeed within range"),
            }
        }
        // filled 0..MAX, all distinct
        assert_eq!(got.len(), MAX_AUTO_BINDINGS_PER_SET as usize);
        let expected: Vec<u32> = (0..MAX_AUTO_BINDINGS_PER_SET).collect();
        assert_eq!(got, expected);
        // now the set is full within range
        assert_eq!(r.alloc(3), None);
        // other sets unaffected
        assert_eq!(r.alloc(4), Some(0));
    }

    #[test]
    fn deterministic() {
        let build = || {
            let mut r = Registry::new();
            let _ = r.reserve(0, 2);
            let _ = r.alloc(0);
            let _ = r.reserve(1, 0);
            let _ = r.alloc(0);
            r
        };
        assert_eq!(build(), build());
        assert_eq!(build().slots(), build().slots());
    }
}
