//! Let the model ask, instead of guessing.
//!
//! A model choosing `water` for a wall does not know it will flow
//! away. Choosing `glass` for a floor does not know you can see
//! through it, `ice` that it melts near a torch, `sand` that it falls,
//! `glowstone` that it lights the room. All of that is in the block
//! table and none of it was ever visible to the thing making the
//! decision. The prompt said "material=BLOCK" and left the rest to
//! whatever the model remembered about a different game.
//!
//! So: a QUERY the model can make mid-turn, answered in JSON.
//!
//!   {"q":"block","name":"water"}
//!   -> {"name":"water","family":"Liquid","flows":true,"solid":false,
//!       "hardness":0.0,"transparent":true,"light":0,
//!       "warning":"flows away from where you place it"}
//!
//!   {"q":"like","of":"deepslate"}     what this world has instead
//!   {"q":"for","want":"dark and hard"} what to use for a purpose
//!   {"q":"try","block":"sand","on":"air"}  what HAPPENS if you do
//!
//! The last one is the interesting one and is why this is a probe and
//! not a lookup table. "What happens if I put sand here" has an answer
//! -- it falls -- and a model that can ask stops building floating
//! sand. `simulate` runs the small handful of behaviours that make a
//! block surprising, and reports them in the words a builder would
//! use rather than as flags.
//!
//! WHY JSON AND NOT PROSE
//!
//! A small model asked to write Python produced three different
//! spellings of one variable in twelve lines. It cannot be trusted to
//! write code, but it can be trusted to fill in a two-key object, and
//! a malformed one is detectable and answerable with a correction
//! rather than a crash. The queries are deliberately flat: no nesting,
//! no arrays, no optional structure.

use crate::palette::{self, Family, Finish};

/// What a block does that a name does not say.
#[derive(Debug, Clone, PartialEq)]
pub struct Behaviour {
    pub solid: bool,
    /// Spreads and drains away from where it is placed.
    pub flows: bool,
    /// Falls when nothing is under it.
    pub falls: bool,
    /// You can see through it.
    pub transparent: bool,
    /// Light it gives off, 0-15.
    pub light: u8,
    /// Melts, burns or otherwise does not stay put in the wrong place.
    pub fragile: Option<&'static str>,
    /// The one thing a builder must know before using it.
    pub warning: Option<&'static str>,
}

/// The behaviours that make a block surprising. Only the ones that
/// change a BUILD decision are here -- a model does not need to know
/// the tool tier, it needs to know the wall will drain away.
pub fn behaviour(name: &str) -> Behaviour {
    let b = palette::by_name(name);
    let fam = b.map(|x| x.family).unwrap_or(Family::Stone);
    let fin = b.map(|x| x.finish).unwrap_or(Finish::Rough);
    let mut r = Behaviour {
        solid: !matches!(fam, Family::Liquid | Family::Air),
        flows: fam == Family::Liquid,
        falls: matches!(name, "sand" | "gravel" | "snow"),
        transparent: fin == Finish::Translucent || fam == Family::Air,
        light: match name { "glowstone" => 15, "lava" => 15, "torch" => 14, _ => 0 },
        fragile: None,
        warning: None,
    };
    r.fragile = match name {
        "ice" => Some("melts near light or lava"),
        "snow" => Some("melts in warm climates"),
        "leaves" => Some("decays away from a trunk"),
        "lava" => Some("sets fire to what it touches"),
        _ => None,
    };
    r.warning = match name {
        "water" | "lava" => Some("flows away from where you place it; it will not hold a shape \
                                  unless something contains it"),
        "sand" | "gravel" => Some("falls when nothing is under it; do not use it for anything \
                                   that overhangs"),
        "snow" => Some("falls, and melts in a warm climate"),
        "glass" | "ice" => Some("you can see through it, so whatever is behind it is part of \
                                 the design"),
        "glowstone" => Some("lights everything around it; a wall of it is a lamp"),
        "leaves" => Some("decays unless a trunk is near"),
        _ => None,
    };
    r
}

fn esc(s: &str) -> String { s.replace('\\', "\\\\").replace('"', "\\\"") }

