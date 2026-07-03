//! Live, lock-free debug toggles + atmosphere "looks", driven by the in-game
//! settings panel (Esc).
//!
//! Purpose: A/B many rendering / sim behaviours at runtime without recompiling
//! or restarting. The settings window writes these; render and sim code read
//! them. Access is through the process-global [`DEBUG`] — no `Arc`, no `Mutex`,
//! no plumbing through call sites. Reads are `Relaxed` atomics, correct here:
//! these are advisory debug switches, not synchronisation.
//!
//! The "looks" are the headline feature: each is a preset that retunes the
//! per-frame atmosphere (sun / sky / ambient / fog) the bin already builds, so
//! switching looks needs no shader recompile — just pick one in the panel.
//! `apply_look` operates on a `LookParams` value the caller fills from its own
//! atmosphere struct, so this module needs NO dependency on the renderer's
//! types (which keeps it from breaking when the render tree's layout moves).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};

/// Process-global debug settings. `const`-constructed so it lives in a `static`.
pub static DEBUG: DebugSettings = DebugSettings::new();

pub struct DebugSettings {
    /// Selected atmosphere look (see [`Look::ALL`]). 0 = default.
    pub look: AtomicU32,
    /// Live winding/cull sweep: index into the chunk pipeline's 6 front-face
    /// \u00d7 cull variants. Read at bind time, so changing it is instant
    /// (no re-mesh). 0 = CW + cull BACK (default).
    pub winding_variant: AtomicU32,
    /// Fragment-shader visualisation mode (see [`ShaderMode`]). Reserved for the
    /// `chunk.frag` debug-uniform hook; no-op until that uniform is wired.
    pub shader_mode: AtomicU32,
    /// Panel sets this to request a clean shutdown (the bin polls it).
    pub request_quit: AtomicBool,

    // ── Render toggles ─────────────────────────────────────────────────────
    pub disable_fog: AtomicBool,
    pub no_cull: AtomicBool,
    pub disable_ssr: AtomicBool,
    pub disable_bloom: AtomicBool,
    pub disable_shadows: AtomicBool,

    // ── World / sim toggles ────────────────────────────────────────────────
    pub freeze_lod: AtomicBool,
    pub freeze_time: AtomicBool,
    pub pause_sim: AtomicBool,
    pub mesher_mid_gpu: AtomicBool,
    pub neutral_tint: AtomicBool,
    pub show_chunk_borders: AtomicBool,

    // ── fog A/B (shipped debug knobs) ──────────────────────────────────────
    /// Fog rendering approach for A/B testing (see [`FogApproach`]). Read by
    /// the fog pass at its density source. 0 = off. (hook: wire at the fog
    /// shader / froxel-vs-analytic branch — the consuming pass lives wherever
    /// the active fog work is.)
    pub fog_approach: AtomicU32,
    /// MacCormack advection clamp for the 2.5D moisture sim. true = clamp the
    /// corrected value to its source-cell neighbourhood (monotone: no negative
    /// density, no ringing). false = raw MacCormack (sharper, can overshoot).
    pub advect_clamp: AtomicBool,
    /// Volumetric march step count — the main perf knob for the raymarching
    /// approaches. Read by the fog march. (hook)
    pub fog_steps: AtomicU32,
    /// r592: foliage-disturber strength multiplier (×10 fixed-point; 10 = ×1.0, the
    /// r591 default). A live F3 slider so the operator can amp the "plants part around
    /// you" push to confirm it's working. Uploaded in `sun_color.a`, read by chunk.vert.
    pub foliage_disturb: AtomicU32,
}

impl DebugSettings {
    pub const fn new() -> Self {
        Self {
            look: AtomicU32::new(0),
            // r119/r120: default 2 = CW + cull NONE — the variant the operator confirmed
            // on hardware. (This initializer is const, so the UNENDLICH_WINDING env
            // override happens at runtime in the bin, not here.)
            winding_variant: AtomicU32::new(2),
            shader_mode: AtomicU32::new(0),
            request_quit: AtomicBool::new(false),
            disable_fog: AtomicBool::new(false),
            no_cull: AtomicBool::new(false),
            disable_ssr: AtomicBool::new(false),
            disable_bloom: AtomicBool::new(false),
            disable_shadows: AtomicBool::new(false),
            freeze_lod: AtomicBool::new(false),
            freeze_time: AtomicBool::new(false),
            pause_sim: AtomicBool::new(false),
            mesher_mid_gpu: AtomicBool::new(false),
            neutral_tint: AtomicBool::new(false),
            show_chunk_borders: AtomicBool::new(false),
            fog_approach: AtomicU32::new(0),
            advect_clamp: AtomicBool::new(true),
            fog_steps: AtomicU32::new(12),
            foliage_disturb: AtomicU32::new(10), // ×10 fixed-point ⇒ ×1.0 default
        }
    }
}

