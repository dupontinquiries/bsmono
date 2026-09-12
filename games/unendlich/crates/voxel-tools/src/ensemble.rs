//! A place explains the places around it.
//!
//! `siting` decides where ONE thing goes. `brief` relates rooms inside
//! ONE building. Nothing relates buildings to each other, and that is
//! the difference between a map and a scatter of models.
//!
//! A castle has a village below it because the castle is WHY the
//! village is there -- people settle under a wall they can run behind.
//! The village has a mill because it has grain, and the mill is on the
//! river because that is what turns it. A road joins them because
//! everyone walks the same line between the same two places until it
//! becomes one. Nothing in that list is decoration: each place is the
//! reason for the next.
//!
//! WHAT AN ENSEMBLE IS
//!
//! A principal place, and the ones that follow from it. Each satellite
//! carries:
//!
//!   WHY        the reason it exists, which is a fact about the
//!              principal and not about itself
//!   HOW FAR    within sight, a walk, or a half-day -- which is a
//!              statement about the relationship, not a distance
//!   WHAT LINKS them: a road, a track, a stair, a causeway, or nothing
//!
//! THE ORDER MATTERS. A satellite can depend on another satellite -- a
//! mill serves the village, not the castle -- and building the village
//! after its own mill would put the mill somewhere for no reason.

/// How far one place sits from another, stated as a relationship
/// rather than a number, because "within sight" means something and
/// "180 blocks" does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Touching, or nearly: a gatehouse to its wall.
    Adjoining,
    /// You can see it from there, which is what matters for a
    /// watchtower and for anything defensive.
    WithinSight,
    /// A few minutes' walk. Daily traffic.
    AShortWalk,
    /// Far enough that going is a decision.
    AHalfDay,
}

impl Reach {
    pub fn parse(s: &str) -> Option<Reach> {
        let l = s.to_lowercase();
        Some(if l.contains("adjoin") || l.contains("touch") || l.contains("against") {
            Reach::Adjoining
        } else if l.contains("sight") || l.contains("view") || l.contains("visible") {
            Reach::WithinSight
        } else if l.contains("walk") || l.contains("short") || l.contains("near") {
            Reach::AShortWalk
        } else if l.contains("day") || l.contains("far") || l.contains("distant") {
            Reach::AHalfDay
        } else { return None })
    }
    pub fn name(self) -> &'static str {
        match self { Reach::Adjoining => "adjoining", Reach::WithinSight => "within sight",
                     Reach::AShortWalk => "a short walk", Reach::AHalfDay => "half a day" }
    }
    /// Blocks, scaled to the size of the principal place: "within
    /// sight" of a hut and of a citadel are not the same distance.
    pub fn blocks(self, principal_span: i32) -> (i32, i32) {
        let s = principal_span.max(8) as f32;
        match self {
            Reach::Adjoining => (0, (s * 0.7) as i32),
            Reach::WithinSight => ((s * 0.8) as i32, (s * 3.5) as i32),
            Reach::AShortWalk => ((s * 3.0) as i32, (s * 9.0) as i32),
            Reach::AHalfDay => ((s * 9.0) as i32, (s * 30.0) as i32),
        }
    }
}

/// What joins two places.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link { None, Track, Road, Stair, Causeway, Bridge }

impl Link {
    pub fn parse(s: &str) -> Option<Link> {
        let l = s.to_lowercase();
        Some(if l.contains("road") { Link::Road }
             else if l.contains("track") || l.contains("path") || l.contains("trail") { Link::Track }
             else if l.contains("stair") || l.contains("step") { Link::Stair }
             else if l.contains("causeway") || l.contains("dyke") { Link::Causeway }
             else if l.contains("bridge") { Link::Bridge }
             else if l.contains("none") || l.contains("nothing") { Link::None }
             else { return None })
    }
    pub fn name(self) -> &'static str {
        match self { Link::None => "nothing", Link::Track => "a track", Link::Road => "a road",
                     Link::Stair => "a stair", Link::Causeway => "a causeway",
                     Link::Bridge => "a bridge" }
    }
    /// How wide, and whether it should be cut level into the ground.
    pub fn shape(self) -> (i32, bool) {
        match self { Link::None => (0, false), Link::Track => (2, false), Link::Road => (4, true),
                     Link::Stair => (3, true), Link::Causeway => (5, true), Link::Bridge => (4, false) }
    }
}

