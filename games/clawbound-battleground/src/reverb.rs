//! reverb -- the space sounds happen in: a stereo reverb and an echo, set by
//! where the listener is.
//!
//! [`SpaceFx`] is realtime DSP for the mixer's send bus (it runs in the audio
//! callback): a Freeverb-style reverb -- eight damped feedback combs and four
//! allpasses per ear, the right ear's delays spread from the left's so the
//! tail is wide -- and a stereo ping-pong echo whose feedback loop is
//! low-passed (each repeat duller, as off real rock). Every buffer is
//! allocated in `new`; parameter changes glide, so moving from a gorge into
//! a cave never clicks.
//!
//! [`space_at`] is the acoustics of a place: in the jungle, the existing
//! place model (dense damped foliage, the stone gates echoing); on Twinfang
//! Pass, measured from the canyon itself -- the echo's delay from the
//! distance to the nearest rock wall (a tight gorge slaps back fast, an
//! open basin answers late), and inside a cave a short, dense, ringing room.

use glam::Vec3;

/// A space's sound.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Space {
    /// Reverb size 0..1 (comb feedback).
    pub room: f32,
    /// High-frequency damping in the tail 0..1 (foliage absorbs; stone does not).
    pub damp: f32,
    /// Reverb level.
    pub wet: f32,
    /// Echo delay, seconds (0: no echo).
    pub echo_time: f32,
    /// Echo feedback 0..0.8 (how many repeats).
    pub echo_fb: f32,
    /// Echo level.
    pub echo_wet: f32,
}

impl Space {
    pub const DRY: Space = Space { room: 0.3, damp: 0.6, wet: 0.0, echo_time: 0.0, echo_fb: 0.0, echo_wet: 0.0 };
}

// Freeverb's tunings at 44.1 kHz; scaled to the device rate.
const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASSES: [usize; 4] = [556, 441, 341, 225];
const SPREAD: usize = 23;
/// Longest echo, seconds.
pub const MAX_ECHO: f32 = 1.2;

struct Comb {
    buf: Vec<f32>,
    i: usize,
    store: f32,
}

impl Comb {
    fn new(n: usize) -> Comb {
        Comb { buf: vec![0.0; n.max(1)], i: 0, store: 0.0 }
    }
    #[inline]
    fn run(&mut self, x: f32, fb: f32, damp: f32) -> f32 {
        let y = self.buf[self.i];
        self.store = y * (1.0 - damp) + self.store * damp;
        self.buf[self.i] = x + self.store * fb;
        self.i = (self.i + 1) % self.buf.len();
        y
    }
}

struct Allpass {
    buf: Vec<f32>,
    i: usize,
}

impl Allpass {
    fn new(n: usize) -> Allpass {
        Allpass { buf: vec![0.0; n.max(1)], i: 0 }
    }
    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let b = self.buf[self.i];
        let y = b - x;
        self.buf[self.i] = x + b * 0.5;
        self.i = (self.i + 1) % self.buf.len();
        y
    }
}

/// The reverb and echo, for one stereo send.
pub struct SpaceFx {
    combs: [Vec<Comb>; 2],
    allpasses: [Vec<Allpass>; 2],
    echo: [Vec<f32>; 2],
    echo_i: usize,
    echo_lp: [f32; 2],
    rate: f32,
    /// Where the parameters are now, and where they are gliding to.
    now: Space,
    pub target: Space,
}

impl SpaceFx {
    pub fn new(rate: u32) -> SpaceFx {
        let k = rate as f32 / 44_100.0;
        let scale = |n: usize| ((n as f32 * k) as usize).max(1);
        let mk = |spread: usize| -> (Vec<Comb>, Vec<Allpass>) {
            (COMBS.iter().map(|&n| Comb::new(scale(n + spread))).collect(), ALLPASSES.iter().map(|&n| Allpass::new(scale(n + spread))).collect())
        };
        let (cl, al) = mk(0);
        let (cr, ar) = mk(SPREAD);
        let echo_len = (MAX_ECHO * rate as f32) as usize + 1;
        SpaceFx {
            combs: [cl, cr],
            allpasses: [al, ar],
            echo: [vec![0.0; echo_len], vec![0.0; echo_len]],
            echo_i: 0,
            echo_lp: [0.0; 2],
            rate: rate as f32,
            now: Space::DRY,
            target: Space::DRY,
        }
    }

