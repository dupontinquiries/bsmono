//! BAKING A SWING, AND CHAINING SWINGS INTO A COMBO.
//!
//! `swing.rs` captures: it turns a stream of IMU samples into a `Swing` with
//! an orientation track at whatever rate the phone happened to deliver, over
//! whatever duration the arm happened to take. That is a recording, not an
//! asset. This module is the bake — and then the part that was actually
//! asked for, which is combos.
//!
//! ## Why a recording is not a clip
//!
//! Three things are wrong with playing a capture back directly:
//!
//! * **Its rate is the phone's, not the game's.** Sample spacing varies, and
//!   a dropped run leaves a hole. A clip is resampled to a fixed count so
//!   playback costs a lookup instead of a search.
//! * **Its duration is the arm's.** A recorded swing took 340 ms because that
//!   is how fast someone moved; the same arc may need to play in 180 ms as a
//!   fast attack and 500 ms as a heavy one. Duration has to be a playback
//!   parameter, not baked in.
//! * **Its frame is the phone's.** The capture starts wherever the handset
//!   was pointing. A clip has to start at identity so it can be applied to a
//!   weapon in any pose.
//!
//! ## What makes a combo a combo
//!
//! Not a list — but also not what I first wrote here. My first version scored
//! a link by the angle between one clip's END ORIENTATION and the next's
//! START, and a test rejected it immediately: every clip is rebased to start
//! at identity, so consecutive clips COMPOSE and can never leave a positional
//! gap. Clips are C0-continuous by construction. Scoring C0 measures the
//! first clip's own sweep and calls it a seam.
//!
//! What decides whether a chain reads as one motion is C1 — whether the
//! weapon is still travelling the same way across the seam. Swing right then
//! right again and the arm carries through; swing right then left and it has
//! to stop, reverse and reaccelerate, and that reversal is a beat whether or
//! not the animation admits it. So a link compares the EXIT DIRECTION of one
//! arc with the ENTRY DIRECTION of the next, which is measurable at authoring
//! time rather than discovered in play.
//!
//! Recovery is the escape hatch and the design lever: a gap that is too wide
//! to flow is not necessarily a bad combo, it is one that needs a beat to get
//! the weapon back. `link` reports how long that beat has to be, which is the
//! number a designer actually tunes — a fast chain wants small gaps, and a
//! heavy finisher can afford a wind-up.

use glam::Quat;

use crate::swing::{ArcKind, Swing};

/// How many orientation samples a baked clip holds. Enough that a fast arc
/// does not step visibly, small enough that a library of them is cheap.
pub const CLIP_SAMPLES: usize = 32;

/// The widest gap between two clips that still reads as one motion, radians.
/// Beyond this the weapon has to travel, and travelling takes time.
pub const FLOW_GAP_RAD: f32 = 0.45;

/// How fast the weapon can be moved during a recovery beat, rad/s. Used to
/// turn a gap into a duration.
pub const RECOVER_RATE: f32 = 9.0;

/// A baked, replayable arc.
#[derive(Clone, Debug, PartialEq)]
pub struct ArcClip {
    /// Orientation relative to the clip's own start, uniformly spaced.
    /// `frames[0]` is always identity.
    pub frames: Vec<Quat>,
    pub kind: ArcKind,
    /// Total angle swept, radians — carried so a hit can scale with it
    /// without replaying the clip.
    pub sweep_rad: f32,
    /// Where the speed peaked, 0..1. Preserved through the resample because
    /// it is what distinguishes a flick from a committed strike, and a
    /// uniform resample would otherwise throw the timing away.
    pub peak_at: f32,
    /// What the swing originally took, seconds. A default, not a constraint.
    pub captured_s: f32,
}