fn block_json(name: &str) -> String {
    let Some(b) = palette::by_name(name) else {
        return format!("{{\"error\":\"no block called {}\",\"try\":\"{{\\\"q\\\":\\\"like\\\",\\\"of\\\":\\\"{}\\\"}}\"}}",
                       esc(name), esc(name));
    };
    let h = behaviour(name);
    let mut s = format!("{{\"name\":\"{}\",\"family\":\"{:?}\",\"finish\":\"{:?}\",\
                          \"colour\":[{},{},{}],\"hardness\":{:.1},\"solid\":{},\"flows\":{},\
                          \"falls\":{},\"transparent\":{},\"light\":{},\"uses\":\"{}\"",
                        b.name, b.family, b.finish, b.rgb.0, b.rgb.1, b.rgb.2, b.hardness,
                        h.solid, h.flows, h.falls, h.transparent, h.light, esc(b.uses));
    if let Some(f) = h.fragile { s.push_str(&format!(",\"fragile\":\"{}\"", esc(f))); }
    if let Some(w) = h.warning { s.push_str(&format!(",\"warning\":\"{}\"", esc(w))); }
    s.push('}');
    s
}

/// What happens if you place `block` where there is `on`.
///
/// The point of the whole module: "what happens if I put sand here"
/// has an answer, and a model that can ask stops building floating
/// sand. This is a small, honest simulation of the handful of
/// behaviours that surprise a builder -- not a physics engine.
pub fn simulate(block: &str, on: &str) -> String {
    let h = behaviour(block);
    let under = behaviour(on);
    let mut what: Vec<String> = Vec::new();
    let mut ok = true;
    if palette::by_name(block).is_none() {
        return format!("{{\"error\":\"no block called {}\"}}", esc(block));
    }
    if h.falls && (on == "air" || !under.solid) {
        what.push("it falls until it lands on something solid".into());
        ok = false;
    }
    if h.flows {
        what.push("it spreads outward and downward, and will not hold a shape".into());
        ok = false;
    }
    if h.flows && on == "lava" {
        what.push("water on lava makes stone".into());
    }
    if block == "lava" && palette::by_name(on).is_some_and(|b| b.family == Family::Wood) {
        what.push("it sets the wood alight".into());
        ok = false;
    }
    if block == "ice" && under.light > 0 {
        what.push("it melts: the block under it gives off light".into());
        ok = false;
    }
    if h.light > 0 {
        what.push(format!("it lights everything within about {} blocks", h.light));
    }
    if h.transparent {
        what.push("you can see through it".into());
    }
    if !h.solid && !h.flows {
        what.push("nothing is placed: this is not a material".into());
        ok = false;
    }
    if what.is_empty() { what.push("it stays where you put it".into()); }
    format!("{{\"block\":\"{}\",\"on\":\"{}\",\"holds\":{},\"what\":\"{}\"{}}}",
            esc(block), esc(on), ok, esc(&what.join("; ")),
            if ok { String::new() } else {
                let alt = palette::nearest_to(
                    palette::by_name(block).map(|b| b.rgb).unwrap_or((128, 128, 128)),
                    2.0, Family::Stone, Finish::Rough);
                format!(",\"instead\":\"{}\"", alt.name)
            })
}