    /// Process `send` (interleaved stereo) and ADD the wet result into
    /// `out` (interleaved, `channels` wide). Allocation- and lock-free.
    pub fn process_add(&mut self, send: &[f32], out: &mut [f32], channels: usize) {
        let frames = (send.len() / 2).min(out.len() / channels.max(1));
        // glide the parameters toward the target over ~80 ms
        let k = 1.0 - (-(frames as f32) / (0.08 * self.rate)).exp();
        let t = self.target;
        let n = &mut self.now;
        n.room += (t.room - n.room) * k;
        n.damp += (t.damp - n.damp) * k;
        n.wet += (t.wet - n.wet) * k;
        n.echo_time += (t.echo_time - n.echo_time) * k;
        n.echo_fb += (t.echo_fb - n.echo_fb) * k;
        n.echo_wet += (t.echo_wet - n.echo_wet) * k;
        let fb = 0.7 + 0.28 * n.room.clamp(0.0, 1.0);
        let damp = n.damp.clamp(0.0, 0.95) * 0.4;
        let delay = ((n.echo_time.clamp(0.0, MAX_ECHO) * self.rate) as usize).clamp(1, self.echo[0].len() - 1);
        let efb = n.echo_fb.clamp(0.0, 0.8);
        let len = self.echo[0].len();
        for f in 0..frames {
            let (l, r) = (send[f * 2], send[f * 2 + 1]);
            let input = (l + r) * 0.015; // Freeverb's fixed input gain
            let mut wet = [0.0f32; 2];
            for ch in 0..2 {
                let mut acc = 0.0;
                for c in self.combs[ch].iter_mut() {
                    acc += c.run(input, fb, damp);
                }
                for a in self.allpasses[ch].iter_mut() {
                    acc = a.run(acc);
                }
                wet[ch] = acc;
            }
            // ping-pong echo: each ear's line feeds the other, low-passed
            let read = (self.echo_i + len - delay) % len;
            let (el, er) = (self.echo[0][read], self.echo[1][read]);
            self.echo_lp[0] += (er - self.echo_lp[0]) * 0.45;
            self.echo_lp[1] += (el - self.echo_lp[1]) * 0.45;
            self.echo[0][self.echo_i] = l + self.echo_lp[0] * efb;
            self.echo[1][self.echo_i] = r + self.echo_lp[1] * efb;
            self.echo_i = (self.echo_i + 1) % len;
            let o = &mut out[f * channels..f * channels + channels];
            let (wl, wr) = (wet[0] * n.wet + el * n.echo_wet, wet[1] * n.wet + er * n.echo_wet);
            if channels == 1 {
                o[0] += (wl + wr) * 0.5;
            } else {
                o[0] += wl;
                o[1] += wr;
            }
        }
        // no denormals creeping in on a silent tail
        for line in self.echo.iter_mut() {
            for v in [self.echo_i, (self.echo_i + len - 1) % len] {
                if line[v].abs() < 1e-20 {
                    line[v] = 0.0;
                }
            }
        }
    }
}

/// How a place sounds, for a listener at `pos` on the active map.
pub fn space_at(pos: Vec3) -> Space {
    match crate::map::active() {
        crate::map::MapId::Pass => pass_space(pos),
        crate::map::MapId::Jungle => jungle_space(pos),
    }
}

