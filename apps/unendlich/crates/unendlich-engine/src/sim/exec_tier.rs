//! Exec tier — the deterministic CPU/GPU boundary for sim ticks.
//!
//! The engine's sim is fixed-point and rollback-/netcode-deterministic. GPU
//! compute is float and is **not** reproducible across vendors, and reading it
//! back stalls the frame — so anything whose state the rollback or persistence
//! layer depends on (a tree's collidable wood, a tamed dragon's brain, a tracked
//! mob's position) **must stay on the CPU**. Moving *that* to the GPU silently
//! breaks determinism.
//!
//! But most of what's eating the CPU and spiking the frame isn't authoritative:
//! it's the *bulk cosmetic layer* — swaying grass and leaves, distant ambient
//! critters flocking, fog. None of it feeds back into the sim, so it can be
//! computed (and meshed) entirely GPU-side, never touching the CPU apply path or
//! the upload queue. Offloading it doesn't *compete* with the upload queue — it
//! *removes* work from it, which is how it lands "consistently."
//!
//! This module is the pure decision: given what an entity is and how far away it
//! is, [`exec_tier`] returns where its tick runs. The GPU passes for the
//! [`ExecTier::GpuApprox`] set live on the shaders thread; this is the contract
//! that tells them exactly which entities they own — and, just as importantly,
//! which they must never touch.

/// What an entity is, for the determinism boundary. The split is *authoritative*
/// (rollback/persist/collision depends on it) vs *cosmetic* (purely visual).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EntityClass {
    /// Trees / wood — collidable and persisted. Authoritative.
    StructuralPlant,
    /// Grass, leaves, flowers — decorative. Cosmetic.
    CosmeticFlora,
    /// A tracked / interactable / tamed creature. Authoritative.
    GameplayMob,
    /// Distant ambient wildlife (background flocks). Cosmetic.
    AmbientMob,
}

impl EntityClass {
    /// Authoritative entities never leave the CPU — rollback and persistence
    /// depend on their exact fixed-point state.
    pub fn is_authoritative(self) -> bool {
        matches!(self, EntityClass::StructuralPlant | EntityClass::GameplayMob)
    }
}

/// Where an entity's tick runs this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExecTier {
    /// Full fixed-point CPU sim — authoritative, rollback-safe.
    CpuFull,
    /// GPU-approximated (cosmetic only): grown/animated and meshed GPU-side, not
    /// persisted, not rollback-critical, off the CPU apply + upload path.
    GpuApprox,
    /// Out of range — skip entirely this frame.
    Dormant,
}

impl ExecTier {
    pub fn runs_on_cpu(self) -> bool {
        self == ExecTier::CpuFull
    }
    pub fn runs_on_gpu(self) -> bool {
        self == ExecTier::GpuApprox
    }
}

/// Cutoffs (Chebyshev block distance from the player) for the offload bands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TierPolicy {
    /// At/within this, cosmetic entities still run full CPU (close, few, accurate).
    pub near_cutoff: i32,
    /// Beyond this, nothing ticks at all (Dormant).
    pub active_cutoff: i32,
}

impl Default for TierPolicy {
    fn default() -> Self {
        TierPolicy { near_cutoff: 16, active_cutoff: 256 }
    }
}

/// Decide where this entity's tick runs. Pure and deterministic.
///
/// - Beyond `active_cutoff` → [`ExecTier::Dormant`] (nothing ticks that far out).
/// - **Authoritative** (structural plants, gameplay mobs) → always
///   [`ExecTier::CpuFull`] in range. *Never* GPU — that is the determinism line.
/// - **Cosmetic** (flora, ambient mobs) → [`ExecTier::CpuFull`] when near (cheap,
///   accurate), else [`ExecTier::GpuApprox`] — the offloaded bulk.
pub fn exec_tier(class: EntityClass, dist_blocks: i32, policy: &TierPolicy) -> ExecTier {
    if dist_blocks > policy.active_cutoff {
        return ExecTier::Dormant;
    }
    if class.is_authoritative() {
        return ExecTier::CpuFull;
    }
    if dist_blocks <= policy.near_cutoff {
        ExecTier::CpuFull
    } else {
        ExecTier::GpuApprox
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use EntityClass::*;

    const P: TierPolicy = TierPolicy { near_cutoff: 16, active_cutoff: 256 };

    #[test]
    fn authoritative_never_runs_on_gpu() {
        // Determinism line: structural plants & gameplay mobs are CpuFull at every
        // in-range distance, never GpuApprox.
        for d in [0, 8, 16, 17, 100, 256] {
            assert_eq!(exec_tier(StructuralPlant, d, &P), ExecTier::CpuFull);
            assert_eq!(exec_tier(GameplayMob, d, &P), ExecTier::CpuFull);
        }
        assert_eq!(exec_tier(StructuralPlant, 257, &P), ExecTier::Dormant);
        assert_eq!(exec_tier(GameplayMob, 9999, &P), ExecTier::Dormant);
    }

    #[test]
    fn cosmetic_offloads_in_the_midband() {
        // Near → CPU (few, accurate); mid → GPU (the offloaded bulk); far → none.
        assert_eq!(exec_tier(CosmeticFlora, 0, &P), ExecTier::CpuFull);
        assert_eq!(exec_tier(CosmeticFlora, 16, &P), ExecTier::CpuFull);
        assert_eq!(exec_tier(CosmeticFlora, 17, &P), ExecTier::GpuApprox);
        assert_eq!(exec_tier(CosmeticFlora, 256, &P), ExecTier::GpuApprox);
        assert_eq!(exec_tier(CosmeticFlora, 257, &P), ExecTier::Dormant);
        assert_eq!(exec_tier(AmbientMob, 100, &P), ExecTier::GpuApprox);
    }

    #[test]
    fn nothing_ticks_beyond_active_cutoff() {
        for c in [StructuralPlant, CosmeticFlora, GameplayMob, AmbientMob] {
            assert_eq!(exec_tier(c, P.active_cutoff + 1, &P), ExecTier::Dormant);
        }
    }

    #[test]
    fn tier_never_gets_more_expensive_outward() {
        // Cost rank CpuFull(2) > GpuApprox(1) > Dormant(0): moving outward only
        // makes a tick cheaper-or-equal, never more expensive.
        let rank = |t: ExecTier| match t {
            ExecTier::CpuFull => 2,
            ExecTier::GpuApprox => 1,
            ExecTier::Dormant => 0,
        };
        for c in [StructuralPlant, CosmeticFlora, GameplayMob, AmbientMob] {
            let mut prev = i32::MAX;
            for d in [0, 16, 17, 128, 256, 257, 1000] {
                let cost = rank(exec_tier(c, d, &P));
                assert!(cost <= prev, "cost rose outward for {:?} at {}", c, d);
                prev = cost;
            }
        }
    }

    #[test]
    fn class_authority_is_correct() {
        assert!(StructuralPlant.is_authoritative());
        assert!(GameplayMob.is_authoritative());
        assert!(!CosmeticFlora.is_authoritative());
        assert!(!AmbientMob.is_authoritative());
    }

    #[test]
    fn tier_predicates() {
        assert!(ExecTier::CpuFull.runs_on_cpu() && !ExecTier::CpuFull.runs_on_gpu());
        assert!(ExecTier::GpuApprox.runs_on_gpu() && !ExecTier::GpuApprox.runs_on_cpu());
        assert!(!ExecTier::Dormant.runs_on_cpu() && !ExecTier::Dormant.runs_on_gpu());
    }
}
