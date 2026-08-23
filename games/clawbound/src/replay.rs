//! replay.rs — a match is its setup plus the keys you pressed.
//!
//! The sim is deterministic: same draft, same terrain seed, same doctrine
//! switches, same inputs, same match, down to the digest. That makes a replay a
//! very small file — a header and a list of the ticks where the input changed —
//! and it makes three things possible that a game like this should have:
//!
//! * **A bug report you can run.** "The lynx got stuck at the ford" is a story;
//!   a replay is the thing that reproduces it.
//! * **A regression test with a real match in it.** The suite is full of
//!   two-warrior fixtures because a whole match is tedious to construct by hand.
//!   A recorded one is a fixture nobody had to build.
//! * **The honest check on determinism.** Recording stores the digest the
//!   original run finished on; playback recomputes it. If they differ, this
//!   sim is not as deterministic as everything else in this crate assumes, and
//!   the netcode that would eventually be built on that assumption would have
//!   found out much later and much worse.
//!
//! The format is text, one line per event, because a replay you can read in a
//! diff is worth more than a few saved bytes — and because the header doubles
//! as a record of which doctrines were switched on when the match was played.

use crate::ancestor::{self, Ancestor};
use crate::battleground::{Battleground, LanePolicy};
use crate::draft::{species, Draft};

/// A recorded match: how it was set up, and every input change in it.
#[derive(Clone, Debug, PartialEq)]
pub struct Replay {
    pub picks_a: [Ancestor; 3],
    pub picks_b: [Ancestor; 3],
    pub seed: u32,
    pub kit_laning: bool,
    pub wombo: bool,
    pub collapse_on_breach: bool,
    pub spoils: bool,
    /// (tick, move x, move z, claw, ability) — only where the input *changed*,
    /// which is what makes a ten-minute match a few dozen lines.
    pub events: Vec<(u32, f32, f32, bool, Option<usize>)>,
    /// Ticks the recording ran for, and the digest it ended on.
    pub ticks: u32,
    pub digest: u64,
}

impl Replay {
    /// Build the battleground this replay describes, ready for tick zero.
    pub fn stage(&self) -> Battleground {
        let policy = if self.kit_laning { LanePolicy::Kit } else { LanePolicy::Seat };
        let mut bg = Battleground::new_with(self.picks_a, self.picks_b, self.seed, policy);
        bg.wombo = self.wombo;
        bg.collapse_on_breach = self.collapse_on_breach;
        bg.spoils = self.spoils;
        bg
    }

    /// Play it back and return the digest it reaches. Equal to `self.digest`
    /// unless something has changed underneath the recording — which is the
    /// interesting case, and why this returns the number rather than a bool.
    pub fn play(&self) -> u64 {
        let mut bg = self.stage();
        let mut next = 0usize;
        let mut held = (0.0f32, 0.0f32, false, None);
        for tick in 0..self.ticks {
            while next < self.events.len() && self.events[next].0 == tick {
                let e = self.events[next];
                held = (e.1, e.2, e.3, e.4);
                next += 1;
            }
            // one-shot actions fire on the tick they were pressed; movement holds
            bg.set_input(held.0, held.1, held.2, held.3);
            held.2 = false;
            held.3 = None;
            bg.step(1.0 / 30.0);
        }
        bg.digest()
    }

    /// Does this replay still produce the match it recorded?
    pub fn verify(&self) -> bool {
        self.play() == self.digest
    }

    pub fn to_text(&self) -> String {
        let names = |p: &[Ancestor; 3]| {
            p.iter().map(|&a| species(a)).collect::<Vec<_>>().join(",")
        };
        let mut s = String::new();
        s.push_str("clawbound-replay 1\n");
        s.push_str(&format!("clan_a {}\n", names(&self.picks_a)));
        s.push_str(&format!("clan_b {}\n", names(&self.picks_b)));
        s.push_str(&format!("seed {}\n", self.seed));
        s.push_str(&format!(
            "doctrine kit_laning={} wombo={} collapse={} spoils={}\n",
            self.kit_laning, self.wombo, self.collapse_on_breach, self.spoils
        ));
        s.push_str(&format!("ticks {}\n", self.ticks));
        s.push_str(&format!("digest {:016x}\n", self.digest));
        for &(t, mx, mz, claw, ab) in &self.events {
            s.push_str(&format!(
                "{t} {mx} {mz} {} {}\n",
                if claw { 1 } else { 0 },
                ab.map(|a| a.to_string()).unwrap_or_else(|| "-".into())
            ));
        }
        s
    }

