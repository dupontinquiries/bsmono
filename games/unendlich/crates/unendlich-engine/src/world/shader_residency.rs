//! Shader-variant residency — the manager for splitting the monolithic
//! `chunk_mesh.comp` into several specialized compute pipelines and loading them
//! on demand. Candidates to split out: opaque vs transparent/water vs foliage
//! meshing, greedy vs naive, and per-LOD variants. The shared parts (the
//! `block_base_color` LUT, the per-face brightness table) become a common
//! include; only the meshing core differs per variant.
//!
//! Only so many pipelines fit the GPU's resident budget at once, so this keeps a
//! **bounded resident set**: requesting a variant that's loaded is a hit (the
//! mesher can dispatch it this frame); a miss enqueues an upload — and, if at
//! capacity, evicts the least-recently-used variant and enqueues its unload. The
//! engine drains [`ShaderResidency::drain_events`] each frame and performs the
//! actual `VkPipeline` create/destroy. This is pure CPU bookkeeping so it's
//! testable; nothing here touches Vulkan.

/// Identifies a compute-shader variant (the engine maps these to pipelines).
pub type VariantId = u32;

/// Work the engine must perform to honour the resident set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShaderEvent {
    /// Create/upload this variant's pipeline.
    Load(VariantId),
    /// Destroy/free this variant's pipeline (made room).
    Unload(VariantId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Slot {
    id: VariantId,
    last_used: u64,
}

/// A bounded LRU resident set of shader variants.
#[derive(Debug, Clone)]
pub struct ShaderResidency {
    capacity: usize,
    resident: Vec<Slot>,
    clock: u64,
    pending: Vec<ShaderEvent>,
}

impl ShaderResidency {
    pub fn new(capacity: usize) -> ShaderResidency {
        ShaderResidency { capacity: capacity.max(1), resident: Vec::new(), clock: 0, pending: Vec::new() }
    }

    pub fn is_resident(&self, v: VariantId) -> bool {
        self.resident.iter().any(|s| s.id == v)
    }

    pub fn resident_count(&self) -> usize {
        self.resident.len()
    }

    /// Request a variant for this frame. Returns `true` if it's already resident
    /// (dispatch now); `false` if it had to be scheduled for load (the mesher
    /// should defer that brick/chunk a frame until the pipeline is ready).
    /// On a miss at capacity, evicts the least-recently-used variant.
    pub fn request(&mut self, v: VariantId) -> bool {
        self.clock += 1;
        if let Some(slot) = self.resident.iter_mut().find(|s| s.id == v) {
            slot.last_used = self.clock; // keep it warm
            return true;
        }
        // Miss. Make room if needed.
        if self.resident.len() >= self.capacity {
            // Evict LRU (deterministic tie-break: lowest id among oldest).
            let victim = self
                .resident
                .iter()
                .enumerate()
                .min_by_key(|(_, s)| (s.last_used, s.id))
                .map(|(i, s)| (i, s.id))
                .unwrap();
            self.resident.remove(victim.0);
            self.pending.push(ShaderEvent::Unload(victim.1));
        }
        self.resident.push(Slot { id: v, last_used: self.clock });
        self.pending.push(ShaderEvent::Load(v));
        false
    }

    /// Take the queued load/unload work for the engine to perform, clearing it.
    pub fn drain_events(&mut self) -> Vec<ShaderEvent> {
        std::mem::take(&mut self.pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_request_schedules_a_load_and_is_not_ready_yet() {
        let mut r = ShaderResidency::new(3);
        assert!(!r.request(7), "cold variant isn't ready this frame");
        assert!(r.is_resident(7));
        assert_eq!(r.drain_events(), vec![ShaderEvent::Load(7)]);
        // Now it's warm.
        assert!(r.request(7), "resident now → ready");
        assert!(r.drain_events().is_empty(), "no new work for a hit");
    }

    #[test]
    fn capacity_evicts_least_recently_used() {
        let mut r = ShaderResidency::new(2);
        r.request(1);
        r.request(2);
        r.drain_events();
        r.request(1); // touch 1 so 2 is now the LRU
        // Requesting a third evicts 2, not 1.
        assert!(!r.request(3));
        assert!(r.is_resident(1) && r.is_resident(3) && !r.is_resident(2));
        let ev = r.drain_events();
        assert!(ev.contains(&ShaderEvent::Unload(2)), "evicted the LRU: {ev:?}");
        assert!(ev.contains(&ShaderEvent::Load(3)));
    }

    #[test]
    fn resident_set_never_exceeds_capacity() {
        let mut r = ShaderResidency::new(3);
        for v in 0..20 {
            r.request(v);
            assert!(r.resident_count() <= 3, "bounded resident set");
        }
    }

    #[test]
    fn drain_clears_the_queue() {
        let mut r = ShaderResidency::new(4);
        r.request(5);
        assert_eq!(r.drain_events().len(), 1);
        assert!(r.drain_events().is_empty(), "second drain is empty");
    }
}
