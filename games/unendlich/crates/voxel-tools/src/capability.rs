//! Ask the model what it can do, rather than guessing from its size.
//!
//! `tier_for(params_billions, ctx)` decides how much syntax a model is
//! shown from its PARAMETER COUNT. That is a proxy and a poor one: an
//! 8B tuned for code writes an expression fine, a 70B tuned for chat
//! may not hold a format for four lines, and a fine-tune of anything
//! can be better or worse at exactly the thing being asked. Size is
//! correlated with capability and is not capability.
//!
//! So the model is given a few small challenges with CHECKABLE
//! answers, once, when it loads. What it passes decides what syntax it
//! is shown. A model that can write an expression gets expressions; a
//! model that cannot gets numbers and is not set up to fail.
//!
//! WHY THIS IS WORTH A TURN
//!
//! It is one short turn per model, cached for the session, and it
//! decides the vocabulary of every turn after it. Showing a small
//! model syntax it cannot write wastes a whole build; withholding
//! syntax a capable model could use wastes the model. Both are more
//! expensive than the probe.
//!
//! THE RUNGS
//!
//!   NUMBERS      a list: `r=3,2,1`. Everything can do this.
//!   EXPRESSIONS  `r=lerp(3,1,t)`, `scale=1-t*0.4`. Needs arithmetic
//!                held in a format.
//!   PIECEWISE    `r=at 0:3, 0.4:6, 0.7:2, 1:0.5` and per-segment
//!                expressions. Needs both, plus keeping a list keyed.
//!   PROGRAMS     assets, passes, flair -- the full language.
//!
//! Each rung is checked by asking for the thing itself and seeing
//! whether the answer parses and evaluates. Not by asking the model
//! whether it can, which is a question no model answers usefully.

use std::collections::BTreeMap;

/// What a model has been shown to be able to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Rung {
    /// Lists of numbers, and nothing else.
    #[default]
    Numbers,
    /// Arithmetic with the sweep and repeat variables in it.
    Expressions,
    /// Values that change along a run, keyed to breakpoints.
    Piecewise,
    /// The whole language: assets, passes, flair, frames.
    Programs,
}

impl Rung {
    pub fn name(self) -> &'static str {
        match self { Rung::Numbers => "numbers", Rung::Expressions => "expressions",
                     Rung::Piecewise => "piecewise", Rung::Programs => "programs" }
    }
    pub fn all() -> [Rung; 4] { [Rung::Numbers, Rung::Expressions, Rung::Piecewise, Rung::Programs] }
}

/// One challenge with a checkable answer.
pub struct Challenge {
    pub rung: Rung,
    pub ask: &'static str,
    /// Does the reply do what was asked? Given the raw reply.
    pub check: fn(&str) -> bool,
}

fn check_expression(reply: &str) -> bool {
    // Asked for a value that halves as t goes 0 to 1. Any expression
    // that evaluates correctly at both ends passes -- the SHAPE of the
    // answer is not prescribed, only that it works.
    let line = reply.lines().map(|l| l.trim().trim_start_matches("r=").trim())
        .find(|l| !l.is_empty() && !l.starts_with('#'));
    let Some(e) = line else { return false };
    let at = |t: f32| {
        let mut v = BTreeMap::new();
        v.insert("t".to_string(), t);
        crate::expr::eval(e, &v)
    };
    match (at(0.0), at(1.0)) {
        (Ok(a), Ok(b)) => (a - 8.0).abs() < 0.6 && (b - 4.0).abs() < 0.6,
        _ => false,
    }
}

fn check_piecewise(reply: &str) -> bool {
    // Asked for a run that is 2 at the start, swells to 7 in the
    // middle and returns to 1. Parsed as breakpoints; checked by
    // sampling.
    let Some(p) = parse_piecewise(reply) else { return false };
    let s = |t: f32| p.at(t);
    (s(0.0) - 2.0).abs() < 1.0 && (s(0.5) - 7.0).abs() < 1.5 && (s(1.0) - 1.0).abs() < 1.0
}

fn check_program(reply: &str) -> bool {
    // Asked for an asset placed several times: the smallest thing that
    // needs the whole language rather than one shape.
    let (prog, errs) = crate::program::parse(&crate::llm::strip_fences(reply));
    errs.is_empty() && !prog.assets.is_empty() && !prog.passes.is_empty()
}

