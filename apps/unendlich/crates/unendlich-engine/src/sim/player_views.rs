//! `sim::player_views` — the dynamic camera/view layer for local splitscreen.
//!
//! Three jobs, all pure (the renderer does the GPU work; this decides geometry):
//!   1. **Per-player view IDs** — a stable `ViewId` per active player so the
//!      renderer can tag its per-player passes/resources.
//!   2. **Animated layout** — when a player joins/leaves, the split rectangles
//!      transition (slide/blend) from their current position to the new
//!      [`viewport_split::layout`] target over a short duration, instead of
//!      snapping. `step(dt)` advances the blend; the renderer reads the current
//!      (interpolated) viewport each frame and can cross-fade the two framebuffers.
//!   3. **Overlap detection** — given each view's frustum planes and a candidate
//!      set of chunk AABBs, compute which chunks are visible to ≥2 players. Those
//!      are culled/meshed once and shared (the players share one GPU context), so
//!      the renderer's total work is the *union*, not the sum, of the views.

use crate::sim::chunk_frustum_cull::{cull, Aabb};
use crate::sim::viewport_split::{layout, SplitPref, Viewport};

pub type ViewId = u32;

/// Default transition duration (seconds) for a join/leave layout change.
pub const DEFAULT_ANIM_SECS: f32 = 0.35;

/// One player's rendering view: a stable id and its animated viewport rect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerView {
    pub id: ViewId,
    /// Current (interpolated) rect — what the renderer scissors/blends to this frame.
    pub viewport: Viewport,
    /// The layout target the rect is animating toward.
    pub target: Viewport,
    /// Where this rect started the current transition (for the renderer's blend).
    pub from: Viewport,
}

/// The registry of active player views + the running layout animation.
#[derive(Clone, Debug)]
pub struct ViewRegistry {
    next_id: ViewId,
    views: Vec<PlayerView>,
    pref: SplitPref,
    aspect: f32,
    /// Transition progress 0..1 (1 = settled at target).
    anim_t: f32,
    anim_secs: f32,
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    crate::util::mathx::lerp(a, b, t) // r342 dedup
}
fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
fn lerp_vp(a: Viewport, b: Viewport, t: f32) -> Viewport {
    Viewport {
        x: lerp(a.x, b.x, t),
        y: lerp(a.y, b.y, t),
        w: lerp(a.w, b.w, t),
        h: lerp(a.h, b.h, t),
    }
}

impl ViewRegistry {
    pub fn new(aspect: f32, pref: SplitPref) -> ViewRegistry {
        ViewRegistry {
            next_id: 1,
            views: Vec::new(),
            pref,
            aspect: aspect.max(0.01),
            anim_t: 1.0,
            anim_secs: DEFAULT_ANIM_SECS,
        }
    }

    pub fn with_anim_secs(mut self, secs: f32) -> Self {
        self.anim_secs = secs.max(0.0);
        self
    }

    pub fn player_count(&self) -> usize {
        self.views.len()
    }
    pub fn views(&self) -> &[PlayerView] {
        &self.views
    }
    pub fn ids(&self) -> Vec<ViewId> {
        self.views.iter().map(|v| v.id).collect()
    }
    pub fn animating(&self) -> bool {
        self.anim_t < 1.0
    }
    pub fn viewport_of(&self, id: ViewId) -> Option<Viewport> {
        self.views.iter().find(|v| v.id == id).map(|v| v.viewport)
    }

    /// Add a local player (clamped to 4). Returns the new `ViewId` and starts the
    /// join transition: the new view slides in from full-screen, the others shrink
    /// to their new slots.
    pub fn add_player(&mut self) -> ViewId {
        if self.views.len() >= 4 {
            // Already full — hand back the last id rather than over-allocating.
            return self.views.last().map(|v| v.id).unwrap_or(0);
        }
        let id = self.next_id;
        self.next_id += 1;
        // The newcomer starts full-screen (a blend/zoom-in); existing views start
        // from where they currently are.
        self.views.push(PlayerView {
            id,
            viewport: Viewport { x: 0.0, y: 0.0, w: 1.0, h: 1.0 },
            target: Viewport { x: 0.0, y: 0.0, w: 1.0, h: 1.0 },
            from: Viewport { x: 0.0, y: 0.0, w: 1.0, h: 1.0 },
        });
        self.relayout();
        id
    }

