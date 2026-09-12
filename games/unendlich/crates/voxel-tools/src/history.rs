//! A place was not built in one afternoon.
//!
//! Everything here is built at a single moment, in one material, to
//! one plan. That is why the results read as models rather than
//! places: a real castle is a Norman keep, a hall thrown up two
//! centuries later against its south wall, a chapel added when someone
//! got rich, and repairs in whatever stone was nearest when the roof
//! fell in. The mismatch IS the interest.
//!
//! A phase is a piece of work done at one time, and it knows two
//! things nothing else here knows:
//!
//!   WHAT CAME BEFORE  it leans on the earlier work, cuts into it,
//!                     reuses its walls. A later hall shares the
//!                     keep's wall because building a second wall
//!                     would have been madness.
//!   HOW LONG AGO      the earliest work has weathered longest, so age
//!                     runs oldest-most-worn. A place where everything
//!                     is equally weathered is a place with no history.
//!
//! WHY THIS IS NOT JUST `transform` APPLIED TWICE
//!
//! A verb acts on a whole build uniformly. Phases apply DIFFERENT
//! verbs at DIFFERENT strengths to DIFFERENT parts, in an order that
//! matches how the place actually came to be -- and the boundary
//! between two phases is the thing worth looking at, which a uniform
//! transform can never produce.

use crate::transform::Verb;

/// One campaign of building.
#[derive(Debug, Clone, PartialEq)]
pub struct Phase {
    pub name: String,
    /// How long ago, in arbitrary units: bigger is older. Only the
    /// order and the spread matter.
    pub age: i32,
    /// What it was built of.
    pub material: String,
    /// The names of earlier phases it leans on: it will share their
    /// walls and cut its doors through them.
    pub against: Vec<String>,
    /// What happened to it since. The oldest work has usually had the
    /// most happen to it, but not always -- a phase can be rebuilt.
    pub weathering: Option<(Verb, f32)>,
    /// One line: what this phase is and why it was done.
    pub because: String,
}

/// A place with a history.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct History {
    pub phases: Vec<Phase>,
}

impl History {
    /// Oldest first, which is the order they must be built in: a phase
    /// cannot lean on something that is not there yet.
    pub fn in_order(&self) -> Vec<&Phase> {
        let mut v: Vec<&Phase> = self.phases.iter().collect();
        v.sort_by_key(|p| -p.age);
        v
    }

    /// Does every phase lean only on work that already exists?
    ///
    /// A phase leaning on something younger than itself is a phase
    /// that was built against a wall that had not been raised, which
    /// is the one mistake this whole idea can make.
    pub fn check(&self) -> Vec<String> {
        let mut errs = Vec::new();
        for p in &self.phases {
            for a in &p.against {
                match self.phases.iter().find(|q| &q.name == a) {
                    None => errs.push(format!("'{}' leans on '{a}', which is not a phase", p.name)),
                    Some(q) if q.age <= p.age => errs.push(format!(
                        "'{}' leans on '{a}', which is not older than it -- a wall cannot be \
                         built against one that is not there yet", p.name)),
                    _ => {}
                }
            }
        }
        if self.phases.len() > 1 {
            let ages: std::collections::BTreeSet<i32> = self.phases.iter().map(|p| p.age).collect();
            if ages.len() == 1 {
                errs.push("every phase is the same age; that is one phase in several pieces".into());
            }
        }
        errs
    }

    /// The weathering each phase should carry, from its age.
    ///
    /// Assigned when a phase does not say for itself, so a history can
    /// be written without deciding what happened to every part of it.
    /// The oldest work has weathered longest; the newest is sharp.
    pub fn weather_by_age(&mut self) {
        let Some(oldest) = self.phases.iter().map(|p| p.age).max() else { return };
        let Some(newest) = self.phases.iter().map(|p| p.age).min() else { return };
        let span = (oldest - newest).max(1) as f32;
        for p in self.phases.iter_mut() {
            if p.weathering.is_some() { continue; }
            let t = (p.age - newest) as f32 / span;
            p.weathering = if t < 0.25 { None }        // new work is sharp
                else if t < 0.6 { Some((Verb::Overgrown, 0.3 + t * 0.3)) }
                else { Some((Verb::Fossilised, 0.3 + t * 0.5)) };
        }
    }