/// The challenges, in order. Each is short: a probe that costs as much
/// as a build is not worth running.
pub fn challenges() -> Vec<Challenge> {
    vec![
        Challenge {
            rung: Rung::Expressions,
            ask: "Write ONE line: an arithmetic expression using the variable t, which equals 8 \
                  when t is 0 and 4 when t is 1. You may use + - * / and lerp(a,b,t). \
                  Write only the expression, nothing else.",
            check: check_expression,
        },
        Challenge {
            rung: Rung::Piecewise,
            ask: "Write ONE line describing a radius that changes along a run, in this form:\n\
                  \x20 at 0:VALUE, 0.5:VALUE, 1:VALUE\n\
                  It should start at 2, swell to 7 halfway, and end at 1. \
                  Write only that line.",
            check: check_piecewise,
        },
        Challenge {
            rung: Rung::Programs,
            ask: "Write an asset called `tooth` that is one box 2 wide, 3 high and 2 deep of \
                  stone, then a pass called `teeth` that adds it. Use exactly this shape:\n\
                  asset NAME\n  part P box w=N h=N d=N material=M\nend\n\
                  pass NAME add\n  repeat ASSET ring count=N radius=N\nend\n\
                  Write only those lines.",
            check: check_program,
        },
    ]
}

/// A value that changes along a run: breakpoints with values between.
///
/// This is the rung between a bare list and a full expression, and it
/// is the one most worth having. A list of radii is already piecewise
/// -- evenly spaced -- and all this adds is saying WHERE each one
/// falls. That is a small step for a model and a large one for what
/// can be described: a swelling, a waist, a sudden step.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Piecewise {
    /// (position 0..1, value), sorted.
    pub points: Vec<(f32, f32)>,
}

impl Piecewise {
    pub fn at(&self, t: f32) -> f32 {
        if self.points.is_empty() { return 0.0; }
        let t = t.clamp(0.0, 1.0);
        if t <= self.points[0].0 { return self.points[0].1; }
        if t >= self.points[self.points.len() - 1].0 { return self.points[self.points.len() - 1].1; }
        for w in self.points.windows(2) {
            if t >= w[0].0 && t <= w[1].0 {
                let span = (w[1].0 - w[0].0).max(0.0001);
                return w[0].1 + (w[1].1 - w[0].1) * (t - w[0].0) / span;
            }
        }
        self.points[self.points.len() - 1].1
    }

    /// Sample into the evenly spaced list the rest of the crate takes,
    /// so a piecewise value works everywhere a list of radii does
    /// without changing a single shape.
    pub fn sample(&self, n: usize) -> Vec<f32> {
        let n = n.clamp(2, 64);
        (0..n).map(|i| self.at(i as f32 / (n - 1) as f32)).collect()
    }
}

/// Read `at 0:2, 0.5:7, 1:1`, or the bare `0:2, 0.5:7, 1:1`.
pub fn parse_piecewise(src: &str) -> Option<Piecewise> {
    let line = src.lines().map(|l| l.trim()).find(|l| l.contains(':') && !l.starts_with('#'))?;
    let body = line.split_once("at ").map(|(_, b)| b).unwrap_or(line);
    let body = body.trim_start_matches("r=").trim();
    let mut points: Vec<(f32, f32)> = body.split(',').filter_map(|p| {
        let (k, v) = p.trim().split_once(':')?;
        // THE VALUE STOPS AT WHITESPACE. A piecewise radius written
        // inside a longer line -- `r=at 0:2, 1:1 h=24 material=stone`
        // -- made the last value `1 h=24 material=stone`, which failed
        // to parse and was silently DROPPED. The run then held its
        // previous value to the end and the swelling came out in the
        // wrong place.
        let v = v.trim().split_whitespace().next()?;
        Some((k.trim().split_whitespace().last()?.parse::<f32>().ok()?, v.parse::<f32>().ok()?))
    }).filter(|(k, v)| (0.0..=1.0).contains(k) && v.is_finite() && *v >= 0.0 && *v < 1000.0)
      .collect();
    if points.len() < 2 { return None; }
    points.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    points.dedup_by(|a, b| (a.0 - b.0).abs() < 0.001);
    Some(Piecewise { points })
}

/// What a model was found able to do.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub rung: Rung,
    /// What was tried and what happened, for the log and the player.
    pub notes: Vec<String>,
}