/// Atmosphere looks shown in the panel. The integer is the value stored in
/// `DEBUG.look`; `apply_look` interprets it. These multiply/tint the base
/// atmosphere so the day/night cycle still drives it — a look only reshapes it.
pub struct Look;
impl Look {
    pub const ALL: &'static [(u32, &'static str)] = &[
        (0, "Default"),
        (1, "Clear (less fog)"),
        (2, "No fog"),
        (3, "Heavy fog"),
        (4, "Golden hour"),
        (5, "Blue hour"),
        (6, "Overcast"),
        (7, "Harsh noon"),
        (8, "Moody (low ambient)"),
        (9, "Light shafts (more rays)"),
        (10, "Dream haze"),
        (11, "Deep night"),
    ];
}

/// Fog approaches shown in the panel as an A/B selector. The integer is stored
/// in `DEBUG.fog_approach`; the fog pass reads it to pick its density source.
/// Labels carry a rough perf hint so it's obvious which option costs what.
pub struct FogApproach;
impl FogApproach {
    pub const ALL: &'static [(u32, &'static str)] = &[
        (0, "off"),
        (1, "froxel (current)"),
        (2, "analytic world fog  \u{00b7} cheap"),
        (3, "analytic + shadow-march  \u{00b7} $$"),
        (4, "advected 2.5D moisture  \u{00b7} sim + $$"),
    ];
}

#[inline]
fn mul3(c: &mut [f32; 3], k: f32) { c[0] *= k; c[1] *= k; c[2] *= k; }
// NOTE: must NOT be feature-gated — `apply_look_inner` (below, unconditional)
// calls this on every build. A prior `#[cfg(any(feature = "render", test))]`
// gate excluded it under the default `gpu` feature while its caller remained,
// breaking `cargo build`/`cargo test` of the engine without `render`.
#[inline]
fn lerp3(c: &mut [f32; 3], t: [f32; 3], f: f32) {
    for i in 0..3 { c[i] = c[i] * (1.0 - f) + t[i] * f; }
}

/// Apply the selected look + independent fog toggle to this frame's atmosphere.
/// Call once per frame after the atmos is built, before it's uploaded.
/// The subset of the per-frame atmosphere the Looks presets retune. A plain
/// owned struct so this module has NO dependency on the renderer's
/// `AtmosphereParams` (or wherever it lives in a given tree): the caller copies
/// the five fields in, calls [`apply_look`], and copies them back.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LookParams {
    pub fog_density: f32,
    pub sun_intensity: f32,
    pub sun_color: [f32; 3],
    pub sky_color: [f32; 3],
    pub ambient_color: [f32; 3],
}

pub fn apply_look(a: &mut LookParams) {
    apply_look_inner(DEBUG.look.load(Relaxed), DEBUG.disable_fog.load(Relaxed), a);
}

