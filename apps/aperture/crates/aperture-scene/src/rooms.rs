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
}

pub const STATION_SPACING: f32 = 15.0;
pub const CORRIDOR_HALF_W: f32 = 11.0;

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
            Station { pos: Vec3::new(x, 0.0, z), env, atlas_row: i, heat, touched, item }
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

}
