//! Flair: what goes on it, where that is, and what to do to it after.
//!
//! The pipeline can now decide what a thing IS, what it holds, and
//! where the rooms go. What it cannot say is the last thing a builder
//! does and the thing that makes a build read as made rather than
//! generated: put crenellations along the top of the wall, except over
//! the gate; buttresses on the outside faces, thinned toward the
//! corners; rivets on every panel, culled to two thirds.
//!
//! Three separate things, and they were being conflated:
//!
//!   GREEBLE    the detail itself -- a merlon, a buttress, a bracket.
//!              A shape or an asset. `gene.rs` evolves these; this
//!              layer is for the ones you can NAME.
//!   LOCATION   where it goes, as a REGION derived from geometry that
//!              already exists: the top edge of this, the outward
//!              faces of that, MINUS anywhere near the other thing.
//!              The negative is what makes it usable -- "along the
//!              parapet except at the gate" is one sentence and two
//!              regions.
//!   MODIFIER   what to do to the placed set afterwards: cull it to a
//!              density, clamp it to a height, taper it along an axis,
//!              jitter it, align it to a grid. Maths over a point set,
//!              applied in order.
//!
//! WHY REGIONS AND NOT COORDINATES
//!
//! The same reason as everywhere else in this crate: a location is a
//! RELATION to something already built. `top_edge of curtain_wall` is
//! true whatever size the wall came out, and it stays true when a
//! review changes the wall. A list of coordinates is wrong the moment
//! anything moves.

use crate::weather::Change;
use std::collections::{BTreeMap, BTreeSet};

/// Which part of a shape a location refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    /// The highest cells: a parapet, a ridge, a rim.
    TopEdge,
    /// The lowest: a plinth, a footing.
    BaseEdge,
    /// Cells on the outside vertical faces.
    Faces,
    /// Vertical corners, where two faces meet.
    Corners,
    /// The whole outer shell.
    Shell,
    /// Everything, inside included.
    All,
    /// The flat top surface, not just its edge.
    Roof,
}

impl Anchor {
    pub fn parse(s: &str) -> Option<Anchor> {
        Some(match s.to_lowercase().as_str() {
            "top_edge" | "top" | "parapet" | "rim" | "crest" => Anchor::TopEdge,
            "base_edge" | "base" | "plinth" | "footing" => Anchor::BaseEdge,
            "faces" | "face" | "walls" | "sides" => Anchor::Faces,
            "corners" | "corner" | "angles" => Anchor::Corners,
            "shell" | "outside" | "skin" => Anchor::Shell,
            "all" | "everything" => Anchor::All,
            "roof" | "top_surface" => Anchor::Roof,
            _ => return None,
        })
    }
}

/// A location: an anchor on one body, minus anchors on others.
///
/// The `except` list is the point. "Along the parapet except over the
/// gate" is a region and a negative, and without the negative every
/// greeble pass either covers the gate or has to be hand-placed.
#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    pub on: String,
    pub anchor: Anchor,
    /// (body, anchor, how far the exclusion reaches).
    pub except: Vec<(String, Anchor, i32)>,
}

/// What to do to a placed set. Applied in the order written.
#[derive(Debug, Clone, PartialEq)]
pub enum Modifier {
    /// Keep this fraction, chosen by a seeded hash. Deterministic.
    Cull { keep: f32 },
    /// Only every Nth along the longest axis: the rhythm a real
    /// parapet has and a full one does not.
    Every { n: i32, offset: i32 },
    /// Clamp y into a band.
    Clamp { min: i32, max: i32 },
    /// Keep only points where an expression over `x`, `y`, `z`, `t`
    /// (position along the run, 0..1) and `d` (distance from the run's
    /// centre) is positive. The general escape hatch.
    Where { expr: String },
    /// Move each point by an expression per axis.
    Offset { dx: String, dy: String, dz: String },
    /// Seeded displacement.
    Jitter { by: i32, seed: u64 },
    /// Snap to a lattice, so a scattered set reads as deliberate.
    Snap { grid: i32 },
    /// Drop points closer together than this: thins a crowd without
    /// culling at random.
    Thin { spacing: i32 },
}