/// Pure form of [`apply_look`] (no global reads) — unit-testable.
fn apply_look_inner(look: u32, disable_fog: bool, a: &mut LookParams) {
    match look {
        1 => { a.fog_density *= 0.20; a.sun_intensity *= 1.10; }
        2 => { a.fog_density = 0.0; }
        3 => { a.fog_density *= 4.0; }
        4 => {
            lerp3(&mut a.sun_color, [1.0, 0.72, 0.40], 0.60);
            lerp3(&mut a.sky_color, [0.95, 0.65, 0.42], 0.35);
            lerp3(&mut a.ambient_color, [0.90, 0.70, 0.50], 0.40);
            a.sun_intensity *= 0.95;
        }
        5 => {
            a.sun_intensity *= 0.50;
            lerp3(&mut a.sky_color, [0.25, 0.35, 0.60], 0.50);
            lerp3(&mut a.ambient_color, [0.30, 0.40, 0.60], 0.50);
            a.fog_density *= 1.30;
        }
        6 => {
            a.sun_intensity *= 0.60;
            let g = [0.60, 0.62, 0.65];
            lerp3(&mut a.sky_color, g, 0.60);
            lerp3(&mut a.ambient_color, g, 0.50);
            a.fog_density *= 1.60;
        }
        7 => {
            a.sun_intensity *= 1.60;
            lerp3(&mut a.sun_color, [1.0, 0.98, 0.95], 0.70);
            a.fog_density *= 0.50;
            mul3(&mut a.ambient_color, 0.90);
        }
        8 => { mul3(&mut a.ambient_color, 0.35); a.sun_intensity *= 1.25; a.fog_density *= 1.10; }
        9 => {
            // Thick-ish fog + strong sun → the froxel volume in-scatters the
            // sun into visible shafts. Cheap "god rays" from existing tech.
            a.fog_density *= 2.20;
            a.sun_intensity *= 1.50;
            lerp3(&mut a.sun_color, [1.0, 0.85, 0.60], 0.50);
        }
        10 => {
            a.fog_density *= 1.80;
            mul3(&mut a.sky_color, 1.15);
            mul3(&mut a.ambient_color, 1.30);
            a.sun_intensity *= 1.10;
        }
        11 => {
            a.sun_intensity *= 0.12;
            mul3(&mut a.ambient_color, 0.30);
            lerp3(&mut a.sky_color, [0.05, 0.07, 0.15], 0.70);
        }
        _ => {}
    }
    if disable_fog { a.fog_density = 0.0; }
}

/// Fragment-shader debug visualisation modes — reserved for a future
/// `chunk.frag` debug-uniform hook (front/back faces, normals, overdraw, …).
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ShaderMode { Normal = 0, FrontFacing = 1, Normals = 2, Overdraw = 3, LodTint = 4 }

impl ShaderMode {
    pub const ALL: &'static [(u32, &'static str)] = &[
        (0, "Normal shading"),
        (1, "Front / back faces"),
        (2, "World normals"),
        (3, "Overdraw"),
        (4, "LOD tint"),
    ];
}

#[cfg(test)]
mod tests {
    use super::{mul3, lerp3};

    #[test]
    fn mul3_scales_each_channel() {
        let mut c = [1.0_f32, 2.0, 3.0];
        mul3(&mut c, 2.0);
        assert_eq!(c, [2.0, 4.0, 6.0]);
    }

    #[test]
    fn lerp3_endpoints_and_midpoint() {
        let mut a = [0.0_f32, 0.0, 0.0];
        lerp3(&mut a, [1.0, 1.0, 1.0], 0.0);
        assert_eq!(a, [0.0, 0.0, 0.0]);
        let mut b = [0.0_f32, 0.0, 0.0];
        lerp3(&mut b, [1.0, 1.0, 1.0], 1.0);
        assert_eq!(b, [1.0, 1.0, 1.0]);
        let mut m = [0.0_f32, 0.0, 0.0];
        lerp3(&mut m, [1.0, 1.0, 1.0], 0.5);
        assert_eq!(m, [0.5, 0.5, 0.5]);
    }

    #[test]
    fn apply_look_inner_fog_and_disable() {
        use super::{apply_look_inner, LookParams};
        let base = LookParams {
            fog_density: 0.012,
            sun_intensity: 1.0,
            sun_color: [1.0, 1.0, 1.0],
            sky_color: [0.5, 0.6, 0.8],
            ambient_color: [0.3, 0.3, 0.3],
        };
        // Look 2 = "no fog" zeroes density; look 3 = "heavy fog" scales it up.
        let mut a = base;
        apply_look_inner(2, false, &mut a);
        assert_eq!(a.fog_density, 0.0);
        let mut b = base;
        apply_look_inner(3, false, &mut b);
        assert!(b.fog_density > base.fog_density);
        // disable_fog overrides whatever the look did.
        let mut c = base;
        apply_look_inner(3, true, &mut c);
        assert_eq!(c.fog_density, 0.0);
        // Unknown look index is a no-op.
        let mut d = base;
        apply_look_inner(999, false, &mut d);
        assert_eq!(d, base);
    }
}
