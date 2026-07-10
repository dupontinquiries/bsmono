//! r690: builder dragons — from
//! `04_Creatures_and_Dragons/builder_genome_revised.docx`.
//!
//! A rare set of `BLD` alleles turns a dragon from a nesting animal into a
//! *constructor*. The doc fixes the allele→expression map and prevalence:
//!
//! | Expression   | Alleles            | Behaviour                              | ~   |
//! |--------------|--------------------|----------------------------------------|-----|
//! | Non-builder  | none               | standard nesting / claiming            | 90% |
//! | Shaper       | BLD-1              | impulsive, no coherence; organic blobs | 5%  |
//! | Neatener     | BLD-3              | modifies existing structures           | 3%  |
//! | Builder      | BLD-1 + BLD-3      | full drive WITH pattern coherence      |1.5% |
//! | Architect    | BLD-1+BLD-2+BLD-3  | Builder + material sensing at range    |0.3% |
//!
//! BLD-1 = constructive drive, BLD-3 = pattern-coherence sense (feels whether the
//! current state matches an internal template), BLD-2 = material sensing (haptic
//! composition-at-range in the facial scales). The *element* then determines the
//! characteristic architecture (Earth → megalithic compression, Fire → fused
//! glass, Water → carved ice, Air → wind-spires, multi-element → refined joinery).
//!
//! Pure, std-only, deterministic — genome/element in, expression + capabilities +
//! style out. No world or entity deps.

/// A dragon's builder phenotype, from its `BLD` alleles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuilderExpression {
    /// No BLD alleles — standard nesting/claiming (~90%).
    NonBuilder,
    /// BLD-1 only — impulsive, no pattern coherence; organic irregular forms (~5%).
    Shaper,
    /// BLD-3 only — modifies/tidies existing structures (~3%).
    Neatener,
    /// BLD-1 + BLD-3 — full constructive drive with pattern coherence (~1.5%).
    Builder,
    /// BLD-1 + BLD-2 + BLD-3 — Builder plus material sensing at range (~0.3%).
    Architect,
}

/// Map the three `BLD` alleles to a phenotype (doc table).
///
/// BLD-2 (material sensing) only elevates a full Builder to an Architect; on its
/// own or without pattern coherence it doesn't create constructive drive, so it's
/// inert outside the Architect combination.
pub fn builder_expression(bld1: bool, bld2: bool, bld3: bool) -> BuilderExpression {
    match (bld1, bld2, bld3) {
        (true, true, true) => BuilderExpression::Architect,
        (true, _, true) => BuilderExpression::Builder,
        (true, _, false) => BuilderExpression::Shaper,
        (false, _, true) => BuilderExpression::Neatener,
        (false, _, false) => BuilderExpression::NonBuilder,
    }
}

/// r694: classify a dragon's builder phenotype from its spawn **seed** — the rare
/// BLD alleles rolled from decorrelated seed bits at roughly the doc prevalences
/// (BLD-1 constructive drive ~7%, BLD-3 pattern-coherence ~5%, BLD-2 material-
/// sensing ~15% among the drive+coherence carriers). The vast majority come out
/// NonBuilder; Builders/Architects are rare. Deterministic per seed.
pub fn from_seed(seed: u64) -> BuilderExpression {
    let u = |shift: u32| ((seed >> shift) & 0xFFFF) as f32 / 65535.0;
    let bld1 = u(8) < 0.07; // constructive drive
    let bld3 = u(24) < 0.05; // pattern coherence
    let bld2 = u(44) < 0.15; // material sensing (only escalates a Builder to Architect)
    builder_expression(bld1, bld2, bld3)
}

impl BuilderExpression {
    /// Approximate population prevalence (doc), as a fraction in `[0,1]`.
    pub fn prevalence(self) -> f32 {
        match self {
            BuilderExpression::NonBuilder => 0.900,
            BuilderExpression::Shaper => 0.050,
            BuilderExpression::Neatener => 0.030,
            BuilderExpression::Builder => 0.015,
            BuilderExpression::Architect => 0.003,
        }
    }

