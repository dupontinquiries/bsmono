//! Per-block surface material — the compact optical lookup keyed by [`BlockId`]:
//! roughness, reflectivity (F0), relief depth, and emissive. This is the "base
//! material" layer of the texturing plan; spatial variation is layered on by the
//! shader (world-space noise) and by physics-driven weathering via
//! [`SurfaceMaterial::with_weathering`].
//!
//! Quantized to four `u8`s (4 bytes per block) so the whole table is a tiny dense
//! array indexed by block id — no real compression needed at a few hundred
//! entries. Optical roughness here is the microfacet/specular term and is
//! deliberately *separate* from physics friction; the two may be correlated but
//! are distinct quantities, so they live apart.

use crate::world::blocks::BlockId;

/// Optical surface parameters, each `0..=255` mapping to `[0,1]`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SurfaceMaterial {
    /// Microfacet roughness: 0 = mirror-smooth, 255 = fully matte.
    pub roughness: u8,
    /// Base reflectance / specular intensity (F0).
    pub reflectivity: u8,
    /// Relief/parallax depth scale (for optional height-based detail).
    pub height: u8,
    /// Self-illumination (lava glows; most blocks are 0).
    pub emissive: u8,
}

impl SurfaceMaterial {
    const fn new(roughness: u8, reflectivity: u8, height: u8, emissive: u8) -> SurfaceMaterial {
        SurfaceMaterial { roughness, reflectivity, height, emissive }
    }

    pub fn roughness_f(self) -> f32 {
        self.roughness as f32 / 255.0
    }
    pub fn reflectivity_f(self) -> f32 {
        self.reflectivity as f32 / 255.0
    }
    pub fn height_f(self) -> f32 {
        self.height as f32 / 255.0
    }
    pub fn emissive_f(self) -> f32 {
        self.emissive as f32 / 255.0
    }

    /// Modulate the base material by a physics-maintained surface state: `erosion`
    /// and `wetness`, each in `[0,1]`. Erosion roughens and pits the surface and
    /// dulls its reflectance; wetness does the opposite — smooths and brightens
    /// the specular (a wet surface is glossier). Both are clamped, and glow is
    /// left untouched. This is the seam where the physics erosion/wetness field
    /// drives the look without that field living in the shading code.
    pub fn with_weathering(self, erosion: f32, wetness: f32) -> SurfaceMaterial {
        let e = erosion.clamp(0.0, 1.0);
        let w = wetness.clamp(0.0, 1.0);
        let q = |v: f32| v.clamp(0.0, 255.0) as u8;
        SurfaceMaterial {
            roughness: q(self.roughness as f32 + e * 40.0 - w * 120.0),
            reflectivity: q(self.reflectivity as f32 - e * 10.0 + w * 90.0),
            height: q(self.height as f32 + e * 50.0),
            emissive: self.emissive,
        }
    }
}

/// The base optical material for a block. Unmapped blocks get a neutral matte
/// default.
pub fn material(b: BlockId) -> SurfaceMaterial {
    match b {
        // matte minerals and ground
        BlockId::Stone => SurfaceMaterial::new(230, 15, 60, 0),
        BlockId::Cobblestone => SurfaceMaterial::new(240, 12, 90, 0),
        BlockId::Bedrock => SurfaceMaterial::new(250, 6, 50, 0),
        BlockId::Dirt => SurfaceMaterial::new(245, 8, 40, 0),
        BlockId::Grass => SurfaceMaterial::new(235, 10, 30, 0),
        BlockId::Sand => SurfaceMaterial::new(220, 18, 25, 0),
        // smooth and reflective
        BlockId::Water => SurfaceMaterial::new(30, 110, 5, 0),
        // glowing
        BlockId::Lava => SurfaceMaterial::new(180, 30, 20, 230),
        _ => SurfaceMaterial::new(200, 20, 30, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn water_is_smoother_and_shinier_than_stone() {
        let water = material(BlockId::Water);
        let stone = material(BlockId::Stone);
        assert!(water.roughness < stone.roughness, "water is glossier");
        assert!(water.reflectivity > stone.reflectivity, "water reflects more");
    }

    #[test]
    fn lava_glows_stone_does_not() {
        assert!(material(BlockId::Lava).emissive > 0);
        assert_eq!(material(BlockId::Stone).emissive, 0);
    }

    #[test]
    fn erosion_roughens_wetness_smooths() {
        let base = material(BlockId::Stone);
        let eroded = base.with_weathering(1.0, 0.0);
        let wet = base.with_weathering(0.0, 1.0);
        assert!(eroded.roughness >= base.roughness, "erosion roughens");
        assert!(eroded.height >= base.height, "erosion pits the surface");
        assert!(wet.roughness < base.roughness, "wet is glossier");
        assert!(wet.reflectivity > base.reflectivity, "wet reflects more");
    }

    #[test]
    fn weathering_stays_in_range_and_unit_accessors_bounded() {
        // Extreme inputs (and out-of-range) never escape 0..=255 / [0,1].
        let m = material(BlockId::Sand).with_weathering(5.0, -2.0);
        assert!(m.roughness_f() >= 0.0 && m.roughness_f() <= 1.0);
        assert!(m.reflectivity_f() >= 0.0 && m.reflectivity_f() <= 1.0);
    }
}