/// One flair instruction.
#[derive(Debug, Clone, PartialEq)]
pub struct Flair {
    pub greeble: String,
    pub at: Location,
    pub modifiers: Vec<Modifier>,
    pub material: Option<String>,
}

/// A named body the locations refer to: whatever a previous pass
/// built, keyed by its pass or part name.
pub type Bodies = BTreeMap<String, BTreeSet<(i32, i32, i32)>>;

fn hash(a: i64, b: i64, s: u64) -> u64 {
    let mut h = (a as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (b as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ s;
    h ^= h >> 31; h = h.wrapping_mul(0x7FB5_D329_728E_A185); h ^ (h >> 27)
}

/// The cells an anchor selects from a body.
pub fn select(body: &BTreeSet<(i32, i32, i32)>, a: Anchor) -> Vec<(i32, i32, i32)> {
    if body.is_empty() { return Vec::new(); }
    let solid = |c: (i32, i32, i32)| body.contains(&c);
    match a {
        Anchor::All => body.iter().copied().collect(),
        Anchor::Shell => body.iter().copied()
            .filter(|c| [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)]
                .iter().any(|d| !solid((c.0 + d.0, c.1 + d.1, c.2 + d.2)))).collect(),
        Anchor::Faces => body.iter().copied()
            // a face cell has air to one SIDE; a roof cell has air above
            .filter(|c| [(1, 0, 0), (-1, 0, 0), (0, 0, 1), (0, 0, -1)]
                .iter().any(|d| !solid((c.0 + d.0, c.1, c.2 + d.2))))
            .filter(|c| solid((c.0, c.1 + 1, c.2)))
            .collect(),
        Anchor::Roof => {
            // the top cell of each column, where there is open air above
            let mut top: BTreeMap<(i32, i32), i32> = BTreeMap::new();
            for c in body { let e = top.entry((c.0, c.2)).or_insert(c.1); *e = (*e).max(c.1); }
            top.into_iter().map(|((x, z), y)| (x, y, z)).collect()
        }
        Anchor::TopEdge => {
            // the top of each column, but only columns on the shell --
            // a parapet runs along the wall, not across the roof
            let mut top: BTreeMap<(i32, i32), i32> = BTreeMap::new();
            for c in body { let e = top.entry((c.0, c.2)).or_insert(c.1); *e = (*e).max(c.1); }
            top.iter().filter(|((x, z), _)| {
                [(1, 0), (-1, 0), (0, 1), (0, -1)].iter()
                    .any(|(dx, dz)| !top.contains_key(&(x + dx, z + dz)))
            }).map(|((x, z), y)| (*x, *y, *z)).collect()
        }
        Anchor::BaseEdge => {
            let mut bot: BTreeMap<(i32, i32), i32> = BTreeMap::new();
            for c in body { let e = bot.entry((c.0, c.2)).or_insert(c.1); *e = (*e).min(c.1); }
            bot.iter().filter(|((x, z), _)| {
                [(1, 0), (-1, 0), (0, 1), (0, -1)].iter()
                    .any(|(dx, dz)| !bot.contains_key(&(x + dx, z + dz)))
            }).map(|((x, z), y)| (*x, *y, *z)).collect()
        }
        Anchor::Corners => {
            let cols: BTreeSet<(i32, i32)> = body.iter().map(|c| (c.0, c.2)).collect();
            // a corner column has TWO adjacent sides missing, not one
            // A corner turns: it is missing a neighbour in x AND one in
            // z. Counting "two missing" made every column of a thin
            // wall a corner -- both its faces are open, but they are
            // the same axis, so it is a straight run.
            let corner: Vec<(i32, i32)> = cols.iter().copied().filter(|(x, z)| {
                let miss_x = !cols.contains(&(x + 1, *z)) || !cols.contains(&(x - 1, *z));
                let miss_z = !cols.contains(&(*x, z + 1)) || !cols.contains(&(*x, z - 1));
                miss_x && miss_z
            }).collect();
            body.iter().copied().filter(|c| corner.contains(&(c.0, c.2))).collect()
        }
    }
}

/// Resolve a location to its cells: the anchor, minus the exclusions.
pub fn resolve(loc: &Location, bodies: &Bodies) -> (Vec<(i32, i32, i32)>, Vec<String>) {
    let mut notes = Vec::new();
    let Some(body) = bodies.get(&loc.on) else {
        notes.push(format!("no body called '{}'; known: {}", loc.on,
                           bodies.keys().cloned().collect::<Vec<_>>().join(", ")));
        return (Vec::new(), notes);
    };
    let mut cells = select(body, loc.anchor);
    let before = cells.len();
    for (name, anchor, reach) in &loc.except {
        let Some(other) = bodies.get(name) else {
            notes.push(format!("cannot exclude '{name}': no such body"));
            continue;
        };
        let mask: Vec<(i32, i32, i32)> = select(other, *anchor);
        if mask.is_empty() { continue; }
        let r = (*reach).max(0);
        // by column, because an exclusion is usually a plan-view idea:
        // "not over the gate" means not above it either
        let cols: BTreeSet<(i32, i32)> = mask.iter().map(|c| (c.0, c.2)).collect();
        cells.retain(|c| !cols.iter().any(|(x, z)| (c.0 - x).abs() <= r && (c.2 - z).abs() <= r));
    }
    if cells.len() < before {
        notes.push(format!("{} of {before} excluded", before - cells.len()));
    }
    (cells, notes)
}

/// Apply the modifiers, in order.
pub fn modify(mut cells: Vec<(i32, i32, i32)>, mods: &[Modifier]) -> (Vec<(i32, i32, i32)>, Vec<String>) {
    let mut notes = Vec::new();
    for m in mods {
        let before = cells.len();
        match m {
            Modifier::Cull { keep } => {
                let k = keep.clamp(0.0, 1.0);
                cells.retain(|c| (hash(c.0 as i64, c.2 as i64 * 31 + c.1 as i64, 0x5EED) % 1000) as f32 / 1000.0 < k);
            }
            Modifier::Every { n, offset } => {
                let n = (*n).max(1);
                // along the LONGEST axis of the set, so a run of
                // merlons steps along the wall rather than across it
                let (lo, hi) = bounds(&cells);
                let along_x = hi.0 - lo.0 >= hi.2 - lo.2;
                cells.retain(|c| (if along_x { c.0 } else { c.2 } - offset).rem_euclid(n) == 0);
            }
            Modifier::Clamp { min, max } => {
                let (lo, hi) = (*min.min(max), *min.max(max));
                cells.retain(|c| c.1 >= lo && c.1 <= hi);
            }
            Modifier::Where { expr } => {
                let (lo, hi) = bounds(&cells);
                let span = ((hi.0 - lo.0).max(hi.2 - lo.2)).max(1) as f32;
                let mid = ((lo.0 + hi.0) as f32 / 2.0, (lo.2 + hi.2) as f32 / 2.0);
                let mut bad = None;
                cells.retain(|c| {
                    if bad.is_some() { return true; }
                    let mut v: BTreeMap<String, f32> = BTreeMap::new();
                    v.insert("x".into(), c.0 as f32); v.insert("y".into(), c.1 as f32);
                    v.insert("z".into(), c.2 as f32);
                    v.insert("t".into(), (c.0 - lo.0) as f32 / span);
                    v.insert("d".into(), ((c.0 as f32 - mid.0).powi(2)
                                          + (c.2 as f32 - mid.1).powi(2)).sqrt());
                    match crate::expr::eval(expr, &v) {
                        Ok(r) => r > 0.0,
                        Err(e) => { bad = Some(e); true }
                    }
                });
                if let Some(e) = bad { notes.push(format!("where {expr:?}: {e}")); }
            }
            Modifier::Offset { dx, dy, dz } => {
                let mut bad = None;
                cells = cells.into_iter().map(|c| {
                    let mut v: BTreeMap<String, f32> = BTreeMap::new();
                    v.insert("x".into(), c.0 as f32); v.insert("y".into(), c.1 as f32);
                    v.insert("z".into(), c.2 as f32);
                    let mut ev = |e: &str| -> i32 {
                        match crate::expr::eval(e, &v) { Ok(r) => r.round() as i32,
                            Err(er) => { bad.get_or_insert(er); 0 } }
                    };
                    (c.0 + ev(dx), c.1 + ev(dy), c.2 + ev(dz))
                }).collect();
                if let Some(e) = bad { notes.push(format!("offset: {e}")); }
            }
            Modifier::Jitter { by, seed } => {
                let b = (*by).max(0);
                if b > 0 {
                    cells = cells.into_iter().map(|c| {
                        let h = hash(c.0 as i64, c.2 as i64, *seed);
                        (c.0 + (h % (b * 2 + 1) as u64) as i32 - b, c.1,
                         c.2 + ((h >> 20) % (b * 2 + 1) as u64) as i32 - b)
                    }).collect();
                }
            }
            Modifier::Snap { grid } => {
                let g = (*grid).max(1);
                let mut seen = BTreeSet::new();
                cells = cells.into_iter()
                    .map(|c| ((c.0 / g) * g, c.1, (c.2 / g) * g))
                    .filter(|c| seen.insert(*c)).collect();
            }
            Modifier::Thin { spacing } => {
                let s = (*spacing).max(1);
                let mut kept: Vec<(i32, i32, i32)> = Vec::new();
                for c in &cells {
                    if kept.iter().any(|k| (k.0 - c.0).abs() < s && (k.2 - c.2).abs() < s
                                        && (k.1 - c.1).abs() < s) { continue; }
                    kept.push(*c);
                }
                cells = kept;
            }
        }
        if cells.len() != before {
            notes.push(format!("{}: {before} -> {}", name_of(m), cells.len()));
        }
    }
    (cells, notes)
}

fn name_of(m: &Modifier) -> &'static str {
    match m {
        Modifier::Cull { .. } => "cull", Modifier::Every { .. } => "every",
        Modifier::Clamp { .. } => "clamp", Modifier::Where { .. } => "where",
        Modifier::Offset { .. } => "offset", Modifier::Jitter { .. } => "jitter",
        Modifier::Snap { .. } => "snap", Modifier::Thin { .. } => "thin",
    }
}

