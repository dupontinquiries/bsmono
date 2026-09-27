//! intro — the match opening, as a timeline.
//!
//! Before the first tick: a slow pan over the whole map, a low run along
//! each lane, the two sides appearing in their bases in a burst of starry
//! mist, a team-versus-team card over the centre with the brass fanfare
//! (`brass`) and the announcer's line, then the camera settles onto the
//! player and play begins. Any input skips it.
//!
//! ## Layer
//!
//! Headless: [`Intro::frame`] maps a time to a camera, a beat, what the
//! overlay shows and who has appeared. No egui, no GPU, no audio device --
//! the app, the headless harness and the tests all read the same frames, and
//! the shells only draw them. The sim does not tick while the intro runs;
//! the intro holds the cats back by marking them not-yet-arrived, and the
//! app puts them into the world at their spawn beat.

use glam::Vec3;

use crate::consts::{self, Lane, Team};

/// Where the camera is: eye, target, vertical field of view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shot {
    pub eye: Vec3,
    pub target: Vec3,
    pub fov_y: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Beat {
    /// The first match's prologue: the world from high above, narrated.
    Prologue,
    /// The whole map, turning slowly.
    Pan,
    /// Low along one lane, from the blue end toward the centre.
    Lane(Lane),
    /// A base, as its cats appear.
    Spawn(Team),
    /// Both sides named, face to face.
    Versus,
    /// Settling onto the player; play starts at the end of it.
    Settle,
}

/// What the overlay should show this frame. All alphas 0..1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Overlay {
    /// Cinema bars top and bottom.
    pub bars: f32,
    pub versus: f32,
    /// The announcer's line, and how present it is.
    pub line: Option<&'static str>,
    pub line_alpha: f32,
    /// The lane's name during a lane run.
    pub caption: Option<&'static str>,
    pub skip_hint: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub shot: Shot,
    pub beat: Beat,
    pub overlay: Overlay,
    /// Seconds since the beat began.
    pub beat_t: f32,
}

/// Beat lengths, seconds. Short enough to sit through twice; skippable.
pub const PAN: f32 = 4.0;
// Longer beats (were 1.6 / 1.8 / 1.4) to make room for slower flights
// between them: the big moves read as jumps (reported).
pub const LANE: f32 = 2.4;
pub const SPAWN: f32 = 2.6;
pub const VERSUS: f32 = 3.6;
pub const SETTLE: f32 = 2.0;
/// When the fanfare starts, relative to the start of the intro: on the
/// first spawn, so the brass swells as the cats appear and peaks on the card.
pub const FANFARE_AT: f32 = PAN + 3.0 * LANE;
/// Gap between cats appearing in one base.
pub const SPAWN_STAGGER: f32 = 0.07;

/// Seconds a move between two beats' shots takes (half before, half after
/// the boundary).
pub const TRANSITION: f32 = 0.9;
/// Half-width, in world units, of the moving average a lane run follows.
pub const LANE_SMOOTH: f32 = 12.0;

/// When each beat after the first begins.
fn beat_starts() -> [f32; 7] {
    let l = PAN + 3.0 * LANE;
    [PAN, PAN + LANE, PAN + 2.0 * LANE, l, l + SPAWN, l + 2.0 * SPAWN, l + 2.0 * SPAWN + VERSUS]
}

/// The prologue's length (the full intro only), and when each narrator line
/// starts within it (`voicescript`, part "narrator").
/// Spaced by each line's own length at a storyteller's ~15 characters a
/// second plus a breath (0.6 s): the first schedule gave 61 characters 3.5 s
/// (the pacing test caught it), and the whole monologue needs ~36 s.
pub const PROLOGUE: f32 = 44.0;
pub const NARRATION: [(f32, &str); 8] = [(1.5, "prologue1"), (6.2, "prologue2"), (10.6, "prologue3"), (15.9, "prologue4"), (21.8, "prologue5"), (26.2, "prologue6"), (32.8, "prologue7"), (38.6, "prologue8")];

