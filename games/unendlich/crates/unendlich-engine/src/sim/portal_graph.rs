//! sim::portal_graph — the dimension/portal data model + linking logic.
//!
//! Groundwork for multiple independent voxel worlds ("dimensions") joined by
//! portals. This is the pure data layer only: dimensions are opaque [`DimId`]
//! handles, a [`Portal`] is an anchored gateway in one dimension, and the
//! [`PortalGraph`] records which portals are **linked** and resolves *"if a
//! traveller enters portal A, where do they emerge?"* — a different dimension,
//! at the partner portal, stepped a little **ahead** of it (so they don't
//! instantly re-enter) and facing its outbound direction. Streaming, rendering,
//! and the actual teleport are the integration layers; this module just owns the
//! links. Deterministic (BTreeMap iteration), panic-free, std-only.

use std::collections::BTreeMap;

/// Opaque handle for an independent world instance.
pub type DimId = u32;
/// Stable id for a portal within a [`PortalGraph`].
pub type PortalId = u32;

/// How far ahead of the exit portal a traveller materialises, along the exit
/// portal's facing — enough to clear the gateway so they don't re-trigger it.
pub const EMERGE_AHEAD: f32 = 1.5;

/// A gateway anchored in one dimension.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Portal {
    pub id: PortalId,
    pub dim: DimId,
    pub pos: [f32; 3],
    /// Outbound yaw (radians, 0 = +X) a traveller faces on emerging here.
    pub facing: f32,
    /// The portal this one is paired with, if any.
    pub link: Option<PortalId>,
}

/// Where a traveller ends up after stepping through a portal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Emergence {
    pub dim: DimId,
    pub pos: [f32; 3],
    pub facing: f32,
}

/// A registry of portals and the links between them.
#[derive(Clone, Debug, Default)]
pub struct PortalGraph {
    portals: BTreeMap<PortalId, Portal>,
    next_id: PortalId,
}

impl PortalGraph {
    pub fn new() -> PortalGraph {
        PortalGraph { portals: BTreeMap::new(), next_id: 0 }
    }

    /// Anchor a new (unlinked) portal in `dim` and return its id.
    pub fn add_portal(&mut self, dim: DimId, pos: [f32; 3], facing: f32) -> PortalId {
        let id = self.next_id;
        self.next_id += 1;
        self.portals.insert(id, Portal { id, dim, pos, facing, link: None });
        id
    }

    pub fn get(&self, id: PortalId) -> Option<&Portal> {
        self.portals.get(&id)
    }

    pub fn len(&self) -> usize {
        self.portals.len()
    }
    pub fn is_empty(&self) -> bool {
        self.portals.is_empty()
    }