fn bounds(c: &[(i32, i32, i32)]) -> ((i32, i32, i32), (i32, i32, i32)) {
    c.iter().fold(((i32::MAX, i32::MAX, i32::MAX), (i32::MIN, i32::MIN, i32::MIN)),
        |(l, h), p| ((l.0.min(p.0), l.1.min(p.1), l.2.min(p.2)),
                     (h.0.max(p.0), h.1.max(p.1), h.2.max(p.2))))
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Applied {
    pub changes: Vec<Change>,
    pub log: Vec<String>,
    pub notes: Vec<String>,
}

/// Place a flair's greeble at every surviving cell of its location.
///
/// `greebles` maps a name to the cells of one instance, relative to
/// its own origin -- exactly what `program::Asset` holds, so the two
/// layers share their vocabulary.
pub fn apply(f: &Flair, bodies: &Bodies,
             greebles: &BTreeMap<String, Vec<((i32, i32, i32), u16)>>) -> Applied {
    let mut out = Applied::default();
    let (cells, notes) = resolve(&f.at, bodies);
    out.notes.extend(notes);
    if cells.is_empty() {
        out.notes.push(format!("'{}' had nowhere to go", f.greeble));
        return out;
    }
    let n0 = cells.len();
    let (cells, notes) = modify(cells, &f.modifiers);
    out.notes.extend(notes);
    let Some(shape) = greebles.get(&f.greeble) else {
        out.notes.push(format!("no greeble called '{}'", f.greeble));
        return out;
    };
    let over = f.material.as_deref().and_then(crate::plan::resolve_block);
    let mut written: BTreeMap<(i32, i32, i32), u16> = BTreeMap::new();
    for at in &cells {
        for ((dx, dy, dz), b) in shape {
            written.insert((at.0 + dx, at.1 + dy + 1, at.2 + dz), over.unwrap_or(*b));
        }
    }
    out.log.push(format!("{}: {n0} site(s) -> {} placement(s), {} cells",
                         f.greeble, cells.len(), written.len()));
    out.changes = written.into_iter().map(|(at, to)| Change { at, to }).collect();
    out
}

// ── the language ─────────────────────────────────────────────────────
//
//   flair merlon on curtain_wall top_edge except gatehouse shell 4
//     every 3
//     jitter 1 seed=7
//   end

pub fn parse(src: &str) -> (Vec<Flair>, Vec<String>) {
    let mut out = Vec::new();
    let mut errs = Vec::new();
    let mut cur: Option<Flair> = None;
    for line in src.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') { continue; }
        let w: Vec<&str> = t.split_whitespace().collect();
        let head = w[0].to_lowercase();
        if head == "end" {
            if let Some(f) = cur.take() { out.push(f); } else { errs.push("stray `end`".into()); }
            continue;
        }
        if head == "flair" {
            if let Some(f) = cur.take() { out.push(f); }
            // flair NAME on BODY ANCHOR [except BODY ANCHOR N] ...
            let Some(name) = w.get(1) else { errs.push("flair needs a name".into()); continue };
            let on_at = w.iter().position(|s| s.eq_ignore_ascii_case("on"));
            let Some(i) = on_at else { errs.push(format!("'{name}' needs `on BODY ANCHOR`")); continue };
            let (Some(body), Some(anchor)) = (w.get(i + 1), w.get(i + 2).and_then(|a| Anchor::parse(a))) else {
                errs.push(format!("'{name}': `on BODY ANCHOR` -- anchors are top_edge, base_edge, \
                                   faces, corners, shell, roof, all"));
                continue;
            };
            let mut except = Vec::new();
            let mut j = i + 3;
            while j < w.len() {
                if w[j].eq_ignore_ascii_case("except") {
                    match (w.get(j + 1), w.get(j + 2).and_then(|a| Anchor::parse(a))) {
                        (Some(b), Some(a)) => {
                            let reach = w.get(j + 3).and_then(|v| v.parse().ok()).unwrap_or(2);
                            except.push((b.to_lowercase(), a, reach));
                            j += 4;
                        }
                        _ => { errs.push(format!("'{name}': `except BODY ANCHOR [reach]`")); j += 1; }
                    }
                } else { j += 1; }
            }
            cur = Some(Flair {
                greeble: name.to_lowercase(),
                at: Location { on: body.to_lowercase(), anchor, except },
                modifiers: Vec::new(),
                material: w.iter().find_map(|s| s.strip_prefix("material=")).map(|s| s.to_string()),
            });
            continue;
        }
        // a modifier line, inside a flair
        let Some(f) = cur.as_mut() else { errs.push(format!("'{t}' is outside any flair")); continue };
        let num = |k: usize, d: i32| w.get(k).and_then(|v| v.parse().ok()).unwrap_or(d);
        let kv = |k: &str| w.iter().find_map(|s| s.strip_prefix(&format!("{k}=")));
        let m = match head.as_str() {
            "cull" => Modifier::Cull { keep: w.get(1).and_then(|v| v.parse().ok()).unwrap_or(0.5) },
            "every" => Modifier::Every { n: num(1, 2), offset: kv("offset").and_then(|v| v.parse().ok()).unwrap_or(0) },
            "clamp" => Modifier::Clamp { min: num(1, 0), max: num(2, 255) },
            "where" => Modifier::Where { expr: w[1..].join(" ") },
            "offset" => Modifier::Offset {
                dx: kv("dx").unwrap_or("0").to_string(), dy: kv("dy").unwrap_or("0").to_string(),
                dz: kv("dz").unwrap_or("0").to_string() },
            "jitter" => Modifier::Jitter { by: num(1, 1),
                                           seed: kv("seed").and_then(|v| v.parse().ok()).unwrap_or(1) },
            "snap" => Modifier::Snap { grid: num(1, 2) },
            "thin" => Modifier::Thin { spacing: num(1, 3) },
            other => { errs.push(format!("'{other}' is not a modifier; known: cull, every, clamp, \
                                          where, offset, jitter, snap, thin")); continue; }
        };
        f.modifiers.push(m);
    }
    if let Some(f) = cur { out.push(f); }
    (out, errs)
}