/// The narrator's words for a line, for the subtitles.
pub fn narration_text(key: &str) -> &'static str {
    crate::voicescript::part("narrator").and_then(|p| p.lines.iter().find(|l| l.key == key)).map_or("", |l| l.text)
}

pub fn total() -> f32 {
    PAN + 3.0 * LANE + 2.0 * SPAWN + VERSUS + SETTLE
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp_shot(a: Shot, b: Shot, t: f32) -> Shot {
    let t = smooth(t);
    Shot { eye: a.eye.lerp(b.eye, t), target: a.target.lerp(b.target, t), fov_y: a.fov_y + (b.fov_y - a.fov_y) * t }
}

/// A match opening. `play` is where the gameplay camera will be when it
/// ends (the Settle beat blends into it).
#[derive(Debug, Clone)]
pub struct Intro {
    pub t: f32,
    pub done: bool,
    /// Seconds each god appears at (index = god index), from its team's
    /// spawn beat and its place in the team.
    pub arrive: Vec<f32>,
    teams: Vec<Team>,
    player_start: Vec3,
    play: Shot,
    /// Seconds of prologue before the short intro's timeline (0, or
    /// PROLOGUE for the full intro).
    lead: f32,
}

impl Intro {
    /// `teams[i]` is god i's team; `player_start` the player's spawn;
    /// `play` the gameplay camera it hands over to.
    pub fn new(teams: &[Team], player_start: Vec3, play: Shot) -> Intro {
        let mut k = [0usize; 2];
        let arrive = teams
            .iter()
            .map(|t| {
                let beat = PAN + 3.0 * LANE + if *t == Team::Blue { 0.0 } else { SPAWN };
                let i = k[*t as usize];
                k[*t as usize] += 1;
                beat + 0.35 + i as f32 * SPAWN_STAGGER
            })
            .collect();
        Intro { t: 0.0, done: false, arrive, teams: teams.to_vec(), player_start, play, lead: 0.0 }
    }

    /// The full intro: the narrated prologue, then the short intro.
    pub fn full(teams: &[Team], player_start: Vec3, play: Shot) -> Intro {
        let mut i = Intro::new(teams, player_start, play);
        i.lead = PROLOGUE;
        for a in i.arrive.iter_mut() {
            *a += PROLOGUE;
        }
        i
    }

    pub fn is_full(&self) -> bool {
        self.lead > 0.0
    }

    /// The whole intro's length.
    pub fn length(&self) -> f32 {
        total() + self.lead
    }

    /// When the first cats appear (the fanfare's cue).
    pub fn fanfare_at(&self) -> f32 {
        FANFARE_AT + self.lead
    }

    /// Narrator lines whose start falls in (before, now].
    pub fn narration_due(&self, before: f32, now: f32) -> Vec<&'static str> {
        if self.lead <= 0.0 {
            return Vec::new();
        }
        NARRATION.iter().filter(|(at, _)| *at > before && *at <= now).map(|(_, k)| *k).collect()
    }

    /// Advance; returns the gods (indices) that appear during this step.
    pub fn advance(&mut self, dt: f32) -> Vec<usize> {
        if self.done {
            return Vec::new();
        }
        let before = self.t;
        self.t += dt.max(0.0);
        if self.t >= self.length() {
            self.done = true;
        }
        (0..self.arrive.len()).filter(|&i| self.arrive[i] > before && self.arrive[i] <= self.t).collect()
    }

    /// Skip to the end. Returns every god that had not appeared yet, so the
    /// shell can put them in (without the fanfare of a burst each).
    pub fn skip(&mut self) -> Vec<usize> {
        let t = self.t;
        self.t = self.length();
        self.done = true;
        (0..self.arrive.len()).filter(|&i| self.arrive[i] > t).collect()
    }

    pub fn has_arrived(&self, god: usize) -> bool {
        self.done || self.arrive.get(god).map_or(true, |&a| self.t >= a)
    }

    /// Everything the shells need for time `self.t`.
    /// Everything the shells need for time `self.t`: the beats' shots with
    /// the cuts between them turned into moves (see `TRANSITION`).
    pub fn frame(&self) -> Frame {
        let t = self.t;
        let f = self.raw_frame(t);
        // Across a beat boundary the camera travels from the outgoing shot
        // to the incoming one instead of cutting: eased both ends, and on a
        // long move the eye rises on an arc (base to base on the pass is
        // ~350 u, over the peaks, not through them). Measured before: 137
        // degrees of turn and 353 u of eye travel in ONE frame.
        let lead = self.lead;
        let starts: Vec<f32> = std::iter::once(lead).filter(|l| *l > 0.0).chain(beat_starts().into_iter().map(|b| b + lead)).collect();
        for b in starts {
            // a long move takes longer: base to base on the pass is a 180
            // degree reversal over ~350 u, which in 0.9 s turned 6.3 deg a
            // frame (the smoothness test's limit is 6)
            let probe = (self.raw_frame(b - 0.45).shot.eye - self.raw_frame(b + 0.45).shot.eye).length();
            // slower, and longer the further: 1.3 s, to 2.4 s for the pass's
            // base-to-base flight (was 0.9 to 1.6 -- the big moves read as
            // jumps). 2.4 is the shortest beat: two flights never overlap.
            let dur = (1.3 + probe / 350.0 * 1.5).min(2.4);
            let half = dur * 0.5;
            if b > half && t > b - half && t < b + half {
                let from = self.raw_frame(b - half).shot;
                let to = self.raw_frame(b + half).shot;
                let u = smooth((t - (b - half)) / dur);
                let dist = (to.eye - from.eye).length();
                let arc = (u * std::f32::consts::PI).sin() * (dist * 0.3).min(70.0);
                let shot = Shot {
                    eye: from.eye.lerp(to.eye, u) + Vec3::Y * arc,
                    target: from.target.lerp(to.target, u),
                    fov_y: from.fov_y + (to.fov_y - from.fov_y) * u,
                };
                return Frame { shot, ..f };
            }
        }
        f
    }

    /// The beats' own shots, cut at the boundaries: `frame` smooths these.
    fn raw_frame(&self, t: f32) -> Frame {
        if t < self.lead {
            return self.prologue_frame(t);
        }
        self.raw_short(t - self.lead)
    }

    /// The prologue: the world seen from far and high, a slow sweep that
    /// descends toward where the pan begins; bars in; the narrator's words
    /// as subtitles.
    fn prologue_frame(&self, t: f32) -> Frame {
        let r = consts::arena_radius();
        let u = smooth(t / self.lead);
        // from far beyond the south rim, high, round toward the pan's start
        let a0 = -std::f32::consts::FRAC_PI_2 - 0.9;
        let a1 = -1.9;
        let a = a0 + (a1 - a0) * u;
        // inside the terrain the whole way: starting 1.9 r out (414 u on
        // the pass, whose ground reaches 300) looked past the world's edge
        // into bare void
        let dist = r * (1.25 - 0.3 * u);
        let height = r * (1.05 - 0.2 * u);
        let shot = Shot { eye: Vec3::new(a.cos() * dist, height, a.sin() * dist), target: Vec3::new(0.0, r * 0.05 * (1.0 - u), 0.0), fov_y: 0.95 + 0.05 * u };
        let mut o = Overlay { bars: smooth(t / 1.2), versus: 0.0, line: None, line_alpha: 0.0, caption: None, skip_hint: smooth((t - 1.0) / 0.5) };
        // the subtitle: the line that started last, faded in and out
        if let Some(k) = NARRATION.iter().rposition(|(at, _)| *at <= t) {
            let (at, key) = NARRATION[k];
            let end = NARRATION.get(k + 1).map_or(self.lead, |n| n.0) - 0.3;
            o.line = Some(narration_text(key));
            o.line_alpha = smooth((t - at) / 0.4) * (1.0 - smooth((t - (end - 0.4)) / 0.4));
        }
        Frame { shot, beat: Beat::Prologue, overlay: o, beat_t: t }
    }

    /// The short intro's shots at `t` (seconds from its own start).
    fn raw_short(&self, t: f32) -> Frame {
        let r = consts::arena_radius();
        let bars_in = smooth(t / 0.6);
        let mut o = Overlay { bars: bars_in, versus: 0.0, line: None, line_alpha: 0.0, caption: None, skip_hint: smooth((t - 1.0) / 0.5) * (1.0 - smooth((t - (total() - SETTLE)) / 0.4)) };

        // Pan: a slow quarter orbit, high, looking at the centre.
        if t < PAN {
            let u = t / PAN;
            let a = -1.9 + u * 1.1;
            let shot = Shot { eye: Vec3::new(a.cos() * r * 0.95, r * 0.85 - u * r * 0.2, a.sin() * r * 0.95), target: Vec3::ZERO, fov_y: 1.0 };
            return Frame { shot, beat: Beat::Pan, overlay: o, beat_t: t };
        }
        // Lanes: low, along each from the blue base toward the centre.
        let mut s = t - PAN;
        for (i, lane) in [Lane::Top, Lane::Mid, Lane::Bot].into_iter().enumerate() {
            if s < LANE {
                let u = s / LANE;
                let pts = lane.polyline();
                let len: f32 = pts.windows(2).map(|w| (w[1] - w[0]).length()).sum();
                let at = |d: f32| crate::pass::along(&pts, d.clamp(0.0, len));
                let d = len * (0.12 + 0.28 * u);
                // the lane smoothed: a moving average along it, so the run
                // banks through the polyline's corners instead of snapping
                // its heading at each one
                let smooth_at = |d: f32| {
                    let n = 9;
                    (0..n).map(|k| at(d + (k as f32 / (n - 1) as f32 - 0.5) * 2.0 * LANE_SMOOTH)).fold(Vec3::ZERO, |a, p| a + p) / n as f32
                };
                let (p, ahead) = (smooth_at(d), smooth_at(d + 22.0));
                let side = { let f = (ahead - p).normalize_or_zero(); Vec3::new(-f.z, 0.0, f.x) };
                let sky = crate::map::active() == crate::map::MapId::Sky;
                // Starwell's route to the well runs under the Aerie: that
                // run is flown over it instead
                let drowned = crate::map::active() == crate::map::MapId::Atlantis;
                // (and the Drowned Halls are flown over their walls, through
                // the glass: the halls are too narrow to fly down)
                // (the Thornmaze too: its corridors wind too tightly to fly)
                let maze = crate::map::active() == crate::map::MapId::Maze;
                let over = if sky && lane == Lane::Mid {
                    crate::sky::UPPER_Y + 6.0
                } else if drowned {
                    crate::atlantis::WALL_H - 1.0
                } else if maze {
                    crate::maze::WALL_H + 4.0
                } else {
                    0.0
                };
                let shot = Shot { eye: p + Vec3::Y * (7.0 + over) + side * 3.0, target: ahead + Vec3::Y * (2.0 + over * 0.7), fov_y: 0.95 };
                o.caption = Some(match (lane, sky) {
                    (Lane::Top, false) if maze => "THE WEST WAY",
                    (Lane::Mid, false) if maze => "THE HEART OF THE MAZE",
                    (Lane::Bot, false) if maze => "THE EAST WAY",
                    // one lane; the other two runs are the halls beside it
                    (Lane::Top, false) if drowned => "THE GALLERIES",
                    (Lane::Mid, false) if drowned => "THE GREAT HALL",
                    (Lane::Bot, false) if drowned => "THE DROWNED LIBRARY",
                    (Lane::Top, false) => "TOP LANE",
                    (Lane::Mid, false) => "MID LANE",
                    (Lane::Bot, false) => "BOT LANE",
                    // two lanes, and between them the sky route to the well
                    (Lane::Top, true) => "WEST LANE",
                    (Lane::Mid, true) => "THE MOONWELL",
                    (Lane::Bot, true) => "EAST LANE",
                });
                let _ = i;
                return Frame { shot, beat: Beat::Lane(lane), overlay: o, beat_t: s };
            }
            s -= LANE;
        }
        // Spawns: each base, close, as its cats appear.
        for team in [Team::Blue, Team::Red] {
            if s < SPAWN {
                let f = team.fountain();
                let out = Vec3::new(0.0, 0.0, team.push_z());
                let u = s / SPAWN;
                let shot = Shot { eye: f + out * (16.0 - u * 3.0) + Vec3::new(7.0, 7.5 - u * 1.5, 0.0), target: f + out * 6.0 + Vec3::Y * 1.2, fov_y: 0.9 };
                o.line = Some(match team {
                    Team::Blue => "FROSTFANG, take the field",
                    Team::Red => "EMBERCLAW, take the field",
                });
                o.line_alpha = smooth(s / 0.3) * (1.0 - smooth((s - (SPAWN - 0.3)) / 0.3));
                return Frame { shot, beat: Beat::Spawn(team), overlay: o, beat_t: s };
            }
            s -= SPAWN;
        }
        // Versus: wide over the centre, the card up.
        let wide = Shot { eye: Vec3::new(-r * 0.35, r * 0.28, -r * 0.55), target: Vec3::new(0.0, 6.0, 0.0), fov_y: 1.0 };
        if s < VERSUS {
            o.versus = smooth(s / 0.35) * (1.0 - smooth((s - (VERSUS - 0.4)) / 0.4));
            o.line = Some("the clans meet on the mountain -- may the stars watch over you");
            o.line_alpha = smooth((s - 0.6) / 0.4) * (1.0 - smooth((s - (VERSUS - 0.4)) / 0.4));
            let shot = Shot { eye: wide.eye + Vec3::new(s * 1.5, 0.0, s * 1.0), ..wide };
            return Frame { shot, beat: Beat::Versus, overlay: o, beat_t: s };
        }
        s -= VERSUS;
        // Settle: from the wide shot down onto the player's camera.
        let from = Shot { eye: wide.eye + Vec3::new(VERSUS * 1.5, 0.0, VERSUS * 1.0), ..wide };
        let _ = self.player_start;
        o.bars = 1.0 - smooth(s / SETTLE);
        o.line = Some("BEGIN");
        o.line_alpha = smooth(s / 0.2) * (1.0 - smooth((s - (SETTLE - 0.4)) / 0.4));
        Frame { shot: lerp_shot(from, self.play, s / SETTLE), beat: Beat::Settle, overlay: o, beat_t: s }
    }

    pub fn teams(&self) -> &[Team] {
        &self.teams
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intro_full(n_per: usize) -> Intro {
        let teams: Vec<Team> = (0..2 * n_per).map(|i| if i < n_per { Team::Blue } else { Team::Red }).collect();
        let play = Shot { eye: Vec3::new(0.0, 8.0, -150.0), target: Vec3::new(0.0, 1.5, -140.0), fov_y: 0.95 };
        Intro::full(&teams, Vec3::new(0.0, 0.0, -140.0), play)
    }

    /// The prologue's narration: every line in the script, each inside the
    /// prologue, each given time to be said (~15 characters a second, a
    /// slow storyteller) before the next, and shown as a subtitle.
    #[test]
    fn the_prologue_is_narrated_line_by_line() {
        let f = intro_full(5);
        let script = crate::voicescript::part("narrator").unwrap();
        assert_eq!(script.lines.len(), NARRATION.len());
        for (k, (at, key)) in NARRATION.iter().enumerate() {
            assert!(script.lines.iter().any(|l| l.key == *key), "{key} not scripted");
            assert!(*at > 0.0 && *at < PROLOGUE, "{key} outside the prologue");
            let next = NARRATION.get(k + 1).map_or(PROLOGUE, |n| n.0);
            let need = narration_text(key).len() as f32 / 15.0;
            assert!(next - at >= need, "{key}: {:.1} s to say {} characters", next - at, narration_text(key).len());
        }
        let mut g = intro_full(5);
        g.t = 16.4;
        let o = g.frame().overlay;
        assert_eq!(o.line, Some(narration_text("prologue4")));
        assert!(o.line_alpha > 0.5);
        assert_eq!(f.narration_due(0.0, 6.5), vec!["prologue1", "prologue2"]);
        assert!(intro(5).narration_due(0.0, 40.0).is_empty(), "the short intro is not narrated");
        assert!((f.length() - total() - PROLOGUE).abs() < 1e-4 && (f.fanfare_at() - FANFARE_AT - PROLOGUE).abs() < 1e-4);
    }

    fn intro(n_per: usize) -> Intro {
        let teams: Vec<Team> = (0..2 * n_per).map(|i| if i < n_per { Team::Blue } else { Team::Red }).collect();
        let play = Shot { eye: Vec3::new(0.0, 8.0, -150.0), target: Vec3::new(0.0, 1.5, -140.0), fov_y: 0.95 };
        Intro::new(&teams, Vec3::new(0.0, 0.0, -140.0), play)
    }

    /// The beats come in order, each once, and the intro ends on the player.
    #[test]
    fn the_beats_run_in_order_and_hand_over_to_play() {
        let mut i = intro(5);
        let mut seen: Vec<Beat> = Vec::new();
        while !i.done {
            let f = i.frame();
            assert!(f.shot.eye.is_finite() && f.shot.target.is_finite(), "{f:?}");
            if seen.last() != Some(&f.beat) {
                seen.push(f.beat);
            }
            i.advance(1.0 / 60.0);
        }
        use Beat::*;
        assert_eq!(seen, vec![Pan, Lane(consts::Lane::Top), Lane(consts::Lane::Mid), Lane(consts::Lane::Bot), Spawn(Team::Blue), Spawn(Team::Red), Versus, Settle]);
        // and the full intro: the prologue first, then the same
        let mut f = intro_full(5);
        let mut seen_full: Vec<Beat> = Vec::new();
        while !f.done {
            let b = f.frame().beat;
            if seen_full.last() != Some(&b) {
                seen_full.push(b);
            }
            f.advance(1.0 / 60.0);
        }
        assert_eq!(seen_full[0], Prologue);
        assert_eq!(&seen_full[1..], &seen[..]);
        // the last frame is (nearly) the gameplay camera
        let mut j = intro(5);
        j.t = total() - 1e-4;
        let f = j.frame();
        assert!((f.shot.eye - j.play.eye).length() < 0.05, "{:?} vs {:?}", f.shot.eye, j.play.eye);
        assert!(f.overlay.bars < 0.01, "bars gone by the end");
    }

    /// Every cat appears exactly once, each side during its own spawn beat,
    /// and a 15-a-side team finishes appearing before that beat ends.
    #[test]
    fn every_cat_arrives_once_in_its_own_beat() {
        for n in [5, 15] {
            let mut i = intro(n);
            let mut arrived = vec![0; 2 * n];
            while !i.done {
                let beat = i.frame().beat;
                for g in i.advance(1.0 / 60.0) {
                    arrived[g] += 1;
                    let team = if g < n { Team::Blue } else { Team::Red };
                    assert!(matches!(i.frame().beat, Beat::Spawn(t) if t == team) || matches!(beat, Beat::Spawn(t) if t == team), "god {g} appeared outside its team's beat");
                }
            }
            assert!(arrived.iter().all(|&c| c == 1), "{arrived:?}");
        }
        assert!(0.35 + 14.0 * SPAWN_STAGGER < SPAWN, "15 a side fits the spawn beat");
    }

    /// The camera never cuts: over the whole intro, on both maps, the view
    /// turns at most MAX_TURN degrees and the eye moves at most MAX_STEP
    /// units in a frame. Before: 137 degrees and 353 units at the cuts.
    #[test]
    fn the_camera_moves_smoothly_through_every_beat() {
        // tightened with the slower flights (were 6 and 12; measured now
        // 2.3 deg and 3.7 u at worst): the big moves must stay unhurried
        const MAX_TURN: f32 = 3.0;
        const MAX_STEP: f32 = 4.5;
        for m in crate::map::MapId::ALL {
            crate::map::with(m, || {
                let n = m.team_size();
                for full in [false, true] {
                let mut i = if full { intro_full(n) } else { intro(n) };
                let dt = 1.0 / 60.0;
                let mut prev = i.frame().shot;
                while !i.done {
                    i.advance(dt);
                    let s = i.frame().shot;
                    let turn = (prev.target - prev.eye).normalize().dot((s.target - s.eye).normalize()).clamp(-1.0, 1.0).acos().to_degrees();
                    let step = (s.eye - prev.eye).length();
                    assert!(turn <= MAX_TURN, "{m:?}: the view turned {turn:.1} deg in a frame at t={:.2} ({:?})", i.t, i.frame().beat);
                    // The beats are fixed by the music (2.4 s is the shortest,
                    // and flights may not overlap), so a map twice the
                    // Pass's area crosses more ground in the same flight:
                    // the eye's speed limit scales with the map's size.
                    let max_step = MAX_STEP * (crate::consts::arena_radius() / crate::pass::ARENA_RADIUS).max(1.0);
                    assert!(step <= max_step, "{m:?}: the eye jumped {step:.1} u in a frame at t={:.2} ({:?})", i.t, i.frame().beat);
                    let flat = Vec3::new(s.eye.x, 0.0, s.eye.z).length();
                    assert!(flat < crate::consts::ground_half() * 0.97, "{m:?}: the camera left the world at t={:.2}: {:.0} u out, the ground ends at {:.0}", i.t, flat, crate::consts::ground_half());
                    let ground = crate::render::terrain_height(s.eye.x, s.eye.z);
                    assert!(s.eye.y > ground + 1.0, "{m:?}: the camera went into the ground at t={:.2}: eye {} over ground {ground:.1}", i.t, s.eye);
                    prev = s;
                }
                }
            });
        }
    }

    #[test]
    fn skipping_releases_everyone_not_yet_arrived() {
        let mut i = intro(5);
        i.advance(PAN + 3.0 * LANE + 0.5); // part way through blue's spawn
        let early: Vec<usize> = (0..10).filter(|&g| i.has_arrived(g)).collect();
        assert!(!early.is_empty() && early.len() < 5);
        let rest = i.skip();
        assert_eq!(early.len() + rest.len(), 10);
        assert!(i.done && (0..10).all(|g| i.has_arrived(g)));
        assert!(i.advance(1.0).is_empty());
    }

    #[test]
    fn the_fanfare_lands_on_the_first_spawn_and_the_card_is_up_mid_versus() {
        let mut i = intro(5);
        i.t = FANFARE_AT + 0.01;
        assert_eq!(i.frame().beat, Beat::Spawn(Team::Blue));
        i.t = PAN + 3.0 * LANE + 2.0 * SPAWN + VERSUS * 0.5;
        let f = i.frame();
        assert_eq!(f.beat, Beat::Versus);
        assert!(f.overlay.versus > 0.9 && f.overlay.bars > 0.9);
    }
}