    pub fn summary(&self) -> String {
        self.in_order().iter().map(|p| {
            let w = match &p.weathering {
                Some((v, s)) => format!(", {} at {:.0}%", v.name(), s * 100.0),
                None => String::new(),
            };
            format!("  {} ({}, {}{w}) -- {}", p.name, p.age, p.material, p.because)
        }).collect::<Vec<_>>().join("\n")
    }
}

/// The prompt that asks for a history.
pub fn prompt(request: &str, span: &str) -> String {
    format!("\
A player asked for: {request}

Places are not built in one afternoon, and the ones worth looking at were not. Give this
one a HISTORY: two to five campaigns of building, {span}.

  phase NAME age=N material=M [against=EARLIER,EARLIER] [weathered=WORD]
    <one line: what was built and why someone bothered>

  age        how long ago, bigger is older. The numbers only have to be in the right
             order and far enough apart to matter.
  against    the earlier phases it is built onto. A later hall shares the keep's wall,
             because raising a second wall beside the first would have been madness.
  weathered  melting, overgrown, collapsing, half-built, fossilised, burnt, sunken,
             frozen, broken. Leave it out and it follows the age.

THE MISMATCH IS THE POINT. Different stone, a different idea of what a building is for,
work abandoned half-done and finished by someone with less money. A place where every
part matches is a place with no history.

EXAMPLE
  a castle
phase keep age=400 material=stone
  A square tower, put up in a hurry by someone who expected to be attacked.
end
phase hall age=250 material=sandstone against=keep
  A great hall built against the keep's south wall once the fighting stopped and the
  family wanted somewhere to eat that was not a cellar.
end
phase chapel age=120 material=quartz against=hall
  A small chapel, added by a widow, in better stone than anything around it.
end
phase repairs age=20 material=cobblestone against=keep,hall
  Patching where the north side fell in, done cheaply and it shows.
end")
}

/// Read a history.
pub fn parse(src: &str) -> (History, Vec<String>) {
    let mut h = History::default();
    let mut errs = Vec::new();
    let mut cur: Option<Phase> = None;
    let mut body = String::new();
    for line in src.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') { continue; }
        let w: Vec<&str> = t.split_whitespace().collect();
        let head = w.first().map(|x| x.to_lowercase()).unwrap_or_default();
        if head == "end" {
            if let Some(mut p) = cur.take() {
                p.because = body.trim().to_string();
                body.clear();
                h.phases.push(p);
            }
            continue;
        }
        if head == "phase" {
            if let Some(mut p) = cur.take() {
                p.because = body.trim().to_string();
                body.clear();
                errs.push(format!("phase '{}' was not closed with `end`", p.name));
                h.phases.push(p);
            }
            let kv_at = w.iter().position(|s| s.contains('=')).unwrap_or(w.len());
            if kv_at <= 1 { errs.push(format!("a phase needs a name: {t:?}")); continue; }
            let kv = |k: &str| w[kv_at..].iter().find_map(|s| s.strip_prefix(&format!("{k}=")));
            cur = Some(Phase {
                name: w[1..kv_at].join(" ").to_lowercase(),
                age: kv("age").and_then(|v| v.parse().ok()).unwrap_or(100),
                material: kv("material").or_else(|| kv("block"))
                    .unwrap_or("stone").to_lowercase(),
                against: kv("against").or_else(|| kv("onto")).map(|v| v.split(',')
                    .map(|x| x.trim().to_lowercase()).filter(|x| !x.is_empty()).collect())
                    .unwrap_or_default(),
                weathering: kv("weathered").and_then(Verb::parse).map(|v| (v, 0.6)),
                because: String::new(),
            });
            continue;
        }
        if cur.is_some() { body.push_str(t); body.push(' '); }
    }
    if let Some(mut p) = cur {
        p.because = body.trim().to_string();
        errs.push(format!("phase '{}' was not closed with `end`", p.name));
        h.phases.push(p);
    }
    errs.extend(h.check());
    (h, errs)
}