    pub fn from_text(text: &str) -> Result<Replay, String> {
        let mut picks_a = None;
        let mut picks_b = None;
        let (mut seed, mut ticks, mut digest) = (0u32, 0u32, 0u64);
        let (mut kit_laning, mut wombo, mut collapse, mut spoils) = (true, false, false, false);
        let mut events = Vec::new();
        let trio = |s: &str| -> Result<[Ancestor; 3], String> {
            let v: Vec<Ancestor> = s
                .split(',')
                .map(|n| Draft::find(n).ok_or_else(|| format!("unknown ancestor {n:?}")))
                .collect::<Result<_, _>>()?;
            if v.len() != 3 {
                return Err(format!("a clan is three, got {}", v.len()));
            }
            Ok([v[0], v[1], v[2]])
        };
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut w = line.split_whitespace();
            let head = w.next().unwrap_or("");
            match head {
                "clawbound-replay" => {
                    if w.next() != Some("1") {
                        return Err("unknown replay version".into());
                    }
                }
                "clan_a" => picks_a = Some(trio(w.next().unwrap_or(""))?),
                "clan_b" => picks_b = Some(trio(w.next().unwrap_or(""))?),
                "seed" => seed = w.next().unwrap_or("0").parse().map_err(|_| "bad seed")?,
                "ticks" => ticks = w.next().unwrap_or("0").parse().map_err(|_| "bad ticks")?,
                "digest" => {
                    digest = u64::from_str_radix(w.next().unwrap_or("0"), 16)
                        .map_err(|_| "bad digest")?
                }
                "doctrine" => {
                    for kv in w {
                        let (k, v) = kv.split_once('=').unwrap_or((kv, "false"));
                        let on = v == "true";
                        match k {
                            "kit_laning" => kit_laning = on,
                            "wombo" => wombo = on,
                            "collapse" => collapse = on,
                            "spoils" => spoils = on,
                            _ => return Err(format!("unknown doctrine {k:?}")),
                        }
                    }
                }
                _ => {
                    // an input line: tick mx mz claw ability
                    let f: Vec<&str> = line.split_whitespace().collect();
                    if f.len() != 5 {
                        return Err(format!("line {}: expected 5 fields", n + 1));
                    }
                    let p = |i: usize| -> Result<f32, String> {
                        f[i].parse().map_err(|_| format!("line {}: bad number", n + 1))
                    };
                    events.push((
                        f[0].parse().map_err(|_| format!("line {}: bad tick", n + 1))?,
                        p(1)?,
                        p(2)?,
                        f[3] == "1",
                        if f[4] == "-" { None } else { f[4].parse().ok() },
                    ));
                }
            }
        }
        Ok(Replay {
            picks_a: picks_a.ok_or("no clan_a")?,
            picks_b: picks_b.ok_or("no clan_b")?,
            seed,
            kit_laning,
            wombo,
            collapse_on_breach: collapse,
            spoils,
            events,
            ticks,
            digest,
        })
    }
}

/// Records a match as it is played. Wraps the same `set_input`/`step` pair a
/// caller would use, so recording is not a different code path that can drift
/// away from the real one.
pub struct Recorder {
    replay: Replay,
    last: (f32, f32, bool, Option<usize>),
    tick: u32,
}

impl Recorder {
    pub fn new(
        picks_a: [Ancestor; 3],
        picks_b: [Ancestor; 3],
        seed: u32,
        bg: &Battleground,
        kit_laning: bool,
    ) -> Recorder {
        Recorder {
            replay: Replay {
                picks_a,
                picks_b,
                seed,
                kit_laning,
                wombo: bg.wombo,
                collapse_on_breach: bg.collapse_on_breach,
                spoils: bg.spoils,
                events: Vec::new(),
                ticks: 0,
                digest: 0,
            },
            last: (0.0, 0.0, false, None),
            tick: 0,
        }
    }

    /// One tick, with the input for it. Only changes are stored, and a one-shot
    /// (a claw or an ability) is always a change, since it fires once.
    pub fn step(&mut self, bg: &mut Battleground, mx: f32, mz: f32, claw: bool, ability: Option<usize>) {
        let now = (mx, mz, claw, ability);
        let moved = (now.0, now.1) != (self.last.0, self.last.1);
        if moved || claw || ability.is_some() || (self.last.2 || self.last.3.is_some()) {
            self.replay.events.push((self.tick, mx, mz, claw, ability));
        }
        self.last = now;
        bg.set_input(mx, mz, claw, ability);
        bg.step(1.0 / 30.0);
        self.tick += 1;
    }

