//! Per-block PBR material parameters + emission — the companion to
//! `block_color`. Where color says *what hue* a block is, this says *how it
//! interacts with light*: roughness, metalness, dielectric reflectance (F0),
//! and emission. Three consumers:
//!
//! - The **light flood-fill** ([`crate::light::propagate_block_light`]) seeds
//!   from [`emission`] — lava, crystal, amber, and faintly diamond ore become
//!   colored light sources for free.
//! - The **mesher** bakes roughness/metalness/F0 per vertex alongside color.
//! - The **shader** evaluates a single analytic specular lobe ([`ggx_d`] +
//!   [`fresnel_schlick`]) on top of the baked diffuse irradiance — cheap,
//!   because the expensive light integration is already precomputed.
//!
//! All pure and deterministic; unit-tested in the default build.

use crate::world::blocks::BlockId;

/// PBR surface parameters for a block. Linear, all in `[0,1]` except emission.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Material {
    /// Microfacet roughness: 0 = mirror-smooth, 1 = fully diffuse.
    pub roughness: f32,
    /// 0 = dielectric (stone, wood, water), 1 = metal (ore veins).
    pub metalness: f32,
    /// Dielectric normal-incidence reflectance (F0). Water ≈ 0.02, most
    /// solids ≈ 0.04, polished/gem ≈ 0.05–0.10. Metals drive reflectance from
    /// their albedo instead, so this is the dielectric base.
    pub reflectance: f32,
    /// Emitted light per channel, 0..=15 (matches [`crate::light::MAX_LIGHT`]).
    /// `[0,0,0]` = not a light source.
    pub emission: [u8; 3],
}

impl Material {
    const fn diffuse(roughness: f32) -> Self {
        Self { roughness, metalness: 0.0, reflectance: 0.04, emission: [0, 0, 0] }
    }
    const fn with_emission(mut self, e: [u8; 3]) -> Self {
        self.emission = e;
        self
    }
    const fn metal(roughness: f32) -> Self {
        Self { roughness, metalness: 0.7, reflectance: 0.04, emission: [0, 0, 0] }
    }
    const fn polished(roughness: f32, reflectance: f32) -> Self {
        Self { roughness, metalness: 0.0, reflectance, emission: [0, 0, 0] }
    }
    pub fn is_emissive(&self) -> bool {
        self.emission != [0, 0, 0]
    }
}

/// The material for a block. A LUT mirroring the block palette; unknown ids
/// fall back to a neutral rough dielectric (never debug-bright, unlike the old
/// magenta color fallback).
pub fn material_for(b: BlockId) -> Material {
    use BlockId::*;
    match b {
        // Rough natural stone / soil / sediment.
        Stone | Cobblestone | Bedrock | Gravel | Dirt | Clay | Sand | RedSandstone
        | Granite | Andesite | Tuff | Schist | Dripstone | CoalOre | Limestone | MossyCobblestone | PolishedStone | PolishedBasalt | ChiseledStoneBricks | SmoothSandstone | Terracotta => {
            Material::diffuse(0.9)
        }
        Basalt | Slate => Material::diffuse(0.85),
        // Polished / crystalline pale stone — smoother, a touch more reflective.
        Marble => Material::polished(0.35, 0.06),
        Calcite => Material::polished(0.5, 0.05),
        Bone => Material::polished(0.6, 0.05),
        // Vegetation / organics — fully diffuse, matte.
        Grass | Moss | OakLeaves | BirchLeaves | Cactus | OakLog | BirchLog
        | Flower | TallGrass | Fern | DeadBush | Vine | Lichen | Mushroom | Cobweb | DarkOakPlanks
        | Carrot | Potato | Cabbage | Sprout => {
            Material::diffuse(0.95)
        }
        // Water: smooth dielectric, low F0, strong angular reflection.
        Water => Material::polished(0.05, 0.02),
        // Ice: smooth, slightly more reflective than water.
        Ice => Material::polished(0.12, 0.04),
        // Snow: bright but matte (subsurface-ish), low specular.
        Snow | SnowBlock => Material::diffuse(0.75),
        // Ore veins read as partially metallic.
        GoldOre => Material::metal(0.3),
        IronOre => Material::metal(0.45),
        DiamondOre => Material::polished(0.2, 0.10).with_emission([2, 2, 3]), // faint sparkle
        // Emissive gems / molten rock — the world's natural light sources.
        Lava => Material::diffuse(0.9).with_emission([15, 7, 1]), // hot orange
        Amber => Material::polished(0.3, 0.07).with_emission([8, 5, 1]), // warm glow
        Crystal => Material::polished(0.15, 0.08).with_emission([4, 3, 8]), // cool violet
        // --- Added blocks ---
        // Natural minerals.
        Sandstone => Material::diffuse(0.9),
        Obsidian => Material::polished(0.25, 0.08), // smooth black volcanic glass
        CopperOre => Material::metal(0.5),
        Quartz => Material::polished(0.4, 0.06),
        // Crafted / building.
        OakPlanks | Bookshelf => Material::diffuse(0.85), // worked wood
        StoneBricks | Bricks => Material::diffuse(0.9),
        Glass => Material::polished(0.05, 0.04),          // smooth dielectric
        IronBlock => Material::metal(0.35),
        GoldBlock => Material::metal(0.2),                // polished metal
        CoalBlock => Material::diffuse(0.85),
        Pumpkin => Material::diffuse(0.8),
        // Air / anything else: neutral, harmless.
        Air => Material::diffuse(1.0),
    }
}

