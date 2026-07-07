//! sim::net::snapshot_interp — client-side interpolation of remote transforms.
//!
//! `entity_replication` delivers host→client snapshots newest-wins, but applying
//! each one directly makes remote players/entities *snap* on every network
//! update. This is the buffer that smooths them: it holds the last few
//! timestamped transforms per remote entity and, at render time, samples a point
//! `INTERP_DELAY` behind the newest data — interpolating between the two
//! bracketing snapshots (Valve/Overwatch-style interpolation). If the render
//! time runs past the newest sample (a dropped/late packet) it extrapolates from
//! the last two, capped at `MAX_EXTRAPOLATION`, then holds. Angles interpolate on
//! the shortest path across the ±π seam. Pure, deterministic, std-only; feed it
//! decoded `EntityState` (fixed-point → f32) and read a smooth transform back.

use std::collections::VecDeque;

/// Seconds behind the newest data to sample, to hide network jitter.
pub const INTERP_DELAY: f32 = 0.10;
/// Cap on how far past the newest sample we extrapolate before holding.
pub const MAX_EXTRAPOLATION: f32 = 0.25;
/// Max buffered samples per entity.
pub const BUFFER_CAP: usize = 32;

const PI: f32 = std::f32::consts::PI;
const TAU: f32 = std::f32::consts::TAU;

/// A timestamped remote transform (client clock seconds).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub time: f32,
    pub pos: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
}

/// Shortest-path angular interpolation (radians).
fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    // wrap the delta into [-π, π] so we turn the short way
    let mut d = (b - a) % TAU;
    if d > PI {
        d -= TAU;
    } else if d < -PI {
        d += TAU;
    }
    a + d * t
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp_sample(s0: &Sample, s1: &Sample, t: f32, time: f32) -> Sample {
    Sample {
        time,
        pos: [
            lerp(s0.pos[0], s1.pos[0], t),
            lerp(s0.pos[1], s1.pos[1], t),
            lerp(s0.pos[2], s1.pos[2], t),
        ],
        yaw: lerp_angle(s0.yaw, s1.yaw, t),
        pitch: lerp_angle(s0.pitch, s1.pitch, t),
    }
}

/// A per-entity interpolation buffer (samples kept in ascending time order).
#[derive(Clone, Debug, Default)]
pub struct InterpBuffer {
    samples: VecDeque<Sample>,
}