    /// Remove a player by id. Starts a transition to the smaller layout.
    pub fn remove_player(&mut self, id: ViewId) -> bool {
        let before = self.views.len();
        self.views.retain(|v| v.id != id);
        if self.views.len() != before {
            self.relayout();
            true
        } else {
            false
        }
    }

    /// Window aspect changed (resize). Re-target without resetting the blend if
    /// already settled (an instant re-fit), else fold into the running transition.
    pub fn set_aspect(&mut self, aspect: f32) {
        self.aspect = aspect.max(0.01);
        let settled = !self.animating();
        self.retarget();
        if settled {
            // snap: no animation for a resize
            self.anim_t = 1.0;
            for v in &mut self.views {
                v.viewport = v.target;
                v.from = v.target;
            }
        }
    }

    /// Recompute targets and (re)start the transition from the current rects.
    fn relayout(&mut self) {
        self.retarget();
        self.anim_t = if self.anim_secs <= 0.0 { 1.0 } else { 0.0 };
        for v in &mut self.views {
            v.from = v.viewport;
        }
        if self.anim_t >= 1.0 {
            for v in &mut self.views {
                v.viewport = v.target;
                v.from = v.target;
            }
        }
    }

    /// Assign each view its layout-target rect (does not touch the blend).
    fn retarget(&mut self) {
        let target = layout(self.views.len().clamp(1, 4) as u8, self.aspect, self.pref);
        for (i, v) in self.views.iter_mut().enumerate() {
            v.target = target.get(i).copied().unwrap_or(v.target);
        }
    }

    /// Advance the join/leave transition and interpolate each view's viewport.
    pub fn step(&mut self, dt: f32) {
        if self.anim_t >= 1.0 {
            return;
        }
        let rate = if self.anim_secs > 0.0 { dt / self.anim_secs } else { 1.0 };
        self.anim_t = (self.anim_t + rate).min(1.0);
        let e = smoothstep(self.anim_t);
        for v in &mut self.views {
            v.viewport = lerp_vp(v.from, v.target, e);
        }
        if self.anim_t >= 1.0 {
            for v in &mut self.views {
                v.viewport = v.target;
                v.from = v.target;
            }
        }
    }
}

/// The result of overlap analysis across all active views.
#[derive(Clone, Debug, PartialEq)]
pub struct VisibilityOverlap {
    /// Visible candidate indices per view (same order as the `view_planes` slice).
    pub per_view: Vec<Vec<usize>>,
    /// Candidates visible to ≥2 views — cull/mesh once, share across those players.
    pub shared: Vec<usize>,
    /// Unique candidates visible to ANY view — the renderer's real work set (sum of
    /// per-view counts would double-count the shared ones).
    pub union_count: usize,
    /// `shared / union` in 0..1 — how much per-view work collapses into shared work.
    /// High when players look at the same place (co-op), ~0 when they're far apart.
    pub overlap_ratio: f32,
}