/// Emission for a block, or `None` if it isn't a light source. Convenience for
/// gathering flood-fill emitters: `emission(b).map(|e| (pos, e))`.
pub fn emission(b: BlockId) -> Option<[u8; 3]> {
    let m = material_for(b);
    m.is_emissive().then_some(m.emission)
}

/// Extra per-channel light attenuation when colored light passes *through* a
/// translucent block (on top of the normal one-level-per-step falloff). Only
/// consulted for non-opaque blocks — opaque blocks stop light entirely — so
/// everything not explicitly tinted is treated as clear (`[0,0,0]`). This is
/// what turns white light blue underwater and green under a canopy.
pub fn transmission(b: BlockId) -> [u8; 3] {
    use BlockId::*;
    match b {
        Water => [3, 1, 0],            // absorbs red>green>blue → deep blue tint
        Ice => [1, 1, 0],              // faint blue
        Amber => [0, 1, 2],            // absorbs blue/green → warm amber
        Crystal => [0, 1, 0],          // passes red+blue → violet
        OakLeaves | BirchLeaves | Vine => [1, 0, 1], // canopy passes green
        // Clear (glass, air) or opaque (light stops anyway): no extra tint.
        _ => [0, 0, 0],
    }
}

/// GGX / Trowbridge–Reitz normal-distribution term `D` for a half-vector
/// alignment `n_dot_h` and `roughness`. This is the shape of the specular
/// highlight: smaller roughness → a tighter, taller lobe. Always ≥ 0.
pub fn ggx_d(n_dot_h: f32, roughness: f32) -> f32 {
    let a = (roughness * roughness).max(1e-4); // α = roughness², clamped
    let a2 = a * a;
    let nh = n_dot_h.clamp(0.0, 1.0);
    let denom = nh * nh * (a2 - 1.0) + 1.0;
    a2 / (std::f32::consts::PI * denom * denom)
}

