//! replay — the moment after the goal.
//!
//! Rocket League's best trick is not physics, it is the replay: the game
//! stops, swings a camera round, and shows you what you just did. Half of
//! why a shot feels good is watching it again.
//!
//! Two parts here. A ring buffer keeps the last twelve seconds of car and
//! ball state at 30 Hz — cheap, fixed size, always running. And a TRICK LOG
//! watches the physics events go by and remembers the interesting ones, so
//! when a goal lands the game can say what actually happened: not "GOAL"
//! but "AIR DRIBBLE INTO A FLIP RESET, 46 METRES".
//!
//! Everything here reads events the simulation already emits — flip resets,
//! dodges, pinches, aerial touches — so a named trick is never a guess.

use glam::{Quat, Vec3};

pub const HZ: f32 = 30.0;
pub const SECONDS: f32 = 12.0;
pub const CAP: usize = (HZ * SECONDS) as usize;

#[derive(Clone, Copy, Default)]
pub struct Frame {
    pub car: Vec3,
    pub car_rot: Quat,
    pub ball: Vec3,
    pub ball_rot: Quat,
    pub boosting: bool,
}

/// What the player did, in the order they did it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Trick {
    /// a plain dodge in the air — the first thing anyone learns
    Flip,
    Aerial,
    AirDribble,
    FlipReset,
    Pinch,
    WaveDash,
    Ceiling,
    LongShot,
    Backboard,
    Redirect,
}

impl Trick {
    pub fn name(self) -> &'static str {
        match self {
            Trick::Flip => "FLIP",
            Trick::Aerial => "AERIAL",
            Trick::AirDribble => "AIR DRIBBLE",
            Trick::FlipReset => "FLIP RESET",
            Trick::Pinch => "PINCH",
            Trick::WaveDash => "WAVE DASH",
            Trick::Ceiling => "CEILING SHOT",
            Trick::LongShot => "LONG BOMB",
            Trick::Backboard => "BACKBOARD READ",
            Trick::Redirect => "REDIRECT",
        }
    }
    /// Rarity, used to pick which tricks get named and to score the moment.
    pub fn weight(self) -> u32 {
        match self {
            Trick::Flip => 1,
            Trick::Aerial => 1,
            Trick::WaveDash => 2,
            Trick::Redirect => 3,
            Trick::LongShot => 3,
            Trick::Backboard => 4,
            Trick::Pinch => 4,
            Trick::AirDribble => 6,
            Trick::Ceiling => 7,
            Trick::FlipReset => 9,
        }
    }
}

pub struct Recorder {
    buf: Vec<Frame>,
    head: usize,
    len: usize,
    acc: f32,
    /// (age in seconds when it happened, trick)
    log: Vec<(f32, Trick)>,
    t: f32,
}

impl Default for Recorder {
    fn default() -> Recorder {
        Recorder {
            buf: vec![Frame::default(); CAP],
            head: 0,
            len: 0,
            acc: 0.0,
            log: Vec::new(),
            t: 0.0,
        }
    }
}

impl Recorder {
    pub fn note(&mut self, tr: Trick) {
        // one of each kind per second is plenty — a long air dribble should
        // not fill the log with a hundred identical entries
        if let Some((last, k)) = self.log.last() {
            if *k == tr && self.t - last < 1.0 {
                return;
            }
        }
        self.log.push((self.t, tr));
        if self.log.len() > 64 {
            self.log.remove(0);
        }
    }

    pub fn push(&mut self, dt: f32, f: Frame) {
        self.t += dt;
        self.acc += dt;
        if self.acc < 1.0 / HZ {
            return;
        }
        self.acc = 0.0;
        self.buf[self.head] = f;
        self.head = (self.head + 1) % CAP;
        self.len = (self.len + 1).min(CAP);
    }

    /// Oldest-to-newest copy of the buffer.
    pub fn frames(&self) -> Vec<Frame> {
        let mut out = Vec::with_capacity(self.len);
        let start = (self.head + CAP - self.len) % CAP;
        for i in 0..self.len {
            out.push(self.buf[(start + i) % CAP]);
        }
        out
    }

    /// Tricks from the last `window` seconds, rarest first.
    pub fn recent(&self, window: f32) -> Vec<Trick> {
        let mut v: Vec<Trick> = self
            .log
            .iter()
            .filter(|(at, _)| self.t - at <= window)
            .map(|(_, k)| *k)
            .collect();
        v.sort_by_key(|t| std::cmp::Reverse(t.weight()));
        v.dedup();
        v
    }

    /// Name the moment: the two best tricks, or plain if it was plain.
    pub fn moment(&self, window: f32) -> (String, u32) {
        let tricks = self.recent(window);
        let score: u32 = tricks.iter().map(|t| t.weight()).sum();
        let title = match tricks.len() {
            0 => "GOAL".to_string(),
            1 => tricks[0].name().to_string(),
            _ => format!("{} INTO {}", tricks[0].name(), tricks[1].name()),
        };
        (title, score)
    }

    pub fn clear_log(&mut self) {
        self.log.clear();
    }
}

/// A saved moment: the frames, what it was called, and how good it was.
pub struct Moment {
    pub title: String,
    pub score: u32,
    pub frames: Vec<Frame>,
}

/// Playback: where the camera should be at time `t` into a replay.
///
/// The rule is simple and reads well: orbit the action slowly, framing the
/// midpoint of car and ball, pulling in as the moment arrives. It looks
/// deliberate without needing a director.
pub fn cinema(frames: &[Frame], t: f32) -> Option<(Vec3, Vec3)> {
    if frames.len() < 2 {
        return None;
    }
    let f = (t * HZ).clamp(0.0, (frames.len() - 1) as f32);
    let i = f.floor() as usize;
    let k = f - i as f32;
    let a = frames[i];
    let b = frames[(i + 1).min(frames.len() - 1)];
    let car = a.car.lerp(b.car, k);
    let ball = a.ball.lerp(b.ball, k);
    let focus = car.lerp(ball, 0.55);
    // how far through the replay we are, 0..1
    let p = f / (frames.len() - 1).max(1) as f32;
    let spread = (car - ball).length().max(6.0);
    let dist = (spread * 1.6 + 14.0) * (1.0 - 0.35 * p);
    let ang = 0.6 + p * 2.2;
    let height = 5.0 + spread * 0.25;
    let eye = focus + Vec3::new(ang.cos() * dist, height, ang.sin() * dist);
    Some((eye, focus))
}

/// Interpolated car and ball pose at time `t` (for drawing the replay).
pub fn pose_at(frames: &[Frame], t: f32) -> Option<Frame> {
    if frames.is_empty() {
        return None;
    }
    let f = (t * HZ).clamp(0.0, (frames.len() - 1) as f32);
    let i = f.floor() as usize;
    let j = (i + 1).min(frames.len() - 1);
    let k = f - i as f32;
    let (a, b) = (frames[i], frames[j]);
    Some(Frame {
        car: a.car.lerp(b.car, k),
        car_rot: a.car_rot.slerp(b.car_rot, k),
        ball: a.ball.lerp(b.ball, k),
        ball_rot: a.ball_rot.slerp(b.ball_rot, k),
        boosting: a.boosting,
    })
}

pub fn length_seconds(frames: &[Frame]) -> f32 {
    (frames.len().max(1) - 1) as f32 / HZ
}
