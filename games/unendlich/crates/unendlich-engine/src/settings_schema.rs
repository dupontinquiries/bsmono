//! settings_schema.rs — pure typed settings model with validation.
//!
//! Graphics / audio / control settings with sane defaults, range clamping, a
//! `validate()` predicate, and a flat key→value form for persistence. Pure,
//! deterministic, panic-free (R1/R11/R21). The options GUI (coordinator) renders
//! these via `menu_layout`; this module just owns the data + rules.

/// Inclusive bounds for a `u32` field.
const RENDER_DIST: (u32, u32) = (2, 64); // chunks
const SHADOW_SIZE: (u32, u32) = (512, 4096); // shadow map edge, px
const LOD_FIDELITY: (u8, u8) = (0, 3);

fn clampu(v: u32, lo: u32, hi: u32) -> u32 {
    v.max(lo).min(hi)
}
fn clampf(v: f32, lo: f32, hi: f32) -> f32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}
/// Parse without ever panicking; fall back to `default` on any error.
fn parse_or<T: core::str::FromStr>(s: Option<&String>, default: T) -> T {
    match s {
        Some(v) => match v.parse::<T>() {
            Ok(x) => x,
            Err(_) => default,
        },
        None => default,
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GraphicsSettings {
    pub render_distance: u32,
    pub lod_fidelity: u8,
    pub shadow_size: u32,
    pub vsync: bool,
}
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct AudioSettings {
    pub master: f32,
    pub music: f32,
    pub sfx: f32,
}
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ControlSettings {
    pub mouse_sensitivity: f32,
    pub invert_y: bool,
}
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Settings {
    pub graphics: GraphicsSettings,
    pub audio: AudioSettings,
    pub controls: ControlSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            graphics: GraphicsSettings {
                render_distance: 12,
                lod_fidelity: 2,
                shadow_size: 2048,
                vsync: true,
            },
            audio: AudioSettings { master: 1.0, music: 0.6, sfx: 0.8 },
            controls: ControlSettings { mouse_sensitivity: 1.0, invert_y: false },
        }
    }
}

impl Settings {
    /// Clamp every numeric field into its valid range (booleans untouched).
    pub fn clamped(mut self) -> Self {
        let g = &mut self.graphics;
        g.render_distance = clampu(g.render_distance, RENDER_DIST.0, RENDER_DIST.1);
        g.lod_fidelity = g.lod_fidelity.max(LOD_FIDELITY.0).min(LOD_FIDELITY.1);
        g.shadow_size = clampu(g.shadow_size, SHADOW_SIZE.0, SHADOW_SIZE.1);
        let a = &mut self.audio;
        a.master = clampf(a.master, 0.0, 1.0);
        a.music = clampf(a.music, 0.0, 1.0);
        a.sfx = clampf(a.sfx, 0.0, 1.0);
        self.controls.mouse_sensitivity = clampf(self.controls.mouse_sensitivity, 0.1, 10.0);
        self
    }

    /// True iff every field already lies in range (and audio values are finite).
    pub fn validate(&self) -> bool {
        let g = &self.graphics;
        let a = &self.audio;
        let c = &self.controls;
        (RENDER_DIST.0..=RENDER_DIST.1).contains(&g.render_distance)
            && (LOD_FIDELITY.0..=LOD_FIDELITY.1).contains(&g.lod_fidelity)
            && (SHADOW_SIZE.0..=SHADOW_SIZE.1).contains(&g.shadow_size)
            && a.master.is_finite() && (0.0..=1.0).contains(&a.master)
            && a.music.is_finite() && (0.0..=1.0).contains(&a.music)
            && a.sfx.is_finite() && (0.0..=1.0).contains(&a.sfx)
            && c.mouse_sensitivity.is_finite() && (0.1..=10.0).contains(&c.mouse_sensitivity)
    }

    /// Flatten to sorted `(key, value)` pairs for persistence.
    pub fn to_kv(&self) -> Vec<(String, String)> {
        let g = &self.graphics;
        let a = &self.audio;
        let c = &self.controls;
        vec![
            ("audio.master".into(), a.master.to_string()),
            ("audio.music".into(), a.music.to_string()),
            ("audio.sfx".into(), a.sfx.to_string()),
            ("controls.invert_y".into(), c.invert_y.to_string()),
            ("controls.mouse_sensitivity".into(), c.mouse_sensitivity.to_string()),
            ("graphics.lod_fidelity".into(), g.lod_fidelity.to_string()),
            ("graphics.render_distance".into(), g.render_distance.to_string()),
            ("graphics.shadow_size".into(), g.shadow_size.to_string()),
            ("graphics.vsync".into(), g.vsync.to_string()),
        ]
    }