/// Answer one query. Never fails: a malformed query gets an
/// explanation and an example, because a model that cannot parse the
/// error will at least see a template it can copy.
pub fn answer(query: &str) -> String {
    let get = |key: &str| -> Option<String> {
        // A hand-rolled reader, because the queries are flat by design
        // and a dependency for `{"q":"block","name":"water"}` is not
        // worth it. Finds "key" then the next quoted value after the
        // colon.
        let pat = format!("\"{key}\"");
        let at = query.find(&pat)? + pat.len();
        let rest = &query[at..];
        let colon = rest.find(':')?;
        let after = &rest[colon + 1..];
        let s = after.find('"')?;
        let e = after[s + 1..].find('"')?;
        Some(after[s + 1..s + 1 + e].to_lowercase())
    };
    let Some(q) = get("q") else {
        return "{\"error\":\"no q\",\"examples\":[\"{\\\"q\\\":\\\"block\\\",\\\"name\\\":\\\"water\\\"}\",\
                \"{\\\"q\\\":\\\"for\\\",\\\"want\\\":\\\"dark and hard\\\"}\",\
                \"{\\\"q\\\":\\\"try\\\",\\\"block\\\":\\\"sand\\\",\\\"on\\\":\\\"air\\\"}\"]}".into();
    };
    match q.as_str() {
        "block" | "what" => match get("name") {
            Some(n) => block_json(&n),
            None => "{\"error\":\"block needs a name\"}".into(),
        },
        "like" | "nearest" => match get("of") {
            Some(n) => {
                let (id, why) = palette::resolve_or_suggest(&n);
                let name = palette::by_id(id).map(|b| b.name).unwrap_or("stone");
                format!("{{\"asked\":\"{}\",\"use\":\"{}\",\"why\":\"{}\"}}", esc(&n), name,
                        esc(&why.unwrap_or_else(|| "it exists here".into())))
            }
            None => "{\"error\":\"like needs an of\"}".into(),
        },
        "for" | "suggest" => match get("want") {
            Some(w) => {
                let found = palette::suggest(&w, 3);
                if found.is_empty() {
                    format!("{{\"want\":\"{}\",\"found\":[],\"note\":\"nothing matched; describe a \
                             colour, a hardness or what it is made of\"}}", esc(&w))
                } else {
                    format!("{{\"want\":\"{}\",\"found\":[{}]}}", esc(&w),
                            found.iter().map(|(b, why)| format!("{{\"name\":\"{}\",\"why\":\"{}\"}}",
                                b.name, esc(why))).collect::<Vec<_>>().join(","))
                }
            }
            None => "{\"error\":\"for needs a want\"}".into(),
        },
        "try" | "simulate" | "place" => {
            let b = get("block").unwrap_or_default();
            let on = get("on").unwrap_or_else(|| "stone".into());
            if b.is_empty() { return "{\"error\":\"try needs a block\"}".into(); }
            simulate(&b, &on)
        }
        "list" | "blocks" => format!("{{\"blocks\":[{}]}}",
            palette::BLOCKS.iter().filter(|b| b.family != Family::Air)
                .map(|b| format!("\"{}\"", b.name)).collect::<Vec<_>>().join(",")),
        other => format!("{{\"error\":\"no query called {}\",\"queries\":[\"block\",\"like\",\
                          \"for\",\"try\",\"list\"]}}", esc(other)),
    }
}