/// The prompt for the flair turn.
pub fn flair_prompt(what: &str, bodies: &[String]) -> String {
    format!("\
You have built: {what}

These are the bodies you can attach detail to:
  {}

Add FLAIR -- the detail that makes it read as made rather than generated. Each one is a
greeble, a place derived from what already exists, and what to do to the set afterwards.

  flair NAME on BODY ANCHOR [except BODY ANCHOR REACH] [material=BLOCK]
    <modifiers>
  end

ANCHORS: top_edge (a parapet), base_edge (a plinth), faces (the vertical sides),
corners, shell (the whole outside), roof (the top surface), all

`except` is the useful part: \"along the parapet EXCEPT over the gate\" is one region and
one negative, and without it the detail covers the thing it should respect.

MODIFIERS, applied in order:
  every N [offset=N]   only every Nth along the run -- rhythm, not coverage
  cull 0.6             keep this fraction, seeded
  thin 4               drop anything closer than 4 to something kept
  clamp MIN MAX        only within this height band
  where EXPR           keep where the expression is positive; x y z are position,
                       t is 0..1 along the run, d is distance from its centre
  offset dx= dy= dz=   move each by an expression
  jitter N [seed=N]    seeded displacement
  snap N               to a lattice

EXAMPLE
flair merlon on curtain_wall top_edge except gatehouse shell 5
  every 3
end
flair buttress on keep faces
  where cos(t * 360) 
  thin 6
end

Write the flair blocks only.", bodies.join("\n  "))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hollow box wall, 20x20, 8 tall, with a gatehouse lump on the
    /// south side.
    fn scene() -> Bodies {
        let mut wall = BTreeSet::new();
        for x in -10i32..=10 { for z in -10i32..=10 { for y in 0..8 {
            if x.abs() == 10 || z.abs() == 10 { wall.insert((x, y, z)); }
        }}}
        let mut gate = BTreeSet::new();
        for x in -3i32..=3 { for z in -12i32..=-8 { for y in 0..10 { gate.insert((x, y, z)); } } }
        let mut keep = BTreeSet::new();
        for x in -4i32..=4 { for z in -4i32..=4 { for y in 0..14 { keep.insert((x, y, z)); } } }
        let mut b = Bodies::new();
        b.insert("curtain_wall".into(), wall);
        b.insert("gatehouse".into(), gate);
        b.insert("keep".into(), keep);
        b
    }

    fn merlons() -> BTreeMap<String, Vec<((i32, i32, i32), u16)>> {
        let mut g = BTreeMap::new();
        g.insert("merlon".to_string(), vec![((0, 0, 0), 1u16), ((0, 1, 0), 1)]);
        g
    }

    #[test]
    fn an_anchor_selects_the_part_of_a_body_it_names() {
        let b = scene();
        let wall = &b["curtain_wall"];
        let top = select(wall, Anchor::TopEdge);
        assert!(!top.is_empty());
        assert!(top.iter().all(|c| c.1 == 7), "the parapet is the top course: {:?}",
                top.iter().map(|c| c.1).collect::<BTreeSet<_>>());
        let base = select(wall, Anchor::BaseEdge);
        assert!(base.iter().all(|c| c.1 == 0));
        // corners are the four, not the whole run
        let corners = select(wall, Anchor::Corners);
        let cols: BTreeSet<(i32, i32)> = corners.iter().map(|c| (c.0, c.2)).collect();
        assert_eq!(cols.len(), 4, "four corners: {cols:?}");
        // and every anchor selects something from a solid body
        for a in [Anchor::TopEdge, Anchor::BaseEdge, Anchor::Faces, Anchor::Corners,
                  Anchor::Shell, Anchor::Roof, Anchor::All] {
            assert!(!select(&b["keep"], a).is_empty(), "{a:?} selected nothing");
        }
    }

    #[test]
    fn the_negative_is_what_makes_a_location_usable() {
        let b = scene();
        let all = Location { on: "curtain_wall".into(), anchor: Anchor::TopEdge, except: vec![] };
        let not_gate = Location { on: "curtain_wall".into(), anchor: Anchor::TopEdge,
                                  except: vec![("gatehouse".into(), Anchor::All, 3)] };
        let (a, _) = resolve(&all, &b);
        let (n, notes) = resolve(&not_gate, &b);
        assert!(n.len() < a.len(), "the exclusion removed something: {} vs {}", n.len(), a.len());
        assert!(notes.iter().any(|s| s.contains("excluded")), "{notes:?}");
        // nothing survives NEAR the gate
        assert!(!n.iter().any(|c| c.2 <= -8 && c.0.abs() <= 3),
                "the parapet stops short of the gatehouse");
        // and it does survive elsewhere on the same wall
        assert!(n.iter().any(|c| c.2 >= 8), "the far side is untouched");
    }

    #[test]
    fn a_missing_body_says_which_names_exist() {
        let b = scene();
        let (cells, notes) = resolve(&Location { on: "barbican".into(), anchor: Anchor::TopEdge,
                                                 except: vec![] }, &b);
        assert!(cells.is_empty());
        assert!(notes[0].contains("no body called 'barbican'") && notes[0].contains("curtain_wall"),
                "{notes:?}");
    }

    #[test]
    fn every_gives_rhythm_along_the_run_not_across_it() {
        let cells: Vec<(i32, i32, i32)> = (-20..=20).map(|x| (x, 8, 0)).collect();
        let (out, notes) = modify(cells, &[Modifier::Every { n: 3, offset: 0 }]);
        assert_eq!(out.len(), 13, "the multiples of 3 in -20..=20");
        assert!(out.windows(2).all(|w| w[1].0 - w[0].0 == 3), "evenly spaced: {out:?}");
        assert!(notes.iter().any(|s| s.starts_with("every:")), "{notes:?}");
    }

    #[test]
    fn where_is_the_escape_hatch_and_a_bad_one_says_so() {
        let cells: Vec<(i32, i32, i32)> = (0..40).map(|x| (x, 4, 0)).collect();
        // keep the first half
        let (out, _) = modify(cells.clone(), &[Modifier::Where { expr: "0.5 - t".into() }]);
        assert!(out.len() > 10 && out.len() < 30, "{} kept", out.len());
        assert!(out.iter().all(|c| c.0 < 25), "the near half: max {}", out.iter().map(|c| c.0).max().unwrap());
        // a bad expression reports and keeps everything rather than
        // silently deleting the lot
        let (out, notes) = modify(cells, &[Modifier::Where { expr: "nosuch * 2".into() }]);
        assert_eq!(out.len(), 40, "nothing was dropped on an error");
        assert!(notes.iter().any(|n| n.contains("not a known value")), "{notes:?}");
    }

    #[test]
    fn thin_and_cull_reduce_differently_and_both_are_deterministic() {
        let cells: Vec<(i32, i32, i32)> = (0..20).flat_map(|x| (0..20).map(move |z| (x, 4, z))).collect();
        let (thin, _) = modify(cells.clone(), &[Modifier::Thin { spacing: 4 }]);
        let (cull, _) = modify(cells.clone(), &[Modifier::Cull { keep: 0.25 }]);
        assert!(thin.len() < 40, "thin spaces them out: {}", thin.len());
        // thin guarantees spacing; cull does not
        for (i, a) in thin.iter().enumerate() {
            for b in &thin[i + 1..] {
                assert!((a.0 - b.0).abs() >= 4 || (a.2 - b.2).abs() >= 4, "{a:?} {b:?} too close");
            }
        }
        assert!((cull.len() as i32 - 100).abs() < 40, "cull keeps about a quarter: {}", cull.len());
        // both repeat exactly
        assert_eq!(modify(cells.clone(), &[Modifier::Cull { keep: 0.25 }]).0, cull);
        assert_eq!(modify(cells, &[Modifier::Thin { spacing: 4 }]).0, thin);
    }

    #[test]
    fn the_language_parses_and_places_merlons_along_a_wall_but_not_over_the_gate() {
        let (flairs, errs) = parse(
            "flair merlon on curtain_wall top_edge except gatehouse all 4\n\
               every 2\n\
             end\n");
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(flairs.len(), 1);
        assert_eq!(flairs[0].at.anchor, Anchor::TopEdge);
        assert_eq!(flairs[0].at.except.len(), 1);
        assert_eq!(flairs[0].modifiers.len(), 1);
        let a = apply(&flairs[0], &scene(), &merlons());
        assert!(!a.changes.is_empty(), "{:?}", a.notes);
        assert!(a.log[0].contains("merlon") && a.log[0].contains("placement(s)"), "{:?}", a.log);
        // they sit ON the parapet, one above it
        assert!(a.changes.iter().all(|c| c.at.1 >= 8), "above the wall top: {:?}",
                a.changes.iter().map(|c| c.at.1).min());
        // and none over the gate
        assert!(!a.changes.iter().any(|c| c.at.2 <= -8 && c.at.0.abs() <= 3));
    }

    #[test]
    fn the_parser_says_what_is_wrong_rather_than_guessing() {
        let (_, e) = parse("flair merlon\n");
        assert!(e.iter().any(|x| x.contains("needs `on BODY ANCHOR`")), "{e:?}");
        let (_, e) = parse("flair merlon on wall sideways\nend\n");
        assert!(e.iter().any(|x| x.contains("anchors are top_edge")), "{e:?}");
        let (_, e) = parse("flair m on wall top_edge\n  frobnicate 2\nend\n");
        assert!(e.iter().any(|x| x.contains("not a modifier") && x.contains("cull, every")), "{e:?}");
        let (_, e) = parse("every 2\n");
        assert!(e.iter().any(|x| x.contains("outside any flair")), "{e:?}");
    }

    #[test]
    fn the_prompt_names_the_bodies_and_its_example_parses() {
        let p = flair_prompt("a castle", &["curtain_wall".into(), "keep".into(), "gatehouse".into()]);
        assert!(p.contains("curtain_wall") && p.contains("gatehouse"));
        assert!(p.contains("rhythm, not coverage"), "the point of `every` is stated");
        assert!(p.contains("except") && p.contains("respect"));
        let ex = p.split("EXAMPLE").nth(1).unwrap();
        let body: String = ex.lines().skip(1)
            .take_while(|l| !l.trim().is_empty() && !l.starts_with("Write"))
            .map(|l| format!("{l}\n")).collect();
        let (f, errs) = parse(&body);
        assert!(errs.is_empty(), "{body}\n{errs:?}");
        assert_eq!(f.len(), 2, "two flairs in the example");
        // and both do something against a real scene
        let b = scene();
        let g = merlons();
        let mut g2 = g.clone();
        g2.insert("buttress".into(), vec![((0, 0, 0), 1u16)]);
        for one in &f {
            let a = apply(one, &b, &g2);
            assert!(!a.changes.is_empty() || !a.notes.is_empty(), "{one:?} did nothing and said nothing");
        }
    }
}