/// What to tell the turn that builds one phase.
///
/// Each phase is built on its own, the way a dossier part is, and for
/// the same reason: the prompt stays the size of one phase however
/// many there are. What it adds is the earlier work it must MEET.
pub fn phase_prompt(h: &History, i: usize, vocabulary: &str) -> Option<String> {
    let ordered = h.in_order();
    let p = ordered.get(i)?;
    let mut s = format!("\
BUILDING ONE PHASE OF: a place made over time
THIS PHASE: {} -- {}
BUILT OF: {}
", p.name, p.because, p.material);
    let earlier: Vec<&&Phase> = ordered[..i].iter()
        .filter(|q| p.against.contains(&q.name)).collect();
    if earlier.is_empty() {
        if i == 0 {
            s.push_str("\nThis is the FIRST work here. Nothing constrains it but the ground.\n");
        } else {
            s.push_str("\nThis stands apart from the earlier work rather than touching it.\n");
        }
    } else {
        s.push_str(&format!("\nIT IS BUILT AGAINST:\n{}\n\
                             Share their walls where you can -- raising a second wall beside \
                             an existing one is what nobody would have done. Cut your doors \
                             THROUGH them.\n",
            earlier.iter().map(|q| format!("  {} ({}, {} older)", q.name, q.material, q.age - p.age))
                .collect::<Vec<_>>().join("\n")));
    }
    if i > 0 {
        s.push_str(&format!("\nIt is {} years newer than the oldest work here, and should not \
                             pretend otherwise: different stone, a different idea of what a \
                             building is for.\n", ordered[0].age - p.age));
    }
    s.push_str(&format!("\n{vocabulary}\n\nWrite the passes for THIS PHASE only.\n"));
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn castle() -> History {
        parse("phase keep age=400 material=stone\n  A square tower, put up in a hurry.\nend\n\
               phase hall age=250 material=sandstone against=keep\n  A great hall against \
               the keep's south wall.\nend\n\
               phase chapel age=120 material=quartz against=hall\n  Added by a widow.\nend\n\
               phase repairs age=20 material=cobblestone against=keep,hall\n  Done cheaply \
               and it shows.\nend\n").0
    }

    #[test]
    fn a_place_is_built_oldest_first_because_nothing_leans_on_the_future() {
        let h = castle();
        assert_eq!(h.phases.len(), 4);
        let order: Vec<&str> = h.in_order().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(order, vec!["keep", "hall", "chapel", "repairs"]);
        assert!(h.check().is_empty(), "{:?}", h.check());
        // and a phase leaning on something YOUNGER is the one mistake
        // this whole idea can make
        // AGE IS HOW LONG AGO: bigger is OLDER. My first version of
        // this named the age=10 phase "old" and expected a complaint,
        // but a phase at age=10 leaning on one at age=500 is new work
        // against old work, which is exactly right. The invalid case is
        // the reverse: OLD work leaning on something younger than it.
        let (bad, errs) = parse("phase ancient age=500 material=stone against=recent\n  x\nend\n\
                                 phase recent age=10 material=stone\n  y\nend\n");
        assert!(errs.iter().any(|e| e.contains("not older than it")), "{errs:?}");
        let _ = bad;
        // leaning on nothing at all is caught too
        let (_, e2) = parse("phase a age=10 material=stone against=ghost\n  x\nend\n");
        assert!(e2.iter().any(|e| e.contains("not a phase")), "{e2:?}");
    }

    #[test]
    fn the_oldest_work_has_weathered_longest() {
        // a place where everything is equally weathered is a place with
        // no history
        let mut h = castle();
        h.weather_by_age();
        let by = |n: &str| h.phases.iter().find(|p| p.name == n).unwrap().weathering;
        assert!(by("keep").is_some(), "four hundred years shows");
        assert!(by("repairs").is_none(), "twenty years does not");
        // and the oldest is worse than the middle
        let (_, keep_s) = by("keep").unwrap();
        let (_, hall_s) = by("hall").unwrap();
        assert!(keep_s > hall_s, "{keep_s} vs {hall_s}");
        // a phase that says for itself is not overridden
        let mut told = parse("phase a age=400 material=stone weathered=burnt\n  x\nend\n\
                              phase b age=10 material=stone\n  y\nend\n").0;
        told.weather_by_age();
        assert_eq!(told.phases[0].weathering.map(|(v, _)| v), Some(Verb::Burnt),
                   "it was told what happened to it");
    }

    #[test]
    fn a_phase_is_told_what_it_must_meet_and_nothing_else() {
        let h = castle();
        // the first has nothing to meet
        let first = phase_prompt(&h, 0, "VOCAB").unwrap();
        assert!(first.contains("This is the FIRST work here"), "{first}");
        assert!(!first.contains("IT IS BUILT AGAINST"));
        // the hall meets the keep, and is told to share its wall
        let hall = phase_prompt(&h, 1, "VOCAB").unwrap();
        assert!(hall.contains("IT IS BUILT AGAINST") && hall.contains("keep"), "{hall}");
        assert!(hall.contains("Share their walls"), "{hall}");
        assert!(hall.contains("150 older"), "it says how much older: {hall}");
        // and it carries ONLY what it leans on: the chapel is not in
        // the hall's prompt
        assert!(!hall.contains("chapel"), "{hall}");
        // the prompt does not grow with the history
        let last = phase_prompt(&h, 3, "VOCAB").unwrap();
        assert!((last.len() as i32 - hall.len() as i32).abs() < 400,
                "{} vs {}", hall.len(), last.len());
    }

    #[test]
    fn later_work_is_told_not_to_pretend_it_is_older() {
        let h = castle();
        let chapel = phase_prompt(&h, 2, "V").unwrap();
        assert!(chapel.contains("280 years newer"), "{chapel}");
        assert!(chapel.contains("should not pretend otherwise"), "{chapel}");
        assert!(chapel.contains("BUILT OF: quartz"), "in its own material: {chapel}");
    }

    #[test]
    fn one_phase_in_several_pieces_is_not_a_history() {
        let (_, errs) = parse("phase a age=100 material=stone\n  x\nend\n\
                               phase b age=100 material=stone\n  y\nend\n");
        assert!(errs.iter().any(|e| e.contains("that is one phase in several pieces")),
                "{errs:?}");
        // a single phase is fine: not everything has a history
        let (one, e2) = parse("phase a age=100 material=stone\n  x\nend\n");
        assert!(e2.is_empty(), "{e2:?}");
        assert_eq!(one.phases.len(), 1);
    }

    #[test]
    fn the_prompt_says_the_mismatch_is_the_point() {
        let p = prompt("a castle", "over four hundred years");
        assert!(p.contains("THE MISMATCH IS THE POINT"), "{p}");
        assert!(p.contains("no history"), "{p}");
        assert!(p.contains("raising a second wall beside the first would have been madness"));
        assert!(p.contains("over four hundred years"), "the span is used");
        // and its own example parses into a valid history
        let body: String = p.split("EXAMPLE").nth(1).unwrap().lines().skip(2)
            .map(|l| format!("{l}\n")).collect();
        let (h, errs) = parse(&body);
        assert!(errs.is_empty(), "{body}\n{errs:?}");
        assert_eq!(h.phases.len(), 4);
        assert_eq!(h.in_order()[0].name, "keep");
        // every phase says why someone bothered
        for ph in &h.phases {
            assert!(ph.because.len() > 20, "'{}' says nothing: {:?}", ph.name, ph.because);
        }
    }

    #[test]
    fn a_ragged_history_is_read_rather_than_refused() {
        // an unclosed phase is kept, not lost
        let (h, errs) = parse("phase a age=200 material=stone\n  first\n\
                               phase b age=50 material=oak_planks\n  second\nend\n");
        assert_eq!(h.phases.len(), 2, "both survive");
        assert!(errs.iter().any(|e| e.contains("not closed")), "{errs:?}");
        assert_eq!(h.phases[0].because, "first");
        // missing fields take defaults rather than dropping the phase
        let (d, _) = parse("phase c\n  something\nend\n");
        assert_eq!(d.phases.len(), 1);
        assert_eq!(d.phases[0].material, "stone");
        assert_eq!(d.phases[0].age, 100);
    }
}