/// The lines that teach a model to use this. Deliberately short: a
/// small model given a long tool description uses none of it.
pub fn prompt_lines() -> &'static str {
    "\
BEFORE choosing a material you may ASK. Write one line starting with ASK and a JSON object,
and the answer comes back before you plan:

  ASK {\"q\":\"block\",\"name\":\"water\"}        what it is and what it does
  ASK {\"q\":\"for\",\"want\":\"dark and hard\"}  what to use for a purpose
  ASK {\"q\":\"like\",\"of\":\"deepslate\"}       what this world has instead
  ASK {\"q\":\"try\",\"block\":\"sand\",\"on\":\"air\"}  what HAPPENS if you place it
  ASK {\"q\":\"list\"}                          every block here

Ask when you are about to use a material you are not sure of. Water flows away, sand falls,
ice melts, glass is see-through -- and a wall made of any of them is not a wall."
}

/// Pull `ASK {...}` lines out of a reply and answer them. Returns the
/// answers and the reply with the asks removed, so a model that asks
/// AND plans in one turn gets both.
pub fn take_asks(reply: &str) -> (Vec<(String, String)>, String) {
    let mut asks = Vec::new();
    let mut rest = String::new();
    for line in reply.lines() {
        let t = line.trim();
        let is_ask = t.len() > 3 && t[..3].eq_ignore_ascii_case("ask")
            && t[3..].trim_start().starts_with('{');
        if is_ask {
            let q = t[3..].trim().to_string();
            let a = answer(&q);
            asks.push((q, a));
        } else {
            rest.push_str(line);
            rest.push('\n');
        }
    }
    (asks, rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_says_what_it_does_not_only_what_it_is() {
        let w = answer("{\"q\":\"block\",\"name\":\"water\"}");
        assert!(w.contains("\"flows\":true"), "{w}");
        assert!(w.contains("\"solid\":false"));
        assert!(w.contains("will not hold a shape"), "the warning is the point: {w}");
        let s = answer("{\"q\":\"block\",\"name\":\"sand\"}");
        assert!(s.contains("\"falls\":true") && s.contains("overhangs"), "{s}");
        let g = answer("{\"q\":\"block\",\"name\":\"glowstone\"}");
        assert!(g.contains("\"light\":15") && g.contains("is a lamp"), "{g}");
        let st = answer("{\"q\":\"block\",\"name\":\"stone\"}");
        assert!(st.contains("\"solid\":true") && !st.contains("warning"),
                "an ordinary block has no warning: {st}");
    }

    #[test]
    fn trying_a_block_answers_what_happens() {
        // the question that makes this a probe and not a table
        let s = simulate("sand", "air");
        assert!(s.contains("\"holds\":false") && s.contains("falls until"), "{s}");
        assert!(s.contains("\"instead\""), "and it offers something that would hold: {s}");
        let w = simulate("water", "stone");
        assert!(w.contains("\"holds\":false") && w.contains("spreads"), "{w}");
        let ok = simulate("stone", "stone");
        assert!(ok.contains("\"holds\":true") && ok.contains("stays where you put it"), "{ok}");
        // and the interesting interactions
        assert!(simulate("lava", "oak_planks").contains("alight"));
        assert!(simulate("ice", "glowstone").contains("melts"));
        assert!(simulate("glass", "stone").contains("see through"));
    }

    #[test]
    fn a_malformed_query_gets_a_template_rather_than_a_crash() {
        // a small model that cannot parse the error will at least see
        // something it can copy
        for bad in ["", "{}", "not json at all", "{\"q\":\"frobnicate\"}",
                    "{\"q\":\"block\"}", "{\"q\":\"try\"}"] {
            let a = answer(bad);
            assert!(a.starts_with('{') && a.ends_with('}'), "{bad:?} -> {a}");
            assert!(a.contains("error"), "{bad:?} -> {a}");
        }
        assert!(answer("").contains("examples"), "and the empty one shows how");
        assert!(answer("{\"q\":\"frobnicate\"}").contains("\"queries\""));
        // an unknown block name points at the query that would help
        let u = answer("{\"q\":\"block\",\"name\":\"deepslate\"}");
        assert!(u.contains("no block called deepslate") && u.contains("like"), "{u}");
    }

    #[test]
    fn asking_and_planning_in_one_turn_gives_both() {
        let (asks, rest) = take_asks(
            "ASK {\"q\":\"try\",\"block\":\"water\",\"on\":\"air\"}\n\
             pass a add\n\
             ASK {\"q\":\"for\",\"want\":\"something that glows\"}\n\
               part x box w=4 material=stone\n\
             end\n");
        assert_eq!(asks.len(), 2);
        assert!(asks[0].1.contains("spreads"), "{:?}", asks[0]);
        assert!(asks[1].1.contains("glowstone"), "{:?}", asks[1]);
        // and the plan survives with the asks stripped out
        assert!(rest.contains("pass a add") && rest.contains("part x"));
        assert!(!rest.contains("ASK"));
        let (prog, errs) = crate::program::parse(&rest);
        assert!(errs.is_empty(), "{rest}\n{errs:?}");
        assert_eq!(prog.passes.len(), 1);
    }

    #[test]
    fn the_queries_that_already_had_answers_are_reachable_from_here() {
        let l = answer("{\"q\":\"like\",\"of\":\"deepslate\"}");
        assert!(l.contains("\"use\""), "{l}");
        let f = answer("{\"q\":\"for\",\"want\":\"pale and fine for trim\"}");
        assert!(f.contains("quartz"), "{f}");
        let all = answer("{\"q\":\"list\"}");
        assert!(all.contains("obsidian") && all.contains("glass") && !all.contains("\"air\""));
    }

    #[test]
    fn the_prompt_is_short_enough_that_a_small_model_reads_it() {
        let p = prompt_lines();
        assert!(p.len() < 700, "{} chars", p.len());
        assert!(p.contains("Water flows away, sand falls"), "it says WHY to ask");
        // every example in it actually works
        for line in p.lines().filter(|l| l.trim_start().starts_with("ASK ")) {
            let q = line.trim().trim_start_matches("ASK ").split("  ").next().unwrap().trim();
            let a = answer(q);
            assert!(!a.contains("\"error\""), "{q} -> {a}");
        }
    }
}