    /// Rebuild from `(key, value)` pairs, defaulting any missing/invalid field,
    /// then clamping. Always returns a valid `Settings`.
    pub fn from_kv(pairs: &[(String, String)]) -> Self {
        let get = |key: &str| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v);
        let d = Settings::default();
        let s = Settings {
            graphics: GraphicsSettings {
                render_distance: parse_or(get("graphics.render_distance"), d.graphics.render_distance),
                lod_fidelity: parse_or(get("graphics.lod_fidelity"), d.graphics.lod_fidelity),
                shadow_size: parse_or(get("graphics.shadow_size"), d.graphics.shadow_size),
                vsync: parse_or(get("graphics.vsync"), d.graphics.vsync),
            },
            audio: AudioSettings {
                master: parse_or(get("audio.master"), d.audio.master),
                music: parse_or(get("audio.music"), d.audio.music),
                sfx: parse_or(get("audio.sfx"), d.audio.sfx),
            },
            controls: ControlSettings {
                mouse_sensitivity: parse_or(get("controls.mouse_sensitivity"), d.controls.mouse_sensitivity),
                invert_y: parse_or(get("controls.invert_y"), d.controls.invert_y),
            },
        };
        s.clamped()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate() {
        assert!(Settings::default().validate());
    }

    #[test]
    fn out_of_range_clamps_into_range() {
        let wild = Settings {
            graphics: GraphicsSettings {
                render_distance: 9999,
                lod_fidelity: 200,
                shadow_size: 1,
                vsync: false,
            },
            audio: AudioSettings { master: 5.0, music: -2.0, sfx: 0.5 },
            controls: ControlSettings { mouse_sensitivity: 1000.0, invert_y: true },
        };
        let c = wild.clamped();
        assert!(c.validate(), "clamped settings must validate");
        assert_eq!(c.graphics.render_distance, RENDER_DIST.1);
        assert_eq!(c.graphics.lod_fidelity, LOD_FIDELITY.1);
        assert_eq!(c.graphics.shadow_size, SHADOW_SIZE.0);
        assert_eq!(c.audio.master, 1.0);
        assert_eq!(c.audio.music, 0.0);
    }

    #[test]
    fn prop_kv_round_trip() {
        // For valid settings, to_kv -> from_kv is identity.
        let cases = [
            Settings::default(),
            Settings {
                graphics: GraphicsSettings { render_distance: 32, lod_fidelity: 0, shadow_size: 1024, vsync: false },
                audio: AudioSettings { master: 0.25, music: 0.0, sfx: 1.0 },
                controls: ControlSettings { mouse_sensitivity: 3.5, invert_y: true },
            },
            Settings {
                graphics: GraphicsSettings { render_distance: 2, lod_fidelity: 3, shadow_size: 4096, vsync: true },
                audio: AudioSettings { master: 0.5, music: 0.5, sfx: 0.5 },
                controls: ControlSettings { mouse_sensitivity: 0.1, invert_y: false },
            },
        ];
        for s in cases {
            assert!(s.validate());
            assert_eq!(Settings::from_kv(&s.to_kv()), s, "round-trip changed settings");
        }
    }

    #[test]
    fn from_kv_defaults_missing_and_ignores_garbage() {
        let pairs = vec![
            ("graphics.render_distance".to_string(), "not_a_number".to_string()),
            ("audio.master".to_string(), "0.3".to_string()),
        ];
        let s = Settings::from_kv(&pairs);
        assert!(s.validate());
        assert_eq!(s.graphics.render_distance, Settings::default().graphics.render_distance);
        assert_eq!(s.audio.master, 0.3);
    }

    #[test]
    fn prop_deterministic() {
        let s = Settings::default();
        assert_eq!(s.to_kv(), s.to_kv());
        assert_eq!(Settings::from_kv(&s.to_kv()), Settings::from_kv(&s.to_kv()));
    }
}