/// Compute the shared/union visibility across views. `view_planes[i]` is view i's
/// six frustum planes (as produced by `frustum_plane_extract::extract`, each plane
/// as `[a,b,c,d]`); `candidates` is the chunk-AABB set to test. Pure; no GPU.
pub fn compute_overlap(view_planes: &[[[f32; 4]; 6]], candidates: &[Aabb]) -> VisibilityOverlap {
    let n = candidates.len();
    // How many views see each candidate.
    let mut seen_by = vec![0u32; n];
    let mut per_view: Vec<Vec<usize>> = Vec::with_capacity(view_planes.len());
    for planes in view_planes {
        let mut vis = Vec::new();
        cull(planes, candidates, &mut vis);
        for &idx in &vis {
            if idx < n {
                seen_by[idx] += 1;
            }
        }
        per_view.push(vis);
    }
    let mut shared = Vec::new();
    let mut union_count = 0usize;
    for (idx, &c) in seen_by.iter().enumerate() {
        if c >= 1 {
            union_count += 1;
        }
        if c >= 2 {
            shared.push(idx);
        }
    }
    let overlap_ratio = if union_count > 0 {
        shared.len() as f32 / union_count as f32
    } else {
        0.0
    };
    VisibilityOverlap { per_view, shared, union_count, overlap_ratio }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn ids_are_stable_and_sequential() {
        let mut r = ViewRegistry::new(16.0 / 9.0, SplitPref::Auto);
        let a = r.add_player();
        let b = r.add_player();
        assert_eq!(r.player_count(), 2);
        assert_ne!(a, b);
        assert_eq!(r.ids(), vec![a, b]);
        // removing the first keeps the second's id stable
        assert!(r.remove_player(a));
        assert_eq!(r.ids(), vec![b]);
        assert!(r.viewport_of(b).is_some());
        assert!(r.viewport_of(a).is_none());
    }

    #[test]
    fn join_animates_from_fullscreen_to_split_then_settles() {
        let mut r = ViewRegistry::new(16.0 / 9.0, SplitPref::Columns);
        r.add_player(); // single → full screen, settles immediately (1-up target == full)
        r.step(1.0);
        let id2 = r.add_player(); // now a 2-up split begins animating
        assert!(r.animating(), "join starts a transition");
        // mid-transition the new view is between full-screen and its split slot
        r.step(DEFAULT_ANIM_SECS * 0.5);
        let mid = r.viewport_of(id2).unwrap();
        assert!(mid.w < 1.0 && mid.w > 0.5, "shrinking in from full, got w={}", mid.w);
        // finish
        r.step(DEFAULT_ANIM_SECS);
        assert!(!r.animating());
        let settled = r.viewport_of(id2).unwrap();
        // 2-up columns → right strip is x=0.5,w=0.5
        assert!(approx(settled.w, 0.5) && approx(settled.x, 0.5), "settled at split slot: {:?}", settled);
    }

    #[test]
    fn full_house_clamps_at_four() {
        let mut r = ViewRegistry::new(1.0, SplitPref::Auto);
        for _ in 0..6 {
            r.add_player();
        }
        assert_eq!(r.player_count(), 4, "clamped to 4 local players");
    }

    #[test]
    fn overlap_shared_union_and_ratio() {
        use crate::sim::frustum_plane_extract::extract;
        // Two identical views (same camera) → everything they see is shared.
        // Build a trivial ortho-ish viewproj that keeps a unit box in front visible.
        // Easiest deterministic check: identical planes ⇒ per-view sets equal ⇒
        // shared == union.
        let vp: [[f32; 4]; 4] = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let planes_struct = extract(&vp);
        let planes: [[f32; 4]; 6] = std::array::from_fn(|i| {
            [planes_struct[i].a, planes_struct[i].b, planes_struct[i].c, planes_struct[i].d]
        });
        let boxes = vec![
            Aabb { min: [-0.5, -0.5, -0.5], max: [0.5, 0.5, 0.5] },
            Aabb { min: [-0.2, -0.2, -0.2], max: [0.2, 0.2, 0.2] },
        ];
        let two_same = [planes, planes];
        let r = compute_overlap(&two_same, &boxes);
        assert_eq!(r.per_view.len(), 2);
        // whatever is visible is visible to BOTH identical views ⇒ all shared
        assert_eq!(r.shared.len(), r.union_count);
        if r.union_count > 0 {
            assert!(approx(r.overlap_ratio, 1.0), "identical views fully overlap");
        }
        // a single view never has 'shared' (needs >=2 viewers)
        let one = [planes];
        let r1 = compute_overlap(&one, &boxes);
        assert!(r1.shared.is_empty());
    }

    #[test]
    fn empty_registry_and_zero_candidates_are_safe() {
        let r = ViewRegistry::new(1.0, SplitPref::Auto);
        assert_eq!(r.player_count(), 0);
        let ov = compute_overlap(&[], &[]);
        assert_eq!(ov.union_count, 0);
        assert_eq!(ov.overlap_ratio, 0.0);
    }
}

// ── service_registry wiring (S2) ───────────────────────────────────
// Wires player_views' ViewRegistry (split-screen view layout). Neutral:
// no players registered, so step() just advances the (settled) animation.
use crate::sim::service_registry::{Service, ServiceCtx, ServiceEntry, SERVICES};
use linkme::distributed_slice;

struct PlayerViewsService(ViewRegistry);
impl Service for PlayerViewsService {
    fn name(&self) -> &'static str {
        "player_views"
    }
    fn tick(&mut self, ctx: &ServiceCtx) {
        self.0.step(ctx.dt);
    }
}
#[distributed_slice(SERVICES)]
static PLAYER_VIEWS_SERVICE: ServiceEntry = ServiceEntry {
    name: "player_views",
    make: || Box::new(PlayerViewsService(ViewRegistry::new(16.0 / 9.0, SplitPref::Auto))),
};