    /// Has BLD-1 — the constructive drive to place/assemble material.
    pub fn can_construct(self) -> bool {
        matches!(
            self,
            BuilderExpression::Shaper | BuilderExpression::Builder | BuilderExpression::Architect
        )
    }

    /// r712: builder rank (0 NonBuilder .. 4 Architect) for
    /// `legendary_tier::builder_construct_tier`, mapping this phenotype to the tier
    /// of Builder Construct it raises.
    pub const fn construct_rank(self) -> u8 {
        match self {
            BuilderExpression::NonBuilder => 0,
            BuilderExpression::Shaper => 1,
            BuilderExpression::Neatener => 2,
            BuilderExpression::Builder => 3,
            BuilderExpression::Architect => 4,
        }
    }

    /// Has BLD-3 — the pattern-coherence sense (feels template match), which also
    /// lets it modify/tidy existing structures.
    pub fn has_pattern_coherence(self) -> bool {
        matches!(
            self,
            BuilderExpression::Neatener | BuilderExpression::Builder | BuilderExpression::Architect
        )
    }

    /// Has BLD-2 — perceives material composition at range (Architect only).
    pub fn has_material_sensing(self) -> bool {
        matches!(self, BuilderExpression::Architect)
    }

    /// Produces *coherent* structures (drive + coherence together): Builder or
    /// Architect. A Shaper builds impulsive blobs; a Neatener only edits.
    pub fn builds_coherently(self) -> bool {
        self.can_construct() && self.has_pattern_coherence()
    }

    pub fn name(self) -> &'static str {
        match self {
            BuilderExpression::NonBuilder => "Non-builder",
            BuilderExpression::Shaper => "Shaper",
            BuilderExpression::Neatener => "Neatener",
            BuilderExpression::Builder => "Builder",
            BuilderExpression::Architect => "Architect",
        }
    }
}

/// The characteristic architecture a builder dragon produces, by bending element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchitectureStyle {
    /// Earth — megalithic walls, tiered terracing, column-and-lintel, barrel vaults.
    Megalithic,
    /// Fire — fused-glass towers, fired-brick arches, vitrified floors.
    FusedGlass,
    /// Water — smooth carved stone, ice vaulting, sediment-layered platforms.
    CarvedIce,
    /// Air — tall open structures, wind-driven spires, floating placement.
    WindSpire,
    /// Multi-element — sequential earth-shape / fire-fuse / air-dress; the most
    /// refined phenotype, mixing material types + joinery.
    MultiMaterial,
    /// Non-constructive element / non-builder — no characteristic architecture.
    None,
}

/// The architecture style for a builder's bending element (0=Earth 1=Water
/// 2=Fire 3=Air; anything else → `None`). Use [`multi_material`] when a dragon
/// commands more than one constructive element.
pub fn architecture_style(element_idx: u8) -> ArchitectureStyle {
    match element_idx {
        0 => ArchitectureStyle::Megalithic, // Earth
        1 => ArchitectureStyle::CarvedIce,  // Water (incl. Ice sub-discipline)
        2 => ArchitectureStyle::FusedGlass, // Fire
        3 => ArchitectureStyle::WindSpire,  // Air
        _ => ArchitectureStyle::None,
    }
}

/// A dragon that can apply two or more constructive elements builds in the
/// refined multi-material style (earth to shape, fire to fuse, air to dress).
pub fn multi_material(constructive_element_count: u8) -> ArchitectureStyle {
    if constructive_element_count >= 2 {
        ArchitectureStyle::MultiMaterial
    } else {
        ArchitectureStyle::None
    }
}

/// The two constructive breath modes (doc §breath): a narrow **Focused Thread**
/// (Fire/Air — precision cutting, joinery fusion) and a broad **Area Diffusion**.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreathMode {
    FocusedThread,
    AreaDiffusion,
}