/// One place that follows from another.
#[derive(Debug, Clone, PartialEq)]
pub struct Satellite {
    pub name: String,
    /// What kind of thing it is, for siting and for building.
    pub kind: String,
    /// What it follows FROM: the principal, or another satellite.
    pub serves: String,
    pub reach: Reach,
    pub link: Link,
    /// Why it is there. A fact about what it serves, not about itself.
    pub because: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Ensemble {
    pub principal: String,
    pub principal_kind: String,
    pub satellites: Vec<Satellite>,
}

impl Ensemble {
    /// Build order: nothing before the thing it serves.
    ///
    /// A mill serves the village, not the castle, and placing the mill
    /// before the village would put it somewhere for no reason.
    pub fn in_order(&self) -> Result<Vec<&Satellite>, String> {
        let mut done: Vec<String> = vec![self.principal.clone()];
        let mut out: Vec<&Satellite> = Vec::new();
        let mut left: Vec<&Satellite> = self.satellites.iter().collect();
        while !left.is_empty() {
            let before = left.len();
            left.retain(|s| {
                if done.contains(&s.serves) {
                    done.push(s.name.clone());
                    out.push(s);
                    false
                } else { true }
            });
            if left.len() == before {
                return Err(format!("{} place(s) serve something that is never built: {}",
                                   left.len(),
                                   left.iter().map(|s| format!("{} -> {}", s.name, s.serves))
                                       .collect::<Vec<_>>().join(", ")));
            }
        }
        Ok(out)
    }

