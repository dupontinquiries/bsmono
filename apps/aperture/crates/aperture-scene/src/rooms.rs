//! rooms — the file as a place. Each outline item becomes a STATION along a
//! corridor: a plinth in its kind's material, a plaque carrying its name
//! (static inscription atlas), and — the part that makes spatial memory
//! work — its own environmental character. This is logscope's `EnvParams`
//! pattern fed by CODE shape instead of conversation shape: parameters in,
//! deterministic room character out, so `parse::header` is always the cool
//! blue room past the stone arch and you never get lost. Same file, same
//! rooms, every time — determinism is the whole point, so everything
//! derives from a name hash and a kind, never from randomness.

use aperture_core::syntax::{OutlineItem, OutlineKind};
use scene_kit::spatial::Vec3;

/// A room's environmental character — the subset of render uniforms that
/// carry mood. Fed to the frame's `Uniforms` (lerped during flight).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoomEnv {
    pub sun_col: [f32; 3],
    pub sky: [f32; 3],
    pub fog_col: [f32; 3],
    pub fog: f32,
    pub exposure: f32,
    pub saturation: f32,
}

pub const BASE_ENV: RoomEnv = RoomEnv {
    sun_col: [1.0, 0.92, 0.78],
    sky: [0.34, 0.40, 0.52],
    fog_col: [0.09, 0.095, 0.115],
    fog: 0.0045,
    exposure: 1.32,
    saturation: 1.1,
};

pub fn lerp_env(a: &RoomEnv, b: &RoomEnv, t: f32) -> RoomEnv {
    let t = t.clamp(0.0, 1.0);
    let l = |x: f32, y: f32| x + (y - x) * t;
    let l3 = |x: [f32; 3], y: [f32; 3]| [l(x[0], y[0]), l(x[1], y[1]), l(x[2], y[2])];
    RoomEnv {
        sun_col: l3(a.sun_col, b.sun_col),
        sky: l3(a.sky, b.sky),
        fog_col: l3(a.fog_col, b.fog_col),
        fog: l(a.fog, b.fog),
        exposure: l(a.exposure, b.exposure),
        saturation: l(a.saturation, b.saturation),
    }
}

