//! What a thing IS made of, before what it holds.
//!
//! THE LAYER THAT WAS MISSING
//!
//! `brief.rs` schedules rooms. But you cannot schedule rooms for "a
//! castle" until you know a castle is a curtain wall, a keep, a
//! gatehouse and a courtyard -- four SECTIONS, of very different
//! sizes, some of which are the thing and some of which are extras.
//! Going straight from a request to a room list means the model
//! invents the whole structure every time, and invents it differently
//! every time.
//!
//! So: a TYPOLOGY. A defined structure per type, with
//!
//!   ESSENTIAL sections   without these it is not that thing. A castle
//!                        with no wall is a manor house.
//!   OPTIONAL sections    common and not required: a chapel, stables.
//!   AREA SHARES          how the whole divides. A keep is not the
//!                        same size as a gatehouse and never was.
//!   CONSTRAINTS          what must hold: the wall encloses the rest,
//!                        the gatehouse is IN the wall, the keep is
//!                        the tallest thing.
//!
//! TRIAGE
//!
//! A client says five things and they are not the same KIND of thing.
//! "It should be on a cliff" is a constraint. "With a chapel" adds a
//! section. "The keep should be enormous" modifies an essential one.
//! "It should feel forbidding" is neither -- it is a quality, and it
//! belongs on every section rather than being one.
//!
//! Nothing was sorting these. Every wish arrived as undifferentiated
//! prose and the model did what it could, which is why "a castle with
//! a chapel on a cliff" produced a chapel-shaped castle on flat ground
//! as often as not. `triage` puts each wish in exactly one of four
//! places, and each place has somewhere to go: modifications adjust a
//! section's share or constraints, additions become sections,
//! constraints reach site selection, qualities reach every section.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need { Essential, Optional }

/// One part of a building type.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub name: String,
    pub need: Need,
    /// Its share of the whole, as a weight. Normalised against its
    /// siblings, so the numbers are relative and a modification can
    /// scale one without rewriting the rest.
    pub share: f32,
    /// How many storeys, relative: 1 is ordinary, 3 is a keep.
    pub prominence: f32,
    /// What must hold about it, in words the layout and the checks can
    /// read: `encloses`, `inside`, `perimeter`, `tallest`, `ground`,
    /// `detached`, `axial`.
    pub constraints: Vec<String>,
    /// Rooms this section typically holds, as a starting schedule.
    pub rooms: Vec<(&'static str, i32)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Typology {
    pub name: &'static str,
    pub summary: &'static str,
    pub sections: Vec<Section>,
    /// Gross floor area for an ORDINARY one of these, in blocks
    /// squared. A castle is not a house and "huge" means something
    /// different to each: a huge house is a manor, a huge castle is a
    /// fortress. Scale words multiply THIS, so the type sets the
    /// register and the word sets the degree.
    pub base_area: i32,
}

/// What a scale word does to the base. Multiplicative, because scale
/// is a ratio: "twice as big" is meaningful and "500 bigger" is not.
///
/// `share *= 2.4` on a section was the only thing "huge" did before,
/// which scales one part's slice of a FIXED total -- a huge keep stole
/// its area from the courtyard and the castle stayed the same size.
/// The whole never grew, so "a huge fortress" built the same 4,000 as
/// "a fortress".
pub fn scale_word(w: &str) -> Option<f32> {
    Some(match w {
        "tiny" | "miniature" => 0.15,
        "small" | "modest" | "little" | "humble" => 0.4,
        "" | "ordinary" | "normal" => 1.0,
        "large" | "big" | "grand" | "sizeable" => 2.2,
        "huge" | "great" | "major" => 4.5,
        "enormous" | "vast" | "massive" | "immense" => 9.0,
        "colossal" | "titanic" | "monumental" | "gigantic" => 16.0,
        _ => return None,
    })
}

/// The gross floor area a request implies. The type sets the register,
/// the scale word sets the degree, and an explicit number beats both.
pub fn scale_for(request: &str, t: &Typology) -> (i32, String) {
    let low = request.to_lowercase();
    let words: Vec<&str> = low.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    // an explicit area wins outright: "a castle of 20000"
    if let Some(n) = words.iter().enumerate().find_map(|(i, w)| {
        let n: i32 = w.parse().ok()?;
        if n < 200 { return None; }
        // guard against reading a year or a count as an area
        let prev = i.checked_sub(1).and_then(|j| words.get(j)).copied().unwrap_or("");
        if ["of", "about", "around", "roughly"].contains(&prev) || n >= 1000 { Some(n) } else { None }
    }) { return (n.clamp(200, 4_000_000), format!("{n} was asked for outright")); }
    // the biggest scale word present, so "a large huge castle" is huge
    let found = words.iter().filter_map(|w| scale_word(w).map(|f| (*w, f)))
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    match found {
        Some((w, f)) => ((t.base_area as f32 * f).round() as i32,
                         format!("a {w} {} is {f}x the ordinary {}", t.name, t.base_area)),
        None => (t.base_area, format!("an ordinary {} is {}", t.name, t.base_area)),
    }
}