impl BreathMode {
    /// Focused Thread needs Fire or Air (element 2 or 3); Area Diffusion is
    /// available to any constructive element.
    pub fn available(self, element_idx: u8) -> bool {
        match self {
            BreathMode::FocusedThread => element_idx == 2 || element_idx == 3,
            BreathMode::AreaDiffusion => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_seed_is_mostly_non_builders_and_reaches_every_phenotype() {
        let mut counts = [0u32; 5]; // NonBuilder, Shaper, Neatener, Builder, Architect
        let n = 20_000u64;
        for s in 0..n {
            let seed = s.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            let idx = match from_seed(seed) {
                BuilderExpression::NonBuilder => 0,
                BuilderExpression::Shaper => 1,
                BuilderExpression::Neatener => 2,
                BuilderExpression::Builder => 3,
                BuilderExpression::Architect => 4,
            };
            counts[idx] += 1;
        }
        // NonBuilders dominate (doc ~90%); allow a wide band.
        assert!(counts[0] as f32 / n as f32 > 0.80, "non-builders {}", counts[0]);
        // Shapers are the most common builder variant.
        assert!(counts[1] > counts[3], "shapers should outnumber full builders");
        // every phenotype occurs at least once across 20k seeds.
        assert!(counts.iter().all(|&c| c > 0), "all phenotypes reachable: {counts:?}");
    }

    #[test]
    fn allele_combinations_map_to_the_doc_phenotypes() {
        assert_eq!(builder_expression(false, false, false), BuilderExpression::NonBuilder);
        assert_eq!(builder_expression(true, false, false), BuilderExpression::Shaper);
        assert_eq!(builder_expression(false, false, true), BuilderExpression::Neatener);
        assert_eq!(builder_expression(true, false, true), BuilderExpression::Builder);
        assert_eq!(builder_expression(true, true, true), BuilderExpression::Architect);
        // BLD-2 without the full builder set is inert
        assert_eq!(builder_expression(false, true, false), BuilderExpression::NonBuilder);
        assert_eq!(builder_expression(true, true, false), BuilderExpression::Shaper);
    }

    #[test]
    fn capabilities_follow_the_alleles() {
        assert!(BuilderExpression::Shaper.can_construct());
        assert!(!BuilderExpression::Shaper.has_pattern_coherence());
        assert!(!BuilderExpression::Shaper.builds_coherently());
        assert!(BuilderExpression::Neatener.has_pattern_coherence());
        assert!(!BuilderExpression::Neatener.can_construct());
        assert!(BuilderExpression::Builder.builds_coherently());
        assert!(!BuilderExpression::Builder.has_material_sensing());
        assert!(BuilderExpression::Architect.has_material_sensing());
        assert!(BuilderExpression::Architect.builds_coherently());
    }

    #[test]
    fn prevalences_are_rare_and_ordered() {
        assert!(BuilderExpression::NonBuilder.prevalence() > 0.8);
        assert!(BuilderExpression::Architect.prevalence() < BuilderExpression::Builder.prevalence());
        assert!(BuilderExpression::Builder.prevalence() < BuilderExpression::Neatener.prevalence());
        // the builder phenotypes together are a small minority
        let builders = BuilderExpression::Shaper.prevalence()
            + BuilderExpression::Neatener.prevalence()
            + BuilderExpression::Builder.prevalence()
            + BuilderExpression::Architect.prevalence();
        assert!(builders < 0.11);
    }

    #[test]
    fn element_styles_and_multi_material() {
        assert_eq!(architecture_style(0), ArchitectureStyle::Megalithic);
        assert_eq!(architecture_style(2), ArchitectureStyle::FusedGlass);
        assert_eq!(architecture_style(9), ArchitectureStyle::None);
        assert_eq!(multi_material(1), ArchitectureStyle::None);
        assert_eq!(multi_material(3), ArchitectureStyle::MultiMaterial);
    }

    #[test]
    fn focused_thread_needs_fire_or_air() {
        assert!(BreathMode::FocusedThread.available(2)); // fire
        assert!(BreathMode::FocusedThread.available(3)); // air
        assert!(!BreathMode::FocusedThread.available(0)); // earth
        assert!(BreathMode::AreaDiffusion.available(0)); // any
    }
}
