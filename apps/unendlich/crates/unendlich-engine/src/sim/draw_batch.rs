//! `sim::draw_batch` — pure instanced-draw grouping (WID `hh-r332-t2`).
//!
//! The r331 M2 profile shows the `record` pass at p50 4.0 / p90 8.3 ms with
//! 100 % of frames over 1 ms — far too high for the v0.20 indirect path's 3–4
//! GPU calls/frame, so the cost is **per-instance CPU** in the record loop:
//! today the fallback issues one `cmd_draw_indexed(.., instance_count = 1, ..)`
//! per mob/foliage/overlay instance. This core groups a frame's instanced draws
//! by `(pipeline_key, mesh_id)` so the loop can emit **one** instanced draw per
//! group (`instance_count = N`) instead of one per instance.
//!
//! It reuses the key representation the existing draw cores already standardize
//! on — the `pipeline` field is a `crate::pipeline_key`-encoded `u64`, exactly
//! the `pipeline: u64` that `crate::draw_key::draw_key` consumes — so this is a
//! grouping layer over those keys, not a restructuring of them. Kept
//! dependency-free (pipeline treated as an opaque `u64`) so it gates standalone.
//!
//! **Order:** grouping is stable — groups appear in first-appearance order and
//! instances keep their relative order within a group, so a caller that already
//! sorted draws front-to-back (via `draw_key`) keeps that order. Collapsing
//! same-(pipeline, mesh) instances into one instanced call is render-equivalent
//! for opaque instanced geometry: same triangles, fewer record calls.
//!
//! Pure, deterministic, panic-free: no IO/clock/RNG, no `.unwrap()`/`.expect()`.

use std::collections::BTreeMap;

/// One queued instanced draw before grouping — one per instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawItem {
    /// Pipeline-state key (a `crate::pipeline_key::encode(..)` result). Primary
    /// group key — different pipeline state cannot share an instanced call.
    pub pipeline: u64,
    /// Mesh / geometry id. Secondary group key — an instanced draw replays one
    /// mesh `N` times, so instances must share a mesh.
    pub mesh_id: u32,
    /// Opaque per-instance id (its slot in the instance buffer). Carried through
    /// in stable order so the bin can gather a group's instances; never splits a
    /// group.
    pub instance: u32,
}

/// A collapsed instanced draw: one record-time `cmd_draw_indexed` covering all
/// `instances`, replacing one call per instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrawGroup {
    pub pipeline: u64,
    pub mesh_id: u32,
    /// The grouped instance ids, in first-seen (stable) order.
    pub instances: Vec<u32>,
}

impl DrawGroup {
    /// Instance count for the single instanced draw call (`>= 1`).
    pub fn instance_count(&self) -> u32 {
        self.instances.len() as u32
    }
}

/// Group instanced draws by `(pipeline, mesh_id)`, preserving first-appearance
/// group order and within-group instance order (a stable group-by — it does
/// **not** re-sort, so the caller's existing front-to-back order is kept).
///
/// `[A, B, A]` ⇒ `[A{0,2}, B{1}]`: render-equivalent for instanced geometry.
pub fn group(draws: &[DrawItem]) -> Vec<DrawGroup> {
    let mut groups: Vec<DrawGroup> = Vec::new();
    // (pipeline, mesh_id) -> index into `groups`. BTreeMap (not HashMap) keeps
    // the lookup free of any RandomState/RNG; output order is driven purely by
    // first appearance in `draws`, so it is fully deterministic.
    let mut index: BTreeMap<(u64, u32), usize> = BTreeMap::new();
    for it in draws {
        let key = (it.pipeline, it.mesh_id);
        match index.get(&key) {
            Some(&g) => groups[g].instances.push(it.instance),
            None => {
                index.insert(key, groups.len());
                groups.push(DrawGroup {
                    pipeline: it.pipeline,
                    mesh_id: it.mesh_id,
                    instances: vec![it.instance],
                });
            }
        }
    }
    groups
}

/// Number of distinct `(pipeline, mesh_id)` groups — i.e. the instanced draw
/// calls remaining after grouping. Equal to `group(draws).len()` but without
/// building the instance lists.
pub fn group_count(draws: &[DrawItem]) -> usize {
    let mut keys: BTreeMap<(u64, u32), ()> = BTreeMap::new();
    for it in draws {
        keys.insert((it.pipeline, it.mesh_id), ());
    }
    keys.len()
}

/// Before/after record-call accounting for the grouping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GroupingSavings {
    /// Draw items in (one record call each, pre-grouping).
    pub instances: usize,
    /// Instanced groups out (one record call each, post-grouping).
    pub groups: usize,
    /// Record calls eliminated (`instances - groups`, saturating).
    pub calls_saved: usize,
}