fn jungle_space(pos: Vec3) -> Space {
    use crate::audio::Place;
    let a = crate::audio::acoustics_at(pos);
    match a.kind {
        Place::Jungle => Space { room: 0.45, damp: 0.85, wet: 0.16, echo_time: 0.0, echo_fb: 0.0, echo_wet: 0.0 },
        Place::Lane => Space { room: 0.5, damp: 0.6, wet: 0.2, echo_time: 0.0, echo_fb: 0.0, echo_wet: 0.0 },
        Place::River => Space { room: 0.5, damp: 0.5, wet: 0.22, echo_time: 0.11, echo_fb: 0.15, echo_wet: 0.12 },
        Place::Base => Space { room: 0.85, damp: 0.3, wet: 0.38, echo_time: 0.26, echo_fb: 0.3, echo_wet: 0.22 },
        Place::CliffRim => Space { room: 0.75, damp: 0.35, wet: 0.3, echo_time: 0.42, echo_fb: 0.35, echo_wet: 0.28 },
        Place::Bridge => Space { room: 0.6, damp: 0.45, wet: 0.26, echo_time: 0.09, echo_fb: 0.2, echo_wet: 0.14 },
    }
}

fn pass_space(pos: Vec3) -> Space {
    // inside a cave: short, dense, ringing
    if crate::pass::caves().iter().any(|c| c.covers(pos.x, pos.z)) {
        return Space { room: 0.55, damp: 0.2, wet: 0.5, echo_time: 0.05, echo_fb: 0.25, echo_wet: 0.1 };
    }
    // the canyon: the echo comes back off the nearest rock wall. World
    // units are taken as metres and the path doubled (there and back),
    // then stretched x2.5 for drama -- a real 12 m slap-back is barely a
    // flam; this reads as a canyon.
    let wall = (-crate::pass::floor().sdf(pos.x, pos.z)).max(0.5);
    let echo_time = (wall * 2.0 / 343.0 * 2.5).clamp(0.06, MAX_ECHO * 0.9);
    // walls close: bright, strong echo; far: fainter, more repeats
    let near = (1.0 - wall / 40.0).clamp(0.0, 1.0);
    Space { room: 0.7 + 0.2 * near, damp: 0.25, wet: 0.24 + 0.1 * near, echo_time, echo_fb: 0.28 + 0.2 * (1.0 - near), echo_wet: 0.18 + 0.14 * near }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn impulse_response(fx: &mut SpaceFx, secs: f32) -> Vec<f32> {
        let rate = fx.rate as usize;
        let n = (secs * rate as f32) as usize;
        let mut send = vec![0.0f32; n * 2];
        send[0] = 1.0;
        send[1] = 1.0;
        let mut out = vec![0.0f32; n * 2];
        // the parameters glide in: settle them first
        fx.now = fx.target;
        for (s, o) in send.chunks(1024).zip(out.chunks_mut(1024)) {
            fx.process_add(s, o, 2);
        }
        out
    }

    /// A reverb tail: it arrives, it decays, it never blows up, and the two
    /// ears differ (a wide tail, not a mono one).
    #[test]
    fn the_reverb_tail_decays_and_is_wide() {
        let mut fx = SpaceFx::new(48_000);
        fx.target = Space { room: 0.85, damp: 0.3, wet: 1.0, echo_time: 0.0, echo_fb: 0.0, echo_wet: 0.0 };
        let out = impulse_response(&mut fx, 3.0);
        let energy = |a: usize, b: usize| out[a * 2..b * 2].iter().map(|x| x * x).sum::<f32>();
        let early = energy(0, 24_000);
        let late = energy(96_000, 120_000);
        assert!(early > 1e-4, "no tail: {early}");
        assert!(late < early * 0.2, "not decaying: {late} vs {early}");
        assert!(out.iter().all(|x| x.is_finite() && x.abs() < 4.0));
        let diff: f32 = out.chunks(2).map(|c| (c[0] - c[1]).abs()).sum();
        assert!(diff > 1e-3, "left and right identical: a mono tail");
    }

    /// The echo comes back after its delay, duller and quieter each time.
    #[test]
    fn the_echo_returns_on_time_and_fades() {
        let mut fx = SpaceFx::new(48_000);
        fx.target = Space { room: 0.3, damp: 0.5, wet: 0.0, echo_time: 0.25, echo_fb: 0.5, echo_wet: 1.0 };
        let out = impulse_response(&mut fx, 1.2);
        let mono: Vec<f32> = out.chunks(2).map(|c| c[0].abs() + c[1].abs()).collect();
        let peak = |a: usize, b: usize| (a..b).max_by(|&i, &j| mono[i].partial_cmp(&mono[j]).unwrap()).unwrap();
        let first = peak(1000, 20_000);
        assert!((first as i32 - 12_000).abs() < 50, "first echo at sample {first}, want 12000 (0.25 s)");
        let second = peak(first + 1000, first + 20_000);
        assert!((second as i32 - 24_000).abs() < 200, "second at {second}");
        assert!(mono[second] < mono[first], "each repeat quieter");
    }

    /// At the most feedback allowed, nothing runs away; a change of space
    /// glides rather than jumps.
    #[test]
    fn stable_at_full_feedback_and_changes_glide() {
        let mut fx = SpaceFx::new(44_100);
        fx.target = Space { room: 1.0, damp: 0.0, wet: 1.0, echo_time: 0.1, echo_fb: 1.0, echo_wet: 1.0 };
        let noise: Vec<f32> = (0..88_200).map(|i| ((i as f32 * 12.9898).sin() * 43758.545).fract() - 0.5).collect();
        let mut out = vec![0.0f32; noise.len()];
        for (s, o) in noise.chunks(512).zip(out.chunks_mut(512)) {
            fx.process_add(s, o, 2);
        }
        assert!(out.iter().all(|x| x.is_finite() && x.abs() < 20.0), "ran away");
        let mut g = SpaceFx::new(48_000);
        g.target = Space { wet: 1.0, ..Space::DRY };
        let mut o = vec![0.0f32; 256];
        g.process_add(&vec![0.0; 256], &mut o, 2);
        assert!(g.now.wet < 0.1, "jumped to the new space in one block: {}", g.now.wet);
    }

    /// The places sound like themselves: the pass's echo comes back later
    /// the further the walls are; a cave rings wetter than the open canyon.
    #[test]
    fn places_sound_like_themselves() {
        crate::map::with(crate::map::MapId::Pass, || {
            let f = crate::pass::floor();
            // sample the floor for a near-wall point and a far-from-wall one
            let (mut near, mut far) = (None, None);
            for x in (-200..=200).step_by(4) {
                for z in (-200..=200).step_by(4) {
                    let d = -f.sdf(x as f32, z as f32);
                    if d > 1.0 && d < 4.0 && near.is_none() {
                        near = Some(Vec3::new(x as f32, 0.0, z as f32));
                    }
                    if d > 20.0 && far.is_none() {
                        far = Some(Vec3::new(x as f32, 0.0, z as f32));
                    }
                }
            }
            let (near, far) = (space_at(near.unwrap()), space_at(far.unwrap()));
            assert!(far.echo_time > near.echo_time * 2.0, "{} vs {}", far.echo_time, near.echo_time);
            let cave = crate::pass::caves()[0];
            let mut inside = None;
            for x in (-220..=220).step_by(2) {
                for z in (-220..=220).step_by(2) {
                    if inside.is_none() && cave.covers(x as f32, z as f32) {
                        inside = Some(Vec3::new(x as f32, 0.0, z as f32));
                    }
                }
            }
            let c = space_at(inside.unwrap());
            assert!(c.wet > near.wet && c.echo_time < 0.1, "a cave rings, short: {c:?}");
        });
        crate::map::with(crate::map::MapId::Jungle, || {
            let s = space_at(Vec3::new(40.0, 0.0, 40.0));
            assert!(s.wet > 0.0 && s.damp > 0.2);
        });
    }
}