impl Found {
    pub fn summary(&self) -> String {
        format!("this model can write {}", self.rung.name())
    }
    pub fn can(&self, r: Rung) -> bool { self.rung >= r }
}

/// Run the challenges against a completer and see how far it gets.
///
/// STOPS AT THE FIRST FAILURE. The rungs build on each other --
/// piecewise needs the arithmetic that expressions need -- so a model
/// that cannot write an expression will not write a keyed list of
/// them, and asking is a wasted turn.
pub fn probe(c: &mut dyn crate::llm::Completer, params: Option<f32>, ctx: usize) -> Found {
    let mut found = Found { rung: Rung::Numbers, notes: Vec::new() };
    // A GUESS AS THE STARTING POINT, not the answer: if the probe
    // cannot run at all, the size-based tier is better than assuming
    // the worst.
    let guessed = match crate::prompt::tier_for(params, ctx) {
        crate::prompt::Tier::Tiny => Rung::Numbers,
        crate::prompt::Tier::Compact => Rung::Numbers,
        crate::prompt::Tier::Medium => Rung::Expressions,
        crate::prompt::Tier::Full => Rung::Piecewise,
    };
    for ch in challenges() {
        match c.complete("Answer in exactly the form asked. No explanation.", ch.ask) {
            Ok(reply) => {
                if (ch.check)(&reply) {
                    found.rung = ch.rung;
                    found.notes.push(format!("{}: yes", ch.rung.name()));
                } else {
                    found.notes.push(format!("{}: no -- {:?}", ch.rung.name(),
                                             reply.trim().chars().take(50).collect::<String>()));
                    break;
                }
            }
            Err(e) => {
                found.notes.push(format!("{}: could not ask ({e})", ch.rung.name()));
                // the probe failed rather than the model: fall back to
                // the guess instead of punishing it for a broken link
                if found.rung == Rung::Numbers { found.rung = guessed; }
                break;
            }
        }
    }
    found
}

