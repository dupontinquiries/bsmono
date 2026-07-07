//! `voice_spatial` — pure spatial-audio parameters for game-aware voice chat. No audio I/O and
//! no networking here: this layer turns world geometry into the numbers a voice backend applies —
//! how much a remote speaker is attenuated and low-passed by the blocks between you (occlusion),
//! and how much reverb your space implies (a tight room rings differently than a cavern, and open
//! air barely rings at all). The world is abstracted behind a `solid_at` closure so it is fully
//! testable against a mock voxel field.
//!
//! Pipeline role: each frame, for every audible remote speaker, call [`occlusion`] with the local
//! listener + that speaker; once per frame call [`reverb`] for the listener's space. The backend
//! then sets per-source gain + a low-pass cutoff and a shared reverb send.

/// Occlusion result for one speaker→listener path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Occlusion {
    /// Linear gain in [0,1]: distance falloff × per-block muffling.
    pub gain: f32,
    /// Low-pass amount in [0,1]: 0 = open line of sight (full band), 1 = heavily muffled.
    pub lowpass: f32,
    /// Straight-line distance (blocks).
    pub distance: f32,
    /// Number of solid blocks the line passes through.
    pub solids: u32,
}

/// Tunables for [`occlusion`].
#[derive(Debug, Clone, Copy)]
pub struct VoiceParams {
    /// Distance (blocks) at which distance-gain has fallen to ~0.5.
    pub ref_distance: f32,
    /// Max distance (blocks) a voice carries at all.
    pub max_distance: f32,
    /// Gain multiplier per solid block crossed (0..1; lower = walls block more).
    pub per_block_gain: f32,
    /// Low-pass added per solid block (accumulates toward 1).
    pub per_block_lowpass: f32,
}

impl Default for VoiceParams {
    fn default() -> Self {
        Self {
            ref_distance: 16.0,
            max_distance: 96.0,
            per_block_gain: 0.55,
            per_block_lowpass: 0.40,
        }
    }
}

/// Count solid blocks on the segment a→b via a DDA voxel walk (Amanatides–Woo).
/// Endpoints' own cells are not counted as occluders (you and the speaker aren't walls).
pub fn solids_between(
    a: [f32; 3],
    b: [f32; 3],
    solid_at: &impl Fn(i32, i32, i32) -> bool,
) -> u32 {
    let dir = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    if len < 1e-6 {
        return 0;
    }
    let d = [dir[0] / len, dir[1] / len, dir[2] / len];
    let mut vox = [a[0].floor() as i32, a[1].floor() as i32, a[2].floor() as i32];
    let end = [b[0].floor() as i32, b[1].floor() as i32, b[2].floor() as i32];
    let step = [sign(d[0]), sign(d[1]), sign(d[2])];
    // distance along the ray to the next voxel boundary, and per-voxel increment, per axis
    let mut t_max = [0.0f32; 3];
    let mut t_delta = [f32::INFINITY; 3];
    for k in 0..3 {
        if d[k].abs() < 1e-9 {
            t_max[k] = f32::INFINITY;
        } else {
            let next = if d[k] > 0.0 {
                (vox[k] as f32 + 1.0) - a[k]
            } else {
                a[k] - vox[k] as f32
            };
            t_max[k] = next / d[k].abs();
            t_delta[k] = 1.0 / d[k].abs();
        }
    }
    let mut solids = 0u32;
    let mut guard = 0;
    let max_steps = (len as i32 + 3) * 3 + 8; // generous bound; prevents runaway
    while guard < max_steps {
        guard += 1;
        // advance to the next voxel along the smallest t_max axis
        let axis = if t_max[0] < t_max[1] {
            if t_max[0] < t_max[2] { 0 } else { 2 }
        } else if t_max[1] < t_max[2] {
            1
        } else {
            2
        };
        if t_max[axis] > len {
            break; // passed the endpoint
        }
        vox[axis] += step[axis];
        t_max[axis] += t_delta[axis];
        if vox == end {
            break; // reached the speaker's cell; don't count it
        }
        if solid_at(vox[0], vox[1], vox[2]) {
            solids += 1;
        }
    }
    solids
}