fn hash(name: &str) -> u32 {
    // FNV-1a: tiny, stable, good spread for short idents.
    let mut h: u32 = 0x811c_9dc5;
    for b in name.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// Kind sets the room's BASE character; the name hash tilts hue and light so
/// two functions still feel like different rooms of the same wing.
pub fn room_env(kind: OutlineKind, name: &str) -> RoomEnv {
    let h = hash(name);
    let t1 = (h & 0xff) as f32 / 255.0;         // hue tilt
    let t2 = ((h >> 8) & 0xff) as f32 / 255.0;  // light tilt
    let mut e = BASE_ENV;
    match kind {
        OutlineKind::Fn => {
            // warm workshops
            e.sun_col = [1.0, 0.88 + 0.06 * t1, 0.66 + 0.12 * t2];
            e.sky = [0.36, 0.40, 0.50];
        }
        OutlineKind::Struct | OutlineKind::Enum => {
            // cool stone halls
            e.sun_col = [0.82, 0.90, 1.0];
            e.sky = [0.30 + 0.08 * t1, 0.42, 0.60];
            e.fog_col = [0.08, 0.10, 0.14];
        }
        OutlineKind::Trait => {
            // violet library
            e.sun_col = [0.92, 0.82, 1.0];
            e.sky = [0.40, 0.34, 0.56];
            e.saturation = 1.18;
        }
        OutlineKind::Impl => {
            // amber forge
            e.sun_col = [1.0, 0.78, 0.52];
            e.sky = [0.38, 0.36, 0.44];
            e.exposure = 1.38;
        }
        OutlineKind::Mod => {
            // deep gate: fog thickens, light drops — a threshold, not a room
            e.fog = 0.008;
            e.exposure = 1.2;
            e.sky = [0.28, 0.34, 0.46];
        }
        OutlineKind::Const => {
            e.saturation = 0.95;
        }
    }
    // name-hash tilt applied across the board so siblings differ
    e.sun_col[2] = (e.sun_col[2] + 0.10 * (t1 - 0.5)).clamp(0.3, 1.0);
    e.sky[0] = (e.sky[0] + 0.06 * (t2 - 0.5)).clamp(0.1, 0.7);
    e
}

/// Plinth material per kind (the context-panel materials, standing for what
/// they hold): fns on wood, types on stone, mods as moss gates, consts paper.
pub fn room_mat(kind: OutlineKind) -> f32 {
    match kind {
        OutlineKind::Fn => 1.0,
        OutlineKind::Struct | OutlineKind::Enum | OutlineKind::Trait => 2.0,
        OutlineKind::Impl => 1.0,
        OutlineKind::Mod => 4.0,
        OutlineKind::Const => 0.0,
    }
}

pub struct Station {
    pub item: OutlineItem,
    pub pos: Vec3,
    pub env: RoomEnv,
    /// Which atlas row carries this station's plaque text.
    pub atlas_row: usize,
    /// Churn heat in [0, 1] from `gitspace::heat` over the item's blame
    /// span: 1 = touched today, 0 = ancient (or no repo). Fresh masonry
    /// glows; old stone cools.
    pub heat: f32,
    /// Did the DIFF currently on screen touch this item's line span? Answers
    /// "what did this commit change" at a glance, independent of heat (heat
    /// is about the item's whole history; this is about the one diff shown
    /// right now — a revision preview, or the live worktree's uncommitted
    /// lines).
    pub touched: bool,
    /// Ancient by blame — heat below `DORMANT_HEAT`. `gitspace::heat` is
    /// asymptotic (half-life a week; it approaches zero but never quite
    /// reaches it for a line that's ever been committed at all), so this is
    /// a threshold, not an exact-zero check — "hasn't moved in months," not
    /// "was never written." A room reading that way feels different from
    /// one that's just quiet this week; logscope's `region_plan` has a
    /// `SealedCave` region type for exactly this shape of thing (dead
    /// branches), gated behind a comment admitting the real per-branch
    /// signal wasn't wired through yet and a seeded stand-in filled in for
    /// it instead. Aperture has the real signal (blame, not a guess) — this
    /// is that signal, not a placeholder.
    pub dormant: bool,
    /// The item's line span exceeds `LARGE_LINES` — logscope's `Chamber`
    /// (its "token-heavy" region, sized up because there's more of it).
    /// Independent of `dormant`: a room can be both large AND untouched.
    pub large: bool,
}

pub const STATION_SPACING: f32 = 15.0;
pub const CORRIDOR_HALF_W: f32 = 11.0;
/// Line-span threshold for `large`: long enough that "this is a big one"
/// is a fair read, short enough that ordinary multi-line functions don't
/// all qualify — roughly a screenful at the editor's default zoom.
pub const LARGE_LINES: usize = 15;
/// Heat threshold for `dormant`: below this reads as "hasn't moved in
/// months" (`gitspace::heat`'s half-life is a week, so ~0.05 corresponds to
/// roughly 19 half-lives — comfortably "ancient," not merely "quiet lately").
pub const DORMANT_HEAT: f32 = 0.05;

/// Lay the outline down a corridor along −z: stations alternate sides,
/// `mod` items sit centered (they read as gates, not rooms).
pub fn layout(outline: Vec<OutlineItem>) -> Vec<Station> {
    layout_full(outline, &[], 0, &[])
}

/// Layout with per-line last-touched times (`gitspace::line_ages`): each
/// station's heat is the MAX heat over its line span — one hot line makes a
/// warm room, because "what changed lately" is the question wayfinding
/// answers.
pub fn layout_with_ages(outline: Vec<OutlineItem>, ages: &[i64], now: i64) -> Vec<Station> {
    layout_full(outline, ages, now, &[])
}

/// The full layout: heat from blame ages, PLUS `touched` from whatever diff
/// is currently on screen (`changed`, half-open line ranges — the same
/// shape `gitspace::changed_ranges`/`worktree_changed` return). A station is
/// touched if ANY changed range overlaps its item's line span.
pub fn layout_full(outline: Vec<OutlineItem>, ages: &[i64], now: i64, changed: &[(usize, usize)]) -> Vec<Station> {
    outline
        .into_iter()
        .enumerate()
        .map(|(i, item)| {
            let z = -10.0 - i as f32 * STATION_SPACING;
            let x = if item.kind == OutlineKind::Mod {
                0.0
            } else if i % 2 == 0 {
                -(CORRIDOR_HALF_W - 3.0)
            } else {
                CORRIDOR_HALF_W - 3.0
            };
            let env = room_env(item.kind, &item.name);
            let heat = ages
                .get(item.line..=item.end_line.min(ages.len().saturating_sub(1)))
                .unwrap_or(&[])
                .iter()
                .map(|t| aperture_core::gitspace::heat(now, *t))
                .fold(0.0f32, f32::max);
            let touched = changed.iter().any(|(a, b)| item.line < *b && *a <= item.end_line);
            // "Never touched" is only a real claim when there was blame data
            // to have observed it in — no repo (or previewing a revision,
            // where ages isn't fetched) must read as UNKNOWN, not dormant.
            // heat==0 with no `ages` at all would otherwise mark every
            // station dormant, which is a claim the data can't support.
            let dormant = !ages.is_empty() && heat < DORMANT_HEAT;
            let large = item.end_line.saturating_sub(item.line) > LARGE_LINES;
            Station { pos: Vec3::new(x, 0.0, z), env, atlas_row: i, heat, touched, dormant, large, item }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: OutlineKind, name: &str, line: usize) -> OutlineItem {
        OutlineItem { kind, name: name.into(), sig: format!("sig of {name}"), line, end_line: line + 3 }
    }

    #[test]
    fn layout_is_deterministic_and_alternates() {
        let o = vec![item(OutlineKind::Fn, "a", 0), item(OutlineKind::Struct, "B", 4), item(OutlineKind::Mod, "gate", 8)];
        let s1 = layout(o.clone());
        let s2 = layout(o);
        assert_eq!(s1.len(), 3);
        assert!(s1.iter().zip(&s2).all(|(a, b)| a.pos == b.pos && a.env == b.env), "same file, same rooms");
        assert!(s1[0].pos.x < 0.0 && s1[1].pos.x > 0.0, "alternating sides");
        assert_eq!(s1[2].pos.x, 0.0, "mods sit centered — gates, not rooms");
        assert!(s1[2].pos.z < s1[1].pos.z && s1[1].pos.z < s1[0].pos.z, "monotonic corridor");
    }

    #[test]
    fn envs_are_deterministic_and_distinct() {
        let a = room_env(OutlineKind::Fn, "alpha");
        let a2 = room_env(OutlineKind::Fn, "alpha");
        let b = room_env(OutlineKind::Fn, "beta");
        assert_eq!(a, a2, "same name, same room, every time");
        assert_ne!(a, b, "sibling fns still feel different");
        let s = room_env(OutlineKind::Struct, "alpha");
        assert_ne!(a.sun_col, s.sun_col, "kind sets the wing's character");
        let gate = room_env(OutlineKind::Mod, "m");
        assert!(gate.fog > a.fog, "mod gates thicken the air");
    }

    #[test]
    fn env_lerp_moves_between_rooms() {
        let a = room_env(OutlineKind::Fn, "x");
        let b = room_env(OutlineKind::Trait, "y");
        let mid = lerp_env(&a, &b, 0.5);
        assert!((mid.sun_col[1] - (a.sun_col[1] + b.sun_col[1]) / 2.0).abs() < 1e-5);
        assert_eq!(lerp_env(&a, &b, 0.0), a);
        assert_eq!(lerp_env(&a, &b, 1.0), b);
    }
    #[test]
    fn heat_aggregates_max_over_the_item_span_and_defaults_cold() {
        let o = vec![item(OutlineKind::Fn, "old", 0), item(OutlineKind::Fn, "hot", 4)];
        let now = 1_000_000_000i64;
        // lines 0..=3 ancient, lines 4..=7 with one touched-now line.
        let mut ages = vec![now - 400 * 86_400; 8];
        ages[5] = now;
        let st = layout_with_ages(o.clone(), &ages, now);
        assert!(st[0].heat < 0.05, "ancient room stays cold: {}", st[0].heat);
        assert!(st[1].heat > 0.99, "one hot line makes a warm room: {}", st[1].heat);
        let bare = layout(o);
        assert_eq!(bare[0].heat, 0.0, "no repo, no heat");
    }

    #[test]
    fn touched_marks_only_stations_the_diff_actually_overlaps() {
        let o = vec![
            item(OutlineKind::Fn, "a", 0),   // lines 0..3
            item(OutlineKind::Fn, "b", 10),  // lines 10..13
            item(OutlineKind::Fn, "c", 20),  // lines 20..23
        ];
        // A diff touching lines 2..3 (inside a) and 15..16 (a gap between
        // b's end and c's start) — b should stay untouched despite being
        // "between" the changes.
        let st = layout_full(o, &[], 0, &[(2, 3), (15, 16)]);
        assert!(st[0].touched, "a overlaps 2..3");
        assert!(!st[1].touched, "b: 10..13 vs a gap at 15..16 — no overlap");
        assert!(!st[2].touched, "c: 20..23 vs 15..16 — no overlap");

        // A range spanning exactly a's boundary (end_line inclusive edge).
        let st2 = layout_full(vec![item(OutlineKind::Fn, "a", 0)], &[], 0, &[(3, 4)]);
        assert!(st2[0].touched, "diff at the item's own end_line (3) overlaps");
        let st3 = layout_full(vec![item(OutlineKind::Fn, "a", 0)], &[], 0, &[(4, 5)]);
        assert!(!st3[0].touched, "diff strictly past end_line does not");
    }

    #[test]
    fn plain_layout_and_layout_with_ages_default_untouched() {
        let st = layout(vec![item(OutlineKind::Fn, "a", 0)]);
        assert!(!st[0].touched);
        let st2 = layout_with_ages(vec![item(OutlineKind::Fn, "a", 0)], &[], 0);
        assert!(!st2[0].touched);
    }

    #[test]
    fn dormant_requires_real_blame_data_unknown_is_never_dormant() {
        let items = vec![item(OutlineKind::Fn, "a", 0)]; // lines 0..3
        // No ages at all (no repo / previewing a revision): UNKNOWN, not dormant.
        let unknown = layout_full(items.clone(), &[], 0, &[]);
        assert!(!unknown[0].dormant, "no blame data means we don't know, not that it's dead");

        // Real ages, all ~3 years old: heat is asymptotically near zero
        // (never exactly zero for a line that was ever committed at all —
        // that's WHY dormant is a threshold, not an exact-zero check).
        let now = 1_000_000_000i64;
        let ages = vec![now - 1000 * 86_400; 4];
        let ancient = layout_full(items.clone(), &ages, now, &[]);
        assert!(ancient[0].dormant, "genuinely observed, genuinely below the dormant threshold");

        // Real ages, recently touched: definitely not dormant.
        let ages_hot = vec![now; 4];
        let hot = layout_full(items, &ages_hot, now, &[]);
        assert!(!hot[0].dormant);
    }

    #[test]
    fn large_is_a_pure_line_span_check_independent_of_dormant() {
        let small = item(OutlineKind::Fn, "small", 0); // end_line = line+3 per the test helper
        let mut big = item(OutlineKind::Fn, "big", 100);
        big.end_line = big.line + 40;
        let st = layout_full(vec![small, big], &[], 0, &[]);
        assert!(!st[0].large, "3-line span: ordinary");
        assert!(st[1].large, "40-line span: large");
        // Both are "unknown" dormant-wise (no ages), independent of size.
        assert!(!st[0].dormant && !st[1].dormant);
    }

}

// ── the living light ───────────────────────────────────────────────────────
//
// A room's RoomEnv is its still character. This is the part that moves:
// each kind of code item gets one of scene-kit's physical light mechanisms
// (the same per-dot functions logscope's scenes use), and the room's whole
// light breathes with it. Deterministic like everything else here — the
// mechanism comes from the kind, the phase from the name hash — so a
// function's plaque always flickers like a reading lamp and a trait's
// always shifts like refracted light, and you know where you are before
// you read the name.

use scene_kit::mesh3d::{modulate_dot, SceneLight};

/// The mechanism for a kind of item. Chosen for what the kind IS:
/// a function is where you read (a lamp); a struct or enum is a fixed
/// crystalline thing (glints); a trait is many implementations seen
/// through one interface (the spectrum); an impl is work under a canopy
/// (dappled); a module is a corridor of openings (shafts); a const is set
/// in stone (sandstone).
pub fn room_light(kind: OutlineKind) -> SceneLight {
    match kind {
        OutlineKind::Fn => SceneLight::LampFlicker,
        OutlineKind::Struct | OutlineKind::Enum => SceneLight::CrystalGlint,
        OutlineKind::Trait => SceneLight::RainbowRefraction,
        OutlineKind::Impl => SceneLight::DappledCanopy,
        OutlineKind::Mod => SceneLight::CanopyShafts,
        OutlineKind::Const => SceneLight::SandstoneShimmer,
        _ => SceneLight::Uniform,
    }
}

/// The room's light at time `t`: (exposure multiplier, hue shift in degrees).
/// Sampled at the plaque's origin dot, so the whole room breathes as one —
/// the per-dot texture is the plaque's business, the room's is the mood.
/// Exposure stays within 0.85..1.05 of the room's own: alive, never dim.
pub fn breathe(kind: OutlineKind, name: &str, t: f32) -> (f32, f32) {
    let seed = hash(name);
    let (b, hue) = modulate_dot(room_light(kind), 0.0, 0.0, seed, t);
    (0.85 + 0.2 * b, hue * 0.25)
}

/// Dust with age. `heat` is the station's blame heat (1 = touched now,
/// 0 = ancient); old rooms hold more fog — the light shafts show in the
/// dust of a corridor nobody has walked in years — and a room touched
/// this session is crisp. Multiplier on the room's fog, 1.0..1.8.
pub fn dust(heat: f32) -> f32 {
    1.0 + 0.8 * (1.0 - heat.clamp(0.0, 1.0))
}

/// Rotate an RGB colour's hue by `deg` — a small, cheap approximation
/// (the YIQ rotation), enough to warm a sun a few degrees.
pub fn hue_rotate(c: [f32; 3], deg: f32) -> [f32; 3] {
    let a = deg.to_radians();
    let (s, co) = a.sin_cos();
    let y = 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
    let i = 0.596 * c[0] - 0.274 * c[1] - 0.322 * c[2];
    let q = 0.211 * c[0] - 0.523 * c[1] + 0.312 * c[2];
    let (i2, q2) = (i * co - q * s, i * s + q * co);
    [(y + 0.956 * i2 + 0.621 * q2).clamp(0.0, 2.0), (y - 0.272 * i2 - 0.647 * q2).clamp(0.0, 2.0), (y - 1.106 * i2 + 1.703 * q2).clamp(0.0, 2.0)]
}

#[cfg(test)]
mod living_light_tests {
    use super::*;

    #[test]
    fn every_kind_has_a_light_and_the_same_name_breathes_the_same_way() {
        for k in [OutlineKind::Fn, OutlineKind::Struct, OutlineKind::Enum, OutlineKind::Trait, OutlineKind::Impl, OutlineKind::Mod, OutlineKind::Const] {
            assert_ne!(room_light(k), SceneLight::Uniform, "{k:?}");
        }
        assert_eq!(breathe(OutlineKind::Fn, "parse_header", 2.5), breathe(OutlineKind::Fn, "parse_header", 2.5));
        let mut lo = f32::MAX; let mut hi = f32::MIN;
        for k in 0..200 {
            let (e, h) = breathe(OutlineKind::Fn, "parse_header", k as f32 * 0.05);
            assert!((0.85..=1.05).contains(&e), "exposure {e}");
            assert!(h.abs() <= 90.0, "hue {h}");
            lo = lo.min(e); hi = hi.max(e);
        }
        assert!(hi - lo > 0.03, "the lamp does not breathe: {lo}..{hi}");
    }

    #[test]
    fn dust_thickens_with_age_and_hue_rotation_is_gentle() {
        assert!((dust(1.0) - 1.0).abs() < 1e-6);
        assert!((dust(0.0) - 1.8).abs() < 1e-6);
        assert!(dust(0.5) > dust(0.9));
        let sun = [1.0, 0.92, 0.78];
        let warm = hue_rotate(sun, 9.0);
        assert!((warm[0] - sun[0]).abs() < 0.15 && (warm[2] - sun[2]).abs() < 0.15, "{warm:?}");
        // Textbook YIQ constants round-trip to within a percent, which is all a mood needs.
        assert!(hue_rotate(sun, 0.0).iter().zip(sun.iter()).all(|(a, b)| (*a - *b).abs() < 0.02));
    }
}