impl ArcClip {
    /// Bake a capture.
    pub fn bake(s: &Swing) -> Option<ArcClip> {
        if s.frames.len() < 2 || s.duration_s <= 0.0 {
            return None;
        }
        // Rebase so the clip starts at identity. BELT-AND-BRACES, not
        // load-bearing: `swing.rs` already pushes an identity first frame, so
        // this is currently a no-op and removing it fails no test. It stays
        // because `bake` should not silently depend on that contract, and it
        // is labelled because an unremarked no-op reads like a check.
        let base = s.frames[0].1.inverse();
        let mut frames = Vec::with_capacity(CLIP_SAMPLES);
        for i in 0..CLIP_SAMPLES {
            let u = i as f32 / (CLIP_SAMPLES - 1) as f32;
            frames.push((base * sample_track(&s.frames, u * s.duration_s)).normalize());
        }
        Some(ArcClip {
            frames,
            kind: s.kind,
            sweep_rad: s.sweep_rad,
            peak_at: s.peak_at,
            captured_s: s.duration_s,
        })
    }

    /// Orientation at `u` in 0..1 through the clip.
    pub fn at(&self, u: f32) -> Quat {
        if self.frames.is_empty() {
            return Quat::IDENTITY;
        }
        let u = u.clamp(0.0, 1.0) * (self.frames.len() - 1) as f32;
        let i = u.floor() as usize;
        let j = (i + 1).min(self.frames.len() - 1);
        self.frames[i].slerp(self.frames[j], u - i as f32).normalize()
    }

    pub fn start(&self) -> Quat {
        self.frames.first().copied().unwrap_or(Quat::IDENTITY)
    }
    pub fn end(&self) -> Quat {
        self.frames.last().copied().unwrap_or(Quat::IDENTITY)
    }
}

/// Slerp a captured `(t, Quat)` track at time `t`.
fn sample_track(track: &[(f32, Quat)], t: f32) -> Quat {
    match track.len() {
        0 => Quat::IDENTITY,
        1 => track[0].1,
        _ => {
            if t <= track[0].0 {
                return track[0].1;
            }
            if t >= track[track.len() - 1].0 {
                return track[track.len() - 1].1;
            }
            // linear scan: 32 lookups over a few hundred samples, once, at
            // bake time. A binary search here would be optimising the one
            // path that never runs in a frame.
            for w in track.windows(2) {
                if t <= w[1].0 {
                    let span = (w[1].0 - w[0].0).max(1e-6);
                    return w[0].1.slerp(w[1].1, ((t - w[0].0) / span).clamp(0.0, 1.0));
                }
            }
            track[track.len() - 1].1
        }
    }
}

/// How two clips join.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Link {
    /// Angle between the first arc's exit direction and the second's entry
    /// direction, radians. 0 is a carry-through, pi is a full reversal.
    pub gap_rad: f32,
    /// Seconds of recovery the weapon needs to cover that gap. Zero when the
    /// clips already flow.
    pub recover_s: f32,
}

impl Link {
    /// The two read as one motion, with no beat between them.
    pub fn flows(&self) -> bool {
        self.gap_rad <= FLOW_GAP_RAD
    }
}

/// The axis a clip is turning about at its start and at its end.
///
/// Taken from the first and last STEP rather than from the endpoints, because
/// the endpoints are orientations and the question is about direction of
/// travel. A step of no rotation has no axis, and `None` says so instead of
/// returning a normalised zero.
fn exit_axis(c: &ArcClip) -> Option<glam::Vec3> {
    let n = c.frames.len();
    if n < 2 {
        return None;
    }
    step_axis(c.frames[n - 2], c.frames[n - 1])
}

fn entry_axis(c: &ArcClip) -> Option<glam::Vec3> {
    if c.frames.len() < 2 {
        return None;
    }
    step_axis(c.frames[0], c.frames[1])
}

fn step_axis(from: Quat, to: Quat) -> Option<glam::Vec3> {
    let d = (to * from.inverse()).normalize();
    // Double cover: -q is the same rotation with a negated axis, so
    // canonicalise on w before reading the axis. ALSO BELT-AND-BRACES at
    // CLIP_SAMPLES=32 — a step between adjacent frames of a sub-2pi arc is
    // about 0.1 rad, whose delta always has w > 0, so this cannot currently
    // fire and removing it fails no test. It becomes load-bearing the moment
    // anyone samples a clip coarsely or composes two steps, which is exactly
    // when it would be missed.
    let d = if d.w < 0.0 { Quat::from_xyzw(-d.x, -d.y, -d.z, -d.w) } else { d };
    glam::Vec3::new(d.x, d.y, d.z).try_normalize()
}