fn sec(name: &str, need: Need, share: f32, prominence: f32,
       constraints: &[&str], rooms: &[(&'static str, i32)]) -> Section {
    Section { name: name.to_string(), need, share, prominence,
              constraints: constraints.iter().map(|s| s.to_string()).collect(),
              rooms: rooms.to_vec() }
}

/// The defined structures. Deliberately few and deliberately opinionated:
/// a typology that lists everything is a menu again, and the point of
/// one is to say what a thing IS.
pub fn typologies() -> Vec<Typology> {
    vec![
        Typology { base_area: 9000, name: "castle", summary: "a wall that encloses, a keep that dominates, a way in",
            sections: vec![
                sec("curtain wall", Need::Essential, 0.30, 1.5, &["encloses", "perimeter"],
                    &[("wall walk", 200), ("corner tower", 60)]),
                sec("keep", Need::Essential, 0.28, 3.0, &["inside", "tallest"],
                    &[("great hall", 240), ("solar", 70), ("undercroft", 120)]),
                sec("gatehouse", Need::Essential, 0.12, 2.0, &["perimeter", "ground"],
                    &[("gate passage", 60), ("guardroom", 50), ("portcullis room", 30)]),
                sec("courtyard", Need::Essential, 0.20, 0.0, &["inside", "ground"],
                    &[("bailey", 400), ("well", 10)]),
                sec("chapel", Need::Optional, 0.06, 1.5, &["inside", "axial"],
                    &[("nave", 90), ("chancel", 40)]),
                sec("stables", Need::Optional, 0.08, 1.0, &["inside", "ground"],
                    &[("stalls", 120), ("tack", 30)]),
                sec("barracks", Need::Optional, 0.10, 1.0, &["inside"],
                    &[("dormitory", 160), ("armoury", 60)]),
            ] },
        Typology { base_area: 5000, name: "temple", summary: "an approach, a threshold, a sanctuary at the end of it",
            sections: vec![
                sec("sanctuary", Need::Essential, 0.30, 2.5, &["axial", "tallest"],
                    &[("cella", 180), ("altar", 30)]),
                sec("hall", Need::Essential, 0.32, 2.0, &["axial", "inside"],
                    &[("nave", 300), ("aisle", 120)]),
                sec("portico", Need::Essential, 0.14, 2.0, &["perimeter", "axial", "ground"],
                    &[("porch", 120)]),
                sec("precinct", Need::Essential, 0.18, 0.0, &["encloses", "ground"],
                    &[("court", 400)]),
                sec("treasury", Need::Optional, 0.06, 1.0, &["inside"], &[("strongroom", 60)]),
                sec("tower", Need::Optional, 0.10, 4.0, &["tallest", "perimeter"], &[("belfry", 40)]),
            ] },
        Typology { base_area: 22000, name: "town", summary: "a centre, streets that reach it, edges that end it",
            sections: vec![
                sec("market square", Need::Essential, 0.14, 0.0, &["inside", "ground"],
                    &[("square", 500), ("well", 10)]),
                sec("dwellings", Need::Essential, 0.42, 1.5, &["ground"],
                    &[("house", 60)]),
                sec("streets", Need::Essential, 0.20, 0.0, &["ground"], &[("street", 600)]),
                sec("civic", Need::Essential, 0.12, 2.0, &["axial"],
                    &[("hall", 200), ("chamber", 60)]),
                sec("walls", Need::Optional, 0.14, 1.5, &["encloses", "perimeter"], &[("rampart", 300)]),
                sec("harbour", Need::Optional, 0.16, 0.0, &["perimeter", "ground"],
                    &[("quay", 300), ("warehouse", 120)]),
                sec("temple", Need::Optional, 0.10, 2.0, &["axial"], &[("nave", 150)]),
            ] },
        Typology { base_area: 260, name: "house", summary: "somewhere to be, somewhere to cook, somewhere to sleep",
            sections: vec![
                sec("living", Need::Essential, 0.34, 1.0, &["daylight", "ground"],
                    &[("hall", 40), ("hearth", 10)]),
                sec("service", Need::Essential, 0.22, 1.0, &["ground"],
                    &[("kitchen", 25), ("store", 15)]),
                sec("sleeping", Need::Essential, 0.30, 1.0, &["private", "daylight"],
                    &[("chamber", 20)]),
                sec("entry", Need::Essential, 0.08, 1.0, &["perimeter", "ground"], &[("porch", 8)]),
                sec("workshop", Need::Optional, 0.16, 1.0, &["ground", "detached"], &[("shop", 40)]),
                sec("byre", Need::Optional, 0.14, 1.0, &["ground", "detached"], &[("stall", 40)]),
            ] },
        Typology { base_area: 900, name: "tower", summary: "a base, a shaft, something at the top worth climbing to",
            sections: vec![
                sec("base", Need::Essential, 0.28, 1.0, &["ground", "perimeter"],
                    &[("entrance", 40), ("store", 60)]),
                sec("shaft", Need::Essential, 0.40, 5.0, &["tallest", "axial"],
                    &[("stair", 30), ("chamber", 50)]),
                sec("summit", Need::Essential, 0.18, 1.0, &["tallest"],
                    &[("watch room", 50), ("parapet", 40)]),
                sec("annexe", Need::Optional, 0.14, 1.0, &["ground", "detached"], &[("lodging", 60)]),
            ] },
    ]
}

/// Other words for the same type. "A huge fortress" found NO typology
/// at all and the whole chain fell over, because `lookup` matched only
/// the literal name -- and nobody says "castle" when they mean a
/// fortress, a citadel or a stronghold.
const ALIASES: &[(&str, &[&str])] = &[
    ("castle", &["fortress", "citadel", "stronghold", "fort", "keep", "palace", "chateau"]),
    ("temple", &["shrine", "cathedral", "church", "basilica", "monastery", "sanctuary", "abbey"]),
    ("town", &["city", "village", "settlement", "hamlet", "port", "metropolis"]),
    ("house", &["cottage", "home", "hut", "dwelling", "manor", "farmhouse", "lodge", "cabin"]),
    ("tower", &["spire", "lighthouse", "watchtower", "minaret", "obelisk", "belfry"]),
];

pub fn lookup(name: &str) -> Option<Typology> {
    let n = name.trim().to_lowercase();
    let words: Vec<&str> = n.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let has = |w: &str| words.iter().any(|x| *x == w);
    // the literal name first, then the aliases, and the LONGEST match
    // wins so "a temple town" is a town rather than a temple only
    // because "town" appears later -- both are found and the last word
    // in the request is the head noun
    // WHOLE WORDS, by position in the word list. `rfind` on the raw
    // string matched "house" inside "lighthouse" and made a lighthouse
    // a dwelling. A type name that is a substring of another word is
    // not a mention of that type.
    let word_at = |needle: &str| -> Option<usize> {
        let parts: Vec<&str> = needle.split(' ').collect();
        (0..words.len()).rev()
            .find(|i| parts.iter().enumerate().all(|(k, p)| words.get(i + k) == Some(p)))
    };
    let mut best: Option<(usize, Typology)> = None;
    for t in typologies() {
        let mut at = word_at(t.name);
        for (base, alts) in ALIASES {
            if *base != t.name { continue; }
            for a in *alts {
                if has(a) {
                    let p = word_at(a);
                    if at.map(|x| p.unwrap_or(0) > x).unwrap_or(true) { at = p; }
                }
            }
        }
        if let Some(p) = at {
            if best.as_ref().map(|(bp, _)| p > *bp).unwrap_or(true) { best = Some((p, t)); }
        }
    }
    best.map(|(_, t)| t)
}

pub fn vocabulary() -> String {
    typologies().iter().map(|t| format!("  {:<8} {}", t.name, t.summary)).collect::<Vec<_>>().join("\n")
}

// ── triage ───────────────────────────────────────────────────────────

/// What kind of thing a client just said.
#[derive(Debug, Clone, PartialEq)]
pub enum Wish {
    /// Change an essential section: bigger, taller, round, ruined.
    Modify { section: String, how: String },
    /// Add an optional or invented section.
    Add { section: String },
    /// Remove a section the type would otherwise have.
    Drop { section: String },
    /// A limit on the whole: site, budget, orientation, material.
    Constrain { what: String },
    /// A feeling. Belongs to everything, so it is not a section.
    Quality { word: String },
    /// Understood as none of the above, and said so rather than
    /// quietly dropped.
    Unplaced { text: String },
}

impl Wish {
    pub fn kind(&self) -> &'static str {
        match self {
            Wish::Modify { .. } => "modify", Wish::Add { .. } => "add",
            Wish::Drop { .. } => "drop", Wish::Constrain { .. } => "constrain",
            Wish::Quality { .. } => "quality", Wish::Unplaced { .. } => "unplaced",
        }
    }
}

const MODIFIERS: &[&str] = &["bigger", "larger", "huge", "enormous", "vast", "massive", "small",
                             "tiny", "taller", "higher", "low", "round", "square", "ruined",
                             "broken", "half", "twin", "double", "wide", "narrow", "deep"];
const CONSTRAINTS: &[&str] = &["cliff", "hill", "island", "river", "coast", "shore", "mountain",
                               "valley", "forest", "desert", "snow", "underground", "budget",
                               "facing", "north", "south", "east", "west", "sea", "lake"];
const QUALITIES: &[&str] = &["forbidding", "welcoming", "grand", "humble", "ancient", "new",
                             "ruined", "sacred", "eerie", "cosy", "austere", "ornate", "bleak",
                             "lively", "abandoned", "fortified", "wealthy", "poor"];

/// Sort one client statement into exactly one place.
///
/// The order matters and is the whole design: a DROP beats an ADD
/// ("without a chapel" names a chapel), a section name beats a
/// modifier ("a huge keep" modifies the keep, it does not add "huge"),
/// and a quality only wins when nothing structural was named --
/// otherwise "a forbidding gatehouse" becomes a quality and loses the
/// gatehouse.
pub fn triage_one(text: &str, t: &Typology) -> Wish {
    let low = text.to_lowercase();
    let words: Vec<&str> = low.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let has = |w: &str| words.iter().any(|x| *x == w);
    // which section, if any, is named? longest name wins, so "curtain
    // wall" beats "wall"
    let mut named: Option<&Section> = None;
    for s in &t.sections {
        if low.contains(&s.name) || s.name.split(' ').all(|w| has(w)) {
            if named.map(|n| n.name.len() < s.name.len()).unwrap_or(true) { named = Some(s); }
        }
    }
    let dropping = has("without") || has("no") || has("skip") || has("omit");
    if let Some(s) = named {
        if dropping { return Wish::Drop { section: s.name.clone() }; }
        let how: Vec<&str> = MODIFIERS.iter().filter(|m| has(m)).copied().collect();
        if !how.is_empty() {
            return Wish::Modify { section: s.name.clone(), how: how.join(" ") };
        }
        // named with no modifier: adding it if optional, else it is
        // already there and the mention is a quality at most
        if s.need == Need::Optional { return Wish::Add { section: s.name.clone() }; }
        if let Some(q) = QUALITIES.iter().find(|q| has(q)) {
            return Wish::Modify { section: s.name.clone(), how: q.to_string() };
        }
        return Wish::Modify { section: s.name.clone(), how: "as stated".into() };
    }
    if let Some(c) = CONSTRAINTS.iter().find(|c| has(c)) {
        return Wish::Constrain { what: format!("{c}: {}", text.trim()) };
    }
    // A bare scale word -- "a huge fortress" -- is about the WHOLE, and
    // `scale_for` has it. Recording it as a modification of nothing, or
    // as a quality, would double-count it.
    if words.iter().any(|w| scale_word(w).is_some()) && named.is_none() {
        return Wish::Constrain { what: format!("scale: {}", text.trim()) };
    }
    if let Some(q) = QUALITIES.iter().find(|q| has(q)) {
        return Wish::Quality { word: q.to_string() };
    }
    // an unknown noun with an adding word is a new section, invented
    if has("with") || has("and") || has("plus") || has("also") {
        let noun = words.iter().rev()
            .find(|w| w.len() > 3 && !["with", "and", "plus", "also", "that", "some"].contains(*w));
        if let Some(n) = noun { return Wish::Add { section: n.to_string() }; }
    }
    Wish::Unplaced { text: text.trim().to_string() }
}

/// Split a request into statements and sort each. Splits on commas,
/// `with`, `and`, semicolons -- how people actually list wishes.
pub fn triage(request: &str, t: &Typology) -> Vec<Wish> {
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    let low = request.to_lowercase();
    for word in low.split_whitespace() {
        let w = word.trim_matches(|c: char| c == ',' || c == ';');
        let breaks = word.ends_with(',') || word.ends_with(';')
            || ["with", "and", "plus", "also", "but", "on", "in", "without"].contains(&w);
        if breaks && !cur.trim().is_empty() {
            parts.push(cur.trim().to_string());
            cur.clear();
        }
        // the breaking word belongs to the NEXT part when it carries
        // meaning: "without a chapel" needs its "without"
        if ["with", "without", "on", "in"].contains(&w) { cur.push_str(w); cur.push(' '); }
        else if !["and", "plus", "also", "but"].contains(&w) { cur.push_str(word); cur.push(' '); }
    }
    if !cur.trim().is_empty() { parts.push(cur.trim().to_string()); }
    // A FRAGMENT IS NOT A WISH. Splitting on "and"/"on"/"with" leaves
    // stubs like "a" and "on a" behind, and each was arriving as an
    // unplaced wish the client had to be told about -- noise that
    // makes the real unplaced ones easy to miss.
    parts.retain(|p| {
        let words: Vec<&str> = p.split_whitespace()
            .filter(|w| !["a", "an", "the", "with", "on", "in", "of", "some", "its"].contains(w))
            .collect();
        !words.is_empty()
    });
    parts.iter().map(|p| triage_one(p, t)).collect()
}

/// The typology with the client's wishes applied.
#[derive(Debug, Clone, PartialEq)]
pub struct Massing {
    pub typology: &'static str,
    /// The gross area the request implied, and why. A massing that
    /// does not carry its own scale gets one invented downstream.
    pub implied_total: i32,
    pub scale_note: String,
    pub sections: Vec<Section>,
    pub constraints: Vec<String>,
    pub qualities: Vec<String>,
    pub unplaced: Vec<String>,
    pub log: Vec<String>,
}

impl Massing {
    /// The area each section gets, from its share and a total.
    pub fn areas(&self, total: i32) -> Vec<(String, i32, f32)> {
        let sum: f32 = self.sections.iter().map(|s| s.share).sum();
        let sum = if sum <= 0.0 { 1.0 } else { sum };
        self.sections.iter()
            .map(|s| (s.name.clone(), ((s.share / sum) * total as f32).round() as i32, s.prominence))
            .collect()
    }
    pub fn summary(&self, total: i32) -> String {
        let mut s = format!("{}: {} section(s) over {total} ({})\n",
                            self.typology, self.sections.len(), self.scale_note);
        for (n, a, p) in self.areas(total) {
            s.push_str(&format!("  {n:<16} {a:>6}  prominence {p:.1}\n"));
        }
        if !self.qualities.is_empty() { s.push_str(&format!("  throughout: {}\n", self.qualities.join(", "))); }
        for c in &self.constraints { s.push_str(&format!("  constraint: {c}\n")); }
        s
    }
}

/// Apply triaged wishes to a typology: this is the step that turns
/// "what the client said" into "what is being built".
/// Apply wishes AND read the scale from the request. The scale is part
/// of what the client said, and reading it anywhere else means the
/// massing carries a number nobody chose.
pub fn apply_for(request: &str, t: &Typology, wishes: &[Wish]) -> Massing {
    let mut m = apply(t, wishes);
    let (total, why) = scale_for(request, t);
    m.implied_total = total;
    m.scale_note = why;
    m
}

pub fn apply(t: &Typology, wishes: &[Wish]) -> Massing {
    let mut sections: Vec<Section> = t.sections.iter()
        .filter(|s| s.need == Need::Essential).cloned().collect();
    let mut m = Massing { typology: t.name, implied_total: t.base_area,
                          scale_note: format!("an ordinary {} is {}", t.name, t.base_area),
                          sections: Vec::new(), constraints: Vec::new(),
                          qualities: Vec::new(), unplaced: Vec::new(), log: Vec::new() };
    for w in wishes {
        match w {
            Wish::Add { section } => {
                if sections.iter().any(|s| &s.name == section) { continue; }
                match t.sections.iter().find(|s| &s.name == section) {
                    Some(s) => { sections.push(s.clone()); m.log.push(format!("added the {section}")); }
                    None => {
                        // invented: a modest section with no special
                        // constraints, because nothing is known about it
                        sections.push(sec(section, Need::Optional, 0.08, 1.0, &["inside"], &[]));
                        m.log.push(format!("added a {section} (not part of a {}, so it is a guess at 8%)", t.name));
                    }
                }
            }
            Wish::Drop { section } => {
                let before = sections.len();
                sections.retain(|s| &s.name != section);
                if sections.len() < before {
                    // Dropping an ESSENTIAL section is allowed and said
                    // loudly. A castle with no wall is a manor house,
                    // and the client may mean that -- but they should
                    // be told what they asked for.
                    let was = t.sections.iter().find(|s| &s.name == section);
                    if was.map(|s| s.need == Need::Essential).unwrap_or(false) {
                        m.log.push(format!("dropped the {section}, which a {} is normally defined by", t.name));
                    } else {
                        m.log.push(format!("dropped the {section}"));
                    }
                }
            }
            Wish::Modify { section, how } => {
                let Some(s) = sections.iter_mut().find(|s| &s.name == section) else {
                    // modifying something not present: add it modified,
                    // because "a huge chapel" plainly wants a chapel
                    if let Some(base) = t.sections.iter().find(|s| &s.name == section) {
                        let mut c = base.clone();
                        scale_by(&mut c, how);
                        m.log.push(format!("added the {section}, {how}"));
                        sections.push(c);
                    }
                    continue;
                };
                scale_by(s, how);
                m.log.push(format!("the {section} is {how}"));
            }
            Wish::Constrain { what } => m.constraints.push(what.clone()),
            Wish::Quality { word } => m.qualities.push(word.clone()),
            Wish::Unplaced { text } => m.unplaced.push(text.clone()),
        }
    }
    m.sections = sections;
    m
}

fn scale_by(s: &mut Section, how: &str) {
    for w in how.split_whitespace() {
        match w {
            "bigger" | "larger" | "wide" | "double" => s.share *= 1.6,
            "huge" | "enormous" | "vast" | "massive" => s.share *= 2.4,
            "small" | "narrow" => s.share *= 0.6,
            "tiny" => s.share *= 0.35,
            "taller" | "higher" => s.prominence *= 1.6,
            "low" => s.prominence *= 0.5,
            "deep" => { s.prominence *= 0.7; s.share *= 1.3; }
            "round" | "square" | "twin" | "half" => s.constraints.push(w.to_string()),
            "ruined" | "broken" | "ancient" | "abandoned" => s.constraints.push("ruined".into()),
            _ => {}
        }
    }
}

/// The schedule a massing implies, as `brief` source: each section's
/// rooms scaled to the area it was given. This is where the layer
/// hands off -- massing decides the sections, the brief schedules
/// inside them.
pub fn to_brief(m: &Massing, total: i32) -> String {
    // The total is what the client HAS: it is the GROSS. Scheduling
    // `total` of net area and then adding circulation put every brief
    // 30% over its own budget by construction, and reported it as a
    // failure of the schedule rather than of this arithmetic.
    const CIRC: f32 = 0.3;
    // CONTINGENCY. Each room's area is rounded up on its own and
    // floored at 4, so a dozen rooms overshoot their target by a few
    // blocks and the brief reports itself over budget by 1 -- true,
    // useless, and indistinguishable from a real overrun. Two percent
    // of headroom absorbs the rounding, which is what a contingency
    // line is for in a real brief.
    const CONTINGENCY: f32 = 0.02;
    let net_total = (total as f32 / (1.0 + CIRC) * (1.0 - CONTINGENCY)).floor() as i32;
    let mut s = format!("# {} -- {} section(s), {net_total} net within {total} gross\n",
                        m.typology, m.sections.len());
    s.push_str(&format!("budget {total}\ncirculation {CIRC}\n"));
    let areas: BTreeMap<String, i32> = m.areas(net_total).into_iter().map(|(n, a, _)| (n, a)).collect();
    for sec in &m.sections {
        let got = areas.get(&sec.name).copied().unwrap_or(0);
        let base: i32 = sec.rooms.iter().map(|(_, a)| *a).sum();
        if sec.rooms.is_empty() || base == 0 {
            s.push_str(&format!("space {} area={got}\n", sec.name));
            continue;
        }
        let f = got as f32 / base as f32;
        for (room, area) in &sec.rooms {
            let a = ((*area as f32 * f).round() as i32).max(4);
            let needs: Vec<&str> = sec.constraints.iter()
                .filter_map(|c| match c.as_str() {
                    "perimeter" => Some("daylight"), "ground" => Some("outside_door"),
                    "tallest" => Some("tall"), _ => None }).collect();
            s.push_str(&format!("space {room} area={a}"));
            if sec.prominence >= 2.0 { s.push_str(&format!(" height={}", sec.prominence.round() as i32)); }
            if !needs.is_empty() { s.push_str(&format!(" needs={}", needs.join(","))); }
            s.push('\n');
        }
    }
    // rooms within a section adjoin; that is what a section IS
    for sec in &m.sections {
        for pair in sec.rooms.windows(2) {
            s.push_str(&format!("adj {} {} should\n", pair[0].0, pair[1].0));
        }
    }
    s
}

/// The massing as a PROGRAM: one pass per section, each honouring its
/// own constraints.
///
/// The constraints were decoration. `encloses`, `tallest`, `perimeter`,
/// `inside`, `axial`, `detached` were parsed, stored, printed -- and
/// nothing read them. Meanwhile `brief::to_program` made ONE box for
/// the whole building and cut rooms out of it, so a castle's curtain
/// wall and its keep were the same block of stone and the wall
/// enclosed nothing.
///
/// One pass per section means the sections exist as named bodies, so
/// flair can attach to them, a review can drop one, and a wall marked
/// `encloses` is actually a ring around the rest.
pub fn to_program(m: &Massing, total: i32, site: i32, wall: &str) -> String {
    let mut s = format!("# {}: {} section(s) over {total}\n", m.typology, m.sections.len());
    let areas: BTreeMap<String, (i32, f32)> = m.areas(total).into_iter()
        .map(|(n, a, p)| (n, (a, p))).collect();
    // the enclosing section sets the site; everything else fits inside
    let half = (site / 2).max(6);
    // sections are built OUTSIDE-IN: what encloses first, so the rest
    // has something to be inside of
    let mut order: Vec<&Section> = m.sections.iter().collect();
    order.sort_by_key(|s| {
        if s.constraints.iter().any(|c| c == "encloses") { 0 }
        else if s.constraints.iter().any(|c| c == "ground") { 1 }
        else if s.constraints.iter().any(|c| c == "tallest") { 3 }
        else { 2 }
    });
    let storey = 5.0;
    // what encloses, if anything: perimeter sections attach to it
    let encloser = m.sections.iter()
        .find(|s| s.constraints.iter().any(|c| c == "encloses"))
        .map(|s| s.name.replace(' ', "_"));
    let encloser = encloser.as_deref();
    for sec in order {
        let (area, prom) = areas.get(&sec.name).copied().unwrap_or((100, 1.0));
        if area < 4 { continue; }
        let h = ((prom.max(0.3) * storey).round() as i32).clamp(2, 120);
        let has = |c: &str| sec.constraints.iter().any(|x| x == c);
        let name = sec.name.replace(' ', "_");
        s.push_str(&format!("pass {} add\n", sec.name));
        if has("encloses") {
            // a ring: the whole site, hollow, so it surrounds
            s.push_str(&format!("  part {name} box w={} h={h} d={} material={wall} hollow\n",
                                half * 2, half * 2));
            s.push_str(&format!("  rel {name} above ground by={}\n", h / 2));
        } else if prom < 0.5 {
            // a courtyard or square: a floor, not a volume
            let side = (area as f32).sqrt().round().max(4.0) as i32;
            s.push_str(&format!("  part {name} slab w={side} d={side} thickness=1 material={wall}\n"));
            s.push_str(&format!("  rel {name} on ground\n"));
        } else {
            let side = ((area as f32 / h.max(1) as f32 * storey).sqrt()).round().max(4.0) as i32;
            s.push_str(&format!("  part {name} box w={side} h={h} d={side} material={wall} hollow\n"));
            // where it sits follows from its constraints
            if has("perimeter") {
                // IN the enclosing wall, not beside the ground. `beside
                // ground` keeps the ground's own height, so a gatehouse
                // sank half into it and came out shorter than the wall
                // it is meant to command. Relating it to the enclosure
                // puts it at the wall's own height, which is also what
                // a gatehouse IS -- a thickening of the wall, not a
                // building that happens to be near one.
                match encloser {
                    Some(e) => s.push_str(&format!("  rel {name} beside {e} dir=south by=-{}\n",
                                                   side / 2)),
                    None => s.push_str(&format!("  rel {name} above ground by={}\n", h / 2)),
                }
            } else {
                s.push_str(&format!("  rel {name} above ground by={}\n", h / 2));
            }
        }
        s.push_str("end\n");
    }
    s
}

/// The prompt for the massing turn -- the one that was missing.
pub fn massing_prompt(request: &str, t: &Typology, wishes: &[Wish]) -> String {
    let mut s = format!("\
The player asked for: {request}

A {} is: {}

It divides into these sections. The essential ones are what make it that thing:
", t.name, t.summary);
    for sec in &t.sections {
        s.push_str(&format!("  {:<14} {:<10} {:.0}% of the whole{}\n", sec.name,
                            if sec.need == Need::Essential { "ESSENTIAL" } else { "optional" },
                            sec.share * 100.0,
                            if sec.constraints.is_empty() { String::new() }
                            else { format!(" [{}]", sec.constraints.join(", ")) }));
    }
    s.push_str("\nWhat the player said, sorted:\n");
    for w in wishes {
        s.push_str(&format!("  {:<10} {}\n", w.kind(), match w {
            Wish::Modify { section, how } => format!("{section}: {how}"),
            Wish::Add { section } | Wish::Drop { section } => section.clone(),
            Wish::Constrain { what } => what.clone(),
            Wish::Quality { word } => word.clone(),
            Wish::Unplaced { text } => format!("{text}  <- could not place this"),
        }));
    }
    s.push_str("\nAdjust the division if it is wrong for what was asked. Reply with ONLY:\n\
                section NAME share=N prominence=N [constraints=a,b]\n\
                total N            the whole floor area\n\
                drop NAME\n\n\
                Shares are relative and are normalised, so they need not sum to anything.\n\
                Change what the player's words require and leave the rest alone.\n\nReply:\n");
    s
}

/// Parse the massing turn's reply back over a typology.
pub fn parse_massing(src: &str, base: &Massing) -> (Massing, i32, Vec<String>) {
    let mut m = base.clone();
    let mut total = 0;
    let mut errs = Vec::new();
    for line in src.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') { continue; }
        let w: Vec<&str> = t.split_whitespace().collect();
        match w.first().map(|v| v.to_lowercase()).as_deref() {
            Some("total") => match w.get(1).and_then(|v| v.parse().ok()) {
                Some(v) => total = v, None => errs.push(format!("total needs a number: {t:?}")) },
            Some("drop") => { let n = w[1..].join(" ").to_lowercase(); m.sections.retain(|s| s.name != n); }
            Some("section") => {
                let kv_at = w.iter().position(|s| s.contains('=')).unwrap_or(w.len());
                if kv_at <= 1 { errs.push(format!("section needs a name: {t:?}")); continue; }
                let name = w[1..kv_at].join(" ").to_lowercase();
                let kv = |k: &str| w[kv_at..].iter().find_map(|s| s.strip_prefix(&format!("{k}=")));
                let share = kv("share").and_then(|v| v.parse::<f32>().ok());
                let prom = kv("prominence").and_then(|v| v.parse::<f32>().ok());
                let cons: Vec<String> = kv("constraints").map(|v| v.split(',')
                    .map(|c| c.trim().to_lowercase()).filter(|c| !c.is_empty()).collect()).unwrap_or_default();
                match m.sections.iter_mut().find(|s| s.name == name) {
                    Some(s) => {
                        if let Some(v) = share { s.share = v.clamp(0.01, 10.0); }
                        if let Some(v) = prom { s.prominence = v.clamp(0.0, 12.0); }
                        if !cons.is_empty() { s.constraints = cons; }
                    }
                    None => m.sections.push(sec(&name, Need::Optional, share.unwrap_or(0.08),
                                                prom.unwrap_or(1.0),
                                                &cons.iter().map(|s| s.as_str()).collect::<Vec<_>>(), &[])),
                }
            }
            _ => {}
        }
    }
    if total <= 0 {
        total = base.implied_total.max(1);
        errs.push(format!("no total given; used the {} implied by the request", total));
    }
    (m, total, errs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typology_says_what_a_thing_is_and_what_is_merely_common() {
        let c = lookup("castle").expect("castles are a type");
        let ess: Vec<&str> = c.sections.iter().filter(|s| s.need == Need::Essential)
            .map(|s| s.name.as_str()).collect();
        assert!(ess.contains(&"curtain wall") && ess.contains(&"keep") && ess.contains(&"gatehouse"),
                "a castle without these is not one: {ess:?}");
        assert!(c.sections.iter().any(|s| s.name == "chapel" && s.need == Need::Optional),
                "and a chapel is common, not required");
        // the shares are not equal, because a keep is not a gatehouse
        let keep = c.sections.iter().find(|s| s.name == "keep").unwrap().share;
        let gate = c.sections.iter().find(|s| s.name == "gatehouse").unwrap().share;
        assert!(keep > gate * 2.0, "a keep dwarfs a gatehouse: {keep} vs {gate}");
        assert!(lookup("a great castle on a hill").is_some(), "found inside a sentence");
        assert!(lookup("zigurat").is_none());
    }

    #[test]
    fn client_wishes_sort_into_four_different_kinds_of_thing() {
        let c = lookup("castle").unwrap();
        let w = triage("a forbidding castle with a huge keep and a chapel, on a cliff", &c);
        let kinds: Vec<&str> = w.iter().map(|x| x.kind()).collect();
        assert!(kinds.contains(&"modify"), "{w:?}");
        assert!(kinds.contains(&"add"), "{w:?}");
        assert!(kinds.contains(&"constrain"), "{w:?}");
        assert!(kinds.contains(&"quality"), "{w:?}");
        // and each landed in the RIGHT one
        assert!(w.iter().any(|x| matches!(x, Wish::Modify { section, how }
            if section == "keep" && how.contains("huge"))), "{w:?}");
        assert!(w.iter().any(|x| matches!(x, Wish::Add { section } if section == "chapel")), "{w:?}");
        assert!(w.iter().any(|x| matches!(x, Wish::Constrain { what } if what.contains("cliff"))), "{w:?}");
        assert!(w.iter().any(|x| matches!(x, Wish::Quality { word } if word == "forbidding")), "{w:?}");
    }

    #[test]
    fn a_section_name_beats_a_quality_so_the_section_is_not_lost() {
        let c = lookup("castle").unwrap();
        // "a forbidding gatehouse" is about the gatehouse; reading it
        // as a quality would drop the gatehouse entirely
        match triage_one("a forbidding gatehouse", &c) {
            Wish::Modify { section, .. } => assert_eq!(section, "gatehouse"),
            o => panic!("{o:?}"),
        }
        // and "without a chapel" is a drop, not an add, though it names one
        match triage_one("without a chapel", &c) {
            Wish::Drop { section } => assert_eq!(section, "chapel"),
            o => panic!("{o:?}"),
        }
        // the longest section name wins: "curtain wall" not "wall"
        let t = lookup("town").unwrap();
        assert!(matches!(triage_one("with big walls", &t), Wish::Modify { .. } | Wish::Add { .. }));
    }

    #[test]
    fn applying_wishes_keeps_the_essentials_and_adds_what_was_asked() {
        let c = lookup("castle").unwrap();
        let m = apply(&c, &triage("a castle with a huge keep and a chapel", &c));
        let names: Vec<&str> = m.sections.iter().map(|s| s.name.as_str()).collect();
        // the four essentials are all there without being asked for
        for e in ["curtain wall", "keep", "gatehouse", "courtyard"] {
            assert!(names.contains(&e), "{e} is essential: {names:?}");
        }
        assert!(names.contains(&"chapel"), "and the chapel was asked for");
        assert!(!names.contains(&"stables"), "but not the stables, which were not");
        // the keep really did grow
        let keep = m.sections.iter().find(|s| s.name == "keep").unwrap();
        assert!(keep.share > c.sections.iter().find(|s| s.name == "keep").unwrap().share);
        // and the areas divide the total
        let areas = m.areas(4000);
        let sum: i32 = areas.iter().map(|(_, a, _)| a).sum();
        assert!((sum - 4000).abs() < 10, "the shares divide the whole: {sum}");
        assert!(areas.iter().find(|(n, _, _)| n == "keep").unwrap().1
                > areas.iter().find(|(n, _, _)| n == "gatehouse").unwrap().1 * 3);
    }

    #[test]
    fn dropping_something_essential_is_allowed_and_said_loudly() {
        let c = lookup("castle").unwrap();
        let m = apply(&c, &triage("a castle without a curtain wall", &c));
        assert!(!m.sections.iter().any(|s| s.name == "curtain wall"));
        assert!(m.log.iter().any(|l| l.contains("normally defined by")),
                "the client is told what they asked for: {:?}", m.log);
    }

    #[test]
    fn an_invented_section_is_added_and_flagged_as_a_guess() {
        let c = lookup("castle").unwrap();
        let m = apply(&c, &[Wish::Add { section: "menagerie".into() }]);
        assert!(m.sections.iter().any(|s| s.name == "menagerie"));
        assert!(m.log.iter().any(|l| l.contains("not part of a castle") && l.contains("guess")),
                "{:?}", m.log);
    }

    #[test]
    fn nothing_a_client_said_is_silently_dropped() {
        let c = lookup("castle").unwrap();
        let w = triage("a castle with flumadiddle", &c);
        let m = apply(&c, &w);
        // it either became a section or is reported unplaced, never lost
        assert!(m.sections.iter().any(|s| s.name == "flumadiddle") || !m.unplaced.is_empty(),
                "{w:?} -> {:?} / {:?}", m.sections.iter().map(|s| &s.name).collect::<Vec<_>>(), m.unplaced);
    }

    #[test]
    fn a_massing_becomes_a_brief_that_reconciles_and_lays_out() {
        let c = lookup("castle").unwrap();
        let m = apply(&c, &triage("a castle with a huge keep and a chapel, on a cliff", &c));
        let src = to_brief(&m, 4000);
        let (b, errs) = crate::brief::parse(&src);
        assert!(errs.is_empty(), "{src}\n{errs:?}");
        assert!(b.spaces.len() >= 8, "every section brought its rooms: {}", b.spaces.len());
        assert!(b.spaces.iter().any(|s| s.name == "great hall"), "{:?}",
                b.spaces.iter().map(|s| &s.name).collect::<Vec<_>>());
        // the keep's rooms are big, because the keep got a big share
        let hall = b.spaces.iter().find(|s| s.name == "great hall").unwrap();
        let gate = b.spaces.iter().find(|s| s.name == "gate passage").unwrap();
        assert!(hall.area > gate.area * 2, "{} vs {}", hall.area, gate.area);
        let r = b.reconcile();
        assert!(r.net > 0);
        assert!(r.ok, "a brief from a massing fits its own budget: {}", r.summary());
        let l = crate::brief::lay_out(&b, ((-40, -40), (40, 40)), 5, 30);
        assert!(l.rooms.len() >= 5, "{}", l.summary());
    }

    #[test]
    fn the_massing_prompt_shows_the_division_and_the_sorted_wishes() {
        let c = lookup("castle").unwrap();
        let w = triage("a castle with a huge keep, on a cliff", &c);
        let p = massing_prompt("a castle with a huge keep, on a cliff", &c, &w);
        assert!(p.contains("ESSENTIAL") && p.contains("optional"));
        assert!(p.contains("modify") && p.contains("constrain"), "the sorting is shown: {p}");
        assert!(p.contains("% of the whole"));
        assert!(p.contains("leave the rest alone"), "it asks for a diff, not a rewrite");
        // and a reply adjusts the massing rather than replacing it
        let m = apply(&c, &w);
        let (m2, total, errs) = parse_massing("total 6000\nsection keep share=0.5 prominence=4\ndrop courtyard\n", &m);
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(total, 6000);
        assert_eq!(m2.sections.iter().find(|s| s.name == "keep").unwrap().share, 0.5);
        assert!(!m2.sections.iter().any(|s| s.name == "courtyard"));
        assert!(m2.sections.iter().any(|s| s.name == "gatehouse"), "the rest is left alone");
        // a reply with no total falls back to what the REQUEST implied,
        // not to a hardcoded number nobody chose
        let (_, t3, e3) = parse_massing("section keep share=0.4\n", &m);
        assert_eq!(t3, m.implied_total);
        assert!(e3.iter().any(|e| e.contains("implied by the request")), "{e3:?}");
    }

    #[test]
    fn every_typology_divides_into_something_buildable() {
        for t in typologies() {
            let ess = t.sections.iter().filter(|s| s.need == Need::Essential).count();
            assert!(ess >= 3, "{} has only {ess} essential section(s)", t.name);
            let share: f32 = t.sections.iter().filter(|s| s.need == Need::Essential)
                .map(|s| s.share).sum();
            assert!((share - 1.0).abs() < 0.15, "{}: essentials share {share}, should be about 1", t.name);
            let m = apply(&t, &[]);
            let src = to_brief(&m, 3000);
            let (b, errs) = crate::brief::parse(&src);
            assert!(errs.is_empty(), "{}: {src}\n{errs:?}", t.name);
            assert!(!b.spaces.is_empty(), "{} schedules nothing", t.name);
            let r = b.reconcile();
            assert!(r.net > 0);
            assert!(r.ok, "{}: {}", t.name, r.summary());
        }
    }

    #[test]
    fn a_huge_fortress_is_really_huge() {
        let c = lookup("castle").unwrap();
        let plain = scale_for("a castle", &c).0;
        let huge = scale_for("a huge fortress", &c).0;
        let vast = scale_for("a vast fortress", &c).0;
        let small = scale_for("a small castle", &c).0;
        assert_eq!(plain, 9000, "an ordinary castle is already substantial");
        assert!(huge >= 40_000, "a huge fortress is {huge}, and should be tens of thousands");
        assert!(vast > huge && small < plain, "{small} < {plain} < {huge} < {vast}");
        // THE TYPE SETS THE REGISTER: a huge house is not a huge castle
        let h = lookup("house").unwrap();
        assert!(scale_for("a huge house", &h).0 < scale_for("a castle", &c).0,
                "a huge house is smaller than an ordinary castle");
        // and the reason is reported, because a number with no reason
        // is a number nobody can argue with
        assert!(scale_for("a huge fortress", &c).1.contains("4.5x"), "{}", scale_for("a huge fortress", &c).1);
    }

    #[test]
    fn an_explicit_area_beats_a_scale_word() {
        let c = lookup("castle").unwrap();
        assert_eq!(scale_for("a small castle of 30000", &c).0, 30000);
        assert!(scale_for("a castle of 30000", &c).1.contains("outright"));
        // but a small number in passing is not an area
        assert_eq!(scale_for("a castle with 4 towers", &c).0, 9000);
    }

    #[test]
    fn the_scale_word_reaches_the_massing_and_the_brief() {
        let c = lookup("castle").unwrap();
        let req = "a huge fortress with a chapel";
        let m = apply_for(req, &c, &triage(req, &c));
        assert!(m.implied_total >= 40_000, "{}", m.summary(m.implied_total));
        // it is not ALSO counted as a modification of some section
        assert!(!m.log.iter().any(|l| l.contains("huge")), "no double counting: {:?}", m.log);
        // and the brief it produces is that big
        let (b, errs) = crate::brief::parse(&to_brief(&m, m.implied_total));
        assert!(errs.is_empty(), "{errs:?}");
        let r = b.reconcile();
        assert!(r.gross > 39_000, "the schedule really is that size: {}", r.summary());
        assert!(r.ok, "{}", r.summary());
        // every room grew with it, rather than there being more rooms
        let hall = b.spaces.iter().find(|s| s.name == "great hall").unwrap();
        let small = crate::brief::parse(&to_brief(&apply_for("a small castle", &c, &[]), 3600)).0;
        let small_hall = small.spaces.iter().find(|s| s.name == "great hall").unwrap();
        assert!(hall.area > small_hall.area * 4, "{} vs {}", hall.area, small_hall.area);
    }

    #[test]
    fn the_words_people_actually_use_find_a_type() {
        // "a huge fortress" found NOTHING and the whole chain fell over
        for (said, want) in [("a huge fortress", "castle"), ("a citadel on a hill", "castle"),
                             ("a great cathedral", "temple"), ("a fishing village", "town"),
                             ("a small cottage", "house"), ("a lighthouse", "tower"),
                             ("a stone keep", "castle"), ("a bustling city", "town")] {
            assert_eq!(lookup(said).map(|t| t.name), Some(want), "{said}");
        }
        // the head noun wins: "a temple town" is a town
        assert_eq!(lookup("a temple town").map(|t| t.name), Some("town"));
        assert_eq!(lookup("a town temple").map(|t| t.name), Some("temple"));
        assert!(lookup("a zigurat").is_none(), "and an unknown word is still unknown");
    }

    #[test]
    fn each_section_becomes_its_own_named_body() {
        let c = lookup("castle").unwrap();
        let req = "a castle with a chapel";
        let m = apply_for(req, &c, &triage(req, &c));
        let src = to_program(&m, m.implied_total, 90, "stone");
        let (prog, errs) = crate::program::parse(&src);
        assert!(errs.is_empty(), "{src}\n{errs:?}");
        let r = crate::program::run(&prog, (0, 64, 0));
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        // the sections are separate bodies, not one block of stone
        for want in ["curtain_wall", "keep", "gatehouse", "chapel"] {
            assert!(r.bodies.contains_key(want), "{want} is its own body: {:?}",
                    r.bodies.keys().collect::<Vec<_>>());
        }
        // so flair can attach to one of them by name
        assert!(r.bodies["curtain_wall"].len() > 100);
    }

    #[test]
    fn the_constraints_are_read_and_not_merely_stored() {
        let c = lookup("castle").unwrap();
        let m = apply_for("a castle", &c, &[]);
        let src = to_program(&m, m.implied_total, 90, "stone");
        let (prog, _) = crate::program::parse(&src);
        let r = crate::program::run(&prog, (0, 64, 0));
        let span = |b: &str| {
            let s = &r.bodies[b];
            (s.iter().map(|c| c.0).max().unwrap() - s.iter().map(|c| c.0).min().unwrap(),
             s.iter().map(|c| c.1).max().unwrap())
        };
        // ENCLOSES: the curtain wall is wider than everything it holds
        let (wall_w, _) = span("curtain_wall");
        let (keep_w, keep_top) = span("keep");
        assert!(wall_w > keep_w, "the wall encloses the keep: {wall_w} vs {keep_w}");
        // TALLEST: the keep dominates, which is what a keep is
        let (_, gate_top) = span("gatehouse");
        assert!(keep_top > gate_top, "the keep is taller than the gatehouse: {keep_top} vs {gate_top}");
        // and a gatehouse COMMANDS the wall it sits in rather than
        // sinking into the ground beside it
        let (_, wall_top) = span("curtain_wall");
        assert!(gate_top >= wall_top, "the gatehouse rises to the wall: {gate_top} vs {wall_top}");
        // and the courtyard, prominence 0, is a floor rather than a volume
        let court = &r.bodies["courtyard"];
        let ch = court.iter().map(|c| c.1).max().unwrap() - court.iter().map(|c| c.1).min().unwrap();
        assert!(ch <= 2, "a courtyard is a surface, not a box: {ch} tall");
    }

    #[test]
    fn what_encloses_is_built_first_so_the_rest_has_something_to_be_inside() {
        let c = lookup("castle").unwrap();
        let m = apply_for("a castle", &c, &[]);
        let (prog, _) = crate::program::parse(&to_program(&m, m.implied_total, 90, "stone"));
        let names: Vec<&str> = prog.passes.iter().map(|p| p.name.as_str()).collect();
        let wall = names.iter().position(|n| *n == "curtain wall").unwrap();
        let keep = names.iter().position(|n| *n == "keep").unwrap();
        assert!(wall < keep, "outside-in: {names:?}");
    }
}