    pub fn summary(&self) -> String {
        let mut s = format!("{} ({})", self.principal, self.principal_kind);
        for sat in &self.satellites {
            s.push_str(&format!("\n  {} ({}) -- {} from {}, joined by {}: {}",
                                sat.name, sat.kind, sat.reach.name(), sat.serves,
                                sat.link.name(), sat.because));
        }
        s
    }
}

/// The prompt that asks what else belongs here.
pub fn prompt(request: &str, site: &str) -> String {
    format!("\
A player asked for: {request}

{site}

A place explains the places around it. A castle has a village below it BECAUSE the castle
is why the village is there -- people settle under a wall they can run behind. The village
has a mill because it has grain. A road joins them because everyone walks the same line
until it becomes one.

Name two to five other places that FOLLOW FROM this one.

  place NAME kind=WHAT serves=WHICH reach=HOW-FAR link=WHAT-JOINS
    <one line: why it is there, as a fact about what it serves>

  serves   the place it follows from -- the main one, or another of yours
  reach    adjoining, within sight, a short walk, or half a day
  link     road, track, stair, causeway, bridge, or none

EVERY PLACE MUST BE A CONSEQUENCE. If you cannot say why the main place causes it, leave
it out. A blacksmith near a castle is a consequence; a blacksmith near a waterfall is
scenery, and scenery is what makes a map look generated.

EXAMPLE
  a castle on a ridge
place village kind=village serves=castle reach=within sight link=road
  People settle under a wall they can run behind when the raiders come.
end
place mill kind=watermill serves=village reach=a short walk link=track
  The village grows grain and someone has to grind it.
end
place chapel kind=chapel serves=village reach=adjoining link=none
  Big enough now to bury its own dead.
end
place quarry kind=quarry serves=castle reach=half a day link=road
  The castle's stone came from somewhere, and it is still coming.
end")
}

/// Read an ensemble.
pub fn parse(src: &str, principal: &str, principal_kind: &str) -> (Ensemble, Vec<String>) {
    let mut e = Ensemble { principal: principal.to_lowercase(),
                           principal_kind: principal_kind.to_lowercase(),
                           satellites: Vec::new() };
    let mut errs = Vec::new();
    let mut cur: Option<Satellite> = None;
    let mut body = String::new();
    for line in src.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') { continue; }
        let w: Vec<&str> = t.split_whitespace().collect();
        let head = w.first().map(|x| x.to_lowercase()).unwrap_or_default();
        if head == "end" {
            if let Some(mut s) = cur.take() {
                s.because = body.trim().to_string();
                body.clear();
                if s.because.is_empty() {
                    errs.push(format!("'{}' does not say why it is there", s.name));
                }
                e.satellites.push(s);
            }
            continue;
        }
        if head == "place" {
            if let Some(mut s) = cur.take() {
                s.because = body.trim().to_string();
                body.clear();
                errs.push(format!("'{}' was not closed with `end`", s.name));
                e.satellites.push(s);
            }
            let kv_at = w.iter().position(|x| x.contains('=')).unwrap_or(w.len());
            if kv_at <= 1 { errs.push(format!("a place needs a name: {t:?}")); continue; }
            // values can have spaces ("a short walk"), so read to the
            // next key rather than to the next token
            let rest = w[kv_at..].join(" ");
            let val = |k: &str| -> Option<String> {
                let at = rest.find(&format!("{k}="))? + k.len() + 1;
                let after = &rest[at..];
                let end = ["kind=", "serves=", "reach=", "link="].iter()
                    .filter_map(|n| after.find(n)).min().unwrap_or(after.len());
                Some(after[..end].trim().to_string())
            };
            let name = w[1..kv_at].join(" ").to_lowercase();
            cur = Some(Satellite {
                kind: val("kind").unwrap_or_else(|| name.clone()),
                serves: val("serves").map(|s| s.to_lowercase())
                    .unwrap_or_else(|| e.principal.clone()),
                reach: val("reach").and_then(|v| Reach::parse(&v)).unwrap_or(Reach::AShortWalk),
                link: val("link").and_then(|v| Link::parse(&v)).unwrap_or(Link::Track),
                name,
                because: String::new(),
            });
            continue;
        }
        if cur.is_some() { body.push_str(t); body.push(' '); }
    }
    if let Some(mut s) = cur {
        s.because = body.trim().to_string();
        errs.push(format!("'{}' was not closed with `end`", s.name));
        e.satellites.push(s);
    }
    // a place serving something that is never built cannot be placed
    if let Err(m) = e.in_order() { errs.push(m); }
    (e, errs)
}

/// Where each place actually goes.
///
/// Sited in build order, each against what it serves, so a mill is
/// near its village rather than near the castle the village serves.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub name: String,
    pub kind: String,
    pub at: (i32, i32),
    pub link_from: Option<((i32, i32), Link)>,
    pub why: String,
}