/// Measure the join between two clips.
pub fn link(a: &ArcClip, b: &ArcClip) -> Link {
    // C1, not C0 — see the module doc. A clip that is not turning at its seam
    // imposes no direction, so it joins anything.
    let gap = match (exit_axis(a), entry_axis(b)) {
        (Some(x), Some(y)) => x.dot(y).clamp(-1.0, 1.0).acos(),
        _ => 0.0,
    };
    Link {
        gap_rad: gap,
        recover_s: if gap <= FLOW_GAP_RAD { 0.0 } else { gap / RECOVER_RATE },
    }
}

/// A chain of arcs, with the beats between them.
#[derive(Clone, Debug, PartialEq)]
pub struct Combo {
    pub clips: Vec<ArcClip>,
    /// `links[i]` joins `clips[i]` to `clips[i+1]`; one shorter than `clips`.
    pub links: Vec<Link>,
}

impl Combo {
    pub fn new(clips: Vec<ArcClip>) -> Combo {
        let links = clips.windows(2).map(|w| link(&w[0], &w[1])).collect();
        Combo { clips, links }
    }
    /// Every join reads as one motion.
    pub fn flows(&self) -> bool {
        self.links.iter().all(|l| l.flows())
    }
    /// Total time to play, at the captured speeds plus every recovery beat.
    pub fn duration_s(&self) -> f32 {
        self.clips.iter().map(|c| c.captured_s).sum::<f32>()
            + self.links.iter().map(|l| l.recover_s).sum::<f32>()
    }
    /// The join that costs the most, for a designer tightening a chain.
    pub fn worst_link(&self) -> Option<(usize, Link)> {
        self.links
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.gap_rad.total_cmp(&b.1.gap_rad))
            .map(|(i, l)| (i, *l))
    }
    pub fn report(&self) -> String {
        format!(
            "{} arcs, {:.2}s, {}",
            self.clips.len(),
            self.duration_s(),
            if self.flows() { "flows".into() } else {
                match self.worst_link() {
                    Some((i, l)) => format!("breaks at {i} ({:.0}deg, {:.0}ms recovery)", l.gap_rad.to_degrees(), l.recover_s * 1000.0),
                    None => "breaks".into(),
                }
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swing::{detect, ImuSample};
    use glam::Vec3;

    const HZ: f32 = 200.0;

    fn recorded(axis: Vec3, rate: f32, secs: f32) -> Swing {
        let g = Vec3::new(0.0, 9.81, 0.0);
        let mut rec = Vec::new();
        let mut t = 0.0f32;
        for _ in 0..40 {
            rec.push(ImuSample { t_s: t, accel: g, gyro: Vec3::ZERO });
            t += 1.0 / HZ;
        }
        let gyro = axis.normalize() * rate;
        for _ in 0..((secs * HZ) as usize) {
            rec.push(ImuSample { t_s: t, accel: g + gyro * 2.0, gyro });
            t += 1.0 / HZ;
        }
        for _ in 0..40 {
            rec.push(ImuSample { t_s: t, accel: g, gyro: Vec3::ZERO });
            t += 1.0 / HZ;
        }
        detect(&rec).into_iter().next().expect("the fixture produced no swing")
    }

    #[test]
    fn a_baked_clip_starts_at_identity_and_keeps_the_arc() {
        let sw = recorded(Vec3::Y, 12.0, 0.25);
        let c = ArcClip::bake(&sw).expect("bake");
        assert_eq!(c.frames.len(), CLIP_SAMPLES);
        assert!(c.start().angle_between(Quat::IDENTITY) < 1e-4, "clip does not start at identity");
        // the arc survives the resample: end orientation still ~3 rad about Y
        let ang = 2.0 * c.end().w.abs().clamp(0.0, 1.0).acos();
        assert!(
            (ang - 3.0).abs() < 0.2,
            "resampling lost the arc: {ang:.2} rad, expected ~3.0"
        );
        assert!((c.sweep_rad - sw.sweep_rad).abs() < 1e-4, "sweep was not carried");
        assert!((c.peak_at - sw.peak_at).abs() < 1e-4, "peak timing was not carried");
    }

    #[test]
    fn playback_is_monotone_and_hits_both_ends() {
        // A clip that jumped around, or that never reached its end, would
        // look like a glitch rather than a swing.
        let c = ArcClip::bake(&recorded(Vec3::Y, 12.0, 0.25)).expect("bake");
        assert!(c.at(0.0).angle_between(c.start()) < 1e-4);
        assert!(c.at(1.0).angle_between(c.end()) < 1e-4);
        let mut prev = 0.0f32;
        for i in 0..=40 {
            let u = i as f32 / 40.0;
            let a = 2.0 * c.at(u).w.abs().clamp(0.0, 1.0).acos();
            assert!(a >= prev - 1e-3, "playback went backwards at u={u:.2}");
            prev = a;
        }
        // out-of-range input is clamped, not wrapped
        assert!(c.at(-5.0).angle_between(c.start()) < 1e-4);
        assert!(c.at(9.0).angle_between(c.end()) < 1e-4);
    }

    #[test]
    fn a_chain_that_continues_flows_and_one_that_jumps_does_not() {
        // The property that makes a combo a combo rather than a playlist.
        let a = ArcClip::bake(&recorded(Vec3::Y, 12.0, 0.20)).expect("bake");
        // continuing: same axis again, so b starts where a ended
        let cont = Combo::new(vec![a.clone(), a.clone()]);
        assert!(cont.flows(), "a continuing chain did not flow: {}", cont.report());
        // reversing: the same arc swung back the other way. The arm has to
        // stop and reaccelerate, which is a beat whether the animation
        // admits it or not.
        let back = ArcClip::bake(&recorded(-Vec3::Y, 12.0, 0.20)).expect("bake");
        let jump = Combo::new(vec![a.clone(), back]);
        assert!(!jump.flows(), "a reversal flowed straight through: {}", jump.report());
        let (_, worst) = jump.worst_link().expect("a link");
        assert!(worst.recover_s > 0.0, "a broken link asked for no recovery");
        assert!(
            jump.duration_s() > cont.duration_s(),
            "the broken chain did not cost the recovery beat"
        );
    }

    #[test]
    fn the_gap_respects_quaternion_double_cover() {
        // q and -q are the same rotation but the negated form's vector part
        // is the axis negated. Without canonicalising on w, a carry-through
        // scores as a full reversal — silently, since pi is a plausible
        // number for a combo link.
        let a = ArcClip::bake(&recorded(Vec3::Y, 12.0, 0.20)).expect("bake");
        let mut negated = a.clone();
        for f in negated.frames.iter_mut() {
            *f = Quat::from_xyzw(-f.x, -f.y, -f.z, -f.w);
        }
        let l = link(&a, &negated);
        let m = link(&a, &a);
        assert!(
            (l.gap_rad - m.gap_rad).abs() < 1e-3,
            "negating the quaternions changed the gap: {:.3} vs {:.3}",
            l.gap_rad,
            m.gap_rad
        );
    }

    #[test]
    fn a_single_clip_combo_has_no_links_and_does_not_panic() {
        let a = ArcClip::bake(&recorded(Vec3::Y, 12.0, 0.20)).expect("bake");
        let one = Combo::new(vec![a]);
        assert!(one.links.is_empty());
        assert!(one.flows(), "a combo with nothing to join reported a break");
        assert!(one.worst_link().is_none());
        let none = Combo::new(Vec::new());
        assert!(none.flows() && none.duration_s() == 0.0);
    }

    #[test]
    fn baking_refuses_a_capture_with_nothing_in_it() {
        let mut empty = recorded(Vec3::Y, 12.0, 0.20);
        empty.frames.clear();
        assert!(ArcClip::bake(&empty).is_none(), "baked a clip from no frames");
        let mut instant = recorded(Vec3::Y, 12.0, 0.20);
        instant.duration_s = 0.0;
        assert!(ArcClip::bake(&instant).is_none(), "baked a zero-length clip");
    }
}