/// Schlick approximation of Fresnel reflectance at angle `cos_theta` with
/// normal-incidence reflectance `f0`. Head-on (`cos_theta = 1`) returns `f0`;
/// grazing (`cos_theta → 0`) returns ~1 (everything reflects at a glancing
/// angle — the rim sheen on water and polished stone).
pub fn fresnel_schlick(cos_theta: f32, f0: f32) -> f32 {
    let c = (1.0 - cos_theta.clamp(0.0, 1.0)).clamp(0.0, 1.0);
    f0 + (1.0 - f0) * c.powi(5)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::blocks::BlockId;

    fn in01(x: f32) -> bool {
        (0.0..=1.0).contains(&x)
    }

    #[test]
    fn every_material_is_in_range() {
        // A representative spread across the palette.
        for b in [
            BlockId::Stone, BlockId::Grass, BlockId::Water, BlockId::Ice,
            BlockId::GoldOre, BlockId::IronOre, BlockId::Lava, BlockId::Amber,
            BlockId::Crystal, BlockId::Marble, BlockId::Snow, BlockId::OakLeaves,
            BlockId::DiamondOre, BlockId::Air,
        ] {
            let m = material_for(b);
            assert!(in01(m.roughness), "{b:?} roughness {}", m.roughness);
            assert!(in01(m.metalness), "{b:?} metalness {}", m.metalness);
            assert!(in01(m.reflectance), "{b:?} reflectance {}", m.reflectance);
            assert!(m.emission.iter().all(|&c| c <= 15), "{b:?} emission too bright");
        }
    }

    #[test]
    fn water_is_smooth_and_low_f0() {
        let w = material_for(BlockId::Water);
        assert!(w.roughness < 0.2, "water should be smooth");
        assert!(w.reflectance < 0.03, "water F0 ≈ 0.02");
        assert_eq!(w.metalness, 0.0, "water is dielectric");
    }

    #[test]
    fn ores_are_metallic_stone_is_not() {
        assert!(material_for(BlockId::GoldOre).metalness > 0.5);
        assert!(material_for(BlockId::IronOre).metalness > 0.3);
        assert_eq!(material_for(BlockId::Stone).metalness, 0.0);
        // Metals are smoother than raw stone.
        assert!(material_for(BlockId::GoldOre).roughness < material_for(BlockId::Stone).roughness);
    }

    #[test]
    fn emitters_emit_others_dont() {
        // Lava is a strong warm source.
        let lava = emission(BlockId::Lava).expect("lava emits");
        assert!(lava[0] > lava[2], "lava is warm (more red than blue)");
        assert!(lava[0] >= 12, "lava is bright");
        // Crystal is a cool source.
        let crystal = emission(BlockId::Crystal).expect("crystal emits");
        assert!(crystal[2] > crystal[0], "crystal is cool (more blue than red)");
        // Amber is warm but dimmer than lava.
        assert!(emission(BlockId::Amber).unwrap()[0] < lava[0]);
        // Plain blocks emit nothing.
        assert_eq!(emission(BlockId::Stone), None);
        assert_eq!(emission(BlockId::Grass), None);
        assert!(!material_for(BlockId::Dirt).is_emissive());
    }

    #[test]
    fn ggx_peaks_when_aligned_and_sharpens_with_smoothness() {
        // The lobe is maximal at perfect alignment (n·h = 1).
        let r = 0.3;
        let peak = ggx_d(1.0, r);
        assert!(ggx_d(0.7, r) < peak, "off-axis is lower than the peak");
        assert!(ggx_d(0.0, r) < peak);
        // Smoother surface → taller, tighter peak.
        assert!(ggx_d(1.0, 0.1) > ggx_d(1.0, 0.8), "smoother → sharper highlight");
        // Always non-negative.
        for nh in [0.0, 0.25, 0.5, 0.75, 1.0] {
            assert!(ggx_d(nh, 0.5) >= 0.0);
        }
    }

    #[test]
    fn fresnel_head_on_is_f0_grazing_is_full() {
        let f0 = 0.04;
        assert!((fresnel_schlick(1.0, f0) - f0).abs() < 1e-6, "head-on = F0");
        assert!(fresnel_schlick(0.0, f0) > 0.99, "grazing ≈ full reflection");
        // Monotonic: reflectance rises as the angle grazes.
        let a = fresnel_schlick(0.9, f0);
        let b = fresnel_schlick(0.5, f0);
        let c = fresnel_schlick(0.1, f0);
        assert!(a < b && b < c, "reflectance increases toward grazing");
    }
}
