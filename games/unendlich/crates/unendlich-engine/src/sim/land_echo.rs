//! `sim::land_echo` — how a loud sound crosses open terrain to a listener.
//!
//! Built for a dragon's roar, which is heard from far enough away that the
//! land shapes it. Given where the sound starts, where you stand, which way you
//! face, and a terrain height lookup, [`plan`] returns:
//!
//! * the **direct path** — arrival time at the speed of sound, inverse-distance
//!   gain, air absorption (the far roar is duller), and **ridge occlusion**: if
//!   terrain rises through the line of sight the sound diffracts over it,
//!   quieter and much duller;
//! * **reflections** off steep terrain faces — hillsides and cliffs found by
//!   marching rays out from the listener. Each is a discrete echo with its own
//!   delay (the extra path length), gain, dullness, and stereo position (from
//!   the face's direction, via the engine's `sound_stereo` convention). A cliff
//!   200 m off your right shoulder answers ~1.2 s later, from the right;
//! * a **tail** — the diffuse wash that is left: a valley rings long and wet,
//!   an open plain gives a short, faint one.
//!
//! Terrain only (the height lookup): no per-block geometry, trees, or player
//! builds. That is the resolution "echoing over the land" needs and the cost a
//! phone can pay — the default [`EchoParams`] probe is 16 rays × 40 samples.
//!
//! Pure. Rendering the plan to samples is `sound_render`.

use crate::sim::sound_stereo;
use std::f32::consts::TAU;

/// Speed of sound (m/s = blocks/s).
pub const SPEED_OF_SOUND: f32 = 343.0;

/// One way the sound reaches the listener.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Path {
    /// Seconds from emission to arrival.
    pub delay_s: f32,
    /// Linear gain (1.0 = the source heard from [`EchoParams::ref_dist`]).
    pub left: f32,
    pub right: f32,
    /// One-pole low-pass cutoff for this path (air, surfaces, diffraction).
    pub lowpass_hz: f32,
}

impl Path {
    pub fn gain(&self) -> f32 {
        (self.left * self.left + self.right * self.right).sqrt()
    }
}

