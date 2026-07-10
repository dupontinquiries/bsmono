//! `world::update_class` — classify a block update by its *source* and route it to
//! the right redraw policy.
//!
//! Not every block change deserves the same response. A player placing a block must
//! remesh the chunk *this frame* — responsiveness is the whole point. But the
//! ambient systems churn far more updates that do **not** need an immediate, full
//! remesh:
//!
//! - **Wind sway** moves foliage every single frame, but the motion is a vertex
//!   animation in the shader — it must **never** trigger a geometry remesh.
//! - **Lighting** changes (a cloud shadow, torch flicker) need a relight, not new
//!   geometry.
//! - **Plant growth** and **erosion** change geometry but can wait: coalesce many
//!   into one deferred/batched remesh instead of one remesh per event.
//! - **Fluid flow** is visible and frequent, so it wants a throttled remesh, not one
//!   per cell update.
//!
//! This composes with [`super::remesh_policy`] (which decides *whether* a given block
//! at a given LOD needs remeshing at all): this module decides *when and how* based
//! on where the update came from. Getting wind and lighting off the remesh path is
//! the FPS win — those are the highest-frequency updates and the cheapest to route
//! away from geometry work.
//!
//! Pure & deterministic.

/// Where a block update came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UpdateClass {
    /// A player placed/broke a block — must be reflected immediately.
    PlayerEdit,
    /// A builder dragon / structure system changed blocks — near-immediate.
    StructureChange,
    /// Fluid (water/lava) advanced — visible, frequent → throttled remesh.
    FluidFlow,
    /// A plant grew / decayed — geometry change, but deferrable.
    Growth,
    /// Terrain erosion/weathering — deferrable, low priority, batchable.
    Erosion,
    /// Foliage wind sway — animated in the shader, no geometry change.
    WindSway,
    /// Lighting-only change (shadow, torch) — relight, no geometry change.
    LightingOnly,
}

/// How the renderer should respond to an update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemeshPolicy {
    /// Remesh this frame at top priority.
    Immediate,
    /// Coalesce and remesh within `max_delay_ticks`.
    Deferred { max_delay_ticks: u32 },
    /// Remesh on a periodic batch every `interval_ticks`.
    Batched { interval_ticks: u32 },
    /// No geometry remesh — the effect lives in the shader (wind vertex animation).
    ShaderOnly,
    /// No geometry remesh — a lighting pass handles it.
    RelightOnly,
}

impl RemeshPolicy {
    /// Geometry-remesh urgency rank (higher = sooner). The two no-remesh policies
    /// share rank 0 so they never pull a chunk onto the remesh path.
    pub const fn remesh_urgency(self) -> u8 {
        match self {
            RemeshPolicy::Immediate => 4,
            RemeshPolicy::Deferred { .. } => 3,
            RemeshPolicy::Batched { .. } => 2,
            RemeshPolicy::ShaderOnly | RemeshPolicy::RelightOnly => 0,
        }
    }
    /// Does this policy require rebuilding chunk geometry at all?
    pub const fn needs_geometry(self) -> bool {
        self.remesh_urgency() > 0
    }
}

impl UpdateClass {
    /// The redraw policy for this update source.
    pub const fn policy(self) -> RemeshPolicy {
        match self {
            UpdateClass::PlayerEdit => RemeshPolicy::Immediate,
            UpdateClass::StructureChange => RemeshPolicy::Immediate,
            UpdateClass::FluidFlow => RemeshPolicy::Deferred { max_delay_ticks: 2 },
            UpdateClass::Growth => RemeshPolicy::Deferred { max_delay_ticks: 20 },
            UpdateClass::Erosion => RemeshPolicy::Batched { interval_ticks: 60 },
            UpdateClass::WindSway => RemeshPolicy::ShaderOnly,
            UpdateClass::LightingOnly => RemeshPolicy::RelightOnly,
        }
    }

    /// Does an update from this source require a geometry remesh?
    pub const fn needs_geometry_remesh(self) -> bool {
        self.policy().needs_geometry()
    }

    /// Remesh-queue priority (higher = sooner). Mirrors the policy urgency but keeps
    /// player edits strictly above structure changes for input responsiveness.
    pub const fn priority(self) -> u8 {
        match self {
            UpdateClass::PlayerEdit => 5,
            UpdateClass::StructureChange => 4,
            UpdateClass::FluidFlow => 3,
            UpdateClass::Growth => 2,
            UpdateClass::Erosion => 1,
            UpdateClass::WindSway | UpdateClass::LightingOnly => 0,
        }
    }
}