    /// Pair two portals bidirectionally. Any prior partner of either is unlinked
    /// first, so links stay a clean matching. Errors on a self-link or an
    /// unknown id.
    pub fn link(&mut self, a: PortalId, b: PortalId) -> Result<(), &'static str> {
        if a == b {
            return Err("cannot link a portal to itself");
        }
        if !self.portals.contains_key(&a) || !self.portals.contains_key(&b) {
            return Err("unknown portal id");
        }
        self.unlink(a);
        self.unlink(b);
        if let Some(p) = self.portals.get_mut(&a) {
            p.link = Some(b);
        }
        if let Some(p) = self.portals.get_mut(&b) {
            p.link = Some(a);
        }
        Ok(())
    }

    /// Break `id`'s link (and its partner's back-link). No-op if unlinked or
    /// unknown.
    pub fn unlink(&mut self, id: PortalId) {
        let partner = self.portals.get(&id).and_then(|p| p.link);
        if let Some(p) = self.portals.get_mut(&id) {
            p.link = None;
        }
        if let Some(bp) = partner {
            if let Some(p) = self.portals.get_mut(&bp) {
                if p.link == Some(id) {
                    p.link = None;
                }
            }
        }
    }

    /// Remove a portal entirely, unlinking its partner.
    pub fn remove_portal(&mut self, id: PortalId) -> Option<Portal> {
        self.unlink(id);
        self.portals.remove(&id)
    }

    /// Resolve stepping through portal `id`: emerge at its partner, `EMERGE_AHEAD`
    /// blocks in front of the partner along its facing, facing that direction.
    /// `None` if `id` is unknown or unlinked.
    pub fn traverse(&self, id: PortalId) -> Option<Emergence> {
        let here = self.portals.get(&id)?;
        let exit = self.portals.get(&here.link?)?;
        let (c, s) = (exit.facing.cos(), exit.facing.sin());
        let pos = [
            exit.pos[0] + c * EMERGE_AHEAD,
            exit.pos[1],
            exit.pos[2] + s * EMERGE_AHEAD,
        ];
        Some(Emergence { dim: exit.dim, pos, facing: exit.facing })
    }

    /// The nearest portal in `dim` within `max_dist` of `pos` (Euclidean),
    /// deterministic on ties (lowest id wins via BTreeMap order). Useful for
    /// "did the traveller just walk into a gateway?".
    pub fn nearest_in_dim(&self, dim: DimId, pos: [f32; 3], max_dist: f32) -> Option<PortalId> {
        let max2 = max_dist * max_dist;
        let mut best: Option<(f32, PortalId)> = None;
        for p in self.portals.values().filter(|p| p.dim == dim) {
            let d = [p.pos[0] - pos[0], p.pos[1] - pos[1], p.pos[2] - pos[2]];
            let d2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
            if d2 <= max2 && best.map_or(true, |(bd2, _)| d2 < bd2) {
                best = Some((d2, p.id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// All distinct dimensions that currently hold at least one portal, sorted.
    pub fn dimensions(&self) -> Vec<DimId> {
        let mut v: Vec<DimId> = self.portals.values().map(|p| p.dim).collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_emerges_at_the_partner_in_its_dimension() {
        let mut g = PortalGraph::new();
        let overworld: DimId = 0;
        let nether: DimId = 1;
        let a = g.add_portal(overworld, [10.0, 64.0, 10.0], 0.0); // faces +X
        let b = g.add_portal(nether, [0.0, 32.0, 0.0], std::f32::consts::FRAC_PI_2); // faces +Z
        g.link(a, b).unwrap();

        let e = g.traverse(a).expect("linked ⇒ emerges");
        assert_eq!(e.dim, nether);
        // stepped EMERGE_AHEAD ahead of b along +Z, at b's height, facing +Z
        assert!((e.pos[0] - 0.0).abs() < 1e-4);
        assert!((e.pos[1] - 32.0).abs() < 1e-4);
        assert!((e.pos[2] - EMERGE_AHEAD).abs() < 1e-4, "should step ahead of the exit, got {:?}", e.pos);
        assert!((e.facing - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
    }

    #[test]
    fn links_are_bidirectional() {
        let mut g = PortalGraph::new();
        let a = g.add_portal(0, [0.0, 0.0, 0.0], 0.0);
        let b = g.add_portal(1, [5.0, 0.0, 0.0], 0.0);
        g.link(a, b).unwrap();
        assert_eq!(g.get(a).unwrap().link, Some(b));
        assert_eq!(g.get(b).unwrap().link, Some(a));
        assert!(g.traverse(b).is_some(), "the return trip resolves too");
    }

    #[test]
    fn an_unlinked_portal_goes_nowhere() {
        let mut g = PortalGraph::new();
        let a = g.add_portal(0, [0.0, 0.0, 0.0], 0.0);
        assert!(g.traverse(a).is_none());
        assert!(g.traverse(999).is_none(), "unknown id ⇒ None");
    }

    #[test]
    fn relinking_drops_the_previous_partner() {
        let mut g = PortalGraph::new();
        let a = g.add_portal(0, [0.0, 0.0, 0.0], 0.0);
        let b = g.add_portal(1, [1.0, 0.0, 0.0], 0.0);
        let c = g.add_portal(2, [2.0, 0.0, 0.0], 0.0);
        g.link(a, b).unwrap();
        g.link(a, c).unwrap(); // a now pairs with c; b must be freed
        assert_eq!(g.get(a).unwrap().link, Some(c));
        assert_eq!(g.get(c).unwrap().link, Some(a));
        assert_eq!(g.get(b).unwrap().link, None, "old partner b must be unlinked");
    }

    #[test]
    fn self_link_and_unknown_ids_are_rejected() {
        let mut g = PortalGraph::new();
        let a = g.add_portal(0, [0.0, 0.0, 0.0], 0.0);
        assert!(g.link(a, a).is_err());
        assert!(g.link(a, 42).is_err());
    }

    #[test]
    fn removing_a_portal_unlinks_its_partner() {
        let mut g = PortalGraph::new();
        let a = g.add_portal(0, [0.0, 0.0, 0.0], 0.0);
        let b = g.add_portal(1, [1.0, 0.0, 0.0], 0.0);
        g.link(a, b).unwrap();
        g.remove_portal(a);
        assert!(g.get(a).is_none());
        assert_eq!(g.get(b).unwrap().link, None, "b's back-link must be cleared");
        assert!(g.traverse(b).is_none());
    }

    #[test]
    fn nearest_in_dim_respects_range_and_dimension() {
        let mut g = PortalGraph::new();
        let near = g.add_portal(0, [1.0, 0.0, 0.0], 0.0);
        let _far = g.add_portal(0, [50.0, 0.0, 0.0], 0.0);
        let _other_dim = g.add_portal(1, [0.5, 0.0, 0.0], 0.0); // closer, but wrong dim
        assert_eq!(g.nearest_in_dim(0, [0.0, 0.0, 0.0], 8.0), Some(near));
        assert_eq!(g.nearest_in_dim(0, [0.0, 0.0, 0.0], 0.5), None, "nothing within a tight radius");
        assert_eq!(g.nearest_in_dim(5, [0.0, 0.0, 0.0], 100.0), None, "empty dimension");
    }

    #[test]
    fn dimensions_lists_distinct_sorted() {
        let mut g = PortalGraph::new();
        g.add_portal(2, [0.0, 0.0, 0.0], 0.0);
        g.add_portal(0, [0.0, 0.0, 0.0], 0.0);
        g.add_portal(2, [1.0, 0.0, 0.0], 0.0);
        assert_eq!(g.dimensions(), vec![0, 2]);
    }

    #[test]
    fn traversal_is_deterministic() {
        let mut g = PortalGraph::new();
        let a = g.add_portal(0, [3.0, 4.0, 5.0], 1.2);
        let b = g.add_portal(1, [7.0, 8.0, 9.0], 2.4);
        g.link(a, b).unwrap();
        assert_eq!(g.traverse(a), g.traverse(a));
    }
}