    /// Close the recording against the match it produced.
    pub fn finish(mut self, bg: &Battleground) -> Replay {
        self.replay.ticks = self.tick;
        self.replay.digest = bg.digest();
        self.replay
    }
}

/// A one-line summary for a replay listing.
pub fn describe(r: &Replay) -> String {
    let names = |p: &[Ancestor; 3]| {
        p.iter().map(|&a| ancestor::kit(a).name).collect::<Vec<_>>().join(", ")
    };
    format!(
        "{} against {} on land {} — {} ticks ({:.0}s), {} input events",
        names(&r.picks_a),
        names(&r.picks_b),
        r.seed,
        r.ticks,
        r.ticks as f32 / 30.0,
        r.events.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record_a_short_match() -> (Replay, u64) {
        let a = [Ancestor::Lynx, Ancestor::Lion, Ancestor::Puma];
        let b = [Ancestor::Tiger, Ancestor::Leopard, Ancestor::Serval];
        let mut bg = Battleground::new_with(a, b, 5, LanePolicy::Kit);
        let mut rec = Recorder::new(a, b, 5, &bg, true);
        for t in 0..300u32 {
            // a little of everything: walking, holding still, clawing, casting
            let (mx, mz) = match (t / 40) % 3 {
                0 => (1.0, 0.0),
                1 => (0.0, -1.0),
                _ => (0.0, 0.0),
            };
            let claw = t % 37 == 0;
            let ability = if t % 91 == 0 { Some(0) } else { None };
            rec.step(&mut bg, mx, mz, claw, ability);
        }
        let digest = bg.digest();
        (rec.finish(&bg), digest)
    }

    #[test]
    fn a_recorded_match_plays_back_to_the_same_digest() {
        let (replay, digest) = record_a_short_match();
        assert_eq!(replay.digest, digest);
        assert!(replay.verify(), "playback diverged from the recording");
        // and twice, because a replay that only works once is not a replay
        assert_eq!(replay.play(), digest);
    }

    #[test]
    fn a_replay_survives_the_round_trip_through_text() {
        let (replay, _) = record_a_short_match();
        let text = replay.to_text();
        let back = Replay::from_text(&text).expect("parses");
        assert_eq!(back, replay);
        assert!(back.verify());
        // the text is readable, and says what was switched on
        assert!(text.starts_with("clawbound-replay 1\n"));
        assert!(text.contains("clan_a lynx,lion,puma"));
        assert!(text.contains("kit_laning=true"));
        // comments and blank lines are tolerated, because people edit these
        let edited = format!("# a bug report\n\n{text}");
        assert_eq!(Replay::from_text(&edited).unwrap(), replay);
    }

    #[test]
    fn only_changes_are_stored() {
        let (replay, _) = record_a_short_match();
        assert!(
            replay.events.len() < replay.ticks as usize / 4,
            "300 ticks of mostly-held input should not be 300 events, got {}",
            replay.events.len()
        );
    }

    #[test]
    fn a_replay_notices_when_the_ground_changes_under_it() {
        // The point of storing the setup: a replay must not silently "verify"
        // against a different match.
        let (replay, _) = record_a_short_match();
        let mut other_land = replay.clone();
        other_land.seed = replay.seed + 1;
        assert!(!other_land.verify(), "a different land is a different match");
        let mut other_clan = replay.clone();
        other_clan.picks_b = [Ancestor::CaveLion, Ancestor::Jaguar, Ancestor::Suanni];
        assert!(!other_clan.verify(), "a different enemy is a different match");
        let mut other_laning = replay.clone();
        other_laning.kit_laning = !replay.kit_laning;
        assert!(!other_laning.verify(), "a different doctrine is a different match");
        // The behaviour switches are stored too, but a ten-second recording
        // cannot prove they matter: the clans have not met yet, so flipping
        // `wombo` or `collapse_on_breach` reproduces this particular match
        // exactly. Asserted where it is true rather than where it reads well.
        let mut other_wombo = replay.clone();
        other_wombo.wombo = !replay.wombo;
        assert_eq!(
            other_wombo.play(),
            replay.digest,
            "before first contact, crowd-control doctrine cannot change anything"
        );
    }

    #[test]
    fn a_broken_file_says_what_is_wrong() {
        assert!(Replay::from_text("").unwrap_err().contains("clan_a"));
        assert!(Replay::from_text("clawbound-replay 2\n").unwrap_err().contains("version"));
        assert!(Replay::from_text("clan_a lion,mouse,puma\n")
            .unwrap_err()
            .contains("unknown ancestor"));
        assert!(Replay::from_text("clan_a lion,puma\n").unwrap_err().contains("three"));
    }
}
