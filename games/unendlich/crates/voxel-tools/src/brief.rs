//! The brief: what spaces, how big, and what must be next to what.
//!
//! Adapted from architectural programming as it is actually taught
//! (Mahgoub, *Architectural Programming*). The claim there is that
//! programming is "the research and decision-making process that
//! defines the PROBLEM to be solved by design" -- it happens before
//! design, and it is the cheapest point at which to change anything.
//!
//! WHAT THIS CRATE WAS MISSING
//!
//! Every building it makes is a hollow box, because nothing describes
//! an INTERIOR PROGRAM. A castle keep is a shell; there is no way to
//! say it holds a great hall of 200 m², a kitchen that must adjoin the
//! hall, stores that must not, and a chapel that wants an east wall.
//! The lecture names three artefacts for exactly this and I had none:
//!
//!   SPACE SCHEDULE      the rooms, their areas, how many of each,
//!                       and a BUDGET the total has to reconcile with
//!   ADJACENCY MATRIX    which spaces must be near which, which must
//!                       not, expressed as a relation and not a plan
//!   SPACE DATA SHEET    per space: what it needs -- daylight, height,
//!                       an outside door, privacy
//!
//! From those three a layout follows, and from a layout, geometry.
//! That ordering is the point: the schedule is a decision about the
//! PROBLEM, and it is checkable long before anything is built. A
//! schedule that overruns its budget by 40% is wrong on paper, which
//! is the cheap place to be wrong.
//!
//! THE ROLES
//!
//! The lecture separates client, programmer and designer, and the
//! separation is not ceremony -- the client says what they want, the
//! programmer says what that costs in area and what must touch what,
//! and the designer resolves it in space. Those are three different
//! questions and a model answers each better alone than all at once.
//! `role_prompt` gives each its own turn.

use std::collections::BTreeSet;

/// How badly two spaces want to be together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Adjacency {
    /// They must share a wall: a kitchen and its servery.
    Must,
    /// Near, not necessarily touching.
    Should,
    /// Indifferent.
    Neutral,
    /// Apart: plant rooms and bedrooms.
    Avoid,
}

impl Adjacency {
    pub fn parse(s: &str) -> Option<Adjacency> {
        match s.to_lowercase().as_str() {
            "must" | "adjacent" | "touching" | "direct" => Some(Adjacency::Must),
            "should" | "near" | "close" => Some(Adjacency::Should),
            "neutral" | "none" => Some(Adjacency::Neutral),
            "avoid" | "apart" | "far" | "separate" => Some(Adjacency::Avoid),
            _ => None,
        }
    }
    /// What a layout pays for putting these `d` apart. Lower is better.
    fn cost(self, d: f32) -> f32 {
        match self {
            Adjacency::Must => d * d * 4.0,
            Adjacency::Should => d * d,
            Adjacency::Neutral => 0.0,
            // a penalty that falls off: being far is enough, further
            // is not better, or every plan flies apart
            Adjacency::Avoid => (60.0 - d).max(0.0) * 2.0,
        }
    }
}

/// One entry in the schedule.
#[derive(Debug, Clone, PartialEq)]
pub struct Space {
    pub name: String,
    /// Floor area of ONE of them, in blocks squared.
    pub area: i32,
    pub count: usize,
    /// Storeys tall. A great hall is not one storey.
    pub height: i32,
    /// What it needs: `daylight`, `outside_door`, `private`, `tall`,
    /// `wet`, `noisy`. Free-form, and used by the layout where it can
    /// be, reported where it cannot.
    pub qualities: BTreeSet<String>,
}

