//! graphics.rs — user-selectable **graphics quality presets**. One enum picks a bundle of quality knobs
//! (geometry detail, atlas resolution, fog draw distance, dynamic-light budget, shadow quality, particle
//! density) so a player can trade fidelity for framerate with a single choice. The app reads a [`Settings`]
//! derived from the preset and feeds each knob to the system that owns it (mesh tessellation, the atlas
//! bake, the renderer's fog/light limits, the particle cap).
//!
//! The presets are ordered Low → Ultra. "Ultra" turns on everything (fine tessellation, the big atlas, long
//! fog, the full dynamic-light and shadow budget); "Low" strips to the cheapest readable version so the
//! software renderer stays smooth on a weak machine.

/// A graphics quality tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphicsPreset {
    Low,
    Medium,
    High,
    Ultra,
}

impl GraphicsPreset {
    pub const ALL: [GraphicsPreset; 4] = [
        GraphicsPreset::Low,
        GraphicsPreset::Medium,
        GraphicsPreset::High,
        GraphicsPreset::Ultra,
    ];

    pub fn name(self) -> &'static str {
        match self {
            GraphicsPreset::Low => "Low",
            GraphicsPreset::Medium => "Medium",
            GraphicsPreset::High => "High",
            GraphicsPreset::Ultra => "Ultra",
        }
    }

    pub fn desc(self) -> &'static str {
        match self {
            GraphicsPreset::Low => "Fastest — minimal geometry, short draw distance, no dynamic shadows.",
            GraphicsPreset::Medium => "Balanced — moderate detail and draw distance, a few dynamic lights.",
            GraphicsPreset::High => "Crisp — fine geometry, long draw distance, dynamic lights and soft shadows.",
            GraphicsPreset::Ultra => "Maximum — full detail, longest draw distance, every dynamic light and shadow.",
        }
    }

    /// The concrete settings bundle for this preset.
    pub fn settings(self) -> Settings {
        match self {
            GraphicsPreset::Low => Settings {
                geom_detail: 0.6,
                atlas_tile: 256,
                fog_scale: 0.7,
                max_dynamic_lights: 0,
                shadows: ShadowQuality::Off,
                particle_scale: 0.5,
                impostor_bias: 0.65, // collapse to impostors sooner
            },
            GraphicsPreset::Medium => Settings {
                geom_detail: 0.85,
                atlas_tile: 512,
                fog_scale: 0.9,
                max_dynamic_lights: 4,
                shadows: ShadowQuality::Off,
                particle_scale: 0.8,
                impostor_bias: 0.85,
            },
            GraphicsPreset::High => Settings {
                geom_detail: 1.0,
                atlas_tile: 512,
                fog_scale: 1.1,
                max_dynamic_lights: 8,
                shadows: ShadowQuality::Blob,
                particle_scale: 1.0,
                impostor_bias: 1.0,
            },
            GraphicsPreset::Ultra => Settings {
                geom_detail: 1.35,
                atlas_tile: 1024,
                fog_scale: 1.3,
                max_dynamic_lights: 12,
                shadows: ShadowQuality::Soft,
                particle_scale: 1.3,
                impostor_bias: 1.25, // keep full detail further out
            },
        }
    }

    pub fn next(self) -> GraphicsPreset {
        let i = Self::ALL.iter().position(|&p| p == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

impl Default for GraphicsPreset {
    fn default() -> Self {
        GraphicsPreset::High
    }
}

/// How dynamic-light shadows are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadowQuality {
    /// No dynamic shadows.
    Off,
    /// A single dark blob under each dynamic-lit dynamic entity (cheap contact shadow).
    Blob,
    /// A soft, larger, alpha-faded blob (still cheap, but reads as a real soft shadow).
    Soft,
}

/// The concrete knobs a graphics preset resolves to. Each is consumed by the subsystem that owns it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Geometry-detail multiplier fed to `mesh::set_detail` (curved-primitive tessellation).
    pub geom_detail: f32,
    /// Material-atlas tile resolution (px per material). Bigger = crisper textures, more VRAM/bake time.
    pub atlas_tile: usize,
    /// Fog draw-distance multiplier — scales the map's baseline fog start/end (further = see more, cost more).
    pub fog_scale: f32,
    /// Maximum dynamic point lights the renderer will apply per frame.
    pub max_dynamic_lights: usize,
    /// Dynamic-light shadow quality.
    pub shadows: ShadowQuality,
    /// Particle-count multiplier (emitter density).
    pub particle_scale: f32,
    /// LOD impostor distance bias — >1 keeps full geometry further out, <1 collapses to impostors sooner.
    pub impostor_bias: f32,
}

impl Default for Settings {
    fn default() -> Self {
        GraphicsPreset::default().settings()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_scale_monotonically_from_low_to_ultra() {
        // each step up should be at least as detailed on every axis, and strictly more on the headline ones
        let s: Vec<Settings> = GraphicsPreset::ALL.iter().map(|p| p.settings()).collect();
        for w in s.windows(2) {
            let (a, b) = (w[0], w[1]);
            assert!(b.geom_detail >= a.geom_detail, "geom detail should not decrease going up a preset");
            assert!(b.atlas_tile >= a.atlas_tile, "atlas resolution should not decrease going up a preset");
            assert!(b.fog_scale >= a.fog_scale, "fog distance should not decrease going up a preset");
            assert!(b.max_dynamic_lights >= a.max_dynamic_lights, "light budget should not decrease going up a preset");
            assert!(b.particle_scale >= a.particle_scale, "particle density should not decrease going up a preset");
        }
        // headline extremes really differ
        assert!(GraphicsPreset::Ultra.settings().geom_detail > GraphicsPreset::Low.settings().geom_detail * 2.0);
        assert_eq!(GraphicsPreset::Low.settings().max_dynamic_lights, 0);
        assert!(GraphicsPreset::Ultra.settings().max_dynamic_lights >= 12);
    }

    #[test]
    fn cycling_presets_visits_all_four() {
        let mut p = GraphicsPreset::Low;
        let mut seen = std::collections::HashSet::new();
        for _ in 0..4 {
            seen.insert(p.name());
            p = p.next();
        }
        assert_eq!(seen.len(), 4, "cycling should visit every preset");
        assert_eq!(p, GraphicsPreset::Low, "cycling four times returns to the start");
    }

    #[test]
    fn shadow_quality_scales_with_preset() {
        assert_eq!(GraphicsPreset::Low.settings().shadows, ShadowQuality::Off);
        assert_eq!(GraphicsPreset::Ultra.settings().shadows, ShadowQuality::Soft);
    }
}