/// Compute the record-call savings grouping would yield for this frame.
pub fn savings(draws: &[DrawItem]) -> GroupingSavings {
    let instances = draws.len();
    let groups = group_count(draws);
    GroupingSavings {
        instances,
        groups,
        calls_saved: instances.saturating_sub(groups),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(pipeline: u64, mesh_id: u32, instance: u32) -> DrawItem {
        DrawItem { pipeline, mesh_id, instance }
    }

    #[test]
    fn collapses_same_key_instances_into_one_group() {
        let draws = [
            item(7, 3, 0),
            item(7, 3, 1),
            item(7, 3, 2),
            item(7, 3, 3),
            item(7, 3, 4),
        ];
        let g = group(&draws);
        assert_eq!(g.len(), 1, "five same-key instances → one group");
        assert_eq!(g[0].instance_count(), 5);
        assert_eq!(g[0].instances, vec![0, 1, 2, 3, 4]);
        assert_eq!(g[0].pipeline, 7);
        assert_eq!(g[0].mesh_id, 3);
    }

    #[test]
    fn distinct_keys_stay_split() {
        // differ by pipeline, by mesh, and both.
        let draws = [
            item(1, 10, 0),
            item(2, 10, 1), // different pipeline
            item(1, 11, 2), // different mesh
            item(2, 11, 3), // both different
        ];
        let g = group(&draws);
        assert_eq!(g.len(), 4, "all keys distinct → no collapsing");
        for grp in &g {
            assert_eq!(grp.instance_count(), 1);
        }
    }

    #[test]
    fn grouping_is_order_stable() {
        // A B A : the second A joins the first group; group order is first-seen.
        let draws = [item(5, 1, 100), item(9, 1, 200), item(5, 1, 300)];
        let g = group(&draws);
        assert_eq!(g.len(), 2);
        // group order = first appearance: A (pipeline 5) then B (pipeline 9).
        assert_eq!((g[0].pipeline, g[0].mesh_id), (5, 1));
        assert_eq!((g[1].pipeline, g[1].mesh_id), (9, 1));
        // within the A group, instance order is preserved.
        assert_eq!(g[0].instances, vec![100, 300]);
        assert_eq!(g[1].instances, vec![200]);
    }

    #[test]
    fn empty_input_yields_empty() {
        assert!(group(&[]).is_empty());
        assert_eq!(group_count(&[]), 0);
        let s = savings(&[]);
        assert_eq!(s, GroupingSavings { instances: 0, groups: 0, calls_saved: 0 });
    }

    #[test]
    fn savings_count_is_correct() {
        // 6 instances across 2 groups (4 of key A, 2 of key B) → save 4 calls.
        let draws = [
            item(1, 1, 0),
            item(1, 1, 1),
            item(2, 2, 2),
            item(1, 1, 3),
            item(2, 2, 4),
            item(1, 1, 5),
        ];
        let s = savings(&draws);
        assert_eq!(s.instances, 6);
        assert_eq!(s.groups, 2);
        assert_eq!(s.calls_saved, 4);
        // savings.groups must agree with the actual grouping + group_count.
        assert_eq!(s.groups, group(&draws).len());
        assert_eq!(s.groups, group_count(&draws));
    }

    #[test]
    fn deterministic() {
        let draws = [
            item(3, 7, 0),
            item(3, 8, 1),
            item(3, 7, 2),
            item(4, 7, 3),
            item(3, 8, 4),
        ];
        assert_eq!(group(&draws), group(&draws));
        assert_eq!(savings(&draws), savings(&draws));
        // total instances are conserved across the grouping (nothing dropped).
        let total: usize = group(&draws).iter().map(|g| g.instances.len()).sum();
        assert_eq!(total, draws.len(), "every instance lands in exactly one group");
    }

    #[test]
    fn group_count_matches_group_len_on_fuzz() {
        // simple deterministic LCG to vary keys; group_count must equal group().len().
        let mut seed = 0x1234_5678u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed
        };
        for _ in 0..50 {
            let n = (next() % 40) as usize;
            let draws: Vec<DrawItem> = (0..n)
                .map(|i| {
                    let p = (next() % 5) as u64;
                    let m = (next() % 5) as u32;
                    item(p, m, i as u32)
                })
                .collect();
            assert_eq!(group_count(&draws), group(&draws).len());
            // and the savings accounting stays consistent.
            let s = savings(&draws);
            assert_eq!(s.calls_saved, s.instances - s.groups);
        }
    }
}