impl Space {
    pub fn total_area(&self) -> i32 { self.area * self.count as i32 }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Brief {
    pub spaces: Vec<Space>,
    /// (a, b, how). Symmetric; stored once.
    pub adjacencies: Vec<(String, String, Adjacency)>,
    /// The area the client has. `None` means unconstrained, which the
    /// lecture would call a missing decision rather than freedom.
    pub budget: Option<i32>,
    /// Circulation, walls and plant, as a fraction of net area. Real
    /// briefs carry 25-40%; ignoring it is how a schedule that
    /// "balances" turns out 30% too small.
    pub gross_factor: f32,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reconciliation {
    pub net: i32,
    pub gross: i32,
    pub budget: Option<i32>,
    /// Positive means over budget.
    pub over: i32,
    pub ok: bool,
    pub lines: Vec<String>,
}

impl Reconciliation {
    pub fn summary(&self) -> String {
        let mut s = format!("net {} + circulation = gross {}", self.net, self.gross);
        if let Some(b) = self.budget {
            s.push_str(&format!(", budget {b}: {}", if self.over > 0 {
                format!("OVER by {} ({:.0}%)", self.over, self.over as f32 / b as f32 * 100.0)
            } else { format!("{} spare", -self.over) }));
        }
        s
    }
}

impl Brief {
    /// Does the schedule fit the budget? The cheapest question in the
    /// whole pipeline, and the one nothing was asking.
    pub fn reconcile(&self) -> Reconciliation {
        let net: i32 = self.spaces.iter().map(|s| s.total_area()).sum();
        let f = if self.gross_factor <= 0.0 { 0.3 } else { self.gross_factor };
        let gross = (net as f32 * (1.0 + f)).round() as i32;
        let over = self.budget.map(|b| gross - b).unwrap_or(0);
        let mut lines: Vec<String> = self.spaces.iter()
            .map(|s| format!("  {:<20} {:>4} x{} = {:>6}{}", s.name, s.area, s.count, s.total_area(),
                             if s.qualities.is_empty() { String::new() }
                             else { format!("  [{}]", s.qualities.iter().cloned().collect::<Vec<_>>().join(", ")) }))
            .collect();
        lines.push(format!("  {:<20} {:>17}", "circulation etc.", gross - net));
        Reconciliation { net, gross, budget: self.budget, over, ok: over <= 0, lines }
    }

    /// Adjacencies naming a space that is not in the schedule. A brief
    /// that says "kitchen adjoins hall" with no kitchen is a mistake
    /// worth catching on paper.
    pub fn dangling(&self) -> Vec<String> {
        let names: BTreeSet<&str> = self.spaces.iter().map(|s| s.name.as_str()).collect();
        let mut out = Vec::new();
        for (a, b, _) in &self.adjacencies {
            for n in [a, b] {
                if !names.contains(n.as_str()) {
                    out.push(format!("adjacency names '{n}', which is not a space in the schedule"));
                }
            }
        }
        out.sort(); out.dedup();
        out
    }

    fn how(&self, a: &str, b: &str) -> Adjacency {
        self.adjacencies.iter()
            .find(|(x, y, _)| (x == a && y == b) || (x == b && y == a))
            .map(|(_, _, h)| *h).unwrap_or(Adjacency::Neutral)
    }
}

// ── the layout ───────────────────────────────────────────────────────

/// A placed room.
#[derive(Debug, Clone, PartialEq)]
pub struct Room {
    pub name: String,
    /// Which of the `count` this is, from 1.
    pub index: usize,
    /// Footprint corner and size, in blocks.
    pub at: (i32, i32),
    pub size: (i32, i32),
    pub height: i32,
    pub qualities: BTreeSet<String>,
}

impl Room {
    pub fn centre(&self) -> (f32, f32) {
        (self.at.0 as f32 + self.size.0 as f32 / 2.0, self.at.1 as f32 + self.size.1 as f32 / 2.0)
    }
    fn overlaps(&self, o: &Room) -> bool {
        self.at.0 < o.at.0 + o.size.0 && o.at.0 < self.at.0 + self.size.0
            && self.at.1 < o.at.1 + o.size.1 && o.at.1 < self.at.1 + self.size.1
    }
    /// Gap between footprints; 0 when they touch or overlap.
    fn gap(&self, o: &Room) -> f32 {
        let dx = (o.at.0 - (self.at.0 + self.size.0)).max(self.at.0 - (o.at.0 + o.size.0)).max(0);
        let dz = (o.at.1 - (self.at.1 + self.size.1)).max(self.at.1 - (o.at.1 + o.size.1)).max(0);
        ((dx * dx + dz * dz) as f32).sqrt()
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Layout {
    pub rooms: Vec<Room>,
    /// Adjacencies the layout failed to honour.
    pub unmet: Vec<String>,
    pub cost: f32,
    pub notes: Vec<String>,
}

impl Layout {
    pub fn summary(&self) -> String {
        format!("{} room(s), cost {:.0}{}", self.rooms.len(), self.cost,
                if self.unmet.is_empty() { String::new() }
                else { format!(", {} adjacency(ies) unmet", self.unmet.len()) })
    }
}

fn hash(a: u64, b: u64) -> u64 {
    let mut h = a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 31; h = h.wrapping_mul(0x7FB5_D329_728E_A185); h ^ (h >> 27)
}

/// A room's footprint from its area: squarish, but not square, because
/// every room being a perfect square reads as a spreadsheet.
fn footprint(area: i32, seed: u64) -> (i32, i32) {
    let a = area.max(4) as f32;
    let ratio = 1.0 + (hash(seed, 7) % 100) as f32 / 100.0 * 0.8;   // 1.0 .. 1.8
    let w = (a / ratio).sqrt().round().max(2.0);
    let d = (a / w).round().max(2.0);
    (w as i32, d as i32)
}

/// Place the schedule inside a footprint, honouring adjacency.
///
/// Greedy seeding then simulated-annealing-ish local moves: rooms are
/// nudged and swapped, and a move is kept when it lowers the cost.
/// Overlap is forbidden outright rather than penalised -- two rooms in
/// the same place is not a worse layout, it is not a layout.
pub fn lay_out(brief: &Brief, bounds: ((i32, i32), (i32, i32)), seed: u64, effort: usize) -> Layout {
    let mut l = Layout::default();
    let (min, max) = bounds;
    let (bw, bd) = (max.0 - min.0, max.1 - min.1);
    if bw < 4 || bd < 4 { l.notes.push("the site is too small to lay anything out".into()); return l; }
    // rooms, biggest first: a great hall placed last has nowhere to go
    let mut want: Vec<(String, i32, i32, usize, BTreeSet<String>)> = Vec::new();
    for s in &brief.spaces {
        for i in 0..s.count.max(1) {
            want.push((s.name.clone(), s.area, s.height.max(1), i + 1, s.qualities.clone()));
        }
    }
    want.sort_by_key(|(_, a, _, _, _)| std::cmp::Reverse(*a));
    let mut placed: Vec<Room> = Vec::new();
    for (n, (name, area, height, index, qualities)) in want.into_iter().enumerate() {
        let (w, d) = footprint(area, seed ^ n as u64);
        if w > bw || d > bd {
            l.notes.push(format!("'{name}' needs {w}x{d} and the site is {bw}x{bd}"));
            continue;
        }
        // try positions, keep the cheapest that does not overlap
        let mut best: Option<(f32, Room)> = None;
        for k in 0..(effort.max(8) * 4) {
            let h = hash(seed ^ n as u64, k as u64);
            let at = (min.0 + (h % (bw - w + 1).max(1) as u64) as i32,
                      min.1 + ((h >> 20) % (bd - d + 1).max(1) as u64) as i32);
            let cand = Room { name: name.clone(), index, at, size: (w, d), height,
                              qualities: qualities.clone() };
            if placed.iter().any(|p| p.overlaps(&cand)) { continue; }
            let c = cost_of(&cand, &placed, brief, bounds);
            if best.as_ref().map(|(bc, _)| c < *bc).unwrap_or(true) { best = Some((c, cand)); }
        }
        match best {
            Some((_, r)) => placed.push(r),
            None => l.notes.push(format!("'{name}' ({w}x{d}) did not fit anywhere left")),
        }
    }
    // local improvement: nudge each room, keep what helps
    for round in 0..effort {
        for i in 0..placed.len() {
            let before = cost_of(&placed[i], &placed_without(&placed, i), brief, bounds);
            let h = hash(seed ^ round as u64, i as u64);
            let step = 1 + (h % 5) as i32;
            let dirs = [(step, 0), (-step, 0), (0, step), (0, -step)];
            let (dx, dz) = dirs[(h >> 8) as usize % 4];
            let mut moved = placed[i].clone();
            moved.at = ((moved.at.0 + dx).clamp(min.0, max.0 - moved.size.0),
                        (moved.at.1 + dz).clamp(min.1, max.1 - moved.size.1));
            let others = placed_without(&placed, i);
            if others.iter().any(|p| p.overlaps(&moved)) { continue; }
            if cost_of(&moved, &others, brief, bounds) < before { placed[i] = moved; }
        }
    }
    // what the layout failed to honour, said plainly
    for (a, b, how) in &brief.adjacencies {
        if *how != Adjacency::Must { continue; }
        let (ra, rb): (Vec<&Room>, Vec<&Room>) = (placed.iter().filter(|r| &r.name == a).collect(),
                                                  placed.iter().filter(|r| &r.name == b).collect());
        if ra.is_empty() || rb.is_empty() { continue; }
        let best = ra.iter().flat_map(|x| rb.iter().map(move |y| x.gap(y)))
            .fold(f32::INFINITY, f32::min);
        if best > 1.5 {
            l.unmet.push(format!("'{a}' must adjoin '{b}' and is {best:.0} away"));
        }
    }
    l.cost = placed.iter().enumerate()
        .map(|(i, r)| cost_of(r, &placed_without(&placed, i), brief, bounds)).sum::<f32>() / 2.0;
    l.rooms = placed;
    l
}

fn placed_without(v: &[Room], i: usize) -> Vec<Room> {
    v.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, r)| r.clone()).collect()
}

fn cost_of(r: &Room, others: &[Room], brief: &Brief, bounds: ((i32, i32), (i32, i32))) -> f32 {
    let mut c = 0.0;
    // COMPACTNESS. Adjacency alone rewards only the pairs that were
    // named, so everything else drifts and the plan sprawls across the
    // whole site with courtyards nobody asked for. A building is
    // compact by default; the schedule says which parts must touch,
    // not that the rest may fly apart.
    let nearest = others.iter().map(|o| r.gap(o)).fold(f32::INFINITY, f32::min);
    if nearest.is_finite() { c += nearest * 3.0; }
    for o in others {
        c += brief.how(&r.name, &o.name).cost(r.gap(o));
    }
    // qualities the layout can actually serve: daylight and an outside
    // door both want a perimeter, so pay for being deep inside
    if r.qualities.contains("daylight") || r.qualities.contains("outside_door") {
        let (min, max) = bounds;
        let inset = (r.at.0 - min.0).min(max.0 - (r.at.0 + r.size.0))
            .min(r.at.1 - min.1).min(max.1 - (r.at.1 + r.size.1));
        c += inset.max(0) as f32 * 3.0;
    }
    if r.qualities.contains("private") {
        // pay for near neighbours regardless of who they are
        c += others.iter().filter(|o| r.gap(o) < 4.0).count() as f32 * 6.0;
    }
    c
}

// ── from a layout to a program ───────────────────────────────────────

/// Emit the layout as program source: one pass raising the mass, one
/// cutting the rooms out of it, one cutting the doors between rooms
/// that must adjoin.
///
/// This is the join between programming and design that the lecture
/// insists on: the program is a decision about the problem, and the
/// geometry follows from it rather than the other way round.
pub fn to_program(l: &Layout, brief: &Brief, wall: &str, floor_y: i32) -> String {
    if l.rooms.is_empty() { return String::new(); }
    let lo = l.rooms.iter().fold((i32::MAX, i32::MAX), |a, r| (a.0.min(r.at.0), a.1.min(r.at.1)));
    let hi = l.rooms.iter().fold((i32::MIN, i32::MIN),
        |a, r| (a.0.max(r.at.0 + r.size.0), a.1.max(r.at.1 + r.size.1)));
    let tallest = l.rooms.iter().map(|r| r.height).max().unwrap_or(1);
    let storey = 5;
    let mut s = String::new();
    s.push_str(&format!("# {} room(s), {} net\n", l.rooms.len(), brief.reconcile().net));
    s.push_str("pass the mass add\n");
    // centred on the rooms' own middle, not on the anchor: a layout
    // whose plan is not symmetric about the origin would otherwise be
    // half outside its own shell
    let (mx, mz) = ((lo.0 + hi.0) / 2, (lo.1 + hi.1) / 2);
    s.push_str(&format!("  part shell box w={} h={} d={} material={wall}\n",
                        hi.0 - lo.0 + 4, tallest * storey + 2, hi.1 - lo.1 + 4));
    s.push_str(&format!("  rel shell at ground dx={mx} dy={} dz={mz}\n", (tallest * storey) / 2));
    s.push_str("end\n");
    // each room hollowed out of it
    s.push_str("pass the rooms cut\n");
    for (i, r) in l.rooms.iter().enumerate() {
        // The room WHERE THE LAYOUT PUT IT. Its centre, offset from
        // the anchor, and its own floor level from its storey count.
        let (cx, cz) = (r.at.0 + r.size.0 / 2, r.at.1 + r.size.1 / 2);
        let cy = floor_y + (r.height * storey) / 2;
        s.push_str(&format!("  part r{i} box w={} h={} d={} material={wall}\n",
                            (r.size.0 - 1).max(1), (r.height * storey - 1).max(2), (r.size.1 - 1).max(1)));
        s.push_str(&format!("  rel r{i} at ground dx={cx} dy={cy} dz={cz}\n"));
    }
    s.push_str("end\n");
    // DOORS. A plan of sealed rooms fails its own reachability check --
    // ten voids nobody can enter is ten voids. Every MUST adjacency
    // gets an opening on the wall between the two rooms, and every
    // room with a perimeter quality gets one to the outside; the rest
    // reach the world through those.
    let mut doors = String::new();
    let mut n = 0;
    let pair_door = |a: &Room, c: &Room, k: usize| -> Option<String> {
        // the two must actually share a wall to have a door in it
        if a.gap(c) > 1.5 { return None; }
        let (ax, az) = a.centre(); let (cx, cz) = c.centre();
        let (mx, mz) = (((ax + cx) / 2.0) as i32, ((az + cz) / 2.0) as i32);
        let along_x = (ax - cx).abs() > (az - cz).abs();
        Some(format!("  part d{k} box w={} h=4 d={} material={wall}\n\
                      rel d{k} at ground dx={mx} dy={} dz={mz}\n",
                     if along_x { 3 } else { 2 }, if along_x { 2 } else { 3 }, floor_y + 2))
    };
    for (x, y, how) in &brief.adjacencies {
        if *how != Adjacency::Must { continue; }
        for a in l.rooms.iter().filter(|r| &r.name == x) {
            for c in l.rooms.iter().filter(|r| &r.name == y) {
                if let Some(d) = pair_door(a, c, n) { doors.push_str(&d); n += 1; }
            }
        }
    }
    for r in l.rooms.iter().filter(|r| r.qualities.contains("outside_door")) {
        // out through the nearest face of the shell
        let (cx, cz) = r.centre();
        let (dx, dz) = if (cx - mx as f32).abs() > (cz - mz as f32).abs() {
            (if cx > mx as f32 { hi.0 - lo.0 } else { -(hi.0 - lo.0) }, 0) } else {
            (0, if cz > mz as f32 { hi.1 - lo.1 } else { -(hi.1 - lo.1) }) };
        doors.push_str(&format!("  part d{n} box w={} h=4 d={} material={wall}\n\
                                 rel d{n} at ground dx={} dy={} dz={}\n",
                                if dz == 0 { 6 } else { 3 }, if dz == 0 { 3 } else { 6 },
                                cx as i32 + dx / 2, floor_y + 2, cz as i32 + dz / 2));
        n += 1;
    }
    if n > 0 {
        s.push_str("pass the doors cut\n");
        s.push_str(&doors);
        s.push_str("end\n");
    }
    s
}

/// The prompt for one ROLE. The lecture separates client, programmer
/// and designer, and the separation is not ceremony: they answer three
/// different questions, and a model answers each better alone.
pub fn role_prompt(role: &str, request: &str, context: &str) -> String {
    match role {
        "client" => format!("\
A player asked for: {request}

You are the CLIENT. Say what you want from this building -- not how it looks, what it is FOR.
Answer in these lines and nothing else:

PURPOSE: <one sentence: what happens in this building>
WHO: <who uses it, and roughly how many>
MUST HAVE: <the things without which it has failed, separated by semicolons>
FEELING: <how it should feel to be in>
SIZE: <how big, in blocks squared of floor, or 'you decide'>

{context}"),
        "programmer" => format!("\
The client asked for: {request}

{context}

You are the PROGRAMMER. Turn that into a SCHEDULE OF SPACES and an ADJACENCY MATRIX. Do not
design anything -- no shapes, no positions. You are deciding the problem, not solving it.

  budget 2400                     total floor area available, or omit
  circulation 0.3                 fraction for walls and corridors
  space NAME area=N [count=N] [height=N] [needs=daylight,outside_door,private,tall,wet,noisy]
  adj A B must|should|avoid

Every space the client must have needs a line. Give AREAS, in blocks squared -- a person needs
about 2, a bed 6, a hall for forty people about 200. Then say what must touch what: a kitchen
must adjoin the space it serves, a plant room must avoid anywhere people sleep.

EXAMPLE
  client wants: a great hall for a lord, with kitchens and lodging
budget 1200
circulation 0.35
space great hall area=240 height=2 needs=daylight,tall
space kitchen area=90 needs=wet,noisy
space buttery area=40
space solar area=70 count=1 needs=daylight,private
space lodging area=45 count=4 needs=daylight
space stores area=60
adj kitchen buttery must
adj buttery great hall must
adj solar great hall should
adj lodging kitchen avoid
adj stores kitchen should

Write the schedule and the adjacencies only."),
        _ => format!("You are the DESIGNER. {request}\n\n{context}"),
    }
}

/// Parse the programmer's reply.
pub fn parse(src: &str) -> (Brief, Vec<String>) {
    let mut b = Brief { gross_factor: 0.3, ..Default::default() };
    let mut errs = Vec::new();
    for line in src.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') { continue; }
        let w: Vec<&str> = t.split_whitespace().collect();
        match w.first().map(|v| v.to_lowercase()).as_deref() {
            Some("budget") => match w.get(1).and_then(|v| v.parse().ok()) {
                Some(v) => b.budget = Some(v),
                None => errs.push(format!("budget needs a number: {t:?}")),
            },
            Some("circulation") | Some("gross") => match w.get(1).and_then(|v| v.parse::<f32>().ok()) {
                Some(v) => b.gross_factor = v.clamp(0.0, 2.0),
                None => errs.push(format!("circulation needs a fraction: {t:?}")),
            },
            Some("space") => {
                // the name runs to the first key=value, so "great hall
                // area=240" works -- a model writes names with spaces
                let kv_at = w.iter().position(|s| s.contains('=')).unwrap_or(w.len());
                if kv_at <= 1 { errs.push(format!("space needs a name: {t:?}")); continue; }
                let name = w[1..kv_at].join(" ").to_lowercase();
                let kv = |k: &str| w[kv_at..].iter().find_map(|s| s.strip_prefix(&format!("{k}=")));
                let Some(area) = kv("area").and_then(|v| v.parse::<i32>().ok()) else {
                    errs.push(format!("space '{name}' has no area=")); continue;
                };
                b.spaces.push(Space {
                    name,
                    area: area.clamp(1, 1_000_000),
                    count: kv("count").and_then(|v| v.parse().ok()).unwrap_or(1).clamp(1, 200),
                    height: kv("height").and_then(|v| v.parse().ok()).unwrap_or(1).clamp(1, 12),
                    qualities: kv("needs").map(|v| v.split(',').map(|q| q.trim().to_lowercase())
                        .filter(|q| !q.is_empty()).collect()).unwrap_or_default(),
                });
            }
            Some("adj") | Some("adjacency") => {
                // `adj great hall kitchen must`: the LAST word is the
                // strength, and the two names split at... the point
                // where the first matches a known space. Names with
                // spaces make this ambiguous, so match against the
                // schedule, which is why adjacencies parse second.
                let Some(how) = w.last().and_then(|s| Adjacency::parse(s)) else {
                    errs.push(format!("adjacency needs must|should|avoid last: {t:?}")); continue;
                };
                let mid = w[1..w.len() - 1].join(" ").to_lowercase();
                match split_pair(&mid, &b.spaces) {
                    Some((a, c)) => b.adjacencies.push((a, c, how)),
                    None => errs.push(format!("cannot tell the two spaces apart in {mid:?}")),
                }
            }
            _ => b.notes.push(t.to_string()),
        }
    }
    for d in b.dangling() { errs.push(d); }
    (b, errs)
}

/// Split "great hall kitchen" into its two space names, using the
/// schedule to disambiguate. Longest known prefix wins.
fn split_pair(s: &str, spaces: &[Space]) -> Option<(String, String)> {
    let names: Vec<&str> = spaces.iter().map(|s| s.name.as_str()).collect();
    let mut best: Option<(String, String)> = None;
    for n in &names {
        if let Some(rest) = s.strip_prefix(n) {
            let rest = rest.trim();
            if rest.is_empty() { continue; }
            if best.as_ref().map(|(a, _)| a.len() < n.len()).unwrap_or(true) {
                best = Some((n.to_string(), rest.to_string()));
            }
        }
    }
    best.or_else(|| s.split_once(' ').map(|(a, b)| (a.to_string(), b.to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keep() -> Brief {
        parse("budget 1200\n\
               circulation 0.35\n\
               space great hall area=240 height=2 needs=daylight,tall\n\
               space kitchen area=90 needs=wet,noisy\n\
               space buttery area=40\n\
               space solar area=70 needs=daylight,private\n\
               space lodging area=45 count=4 needs=daylight\n\
               space stores area=60\n\
               adj kitchen buttery must\n\
               adj buttery great hall must\n\
               adj solar great hall should\n\
               adj lodging kitchen avoid\n").0
    }

    #[test]
    fn a_schedule_parses_including_names_with_spaces() {
        let b = keep();
        assert_eq!(b.spaces.len(), 6);
        let hall = b.spaces.iter().find(|s| s.name == "great hall").expect("names may have spaces");
        assert_eq!(hall.area, 240);
        assert_eq!(hall.height, 2);
        assert!(hall.qualities.contains("daylight") && hall.qualities.contains("tall"));
        assert_eq!(b.spaces.iter().find(|s| s.name == "lodging").unwrap().count, 4);
        assert_eq!(b.budget, Some(1200));
        // and the adjacencies found both names despite the space in one
        assert!(b.adjacencies.iter().any(|(a, c, h)|
            a == "buttery" && c == "great hall" && *h == Adjacency::Must), "{:?}", b.adjacencies);
    }

    #[test]
    fn the_budget_reconciles_and_says_by_how_much_it_does_not() {
        let b = keep();
        let r = b.reconcile();
        // 240 + 90 + 40 + 70 + 4*45 + 60 = 680 net
        assert_eq!(r.net, 680);
        assert_eq!(r.gross, (680.0 * 1.35_f32).round() as i32);
        assert!(r.ok, "{}", r.summary());
        assert!(r.summary().contains("spare"));
        // over budget says so, with a percentage
        let mut over = b.clone();
        over.budget = Some(600);
        let o = over.reconcile();
        assert!(!o.ok);
        assert!(o.summary().contains("OVER by") && o.summary().contains('%'), "{}", o.summary());
        // CIRCULATION IS NOT OPTIONAL: a schedule that ignores it
        // "balances" and then comes out a third too small
        assert!(r.gross > r.net, "walls and corridors cost area");
        assert!(r.lines.iter().any(|l| l.contains("circulation")));
    }

    #[test]
    fn an_adjacency_naming_a_space_that_does_not_exist_is_caught_on_paper() {
        let (_, errs) = parse("space hall area=100\nadj hall pantry must\n");
        assert!(errs.iter().any(|e| e.contains("'pantry', which is not a space")), "{errs:?}");
        // and a space with no area
        let (_, e2) = parse("space hall height=2\n");
        assert!(e2.iter().any(|e| e.contains("has no area")), "{e2:?}");
        let (_, e3) = parse("adj a b sideways\n");
        assert!(e3.iter().any(|e| e.contains("must|should|avoid")), "{e3:?}");
    }

    #[test]
    fn a_layout_puts_what_must_adjoin_together() {
        let b = keep();
        let l = lay_out(&b, ((0, 0), (60, 60)), 7, 40);
        assert_eq!(l.rooms.len(), 9, "six spaces, one of them four times: {}", l.summary());
        // nothing overlaps -- that is not a worse layout, it is not one
        for (i, a) in l.rooms.iter().enumerate() {
            for c in &l.rooms[i + 1..] {
                assert!(!a.overlaps(c), "{} and {} overlap", a.name, c.name);
            }
        }
        // the MUST pairs ended up touching or nearly
        let gap = |x: &str, y: &str| {
            let (ra, rb): (Vec<_>, Vec<_>) = (l.rooms.iter().filter(|r| r.name == x).collect(),
                                              l.rooms.iter().filter(|r| r.name == y).collect());
            ra.iter().flat_map(|a| rb.iter().map(move |b| a.gap(b))).fold(f32::INFINITY, f32::min)
        };
        assert!(gap("kitchen", "buttery") < 12.0, "kitchen to buttery: {}", gap("kitchen", "buttery"));
        // and the AVOID pair did not
        assert!(gap("lodging", "kitchen") > gap("kitchen", "buttery"),
                "lodging is further from the kitchen than the buttery is");
    }

    #[test]
    fn daylight_pushes_a_room_to_the_perimeter() {
        let b = parse("space lit area=60 needs=daylight\n\
                       space dark area=60\n\
                       space core area=60\n").0;
        let l = lay_out(&b, ((0, 0), (40, 40)), 3, 60);
        let inset = |n: &str| {
            let r = l.rooms.iter().find(|r| r.name == n).unwrap();
            (r.at.0).min(40 - (r.at.0 + r.size.0)).min(r.at.1).min(40 - (r.at.1 + r.size.1))
        };
        assert!(inset("lit") <= inset("dark").max(inset("core")),
                "the lit room is nearer an edge: {} vs {} and {}", inset("lit"), inset("dark"), inset("core"));
    }

    #[test]
    fn a_room_too_big_for_the_site_says_so_rather_than_being_squeezed() {
        let b = parse("space vast area=10000\nspace small area=40\n").0;
        let l = lay_out(&b, ((0, 0), (30, 30)), 1, 20);
        assert!(l.notes.iter().any(|n| n.contains("'vast'") && n.contains("site is")), "{:?}", l.notes);
        assert_eq!(l.rooms.len(), 1, "the one that fits still went in");
    }

    #[test]
    fn unmet_adjacencies_are_reported_not_hidden() {
        // a site so tight that things cannot all touch
        let b = parse("space a area=200\nspace b area=200\nspace c area=200\n\
                       adj a b must\nadj b c must\nadj a c must\n").0;
        let l = lay_out(&b, ((0, 0), (60, 60)), 5, 30);
        // whatever it managed, it says what it did not
        assert!(l.summary().contains("room(s)"));
        for u in &l.unmet { assert!(u.contains("must adjoin"), "{u}"); }
    }

    #[test]
    fn the_layout_becomes_a_program_that_parses_and_runs() {
        let b = keep();
        let l = lay_out(&b, ((-30, -30), (30, 30)), 11, 30);
        let src = to_program(&l, &b, "stone", 0);
        assert!(src.contains("pass the mass add") && src.contains("pass the rooms cut"), "{src}");
        let (prog, errs) = crate::program::parse(&src);
        assert!(errs.is_empty(), "{src}\n{errs:?}");
        // mass, rooms, and doors -- the third arrived when sealed rooms
        // turned out to fail their own reachability check
        assert!(prog.passes.len() >= 2, "{}", prog.passes.len());
        assert_eq!(prog.passes[0].polarity, crate::program::Polarity::Add);
        assert_eq!(prog.passes[1].polarity, crate::program::Polarity::Cut,
                   "the rooms are CUT from the mass, which is what a building is");
        let r = crate::program::run(&prog, (0, 64, 0));
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let solid = r.changes.iter().filter(|c| c.to != 0).count();
        let air = r.changes.iter().filter(|c| c.to == 0).count();
        assert!(solid > 1000 && air > 500, "a mass with rooms in it: {solid} solid, {air} air");
    }

    #[test]
    fn the_role_prompts_ask_three_different_questions() {
        let c = role_prompt("client", "a castle keep", "");
        let p = role_prompt("programmer", "a castle keep", "PURPOSE: a lord holds court");
        assert!(c.contains("what it is FOR") && c.contains("MUST HAVE:"), "{c}");
        assert!(!c.contains("area=") && !c.contains("adj "), "the client does not write a schedule");
        assert!(p.contains("SCHEDULE OF SPACES") && p.contains("ADJACENCY MATRIX"));
        assert!(p.contains("Do not") && p.contains("no shapes, no positions"),
                "the programmer decides the problem, not the solution");
        assert!(p.contains("a lord holds court"), "the client's answer carries forward");
        // and the programmer's own example parses, reconciles and lays out
        let ex = p.split("EXAMPLE").nth(1).unwrap();
        let body: String = ex.lines().skip(2)
            .take_while(|l| l.starts_with("budget") || l.starts_with("circulation")
                        || l.starts_with("space") || l.starts_with("adj"))
            .map(|l| format!("{l}\n")).collect();
        let (b, errs) = parse(&body);
        assert!(errs.is_empty(), "{body}\n{errs:?}");
        assert!(b.spaces.len() >= 5 && !b.adjacencies.is_empty());
        assert!(b.reconcile().ok, "the example fits its own budget: {}", b.reconcile().summary());
    }

    #[test]
    fn the_geometry_is_where_the_layout_put_it() {
        // `to_program` computed each room's centre and then THREW IT
        // AWAY, emitting every room at the anchor: a twelve-room plan
        // carved one pile of overlapping voids in the middle, and the
        // layout's whole output was discarded.
        let b = keep();
        let l = lay_out(&b, ((-40, -40), (40, 40)), 11, 40);
        assert!(l.rooms.len() >= 6);
        let (prog, errs) = crate::program::parse(&to_program(&l, &b, "stone", 0));
        assert!(errs.is_empty(), "{errs:?}");
        let r = crate::program::run(&prog, (0, 64, 0));
        let air: std::collections::BTreeSet<(i32, i32, i32)> =
            r.changes.iter().filter(|c| c.to == 0).map(|c| c.at).collect();
        // every room is hollow AT ITS OWN PLACE
        for room in &l.rooms {
            let (cx, cz) = (room.at.0 + room.size.0 / 2, room.at.1 + room.size.1 / 2);
            assert!(air.contains(&(cx, 66, cz)),
                    "'{}' should be hollow at ({cx}, 66, {cz})", room.name);
        }
        // and the voids are SEPARATE: there is solid between two rooms
        // that are not adjacent, which one pile at the origin could
        // never produce
        let spread: std::collections::BTreeSet<i32> = air.iter().map(|c| c.0).collect();
        assert!(spread.len() > 20, "the voids are spread across the plan: {}", spread.len());
        let solid_between = l.rooms.windows(2).any(|w| {
            let mid = ((w[0].at.0 + w[1].at.0) / 2, (w[0].at.1 + w[1].at.1) / 2);
            !air.contains(&(mid.0, 66, mid.1))
        });
        assert!(solid_between, "there are walls between rooms");
    }

    #[test]
    fn the_rooms_have_doors_between_them_and_to_the_outside() {
        // ten voids nobody can enter is ten voids: a plan of sealed
        // rooms fails its own reachability check
        let b = keep();
        let l = lay_out(&b, ((-40, -40), (40, 40)), 11, 40);
        let src = to_program(&l, &b, "stone", 0);
        assert!(src.contains("pass the doors cut"), "{src}");
        let (prog, errs) = crate::program::parse(&src);
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(prog.passes.len(), 3, "mass, rooms, doors");
        let r = crate::program::run(&prog, (0, 64, 0));
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert!(r.log[2].2 > 0, "the doors cut something: {:?}", r.log);
        // a MUST pair that ended up adjacent has an opening between it
        let air: std::collections::BTreeSet<(i32, i32, i32)> =
            r.changes.iter().filter(|c| c.to == 0).map(|c| c.at).collect();
        let mut found = false;
        for (x, y, how) in &b.adjacencies {
            if *how != Adjacency::Must { continue; }
            for a in l.rooms.iter().filter(|r| &r.name == x) {
                for c in l.rooms.iter().filter(|r| &r.name == y) {
                    if a.gap(c) > 1.5 { continue; }
                    let (ax, az) = a.centre(); let (cx, cz) = c.centre();
                    let m = (((ax + cx) / 2.0) as i32, ((az + cz) / 2.0) as i32);
                    if (65..69).any(|yy| air.contains(&(m.0, yy, m.1))) { found = true; }
                }
            }
        }
        assert!(found, "an opening in the wall between two rooms that must adjoin");
    }
}
