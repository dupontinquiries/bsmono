//! `sim::hot_metal` (r464) — hot-metal thermodynamics: metal heats near lava/fire, glows by
//! blackbody temperature, quenches (and flashes steam) in water, and melts at its melting point.
//!
//! Pure + deterministic. The bin tracks hot metal cells and ticks `step_metal`; the steam puff ties
//! into the existing water/steam visuals (water physics interaction), and `glow` feeds a future
//! temperature-graded emissive in the shader. Iron melts at ~1500°C — above lava (~1150°C), so lava
//! makes iron glow but won't melt it (a hotter source — forge/dragon-breath — is needed to melt).

#[inline] fn clamp01(x: f32) -> f32 { x.max(0.0).min(1.0) }
#[inline] fn mix(a: f32, b: f32, t: f32) -> f32 { a + (b - a) * t }

pub const AMBIENT_C: f32 = 18.0;
pub const GLOW_ONSET_C: f32 = 500.0;
pub const MELT_C: f32 = 1500.0;
pub const LAVA_C: f32 = 1150.0;
pub const FIRE_C: f32 = 800.0;

#[derive(Clone, Copy)]
pub struct ThermalParams {
    pub conduct_rate: f32,
    pub cool_rate: f32,
    pub quench_rate: f32,
}
impl Default for ThermalParams {
    fn default() -> Self { ThermalParams { conduct_rate: 0.6, cool_rate: 0.12, quench_rate: 3.5 } }
}

/// Advance a metal cell's temperature (°C) by `dt` seconds.
pub fn step_metal(temp_c: f32, neighbor_hot_c: f32, in_water: bool, dt: f32, p: ThermalParams) -> f32 {
    let mut t = temp_c;
    if neighbor_hot_c > t {
        t += (neighbor_hot_c - t) * (p.conduct_rate * dt).min(1.0);
    }
    let rate = if in_water { p.quench_rate } else { p.cool_rate };
    let over = (t - AMBIENT_C).max(0.0);
    t -= over * (rate * dt).min(1.0) * (1.0 + over / 800.0);
    t.max(AMBIENT_C)
}

/// Flash a steam puff when hot metal touches water (quench).
pub fn quench_steam(temp_c: f32, in_water: bool) -> bool { in_water && temp_c > 120.0 }

/// Reached melting point — caller converts the block to molten/lava.
pub fn is_molten(temp_c: f32) -> bool { temp_c >= MELT_C }

pub struct Glow { pub color: [f32; 3], pub intensity: f32 }

/// Blackbody incandescence (dull red → orange → yellow → white) and a 0..1 intensity.
pub fn glow(temp_c: f32) -> Glow {
    if temp_c < GLOW_ONSET_C {
        return Glow { color: [0.0, 0.0, 0.0], intensity: 0.0 };
    }
    const ANCHORS: [(f32, [f32; 3]); 6] = [
        (500.0, [0.55, 0.05, 0.0]),
        (700.0, [0.85, 0.12, 0.0]),
        (950.0, [1.0, 0.42, 0.05]),
        (1150.0, [1.0, 0.68, 0.22]),
        (1350.0, [1.0, 0.88, 0.55]),
        (1600.0, [1.0, 0.97, 0.92]),
    ];
    let t = temp_c.min(1600.0);
    let mut color = ANCHORS[ANCHORS.len() - 1].1;
    for w in ANCHORS.windows(2) {
        let (t0, c0) = w[0];
        let (t1, c1) = w[1];
        if t <= t1 {
            let f = clamp01((t - t0) / (t1 - t0));
            color = [mix(c0[0], c1[0], f), mix(c0[1], c1[1], f), mix(c0[2], c1[2], f)];
            break;
        }
    }
    Glow { color, intensity: clamp01((temp_c - GLOW_ONSET_C) / (1600.0 - GLOW_ONSET_C)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    const P: ThermalParams = ThermalParams { conduct_rate: 0.6, cool_rate: 0.12, quench_rate: 3.5 };
    #[test] fn heats_toward_lava() {
        let mut t = AMBIENT_C;
        for _ in 0..120 { t = step_metal(t, LAVA_C, false, 0.1, P); }
        assert!(t > 600.0 && t <= LAVA_C);
    }
    #[test] fn cools_in_air() {
        let mut t = 1000.0;
        for _ in 0..200 { t = step_metal(t, AMBIENT_C, false, 0.1, P); }
        assert!(t < 200.0);
    }
    #[test] fn quench_faster_than_air() {
        let air = { let mut t = 1000.0; for _ in 0..20 { t = step_metal(t, AMBIENT_C, false, 0.1, P); } t };
        let water = { let mut t = 1000.0; for _ in 0..20 { t = step_metal(t, AMBIENT_C, true, 0.1, P); } t };
        assert!(water < air);
    }
    #[test] fn steam_when_hot_and_wet() {
        assert!(quench_steam(900.0, true) && !quench_steam(900.0, false) && !quench_steam(40.0, true));
    }
    #[test] fn lava_wont_melt_iron() {
        let mut t = AMBIENT_C;
        for _ in 0..1000 { t = step_metal(t, LAVA_C, false, 0.1, P); }
        assert!(!is_molten(t));
    }
    #[test] fn glow_dark_then_white() {
        assert_eq!(glow(300.0).intensity, 0.0);
        assert!(glow(1500.0).intensity > glow(800.0).intensity);
        assert!(glow(1500.0).color[2] > glow(700.0).color[2]);
    }
}