/// Compute the occlusion parameters for a speaker heard by a listener.
pub fn occlusion(
    listener: [f32; 3],
    speaker: [f32; 3],
    p: &VoiceParams,
    solid_at: &impl Fn(i32, i32, i32) -> bool,
) -> Occlusion {
    let dx = speaker[0] - listener[0];
    let dy = speaker[1] - listener[1];
    let dz = speaker[2] - listener[2];
    let distance = (dx * dx + dy * dy + dz * dz).sqrt();
    if distance >= p.max_distance {
        return Occlusion { gain: 0.0, lowpass: 1.0, distance, solids: 0 };
    }
    let solids = solids_between(listener, speaker, solid_at);
    // distance falloff: 1 at 0, ~0.5 at ref_distance, smoothly to 0 by max_distance
    let dist_gain = (p.ref_distance / (p.ref_distance + distance))
        * (1.0 - (distance / p.max_distance)).clamp(0.0, 1.0);
    let block_gain = p.per_block_gain.powi(solids as i32);
    let gain = (dist_gain * block_gain).clamp(0.0, 1.0);
    // low-pass saturates toward 1 with each block (1 - prod(1-k))
    let lowpass = (1.0 - (1.0 - p.per_block_lowpass).powi(solids as i32)).clamp(0.0, 1.0);
    Occlusion { gain, lowpass, distance, solids }
}

/// Reverb implied by the listener's space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reverb {
    /// Wet amount in [0,1] — how much reverb to mix in.
    pub wet: f32,
    /// Decay time (seconds) — bigger enclosed spaces ring longer.
    pub decay: f32,
    /// Enclosure in [0,1]: fraction of probe rays that hit a wall (1 = sealed, 0 = open sky).
    pub enclosure: f32,
}

/// 14 ray directions: 6 axes + 8 diagonals (normalized), a cheap spherical-ish probe.
fn probe_dirs() -> [[f32; 3]; 14] {
    let d = 1.0 / (3.0f32).sqrt();
    [
        [1.0, 0.0, 0.0], [-1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0], [0.0, -1.0, 0.0],
        [0.0, 0.0, 1.0], [0.0, 0.0, -1.0],
        [d, d, d], [d, d, -d], [d, -d, d], [d, -d, -d],
        [-d, d, d], [-d, d, -d], [-d, -d, d], [-d, -d, -d],
    ]
}

/// Estimate reverb at `listener` by casting probe rays out to `max_dist` and measuring how
/// enclosed and how large the space is. A small sealed room → bright, short, fairly wet; a large
/// cavern → long decay, very wet; open air → near-dry.
pub fn reverb(
    listener: [f32; 3],
    max_dist: f32,
    solid_at: &impl Fn(i32, i32, i32) -> bool,
) -> Reverb {
    let dirs = probe_dirs();
    let mut hits = 0u32;
    let mut hit_dist_sum = 0.0f32;
    for dir in dirs.iter() {
        let end = [
            listener[0] + dir[0] * max_dist,
            listener[1] + dir[1] * max_dist,
            listener[2] + dir[2] * max_dist,
        ];
        if let Some(dist) = first_hit(listener, end, max_dist, solid_at) {
            hits += 1;
            hit_dist_sum += dist;
        } else {
            hit_dist_sum += max_dist; // open direction counts as "far"
        }
    }
    let n = dirs.len() as f32;
    let enclosure = hits as f32 / n;
    let avg_dist = hit_dist_sum / n;
    // wet: enclosed spaces are wetter; open air stays dry even if one ray clips terrain
    let wet = (enclosure * enclosure).clamp(0.0, 1.0) * 0.9;
    // decay grows with the space size (avg distance to walls), 0.2s small room .. ~3s cavern
    let size01 = (avg_dist / max_dist).clamp(0.0, 1.0);
    let decay = 0.2 + size01 * 2.8;
    Reverb { wet, decay, enclosure }
}