/// The diffuse remainder.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tail {
    /// Linear level of the tail relative to the source at `ref_dist`.
    pub level: f32,
    /// RT60-style decay (s).
    pub decay_s: f32,
    /// Fraction of probe rays that found a reflecting face (0 open .. 1 walled in).
    pub enclosure: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EchoPlan {
    pub direct: Path,
    /// Terrain rose through the line of sight.
    pub occluded: bool,
    /// How far the highest terrain between rises above the line of sight (m;
    /// negative = clear by that much).
    pub ridge_m: f32,
    /// Strongest first; at most [`EchoParams::max_reflections`].
    pub reflections: Vec<Path>,
    pub tail: Tail,
    /// Straight-line distance (m).
    pub distance: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EchoParams {
    pub rays: u32,
    pub max_dist: f32,
    pub step: f32,
    /// Ear height above the listener's feet… callers pass the eye position,
    /// so this is only used to test faces against the listener's own level.
    pub face_min_rise: f32,
    /// Minimum rise per sample for terrain to count as a face (not a slope).
    pub face_min_slope: f32,
    pub max_reflections: usize,
    /// Distance at which gain is 1.0.
    pub ref_dist: f32,
}

impl Default for EchoParams {
    fn default() -> Self {
        EchoParams {
            rays: 16,
            max_dist: 320.0,
            step: 8.0,
            face_min_rise: 4.0,
            face_min_slope: 0.45,
            max_reflections: 6,
            ref_dist: 6.0,
        }
    }
}

#[inline]
fn dist3(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Air absorption as a low-pass: bright up close, ~2.5 kHz by 600 m.
pub fn air_lowpass(path_m: f32) -> f32 {
    (16_000.0 * (-path_m.max(0.0) / 320.0).exp()).max(1_800.0)
}

fn inv_dist(path_m: f32, ref_dist: f32) -> f32 {
    (ref_dist / path_m.max(ref_dist)).min(1.0)
}

/// Build the plan. `height(x, z)` is the terrain top at a column (blocks), or
/// `None` where unknown (treated as open — nothing to reflect or occlude).
pub fn plan(
    source: [f32; 3],
    listener: [f32; 3],
    listener_forward: [f32; 3],
    height: &impl Fn(i32, i32) -> Option<f32>,
    p: &EchoParams,
) -> EchoPlan {
    let d = dist3(source, listener);
    let step = p.step.max(1.0);

    // ── direct path + ridge occlusion ────────────────────────────────────────
    let samples = ((d / step) as usize).clamp(1, 128);
    let mut ridge = f32::MIN;
    for i in 1..samples {
        let t = i as f32 / samples as f32;
        let x = source[0] + (listener[0] - source[0]) * t;
        let y = source[1] + (listener[1] - source[1]) * t;
        let z = source[2] + (listener[2] - source[2]) * t;
        if let Some(h) = height(x.floor() as i32, z.floor() as i32) {
            ridge = ridge.max(h - y);
        }
    }
    let occluded = ridge > 1.5;
    let ridge = if ridge == f32::MIN { 0.0 } else { ridge };
    let mut g = inv_dist(d, p.ref_dist);
    let mut lp = air_lowpass(d);
    if occluded {
        // Diffraction over a ridge: the higher it stands over the line, the
        // more is lost; high frequencies go first.
        g *= (0.35 / (1.0 + ridge / 10.0)).max(0.08);
        lp = lp.min(700.0 / (1.0 + ridge / 20.0)).max(180.0);
    }
    let sg = sound_stereo::stereo(g, listener, listener_forward, source);
    let direct = Path { delay_s: d / SPEED_OF_SOUND, left: sg.left, right: sg.right, lowpass_hz: lp };

    // ── reflections: march rays out from the listener for steep faces ───────
    let rays = p.rays.max(1);
    let mut found: Vec<(Path, f32)> = Vec::new();
    let mut hits = 0u32;
    let mut hit_dist_sum = 0.0f32;
    for k in 0..rays {
        let a = TAU * k as f32 / rays as f32;
        let (dx, dz) = (a.cos(), a.sin());
        let mut prev = height(listener[0].floor() as i32, listener[2].floor() as i32);
        let mut r = step;
        while r <= p.max_dist {
            let x = listener[0] + dx * r;
            let z = listener[2] + dz * r;
            let h = height(x.floor() as i32, z.floor() as i32);
            if let (Some(h), Some(ph)) = (h, prev) {
                let rise_over_ear = h - listener[1];
                let slope = (h - ph) / step;
                if rise_over_ear > p.face_min_rise && slope > p.face_min_slope {
                    // The face: reflect from its mid-height, where it faces us.
                    let rp = [x, listener[1] + rise_over_ear * 0.5, z];
                    let path_len = dist3(source, rp) + dist3(rp, listener);
                    // Taller and steeper faces return more; a bounce costs ~40%.
                    let reflectivity = ((rise_over_ear / 25.0).clamp(0.2, 1.0) * slope.min(1.5) / 1.5).clamp(0.1, 1.0) * 0.6;
                    let gain = reflectivity * inv_dist(path_len, p.ref_dist);
                    let sg = sound_stereo::stereo(gain, listener, listener_forward, rp);
                    let path = Path {
                        delay_s: path_len / SPEED_OF_SOUND,
                        left: sg.left,
                        right: sg.right,
                        lowpass_hz: air_lowpass(path_len) * 0.6,
                    };
                    found.push((path, gain));
                    hits += 1;
                    hit_dist_sum += r;
                    break;
                }
            }
            prev = h.or(prev);
            r += step;
        }
    }
    // Merge echoes that arrive within 25 ms of each other (the ear fuses them),
    // keeping the louder, then keep the strongest few.
    found.sort_by(|a, b| a.0.delay_s.partial_cmp(&b.0.delay_s).unwrap_or(std::cmp::Ordering::Equal));
    let mut merged: Vec<(Path, f32)> = Vec::new();
    for (path, gain) in found {
        match merged.last_mut() {
            Some((lp, lg)) if path.delay_s - lp.delay_s < 0.025 => {
                if gain > *lg {
                    *lp = path;
                    *lg = gain;
                }
            }
            _ => merged.push((path, gain)),
        }
    }
    // An "echo" that lands within 40 ms of the direct sound is not heard as
    // one; drop those.
    merged.retain(|(path, _)| path.delay_s - direct.delay_s > 0.04);
    merged.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    merged.truncate(p.max_reflections);
    let reflections: Vec<Path> = merged.into_iter().map(|(path, _)| path).collect();

    // ── tail ────────────────────────────────────────────────────────────────
    let enclosure = hits as f32 / rays as f32;
    let mean_face = if hits > 0 { hit_dist_sum / hits as f32 } else { p.max_dist };
    // A walled valley rings longer the wider it is; open land barely rings.
    let decay_s = 0.6 + 3.2 * enclosure * (mean_face / p.max_dist).clamp(0.15, 1.0);
    // The reverberant field falls off slower than the direct sound, so far
    // away the tail is a larger share of what you hear.
    let level = (0.03 + 0.30 * enclosure) * inv_dist(d * 0.35 + 10.0, p.ref_dist);
    EchoPlan { direct, occluded, ridge_m: ridge, reflections, tail: Tail { level, decay_s, enclosure }, distance: d }
}

/// Extra path length (m) a sound must travel over the terrain between `a`
/// and `b`: 0 with a clear line; otherwise the detour over the highest crest
/// that stands above the line (the input to a diffraction loss, e.g.
/// `dragon_ears::arriving_db`). Samples every `step` m, at most 48 samples.
pub fn barrier_path_diff(a: [f32; 3], b: [f32; 3], height: &impl Fn(i32, i32) -> Option<f32>, step: f32) -> f32 {
    let d = dist3(a, b);
    if !(d > 1.0) {
        return 0.0;
    }
    let n = ((d / step.max(1.0)) as usize).clamp(2, 48);
    let mut best: Option<([f32; 3], f32)> = None;
    for i in 1..n {
        let t = i as f32 / n as f32;
        let x = a[0] + (b[0] - a[0]) * t;
        let y = a[1] + (b[1] - a[1]) * t;
        let z = a[2] + (b[2] - a[2]) * t;
        if let Some(h) = height(x.floor() as i32, z.floor() as i32) {
            let over = h - y;
            if over > 0.0 && best.map_or(true, |(_, o)| over > o) {
                best = Some(([x, h, z], over));
            }
        }
    }
    match best {
        Some((top, _)) => (dist3(a, top) + dist3(top, b) - d).max(0.0),
        None => 0.0,
    }
}

/// One line for a log.
pub fn describe(e: &EchoPlan) -> String {
    let echoes: Vec<String> = e
        .reflections
        .iter()
        .map(|r| format!("{:.2}s {}", r.delay_s - e.direct.delay_s, if r.right > r.left * 1.3 { "R" } else if r.left > r.right * 1.3 { "L" } else { "C" }))
        .collect();
    format!(
        "{:.0} m, arrives {:.2}s{}, {} echo(es) [{}], tail {:.1}s (enclosure {:.2})",
        e.distance,
        e.direct.delay_s,
        if e.occluded { format!(", over a {:.0} m ridge", e.ridge_m) } else { String::new() },
        e.reflections.len(),
        echoes.join(", "),
        e.tail.decay_s,
        e.tail.enclosure
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const FWD: [f32; 3] = [1.0, 0.0, 0.0]; // listener faces +x

    fn flat(_x: i32, _z: i32) -> Option<f32> {
        Some(60.0)
    }

    #[test]
    fn open_plain_direct_only_short_faint_tail() {
        let e = plan([100.0, 62.0, 0.0], [0.0, 61.6, 0.0], FWD, &flat, &EchoParams::default());
        assert!(!e.occluded);
        assert!(e.reflections.is_empty());
        assert!((e.direct.delay_s - 100.0 / SPEED_OF_SOUND).abs() < 1e-3);
        assert!(e.tail.enclosure == 0.0 && e.tail.decay_s < 1.0);
        // Inverse distance.
        let near = plan([12.0, 62.0, 0.0], [0.0, 61.6, 0.0], FWD, &flat, &EchoParams::default());
        assert!(near.direct.gain() > e.direct.gain() * 5.0);
        assert!(near.direct.lowpass_hz > e.direct.lowpass_hz);
    }

    #[test]
    fn a_cliff_answers_late_and_from_its_side() {
        // A 40 m wall along z = -150 (to the listener's... side per sound_stereo).
        let cliff = |_x: i32, z: i32| Some(if z <= -150 { 100.0 } else { 60.0 });
        let e = plan([30.0, 62.0, 0.0], [0.0, 61.6, 0.0], FWD, &cliff, &EchoParams::default());
        assert!(!e.reflections.is_empty(), "{}", describe(&e));
        let first = e.reflections.iter().min_by(|a, b| a.delay_s.partial_cmp(&b.delay_s).unwrap()).unwrap();
        let extra = first.delay_s - e.direct.delay_s;
        // Path ≈ src→wall→listener ≈ 150 + 153 against a 30 m direct: ~0.8 s later.
        assert!((0.6..1.1).contains(&extra), "echo {extra:.2}s later");
        // Panned toward the wall, the same way sound_stereo pans a source there.
        let wall_side = sound_stereo::stereo(1.0, [0.0, 61.6, 0.0], FWD, [0.0, 80.0, -150.0]);
        assert_eq!(first.right > first.left, wall_side.right > wall_side.left);
        assert!(first.gain() < e.direct.gain());
        assert!(first.lowpass_hz < e.direct.lowpass_hz);
    }

    #[test]
    fn a_ridge_between_muffles_the_direct_sound() {
        let ridge = |x: i32, _z: i32| Some(if (40..48).contains(&x) { 85.0 } else { 60.0 });
        let open = plan([90.0, 62.0, 0.0], [0.0, 61.6, 0.0], FWD, &flat, &EchoParams::default());
        let over = plan([90.0, 62.0, 0.0], [0.0, 61.6, 0.0], FWD, &ridge, &EchoParams::default());
        assert!(over.occluded && !open.occluded);
        assert!(over.direct.gain() < open.direct.gain() * 0.4);
        assert!(over.direct.lowpass_hz < 1000.0);
    }

    #[test]
    fn a_valley_rings_longer_and_wetter_than_a_plain() {
        let valley = |_x: i32, z: i32| Some(if z.abs() > 60 { 110.0 } else { 60.0 });
        let v = plan([80.0, 62.0, 0.0], [0.0, 61.6, 0.0], FWD, &valley, &EchoParams::default());
        let p = plan([80.0, 62.0, 0.0], [0.0, 61.6, 0.0], FWD, &flat, &EchoParams::default());
        assert!(v.tail.enclosure > 0.3, "{}", describe(&v));
        assert!(v.tail.decay_s > p.tail.decay_s + 0.5);
        assert!(v.tail.level > p.tail.level * 2.0);
        assert!(v.reflections.len() >= 2);
    }

    #[test]
    fn gentle_hills_are_not_cliffs() {
        // Rises 30 m over 300 m: tall, but 0.1 per metre — no face.
        let hill = |x: i32, z: i32| Some(60.0 + ((x * x + z * z) as f32).sqrt() * 0.1);
        let e = plan([50.0, 66.0, 0.0], [0.0, 61.6, 0.0], FWD, &hill, &EchoParams::default());
        assert!(e.reflections.is_empty(), "{}", describe(&e));
    }

    #[test]
    fn echoes_are_merged_and_capped() {
        // Walled on every side at 100 m: 16 rays all hit, arriving together.
        let bowl = |x: i32, z: i32| Some(if ((x * x + z * z) as f32).sqrt() > 100.0 { 120.0 } else { 60.0 });
        let p = EchoParams::default();
        let e = plan([0.0, 62.0, 0.0], [0.0, 61.6, 0.0], FWD, &bowl, &p);
        assert!(e.reflections.len() <= p.max_reflections);
        let mut d: Vec<f32> = e.reflections.iter().map(|r| r.delay_s).collect();
        d.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!(d.windows(2).all(|w| w[1] - w[0] >= 0.025 - 1e-6), "{d:?}");
    }

    #[test]
    fn barrier_detour_grows_with_the_crest() {
        let a = [0.0, 61.6, 0.0];
        let b = [100.0, 62.0, 0.0];
        assert_eq!(barrier_path_diff(a, b, &flat, 4.0), 0.0);
        let low = |x: i32, _z: i32| Some(if (48..52).contains(&x) { 66.0 } else { 60.0 });
        let high = |x: i32, _z: i32| Some(if (48..52).contains(&x) { 90.0 } else { 60.0 });
        let (dl, dh) = (barrier_path_diff(a, b, &low, 4.0), barrier_path_diff(a, b, &high, 4.0));
        assert!(dl > 0.0 && dh > dl * 5.0, "low {dl:.2} high {dh:.2}");
        // Geometry: a 28 m crest halfway along a 100 m line ≈ 2·√(50²+28²) − 100.
        assert!((dh - (2.0 * (50.0f32 * 50.0 + 28.0 * 28.0).sqrt() - 100.0)).abs() < 3.0);
    }

    #[test]
    fn unknown_terrain_is_open_and_nothing_panics() {
        let none = |_x: i32, _z: i32| None;
        let e = plan([10.0, 60.0, 0.0], [0.0, 60.0, 0.0], FWD, &none, &EchoParams::default());
        assert!(!e.occluded && e.reflections.is_empty());
        let same = plan([0.0, 60.0, 0.0], [0.0, 60.0, 0.0], [0.0, 0.0, 0.0], &flat, &EchoParams::default());
        assert!(same.direct.left.is_finite() && same.direct.right.is_finite());
    }
}