impl InterpBuffer {
    pub fn new() -> InterpBuffer {
        InterpBuffer { samples: VecDeque::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Time of the newest buffered sample, if any.
    pub fn latest_time(&self) -> Option<f32> {
        self.samples.back().map(|s| s.time)
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// Add a snapshot. Samples older than the newest are dropped (a late/reordered
    /// packet can't improve an already-newer interpolation); the buffer is capped
    /// at [`BUFFER_CAP`] (oldest evicted).
    pub fn push(&mut self, s: Sample) {
        if let Some(back) = self.samples.back() {
            if s.time <= back.time {
                return; // not newer than what we have
            }
        }
        self.samples.push_back(s);
        while self.samples.len() > BUFFER_CAP {
            self.samples.pop_front();
        }
    }

    /// Sample the transform at `render_time` (client clock). Interpolates between
    /// the two bracketing snapshots; clamps to the oldest if `render_time` is
    /// before the buffer; extrapolates from the last two (capped at
    /// [`MAX_EXTRAPOLATION`]) if it's past the newest, then holds. `None` only
    /// when the buffer is empty.
    pub fn sample(&self, render_time: f32) -> Option<Sample> {
        let n = self.samples.len();
        if n == 0 {
            return None;
        }
        let oldest = self.samples.front().unwrap();
        let newest = self.samples.back().unwrap();

        if render_time <= oldest.time {
            return Some(*oldest); // clamp: never extrapolate backwards
        }
        if render_time >= newest.time {
            // extrapolate along the last segment, capped, then hold
            if n >= 2 {
                let prev = &self.samples[n - 2];
                let span = newest.time - prev.time;
                if span > 1e-6 {
                    let ahead = (render_time - newest.time).min(MAX_EXTRAPOLATION);
                    let t = 1.0 + ahead / span;
                    return Some(lerp_sample(prev, newest, t, render_time));
                }
            }
            return Some(*newest); // single sample or degenerate span ⇒ hold
        }

        // find the bracket [s0, s1] with s0.time <= render_time < s1.time
        for w in 1..n {
            let s0 = &self.samples[w - 1];
            let s1 = &self.samples[w];
            if render_time < s1.time {
                let span = s1.time - s0.time;
                let t = if span > 1e-6 { (render_time - s0.time) / span } else { 0.0 };
                return Some(lerp_sample(s0, s1, t, render_time));
            }
        }
        Some(*newest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(time: f32, x: f32, yaw: f32) -> Sample {
        Sample { time, pos: [x, 0.0, 0.0], yaw, pitch: 0.0 }
    }

    #[test]
    fn empty_buffer_samples_none() {
        assert!(InterpBuffer::new().sample(1.0).is_none());
    }

    #[test]
    fn interpolates_between_two_snapshots() {
        let mut b = InterpBuffer::new();
        b.push(s(0.0, 0.0, 0.0));
        b.push(s(1.0, 10.0, 0.0));
        let mid = b.sample(0.5).unwrap();
        assert!((mid.pos[0] - 5.0).abs() < 1e-4, "position lerps to the midpoint");
        let quarter = b.sample(0.25).unwrap();
        assert!((quarter.pos[0] - 2.5).abs() < 1e-4);
    }

    #[test]
    fn clamps_before_the_oldest() {
        let mut b = InterpBuffer::new();
        b.push(s(1.0, 5.0, 0.0));
        b.push(s(2.0, 9.0, 0.0));
        let before = b.sample(0.0).unwrap();
        assert_eq!(before.pos[0], 5.0, "held at the oldest, no backward extrapolation");
    }

    #[test]
    fn extrapolates_past_the_newest_but_caps() {
        let mut b = InterpBuffer::new();
        b.push(s(0.0, 0.0, 0.0));
        b.push(s(1.0, 10.0, 0.0)); // velocity 10/s
        // a little past newest ⇒ extrapolated forward
        let ahead = b.sample(1.1).unwrap();
        assert!((ahead.pos[0] - 11.0).abs() < 1e-3, "extrapolated to 11");
        // far past ⇒ capped at MAX_EXTRAPOLATION (0.25s ⇒ +2.5)
        let capped = b.sample(5.0).unwrap();
        assert!((capped.pos[0] - 12.5).abs() < 1e-3, "extrapolation capped");
    }

    #[test]
    fn single_sample_holds() {
        let mut b = InterpBuffer::new();
        b.push(s(1.0, 7.0, 0.0));
        assert_eq!(b.sample(5.0).unwrap().pos[0], 7.0);
        assert_eq!(b.sample(0.0).unwrap().pos[0], 7.0);
    }

    #[test]
    fn yaw_interpolates_the_short_way_across_the_seam() {
        let mut b = InterpBuffer::new();
        // from -170° to +170° should cross through ±180° (Δ 20°), not sweep 340°.
        b.push(s(0.0, 0.0, -170f32.to_radians()));
        b.push(s(1.0, 0.0, 170f32.to_radians()));
        let mid = b.sample(0.5).unwrap();
        // midpoint of the short path is ±180° (≈ ±π)
        let near_pi = (mid.yaw.abs() - PI).abs() < 0.05;
        assert!(near_pi, "yaw took the short way: {} rad", mid.yaw);
    }

    #[test]
    fn out_of_order_and_capacity() {
        let mut b = InterpBuffer::new();
        b.push(s(2.0, 2.0, 0.0));
        b.push(s(1.0, 1.0, 0.0)); // older than newest ⇒ ignored
        assert_eq!(b.len(), 1);
        b.clear();
        for i in 0..(BUFFER_CAP as i32 + 10) {
            b.push(s(i as f32, i as f32, 0.0));
        }
        assert_eq!(b.len(), BUFFER_CAP, "capped");
        assert_eq!(b.latest_time(), Some((BUFFER_CAP as i32 + 9) as f32));
    }

    #[test]
    fn deterministic() {
        let mut b = InterpBuffer::new();
        b.push(s(0.0, 0.0, 0.0));
        b.push(s(1.0, 10.0, 0.0));
        assert_eq!(b.sample(0.37), b.sample(0.37));
    }
}