/// First solid hit distance along a→b (skips the listener's own cell), or None if clear.
fn first_hit(
    a: [f32; 3],
    b: [f32; 3],
    max_dist: f32,
    solid_at: &impl Fn(i32, i32, i32) -> bool,
) -> Option<f32> {
    let dir = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    if len < 1e-6 {
        return None;
    }
    let d = [dir[0] / len, dir[1] / len, dir[2] / len];
    let mut vox = [a[0].floor() as i32, a[1].floor() as i32, a[2].floor() as i32];
    let step = [sign(d[0]), sign(d[1]), sign(d[2])];
    let mut t_max = [f32::INFINITY; 3];
    let mut t_delta = [f32::INFINITY; 3];
    for k in 0..3 {
        if d[k].abs() >= 1e-9 {
            let next = if d[k] > 0.0 {
                (vox[k] as f32 + 1.0) - a[k]
            } else {
                a[k] - vox[k] as f32
            };
            t_max[k] = next / d[k].abs();
            t_delta[k] = 1.0 / d[k].abs();
        }
    }
    let mut guard = 0;
    let max_steps = (max_dist as i32 + 3) * 3 + 8;
    while guard < max_steps {
        guard += 1;
        let axis = if t_max[0] < t_max[1] {
            if t_max[0] < t_max[2] { 0 } else { 2 }
        } else if t_max[1] < t_max[2] {
            1
        } else {
            2
        };
        let t = t_max[axis];
        if t > max_dist {
            return None;
        }
        vox[axis] += step[axis];
        t_max[axis] += t_delta[axis];
        if solid_at(vox[0], vox[1], vox[2]) {
            return Some(t);
        }
    }
    None
}

fn sign(v: f32) -> i32 {
    if v > 0.0 { 1 } else if v < 0.0 { -1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty(_x: i32, _y: i32, _z: i32) -> bool { false }

    #[test]
    fn clear_line_no_solids() {
        let n = solids_between([0.5, 0.5, 0.5], [10.5, 0.5, 0.5], &empty);
        assert_eq!(n, 0);
    }

    #[test]
    fn wall_between_counts() {
        // a single wall plane at x == 5
        let wall = |x: i32, _y: i32, _z: i32| x == 5;
        let n = solids_between([0.5, 0.5, 0.5], [10.5, 0.5, 0.5], &wall);
        assert_eq!(n, 1);
    }

    #[test]
    fn thicker_wall_more_solids() {
        let wall = |x: i32, _y: i32, _z: i32| (5..=7).contains(&x);
        let n = solids_between([0.5, 0.5, 0.5], [12.5, 0.5, 0.5], &wall);
        assert_eq!(n, 3);
    }

    #[test]
    fn occlusion_open_is_louder_than_walled() {
        let p = VoiceParams::default();
        let open = occlusion([0.5, 0.5, 0.5], [8.5, 0.5, 0.5], &p, &empty);
        let wall = |x: i32, _y: i32, _z: i32| x == 4;
        let walled = occlusion([0.5, 0.5, 0.5], [8.5, 0.5, 0.5], &p, &wall);
        assert!(open.gain > walled.gain);
        assert!(walled.lowpass > open.lowpass);
        assert_eq!(open.solids, 0);
        assert_eq!(walled.solids, 1);
    }

    #[test]
    fn occlusion_falls_with_distance() {
        let p = VoiceParams::default();
        let near = occlusion([0.0, 0.0, 0.0], [4.0, 0.0, 0.0], &p, &empty);
        let far = occlusion([0.0, 0.0, 0.0], [40.0, 0.0, 0.0], &p, &empty);
        assert!(near.gain > far.gain);
    }

    #[test]
    fn beyond_max_distance_is_silent() {
        let p = VoiceParams::default();
        let o = occlusion([0.0, 0.0, 0.0], [200.0, 0.0, 0.0], &p, &empty);
        assert_eq!(o.gain, 0.0);
    }

    #[test]
    fn open_air_is_dry() {
        let r = reverb([0.0, 40.0, 0.0], 32.0, &empty);
        assert_eq!(r.enclosure, 0.0);
        assert!(r.wet < 0.05);
    }

    #[test]
    fn sealed_box_is_wet_and_short() {
        // a 6-block-radius sealed cube around the listener
        let bx = |x: i32, y: i32, z: i32| x.abs() >= 6 || y.abs() >= 6 || z.abs() >= 6;
        let r = reverb([0.0, 0.0, 0.0], 64.0, &bx);
        assert!(r.enclosure > 0.99); // all rays hit a wall
        assert!(r.wet > 0.5);
        assert!(r.decay < 1.0); // small room → short decay
    }

    #[test]
    fn cavern_decays_longer_than_small_room() {
        let small = |x: i32, y: i32, z: i32| x.abs() >= 4 || y.abs() >= 4 || z.abs() >= 4;
        let big = |x: i32, y: i32, z: i32| x.abs() >= 40 || y.abs() >= 40 || z.abs() >= 40;
        let rs = reverb([0.0, 0.0, 0.0], 64.0, &small);
        let rb = reverb([0.0, 0.0, 0.0], 64.0, &big);
        assert!(rb.decay > rs.decay);
    }
}