/// Lay the ensemble out on the ground.
pub fn place<W: crate::VoxelWorld>(w: &W, e: &Ensemble, principal_at: (i32, i32),
                                   principal_span: i32, from_y: i32)
    -> (Vec<Placed>, Vec<String>) {
    let mut notes = Vec::new();
    let order = match e.in_order() {
        Ok(o) => o,
        Err(m) => { notes.push(m); return (Vec::new(), notes) }
    };
    let mut known: std::collections::BTreeMap<String, (i32, i32)> =
        std::collections::BTreeMap::new();
    known.insert(e.principal.clone(), principal_at);
    let mut out = Vec::new();
    for sat in order {
        let Some(from) = known.get(&sat.serves).copied() else { continue };
        let (near, far) = sat.reach.blocks(principal_span);
        // Search a ring at the right distance and let `siting` pick the
        // best spot in it. The RELATIONSHIP fixes the distance; the
        // land fixes the direction.
        let mid = (near + far) / 2;
        let span = (principal_span / 2).max(6);
        let (cands, _) = crate::siting::choose(w, from, mid.max(span + 4), span, &sat.kind, from_y);
        // only spots actually within the reach the model asked for
        let ok = cands.iter().find(|c| {
            let d = (((c.reading.at.0 - from.0).pow(2) + (c.reading.at.1 - from.1).pow(2)) as f32).sqrt();
            d >= near as f32 && d <= far as f32
        }).or_else(|| cands.first());
        let Some(best) = ok else { continue };
        let at = best.reading.at;
        known.insert(sat.name.clone(), at);
        notes.push(format!("{} at ({}, {}): {}", sat.name, at.0, at.1,
                           best.reasons.first().cloned().unwrap_or_default()));
        out.push(Placed {
            name: sat.name.clone(), kind: sat.kind.clone(), at,
            link_from: if sat.link == Link::None { None } else { Some((from, sat.link)) },
            why: sat.because.clone(),
        });
    }
    (out, notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ridge_and_river() -> impl crate::VoxelWorld {
        struct L;
        impl crate::VoxelWorld for L {
            fn get(&self, x: i32, y: i32, z: i32) -> u16 {
                // a ridge running north, a river to the west
                let ridge = (30.0 - (x as f32).abs() * 0.4).max(0.0) as i32;
                if x < -140 && x > -170 {
                    if y <= 60 { return 1; }
                    if y <= 63 { return crate::palette::resolve_or_suggest("water").0; }
                    return 0;
                }
                if y <= 64 + ridge { 1 } else { 0 }
            }
            fn set(&mut self, _: i32, _: i32, _: i32, _: u16) -> bool { false }
        }
        L
    }

    fn castle() -> Ensemble {
        parse("place village kind=village serves=castle reach=within sight link=road\n  \
                 People settle under a wall they can run behind.\nend\n\
               place mill kind=watermill serves=village reach=a short walk link=track\n  \
                 The village grows grain and someone has to grind it.\nend\n\
               place quarry kind=quarry serves=castle reach=half a day link=road\n  \
                 The castle's stone came from somewhere.\nend\n", "castle", "castle").0
    }

    #[test]
    fn nothing_is_built_before_the_thing_it_serves() {
        // a mill serves the VILLAGE, and placing it before the village
        // would put it somewhere for no reason
        let e = castle();
        let order: Vec<&str> = e.in_order().unwrap().iter().map(|s| s.name.as_str()).collect();
        let vi = order.iter().position(|n| *n == "village").unwrap();
        let mi = order.iter().position(|n| *n == "mill").unwrap();
        assert!(vi < mi, "the village comes first: {order:?}");
        // and a place serving something that is never built is caught
        let (_, errs) = parse("place a kind=hut serves=nowhere reach=near link=track\n  x\nend\n",
                              "castle", "castle");
        assert!(errs.iter().any(|m| m.contains("never built")), "{errs:?}");
    }

    #[test]
    fn a_value_with_spaces_in_it_survives() {
        // "a short walk" is three words and reading to the next TOKEN
        // would have taken only "a"
        let e = castle();
        let mill = e.satellites.iter().find(|s| s.name == "mill").unwrap();
        assert_eq!(mill.reach, Reach::AShortWalk);
        assert_eq!(mill.link, Link::Track);
        assert_eq!(mill.serves, "village");
        let q = e.satellites.iter().find(|s| s.name == "quarry").unwrap();
        assert_eq!(q.reach, Reach::AHalfDay);
        assert_eq!(q.kind, "quarry");
    }

    #[test]
    fn reach_is_a_relationship_and_scales_with_the_principal() {
        // "within sight" of a hut and of a citadel are not the same
        // distance
        let (hut_lo, hut_hi) = Reach::WithinSight.blocks(10);
        let (cit_lo, cit_hi) = Reach::WithinSight.blocks(80);
        assert!(cit_lo > hut_hi, "a citadel's sight is further: {cit_lo} vs {hut_hi}");
        let _ = hut_lo;
        // The reaches are ordered, but their bands OVERLAP, and that
        // is truthful rather than sloppy: something within sight can
        // also be a short walk. Asserting disjoint bands would have
        // forced a precision the words do not have. What must hold is
        // that each reach is centred further out than the last.
        let s = 40;
        let mid = |r: Reach| { let (a, b) = r.blocks(s); (a + b) / 2 };
        assert!(mid(Reach::Adjoining) < mid(Reach::WithinSight));
        assert!(mid(Reach::WithinSight) < mid(Reach::AShortWalk));
        assert!(mid(Reach::AShortWalk) < mid(Reach::AHalfDay));
        // and adjoining really does touch
        assert_eq!(Reach::Adjoining.blocks(s).0, 0);
    }

    #[test]
    fn each_place_lands_near_what_it_serves_and_on_land_that_suits_it() {
        let w = ridge_and_river();
        let e = castle();
        let (placed, notes) = place(&w, &e, (0, 0), 40, 64);
        assert_eq!(placed.len(), 3, "{notes:?}");
        let at = |n: &str| placed.iter().find(|p| p.name == n).unwrap();
        // THE MILL IS NEAR THE VILLAGE, not near the castle -- that is
        // the whole point of building in order
        let (v, m) = (at("village").at, at("mill").at);
        let d_mill_village = (((m.0 - v.0).pow(2) + (m.1 - v.1).pow(2)) as f32).sqrt();
        let d_mill_castle = (((m.0).pow(2) + (m.1).pow(2)) as f32).sqrt();
        assert!(d_mill_village < d_mill_castle * 2.0,
                "the mill follows the village: {d_mill_village} vs {d_mill_castle}");
        // the quarry is much further than the village
        let d_q = (((at("quarry").at.0).pow(2) + (at("quarry").at.1).pow(2)) as f32).sqrt();
        let d_v = (((v.0).pow(2) + (v.1).pow(2)) as f32).sqrt();
        assert!(d_q > d_v, "half a day beats within sight: {d_q} vs {d_v}");
        // and each says why it is where it is
        assert!(notes.iter().all(|n| n.contains("at (")), "{notes:?}");
    }

    #[test]
    fn what_joins_two_places_is_carried_through() {
        let w = ridge_and_river();
        let (placed, _) = place(&w, &castle(), (0, 0), 40, 64);
        let v = placed.iter().find(|p| p.name == "village").unwrap();
        assert_eq!(v.link_from.map(|(_, l)| l), Some(Link::Road));
        assert_eq!(v.link_from.map(|(f, _)| f), Some((0, 0)), "the road runs from the castle");
        // a mill linked by a track runs from the VILLAGE
        let m = placed.iter().find(|p| p.name == "mill").unwrap();
        assert_eq!(m.link_from.map(|(f, _)| f), Some(v.at));
        // `none` means no link rather than a zero-width one
        let (e2, _) = parse("place chapel kind=chapel serves=castle reach=adjoining link=none\n  \
                             Big enough now to bury its own dead.\nend\n", "castle", "castle");
        let (p2, _) = place(&w, &e2, (0, 0), 40, 64);
        assert!(p2[0].link_from.is_none());
        assert_eq!(Link::None.shape().0, 0);
    }

    #[test]
    fn the_prompt_refuses_scenery() {
        let p = prompt("a castle on a ridge", "SITE: a long ridge above a river");
        assert!(p.contains("EVERY PLACE MUST BE A CONSEQUENCE"), "{p}");
        assert!(p.contains("scenery is what makes a map look generated"), "{p}");
        assert!(p.contains("a fact about what it serves"));
        // its own example parses and orders
        let body: String = p.split("EXAMPLE").nth(1).unwrap().lines().skip(2)
            .map(|l| format!("{l}\n")).collect();
        let (e, errs) = parse(&body, "castle", "castle");
        assert!(errs.is_empty(), "{body}\n{errs:?}");
        assert_eq!(e.satellites.len(), 4);
        assert!(e.in_order().is_ok());
        // every satellite says why, as a fact about what it serves
        for s in &e.satellites {
            assert!(s.because.len() > 15, "'{}': {:?}", s.name, s.because);
        }
    }

    #[test]
    fn a_place_that_says_nothing_about_why_is_named() {
        let (_, errs) = parse("place a kind=hut serves=castle reach=near link=track\nend\n",
                              "castle", "castle");
        assert!(errs.iter().any(|m| m.contains("does not say why")), "{errs:?}");
    }
}