/// Coalesce all updates pending on one chunk into a single effective policy: the
/// most urgent geometry policy wins, so a wind-only chunk stays off the remesh path
/// but a single player edit among a thousand sway updates still forces an immediate
/// remesh. Returns `None` if *no* update needs geometry (the chunk is redrawn
/// purely by the shader / relight path — the common, cheap case).
pub fn effective_policy(updates: &[UpdateClass]) -> Option<RemeshPolicy> {
    updates
        .iter()
        .map(|u| u.policy())
        .filter(|p| p.needs_geometry())
        .max_by_key(|p| p.remesh_urgency())
}

/// Whether a chunk with these pending updates needs any geometry remesh at all.
pub fn needs_remesh(updates: &[UpdateClass]) -> bool {
    updates.iter().any(|u| u.needs_geometry_remesh())
}

/// Whether a chunk needs a relight pass (any lighting-only update present, or any
/// geometry remesh which relights as a side effect).
pub fn needs_relight(updates: &[UpdateClass]) -> bool {
    updates
        .iter()
        .any(|u| matches!(u, UpdateClass::LightingOnly) || u.needs_geometry_remesh())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wind_and_lighting_never_remesh() {
        assert!(!UpdateClass::WindSway.needs_geometry_remesh());
        assert!(!UpdateClass::LightingOnly.needs_geometry_remesh());
        assert_eq!(UpdateClass::WindSway.policy(), RemeshPolicy::ShaderOnly);
        assert_eq!(UpdateClass::LightingOnly.policy(), RemeshPolicy::RelightOnly);
    }

    #[test]
    fn player_and_structure_are_immediate() {
        assert_eq!(UpdateClass::PlayerEdit.policy(), RemeshPolicy::Immediate);
        assert_eq!(UpdateClass::StructureChange.policy(), RemeshPolicy::Immediate);
        assert!(UpdateClass::PlayerEdit.needs_geometry_remesh());
    }

    #[test]
    fn ambient_geometry_is_deferred_or_batched() {
        assert!(matches!(UpdateClass::Growth.policy(), RemeshPolicy::Deferred { .. }));
        assert!(matches!(UpdateClass::Erosion.policy(), RemeshPolicy::Batched { .. }));
        // Growth waits longer than fluid.
        let g = match UpdateClass::Growth.policy() {
            RemeshPolicy::Deferred { max_delay_ticks } => max_delay_ticks,
            _ => 0,
        };
        let f = match UpdateClass::FluidFlow.policy() {
            RemeshPolicy::Deferred { max_delay_ticks } => max_delay_ticks,
            _ => 0,
        };
        assert!(g > f, "growth is more deferrable than fluid: {g} vs {f}");
    }

    #[test]
    fn wind_only_chunk_stays_off_the_remesh_path() {
        // A whole chunk of swaying foliage + a cloud shadow: no geometry remesh.
        let updates = [UpdateClass::WindSway, UpdateClass::WindSway, UpdateClass::LightingOnly];
        assert!(!needs_remesh(&updates), "shader/relight only — no remesh");
        assert_eq!(effective_policy(&updates), None);
        assert!(needs_relight(&updates), "but it does need a relight");
    }

    #[test]
    fn one_player_edit_among_sway_forces_immediate() {
        let mut updates = vec![UpdateClass::WindSway; 1000];
        updates.push(UpdateClass::PlayerEdit);
        updates.push(UpdateClass::Growth);
        assert!(needs_remesh(&updates));
        assert_eq!(effective_policy(&updates), Some(RemeshPolicy::Immediate));
    }

    #[test]
    fn coalesce_picks_most_urgent_geometry_policy() {
        // Growth + erosion (no player edit) → the deferred (growth) wins over batched.
        let updates = [UpdateClass::Erosion, UpdateClass::Growth, UpdateClass::WindSway];
        assert_eq!(effective_policy(&updates), Some(RemeshPolicy::Deferred { max_delay_ticks: 20 }));
    }

    #[test]
    fn priority_orders_player_above_structure_above_ambient() {
        assert!(UpdateClass::PlayerEdit.priority() > UpdateClass::StructureChange.priority());
        assert!(UpdateClass::StructureChange.priority() > UpdateClass::Growth.priority());
        assert!(UpdateClass::Growth.priority() > UpdateClass::Erosion.priority());
        assert_eq!(UpdateClass::WindSway.priority(), 0);
    }

    #[test]
    fn empty_updates_need_nothing() {
        assert!(!needs_remesh(&[]));
        assert!(!needs_relight(&[]));
        assert_eq!(effective_policy(&[]), None);
    }
}