/// The syntax a model at this rung should be shown. Withholding what
/// it can use wastes the model; showing what it cannot wastes a build.
pub fn syntax_for(r: Rung) -> String {
    let mut s = String::from("\
VALUES THAT CHANGE ALONG A RUN. Give a list and it is spread evenly:
  r=6,4,2          thick at the base, thin at the top
");
    if r >= Rung::Piecewise {
        s.push_str("\
  r=at 0:2, 0.35:7, 1:1    or say WHERE each falls: a swelling near the base
");
    }
    if r >= Rung::Expressions {
        s.push_str("\
  scale=1-t*0.4    an expression, with t running 0 to 1 along the repeat
  lift=sin(t*180)*4   sin and cos are in degrees; lerp(a,b,t) is available
");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(&'static str);
    impl crate::llm::Completer for Fake {
        fn name(&self) -> &str { "fake" }
        fn complete(&mut self, _: &str, _: &str) -> Result<String, String> { Ok(self.0.into()) }
    }

    #[test]
    fn capability_is_measured_rather_than_guessed_from_size() {
        // an 8B tuned for code writes an expression fine; a 70B tuned
        // for chat may not hold a format for four lines. Size is
        // CORRELATED with capability and is not capability.
        let mut numbers_only = Fake("I'd suggest something around 8 to start with!");
        let f = probe(&mut numbers_only, Some(70.0), 32768);
        assert_eq!(f.rung, Rung::Numbers, "a big model that fails the check gets numbers");
        assert!(f.notes[0].contains("expressions: no"), "{:?}", f.notes);
        // and a small one that passes gets more
        let mut capable = Fake("lerp(8,4,t)");
        let g = probe(&mut capable, Some(1.5), 4096);
        assert_eq!(g.rung, Rung::Expressions, "a small model that passes is not held back");
        assert!(g.can(Rung::Expressions) && !g.can(Rung::Piecewise));
    }

    #[test]
    fn the_expression_check_accepts_any_answer_that_works() {
        // the SHAPE of the answer is not prescribed, only that it does
        // what was asked
        for good in ["lerp(8,4,t)", "8 - 4*t", "8-t*4", "r=8 - (4 * t)", "  8.0 - 4.0*t  "] {
            assert!(check_expression(good), "{good:?}");
        }
        for bad in ["8", "t", "lerp(4,8,t)", "sure, you could use 8-4t", ""] {
            assert!(!check_expression(bad), "{bad:?}");
        }
    }

    #[test]
    fn piecewise_says_where_each_value_falls() {
        // a list of radii is ALREADY piecewise, evenly spaced. All this
        // adds is saying WHERE -- a small step for a model and a large
        // one for what can be described.
        let p = parse_piecewise("at 0:2, 0.35:7, 1:1").expect("parsed");
        assert_eq!(p.points.len(), 3);
        assert!((p.at(0.0) - 2.0).abs() < 0.01);
        assert!((p.at(0.35) - 7.0).abs() < 0.01);
        assert!((p.at(1.0) - 1.0).abs() < 0.01);
        // between breakpoints it runs smoothly
        let mid = p.at(0.175);
        assert!(mid > 2.0 && mid < 7.0, "{mid}");
        // outside the range it holds, rather than running away
        assert_eq!(p.at(-5.0), 2.0);
        assert_eq!(p.at(9.0), 1.0);
        // AND IT BECOMES A PLAIN LIST, so it works everywhere a list of
        // radii does without changing a single shape
        let s = p.sample(9);
        assert_eq!(s.len(), 9);
        assert!((s[0] - 2.0).abs() < 0.01 && (s[8] - 1.0).abs() < 0.01);
        assert!(s.iter().cloned().fold(0.0f32, f32::max) > 6.0, "the swelling survives: {s:?}");
        // and the sample really is a shape
        let src = format!("part p profile r={} h=24 material=stone\\n",
                          s.iter().map(|x| format!("{x:.1}")).collect::<Vec<_>>().join(","));
        let (sk, errs) = crate::sketch::parse(&src);
        assert!(errs.is_empty(), "{src}\\n{errs:?}");
        let (placed, _) = crate::sketch::resolve(&sk, (0, 64, 0));
        assert!(!crate::sketch::cells(&placed[0], 1).is_empty());
    }

    #[test]
    fn a_ragged_piecewise_answer_is_read_or_refused_cleanly() {
        assert!(parse_piecewise("0:2, 0.5:7, 1:1").is_some(), "the `at` is optional");
        assert!(parse_piecewise("r=0:2, 1:5").is_some());
        // out of order is sorted
        let p = parse_piecewise("at 1:1, 0:2, 0.5:7").unwrap();
        assert!(p.points[0].0 < p.points[1].0 && p.points[1].0 < p.points[2].0);
        // nonsense is refused rather than half-read
        assert!(parse_piecewise("just make it taper").is_none());
        assert!(parse_piecewise("0:2").is_none(), "one point is not a run");
        assert!(parse_piecewise("").is_none());
        // and values outside the range are dropped, not clamped into
        // a shape nobody asked for
        assert!(parse_piecewise("at 0:2, 5:9, 1:1").unwrap().points.len() == 2);
    }

    #[test]
    fn the_probe_stops_at_the_first_failure() {
        // the rungs build on each other: a model that cannot write an
        // expression will not write a keyed list of them, and asking is
        // a wasted turn
        let mut f = Fake("no idea sorry");
        let found = probe(&mut f, Some(8.0), 8192);
        assert_eq!(found.notes.len(), 1, "one question, then it stopped: {:?}", found.notes);
    }

    #[test]
    fn a_broken_link_falls_back_to_the_guess_rather_than_the_floor() {
        struct Dead;
        impl crate::llm::Completer for Dead {
            fn name(&self) -> &str { "dead" }
            fn complete(&mut self, _: &str, _: &str) -> Result<String, String> {
                Err("connection refused".into())
            }
        }
        let found = probe(&mut Dead, Some(70.0), 32768);
        assert!(found.rung > Rung::Numbers, "the probe failed, not the model: {:?}", found.rung);
        assert!(found.notes[0].contains("could not ask"));
    }

    #[test]
    fn a_model_is_shown_what_it_can_use_and_no_more() {
        let n = syntax_for(Rung::Numbers);
        assert!(n.contains("r=6,4,2"));
        assert!(!n.contains("lerp") && !n.contains("at 0:"), "nothing it would fail at: {n}");
        let e = syntax_for(Rung::Expressions);
        assert!(e.contains("lerp") && e.contains("sin"), "{e}");
        let p = syntax_for(Rung::Piecewise);
        assert!(p.contains("at 0:2") && p.contains("lerp"), "the higher rung keeps the lower: {p}");
        // the whole thing stays short even at the top
        assert!(syntax_for(Rung::Programs).len() < 600);
    }
}
