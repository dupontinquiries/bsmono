//! `--arch` — the archaeology layer, driven from a terminal.
//!
//! Same role `campsim`/`raidsim` play for their systems: the whole thing
//! runnable with no window and no GPU, narrating its own reasoning, so the
//! content can be argued with before any of it is drawn. The map in
//! particular is a thing you have to READ to know whether it is any good —
//! whether the ladder moves at the right pace, whether a false lead reads
//! as tempting, whether the Asteri reveal lands — and none of that is
//! visible from a passing test.
//!
//! ```text
//! --arch map                 the archaeological map at your current dig
//! --arch strata              the same, grouped by how deep it goes
//! --arch site <dest>         what is still findable at a destination
//! --arch find <id>           record a find (writes the save)
//! --arch why <claim>         the citation list behind one claim's standing
//! --arch orbit               the Asteri return, computed rather than asserted
//! --arch audit               content checks: orphans, gates, dead ends
//! --arch reset               forget every find
//! ```
//!
//! Finds persist through `save.rs`'s `finds` line, so a dig built here is
//! the same dig the game reads.

use crate::archive::{self, Lens, Stratum, CLAIMS, FINDINGS, RECEIVED};
use crate::provenance::Bearing;
use crate::campaign::CampaignLog;
use crate::orbits;
use crate::groundside::{self, Era};
use crate::patrol;
use crate::provenance::{assay, assay_with, unfound_at, Dig, Ladder, Standing};
use crate::save;
use crate::tradecraft;

/// Everything a gate might need to resolve, kept together so no caller
/// resolves half of it (the bug shape when a second gate kind lands).
struct Keys {
    camp: CampaignLog,
    raids: crate::raidlog::RaidLog,
}
impl Keys {
    fn open(&self, g: &str) -> bool {
        archive::gate_open(g, &self.camp, &self.raids)
    }
}

/// The archaeology layer's tunables, read once. `anchor` and `ladder` are
/// both `cfg` knobs (see tuning.rs); reading them here is what makes the
/// tuning ledger's promise true.
struct Tuned {
    anchor: f32,
    ladder: Ladder,
    era: Era,
}

fn load() -> (save::Save, Dig, Keys, Tuned) {
    let (sv, _, cfg) = save::load();
    let dig = Dig::from_line(&sv.finds, FINDINGS);
    let keys = Keys {
        camp: CampaignLog::from_line(&sv.missions),
        raids: crate::raidlog::RaidLog::from_line(&sv.wardens),
    };
    (sv, dig, keys, Tuned { anchor: cfg.story.anchor, ladder: Ladder::from_cfg(&cfg), era: Era::from_cfg(&cfg) })
}

fn store(mut sv: save::Save, dig: &Dig) {
    sv.finds = dig.line();
    let (_, binds, cfg) = save::load();
    save::store(&sv, &binds, &cfg);
}

pub fn run(argv: &[String]) {
    let i = argv.iter().position(|a| a == "--arch").unwrap_or(0);
    let cmd = argv.get(i + 1).map(String::as_str).unwrap_or("map");
    let arg = argv.get(i + 2).map(String::as_str);

    match cmd {
        "map" => map(false),
        "strata" => map(true),
        "site" => site(arg),
        "find" => find(arg),
        "why" => why(arg),
        "orbit" => orbit(),
        "audit" => audit(),
        "anchor" => anchor_cmd(arg),
        "emit-c" => emit_c(arg),
        "approach" => approach_cmd(arg),
        "carry" => carry(arg),
        "relay" => relay_cmd(arg),
        "city" => city(arg, argv.get(i + 3).map(String::as_str)),
        "read" => read_cmd(arg, argv.get(i + 3).map(String::as_str)),
        "reset" => reset(),
        other => {
            println!("unknown: --arch {other}");
            println!("try: map | strata | site <dest> | find <id> | why <claim>");
            println!("     read <id> [lens] | city [place] [then|now]");
            println!("     carry <id> | relay [id] | approach [guard|sentry|hunter]");
            println!("     anchor [0-100] | orbit | audit");
            println!("     reset | emit-c [path]");
        }
    }
}

// ------------------------------------------------------------------ map ---

fn map(by_stratum: bool) {
    let (_, dig, _, tuned) = load();
    let a = assay_with(CLAIMS, FINDINGS, dig.ids(), &archive::priors(tuned.anchor));

    let drawn = CLAIMS.iter().filter(|c| a[c.id].standing_with(tuned.ladder).drawn()).count();
    println!("THE ARCHAEOLOGICAL MAP");
    println!(
        "{} finds recorded; anchor {:.0}% ({}); {drawn} of {} nodes visible\n",
        dig.count(),
        tuned.anchor * 100.0,
        archive::anchor_name(tuned.anchor),
        CLAIMS.len()
    );

    if drawn == 0 {
        println!("  nothing on it yet. the map is built out of what you pick up.");
        println!("  try `--arch site earth`.");
        return;
    }

    if by_stratum {
        for layer in Stratum::all() {
            let rows: Vec<_> = CLAIMS
                .iter()
                .filter(|c| c.layer == layer as u8 && a[c.id].standing_with(tuned.ladder).drawn())
                .collect();
            let total = CLAIMS.iter().filter(|c| c.layer == layer as u8).count();
            println!("-- LAYER {} — {} ({}/{} visible)", layer as u8, layer.name(), rows.len(), total);
            if rows.is_empty() {
                println!("     (nothing yet — {})\n", layer.gloss());
                continue;
            }
            println!("     {}", layer.gloss());
            for c in rows {
                row(c.id, c.text, &a[c.id]);
            }
            println!();
        }
    } else {
        // strongest first: what the player is most sure of reads at the top
        let mut rows: Vec<_> =
            CLAIMS.iter().filter(|c| a[c.id].standing_with(tuned.ladder).drawn()).collect();
        rows.sort_by_key(|c| (std::cmp::Reverse(a[c.id].standing_with(tuned.ladder)), c.layer, c.id));
        for c in rows {
            print!("  L{} ", c.layer);
            row(c.id, c.text, &a[c.id]);
        }
        println!("\n  `--arch why <node>` for the evidence behind any line.");
    }
}

fn row(id: &str, text: &str, a: &crate::provenance::Assay) {
    let s = a.standing();
    // a claim can be on the map purely because something NAMED it, with
    // nothing yet weighing either way. "0 for" reads as a failure; it is
    // actually the most interesting state on the board.
    let tally = match (a.support(), a.opposition()) {
        (0, 0) => "named, not yet corroborated".to_string(),
        (f, 0) => format!("{f} for"),
        (0, ag) => format!("{ag} against"),
        (f, ag) => format!("{f} for / {ag} against"),
    };
    println!("  [{}]  {}", s.label(), id);
    println!("        {text}");
    println!("        {tally}");
}

fn why(arg: Option<&str>) {
    let Some(id) = arg else {
        println!("--arch why <claim>");
        return;
    };
    let Some(c) = archive::claim(id) else {
        println!("no node '{id}'. `--arch map` lists what is visible.");
        return;
    };
    let (_, dig, _, tuned) = load();
    let a = assay_with(CLAIMS, FINDINGS, dig.ids(), &archive::priors(tuned.anchor));
    let asy = &a[c.id];

    println!("{}\n  {}", c.id, c.text);
    println!("  layer {} — {}", c.layer, Stratum::from_u8(c.layer).name());
    println!("  standing: {}\n", asy.standing().label());

    if !asy.standing().drawn() {
        println!("  nothing found bears on this yet.");
        return;
    }
    // group the citations by the finds that produced them
    for f in FINDINGS.iter().filter(|f| dig.has(f.id)) {
        for l in f.links {
            if l.claim != c.id {
                continue;
            }
            let verb = match l.bearing {
                crate::provenance::Bearing::Supports => "FOR    ",
                crate::provenance::Bearing::Contradicts => "AGAINST",
                crate::provenance::Bearing::Mentions => "NAMES  ",
            };
            println!("  {verb} {} ({}, via {})", f.id, f.site, f.source);
        }
    }
    if asy.support() > 0 || asy.opposition() > 0 {
        println!(
            "\n  {} independent source(s) for, {} against.",
            asy.support(),
            asy.opposition()
        );
        if asy.standing() == Standing::Contested {
            println!("  the map does not average these. somebody is wrong, or lying.");
        }
    }
}

// ----------------------------------------------------------------- site ---

fn site(arg: Option<&str>) {
    let (_, dig, keys, _tuned) = load();
    let Some(dest) = arg else {
        println!("sites with anything still on them:\n");
        for d in patrol::ROUTE {
            let left = unfound_at(FINDINGS, d.id, dig.ids(), |g| keys.open(g)).len();
            let total = FINDINGS.iter().filter(|f| f.site == d.id).count();
            if total > 0 {
                println!("  {:<12} {left} unfound of {total}", d.id);
            }
        }
        println!("\n--arch site <dest>");
        return;
    };
    if !patrol::ROUTE.iter().any(|d| d.id == dest) {
        println!("no destination '{dest}'.");
        return;
    }
    let open = unfound_at(FINDINGS, dest, dig.ids(), |g| keys.open(g));
    let gated: Vec<_> = FINDINGS
        .iter()
        .filter(|f| f.site == dest && !dig.has(f.id))
        .filter(|f| f.gate.map(|g| !keys.open(g)).unwrap_or(false))
        .collect();

    println!("{dest} — {} findable, {} already recovered", open.len(),
        FINDINGS.iter().filter(|f| f.site == dest && dig.has(f.id)).count());
    println!();
    for f in &open {
        println!("  {}", f.id);
        println!("      {}\n", wrap(f.blurb, 68, 6));
    }
    for f in gated {
        println!("  {} — sealed (needs: {})", f.id, f.gate.unwrap());
    }
    if !open.is_empty() {
        println!("`--arch find <id>` to record one.");
    }
}

fn find(arg: Option<&str>) {
    let Some(id) = arg else {
        println!("--arch find <id>");
        return;
    };
    let Some(f) = archive::finding(id) else {
        println!("no finding '{id}'.");
        return;
    };
    let (sv, mut dig, keys, tuned) = load();
    if tuned.era.relays() {
        // the two campaigns have different discovery loops and only one runs.
        // In the archive era nothing reaches the map until it is relayed, so
        // scanning straight onto it would quietly delete the tradecraft layer.
        println!("this is {} — nothing goes straight onto the map here.", tuned.era.name());
        println!("  `--arch carry {id}`, then `--arch relay <id>`.");
        return;
    }
    if let Some(g) = f.gate {
        if !keys.open(g) {
            let what = match g.split_once(':') {
                Some(("warden", w)) => format!("the warden {w} put down"),
                _ => format!("'{g}' cleared"),
            };
            println!("{id} is sealed. it needs {what} first.");
            return;
        }
    }
    let priors = archive::priors(tuned.anchor);
    let before = assay_with(CLAIMS, FINDINGS, dig.ids(), &priors);
    if !dig.record(f.id) {
        println!("{id} is already in the dig.");
        return;
    }
    let after = assay_with(CLAIMS, FINDINGS, dig.ids(), &priors);

    println!("{} — {}\n", f.id, f.site);
    println!("{}\n", wrap(f.blurb, 72, 0));

    // Report nodes that MOVED — where moving means the tally changed, not
    // only the label. The third source putting the dig before the official
    // date does not change a claim that already reads SOURCES DISAGREE, but
    // it is emphatically not nothing: a ladder of independent dates is the
    // whole shape of that discovery, and a reader who is told "nothing
    // moves" on rungs two and three has been shown the wrong thing.
    let mut moved = false;
    let mut latent = false;
    for c in CLAIMS {
        let (b, a) = (&before[c.id], &after[c.id]);
        let (bs, as_) = (b.standing(), a.standing());
        if bs == as_ && b.support() == a.support() && b.opposition() == a.opposition() {
            continue;
        }
        // a claim nobody has asserted yet is not on the board, however much
        // evidence is quietly stacking against it. announcing it would hand
        // the player the official record before they have found the file.
        if !as_.drawn() {
            latent = true;
            continue;
        }
        moved = true;
        if bs == Standing::Dark {
            println!("  NEW NODE   {}  [{}]", c.id, as_.label());
        } else if bs != as_ {
            println!("  {}  [{}] -> [{}]", c.id, bs.label(), as_.label());
        } else {
            println!("  {}  [{}], corroborated further", c.id, as_.label());
        }
        println!("             {}", c.text);
        if b.opposition() != a.opposition() {
            println!(
                "             {} independent source(s) against it now, was {}",
                a.opposition(),
                b.opposition()
            );
        } else if b.support() != a.support() {
            println!(
                "             {} independent source(s) for it now, was {}",
                a.support(),
                b.support()
            );
        }
    }
    if !moved && latent {
        // it bears on something, but on nothing the player has met yet
        println!("  recorded. nothing on the map answers to it yet.");
    } else if !moved {
        println!("  nothing on the map moves. another page of something you already read.");
    }
    store(sv, &dig);
}

/// Show or set the mystery anchor. The whole reason it exists is that
/// neither reading is obviously right, so this prints what CHANGES rather
/// than just the number.
fn anchor_cmd(arg: Option<&str>) {
    let (_, dig, _, tuned) = load();
    if let Some(v) = arg {
        let Ok(pct) = v.trim().trim_end_matches('%').parse::<f32>() else {
            println!("--arch anchor [0-100]");
            return;
        };
        let a = (pct / 100.0).clamp(0.0, 1.0);
        let (sv, binds, mut cfg) = save::load();
        cfg.story.anchor = a;
        save::store(&sv, &binds, &cfg);
        println!("mystery anchor {:.0}% — {}\n", a * 100.0, archive::anchor_name(a));
        show_anchor(a, &dig);
        return;
    }
    println!("mystery anchor {:.0}% — {}\n", tuned.anchor * 100.0, archive::anchor_name(tuned.anchor));
    show_anchor(tuned.anchor, &dig);
    println!("\n  the dial, and what each stop assumes:");
    for stop in [0.0f32, 0.3, 0.6, 1.0] {
        println!(
            "    {:>4.0}%  {:<9} {} assumption(s)",
            stop * 100.0,
            archive::anchor_name(stop),
            archive::priors(stop).len()
        );
    }
    println!("\n  turning it up is not more tutorial. the received account");
    println!("  includes the official record, and the official record is wrong");
    println!("  about at least one thing. HOMEY gives you a lie to catch;");
    println!("  FRESH gives you a world with nothing yet to disbelieve.");
    println!("\n  --arch anchor <0-100>");
}

fn show_anchor(a: f32, dig: &Dig) {
    let taken = archive::received_at(a);
    if taken.is_empty() {
        println!("  nothing is assumed. every node starts dark.");
    } else {
        println!("  taken as known before any digging:");
        for r in &taken {
            println!("    [{:>3.0}%] {}", r.at * 100.0, r.gloss);
        }
    }
    let next = archive::RECEIVED.iter().find(|r| a < r.at);
    if let Some(n) = next {
        println!("\n  next at {:.0}%: {}", n.at * 100.0, n.gloss);
    }
    let with = assay_with(CLAIMS, FINDINGS, dig.ids(), &archive::priors(a));
    let drawn = CLAIMS.iter().filter(|c| with[c.id].standing().drawn()).count();
    println!("\n  at this setting the map opens with {drawn} of {} nodes visible.", CLAIMS.len());
}

/// Emit the C engine's copy of these tables.
///
/// The C game (games/blackstar, `exengine/provenance.c`) needs the same
/// claims, findings and received knowledge to put a standing on its CODEX
/// screen. Hand-copying them would guarantee drift, so they are generated,
/// following the convention `tools/content_build.py --emit-c` already sets
/// for `content_gen.h`: one authored source, a generated header, and a C
/// test that fails if the two stop agreeing.
///
/// This deliberately emits DATA and not the inference. The ladder itself is
/// implemented twice, in two languages, which is a real duplication — it is
/// ~40 lines of comparisons on either side, and `tests/test_provenance.c`
/// replays the same fixtures the Rust suite asserts so a divergence is a
/// test failure rather than a mystery.
fn emit_c(arg: Option<&str>) {
    let path = arg.unwrap_or(C_HEADER_PATH);
    let o = render_c();
    match std::fs::write(path, &o) {
        Ok(()) => println!(
            "wrote {path}\n  {} claims, {} findings, {} received entries",
            CLAIMS.len(),
            FINDINGS.len(),
            RECEIVED.len()
        ),
        Err(e) => println!("could not write {path}: {e}"),
    }
}

/// Where the generated header lives, relative to this crate's root.
pub const C_HEADER_PATH: &str = "../blackstar/engine_src/exengine/archive_gen.h";

/// The header's text.
///
/// Split from the write so a test can compare it against the file on disk.
/// Without that, editing `archive.rs` and forgetting to regenerate leaves the
/// C engine reading a stale roster, silently — the exact drift the generated
/// header exists to prevent, reintroduced one level up.
pub fn render_c() -> String {
    let mut o = String::new();
    o.push_str("/* GENERATED by `skydribble --arch emit-c` -- do not edit by hand.\n");
    o.push_str("   Mirrors games/blackstar-rs/src/archive.rs; the ladder that reads these\n");
    o.push_str("   tables lives in provenance.c and is checked by test_provenance. */\n");
    o.push_str("#ifndef EX_ARCHIVE_GEN_H\n#define EX_ARCHIVE_GEN_H\n#include \"provenance.h\"\n#include <stddef.h>\n\n");

    o.push_str(&format!("static const ex_claim_t ex_gen_claims[{}] = {{\n", CLAIMS.len()));
    for c in CLAIMS {
        o.push_str(&format!(
            "  {{ \"{}\", {}, {}, \"{}\" }},\n",
            c.id,
            c.layer,
            c.asserted as u8,
            esc(c.text)
        ));
    }
    o.push_str("};\n");
    o.push_str(&format!("#define EX_GEN_CLAIM_COUNT {}\n\n", CLAIMS.len()));

    // one link array per finding, then the finding table pointing at them
    for f in FINDINGS {
        if f.links.is_empty() {
            continue;
        }
        o.push_str(&format!(
            "static const ex_link_t ex_gen_links_{}[{}] = {{ ",
            f.id,
            f.links.len()
        ));
        for l in f.links {
            let b = match l.bearing {
                Bearing::Supports => "EX_BEARING_SUPPORTS",
                Bearing::Contradicts => "EX_BEARING_CONTRADICTS",
                Bearing::Mentions => "EX_BEARING_MENTIONS",
            };
            o.push_str(&format!("{{ \"{}\", {} }}, ", l.claim, b));
        }
        o.push_str("};\n");
    }
    o.push('\n');
    o.push_str(&format!("static const ex_finding_t ex_gen_findings[{}] = {{\n", FINDINGS.len()));
    for f in FINDINGS {
        let gate = match f.gate {
            Some(g) => format!("\"{g}\""),
            None => "NULL".to_string(),
        };
        let (links, n) = if f.links.is_empty() {
            ("NULL".to_string(), 0)
        } else {
            (format!("ex_gen_links_{}", f.id), f.links.len())
        };
        let med = match f.medium {
            crate::provenance::Medium::Artifact => "EX_MEDIUM_ARTIFACT",
            crate::provenance::Medium::Terminal => "EX_MEDIUM_TERMINAL",
            crate::provenance::Medium::Site => "EX_MEDIUM_SITE",
        };
        o.push_str(&format!(
            "  {{ \"{}\", \"{}\", \"{}\", {}, {}, {}, {} }},\n",
            f.id, f.site, f.source, med, gate, links, n
        ));
    }
    o.push_str("};\n");
    o.push_str(&format!("#define EX_GEN_FINDING_COUNT {}\n\n", FINDINGS.len()));

    o.push_str(&format!("static const ex_received_t ex_gen_received[{}] = {{\n", RECEIVED.len()));
    for r in RECEIVED {
        let b = match r.bearing {
            Bearing::Supports => "EX_BEARING_SUPPORTS",
            Bearing::Contradicts => "EX_BEARING_CONTRADICTS",
            Bearing::Mentions => "EX_BEARING_MENTIONS",
        };
        o.push_str(&format!(
            "  {{ \"{}\", {}, \"{}\", {:.4}f, \"{}\" }},\n",
            r.claim, b, r.source, r.at, esc(r.gloss)
        ));
    }
    o.push_str("};\n");
    o.push_str(&format!("#define EX_GEN_RECEIVED_COUNT {}\n\n", RECEIVED.len()));
    o.push_str("#endif /* EX_ARCHIVE_GEN_H */\n");
    o
}

/// C string escaping. The prose uses `--` and plain ASCII by house style, so
/// this only has to cope with quotes and backslashes -- but it asserts that
/// rather than assuming it, because a stray newline in a table would emit a
/// header that does not compile.
fn esc(s: &str) -> String {
    debug_assert!(!s.contains('\n'), "table prose must be single-line: {s:?}");
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Read one finding as each of the five, or as one named lens.
///
/// The reason this is a command and not a test: whether four voices on the
/// same object read as four PEOPLE or as one writer doing accents is not
/// something an assertion can tell you. You have to see them stacked.
fn read_cmd(arg: Option<&str>, who: Option<&str>) {
    let Some(id) = arg else {
        println!("--arch read <finding> [patrol|dragon|voltage|kinetic|archivist]");
        return;
    };
    let Some(f) = archive::finding(id) else {
        println!("no finding '{id}'.");
        return;
    };
    println!("{} — {} ({:?})", f.id, f.site, f.medium);
    println!("read: {}\n", archive::surface(f.medium));

    let lenses: Vec<Lens> = match who {
        Some(w) => match Lens::parse(w) {
            Some(l) => vec![l],
            None => {
                println!("no lens '{w}'. one of: patrol dragon voltage kinetic archivist");
                return;
            }
        },
        None => Lens::all().to_vec(),
    };
    for l in lenses {
        // Patrol is the voice everything is authored in, so its "fallback"
        // is not a gap -- it is the base text. Saying otherwise reads as a
        // missing asset rather than a design.
        let tag = match (l, archive::has_reading(f.id, l)) {
            (Lens::Patrol, _) => "",
            (_, true) => "",
            (_, false) => "  (nothing of their own to add)",
        };
        let side = if l.human() { "human campaign" } else { "the ship" };
        println!("  {} ({side}){tag}", l.name());
        println!("    {}\n", wrap(archive::reading(f, l), 66, 4));
    }
}

/// Walk the ground, in one era or the other.
///
/// The point of printing both eras side by side is that it is the only way to
/// tell whether the overlap READS -- whether a player standing on the ridge in
/// the present would recognise it as the place the paperwork lied about.
fn city(arg: Option<&str>, when: Option<&str>) {
    let (_, dig, keys, _tuned) = load();
    let era = match when {
        Some("then") | Some("archive") => Some(Era::Archive),
        Some("now") | Some("present") => Some(Era::Present),
        _ => None,
    };
    let Some(id) = arg else {
        println!("GROUNDSIDE — earth\n");
        match groundside::home(Era::Present) {
            Some(h) => println!("  home: {} (present only; the archive era has none)\n", h.name),
            None => println!("  no home base in either era.\n"),
        }
        for p in groundside::PLACES {
            let held = p.holds.len();
            let got = p.holds.iter().filter(|f| dig.has(f)).count();
            let home = if p.is_home(Era::Present) { "  [HOME]" } else { "" };
            println!("  {:<20} {got}/{held} recovered{home}", p.id);
        }
        println!("\n--arch city <place> [then|now]");
        return;
    };
    let Some(p) = groundside::place(id) else {
        println!("no place '{id}'.");
        return;
    };
    println!("{} — {}", p.name, p.dest);
    if p.is_home(Era::Present) {
        println!("the home base, in both eras. the safehouse and the foothold are one address.");
    }
    println!();
    for e in [Era::Archive, Era::Present] {
        if era.is_some() && era != Some(e) {
            continue;
        }
        let tag = if e == Era::Archive { "THEN (the archive era)" } else { "NOW  (the present)" };
        println!("  {tag}");
        println!("    {}\n", wrap(p.as_of(e), 66, 4));
    }
    if !p.near.is_empty() {
        println!("  within reach: {}", p.near.join(", "));
    }
    let here = tradecraft::relays_at(p.id);
    if !here.is_empty() {
        println!("\n  you can get word out from here:");
        for r in here {
            println!("    {:<20} {}", r.id, r.channel.name());
        }
    }
    if p.holds.is_empty() {
        return;
    }
    println!("\n  evidence here:");
    for f in p.holds.iter().filter_map(|f| archive::finding(f)) {
        let mark = if dig.has(f.id) { "*" } else { " " };
        let sealed = f.gate.map(|g| !keys.open(g)).unwrap_or(false);
        println!("  {mark} {:<24} {:?}{}", f.id, f.medium, if sealed { "  (sealed)" } else { "" });
    }
}

/// Put a finding in the case instead of straight on the map.
///
/// The human campaign's version of `find`: nothing reaches the
/// archaeological map until it is relayed, so this is the half of a discovery
/// that happens in the field.
fn carry(arg: Option<&str>) {
    let Some(id) = arg else {
        println!("--arch carry <finding>");
        return;
    };
    let Some(f) = archive::finding(id) else {
        println!("no finding '{id}'.");
        return;
    };
    let (mut sv, dig, keys, tuned) = load();
    if !tuned.era.relays() {
        println!(
            "this is {} — there is no case; you have a ship. `--arch find {id}`.",
            tuned.era.name()
        );
        return;
    }
    if dig.has(f.id) {
        println!("{id} is already on the map. nothing left to carry.");
        return;
    }
    if let Some(g) = f.gate {
        if !keys.open(g) {
            println!("{id} is sealed.");
            return;
        }
    }
    let (_, _, cfg) = save::load();
    let cap = cfg.story.case_cap as usize;
    let mut case = tradecraft::Case::from_line(&sv.case, FINDINGS);
    match case.take_capped(f.id, cap) {
        Ok(()) => {
            println!("{} — into the case ({}/{cap})\n", f.id, case.len());
            println!("{}\n", wrap(f.blurb, 72, 0));
            println!("  it is not on the map. `--arch relay` to get it out.");
            sv.case = case.line();
            let (_, binds, cfg) = save::load();
            save::store(&sv, &binds, &cfg);
        }
        Err(e) => println!("{e}"),
    }
}

/// Push what fits through a relay, and see what it costs.
fn relay_cmd(arg: Option<&str>) {
    let (mut sv, mut dig, _keys, _t) = load();
    let case = tradecraft::Case::from_line(&sv.case, FINDINGS);
    let Some(id) = arg else {
        println!("{}\n", _t.era.name());
        println!("CARRYING {} item(s)\n", case.len());
        for h in case.held() {
            match groundside::place_of(h) {
                Some(p) => println!("  {h}  (from {})", p.name),
                None => println!("  {h}"),
            }
        }
        println!("\nthe societies, and how their people fight:");
        for soc in tradecraft::Society::all() {
            println!("  {:<12} {:<16} fights as a {}", soc.id(), soc.name(), soc.doctrine());
        }
        println!("\nrelays usable in {}:", _t.era.name());
        for r in tradecraft::RELAYS.iter().filter(|r| tradecraft::available(r, _t.era)) {
            let at = r.at.unwrap_or("wherever you left it");
            println!(
                "  {:<20} {:<16} reach {}  exposure {}  [{}]",
                r.id,
                r.channel.name(),
                r.channel.reach(),
                r.channel.exposure(),
                at
            );
        }
        println!("\n--arch relay <id>");
        return;
    };
    let Some(r) = tradecraft::relay(id) else {
        println!("no relay '{id}'.");
        return;
    };
    let mut case = case;
    let rep = tradecraft::report(&mut case, r);
    println!("{} — {}\n", r.id, r.channel.name());
    println!("{}\n", wrap(r.blurb, 72, 0));
    if rep.landed.is_empty() {
        println!("  nothing left. {}", if r.channel.reach() == 0 {
            "this one does not transmit."
        } else {
            "the case is empty."
        });
    } else {
        for f in &rep.landed {
            dig.record(f);
            println!("  sent: {f}");
        }
        if rep.degraded {
            println!("  ...in their own words. some of the detail will not survive the retelling.");
        }
    }
    if !rep.kept.is_empty() {
        println!("  still carrying {}: {}", rep.kept.len(), rep.kept.join(", "));
    }
    if !rep.overheard_by.is_empty() {
        let who: Vec<&str> = rep.overheard_by.iter().map(|s| s.name()).collect();
        println!("\n  OVERHEARD by {}. they will write their own version.", who.join(", "));
    }
    sv.case = case.line();
    sv.finds = dig.line();
    let (_, binds, cfg) = save::load();
    save::store(&sv, &binds, &cfg);
}

/// Ways into a place, against somebody watching it.
///
/// Runs `perception` -> `tradecraft::infiltrate` -> `approach`, which is the
/// whole stealth stack and had no caller outside its own tests. Printing the
/// same doors against three different watchers is the only way to tell
/// whether the trade READS — whether "go round the side" ever feels like the
/// wrong answer, which is the point of having three doors.
fn approach_cmd(arg: Option<&str>) {
    use crate::perception::Watcher;
    let (who, eyes) = match arg.unwrap_or("guard") {
        "sentry" => ("A SENTRY on the door", Watcher::sentry()),
        "hunter" => ("SOMETHING HUNTING YOU", Watcher::hunter()),
        "guard" => ("A BORED GUARD on a round", Watcher::guard()),
        other => {
            println!("no watcher '{other}'. one of: guard sentry hunter");
            return;
        }
    };
    println!("GETTING IN — {who}");
    println!("  sees {:.0} m, attention {:.2}{}\n", eyes.range_m, eyes.attention,
        if eyes.relentless { ", and it does not stop" } else { "" });

    println!("  {:<22} {:>7} {:>9} {:>11}", "way in", "takes", "exposed", "cost if seen");
    for w in tradecraft::WayIn::all() {
        let (d, _) = w.exposure();
        let open_s =
            if w.exposed_throughout() { w.take_s() } else { w.take_s() * 0.25 };
        println!(
            "  {:<22} {:>6.0}s {:>8.0}s {:>11.0}   ({:.0} m out)",
            w.label(),
            w.take_s(),
            open_s,
            w.seen_cost(&eyes),
            d
        );
    }

    println!("\n  what you would take, by how long you have:");
    for window in [30.0f32, 60.0, 80.0, 120.0, 300.0, 600.0] {
        match tradecraft::best_way_in(window, 0.0, &eyes) {
            Some(w) => println!("    {window:>5.0}s   {}", w.label()),
            None => println!("    {window:>5.0}s   nothing fits"),
        }
    }
    println!("\n  being seen is a cost, not a bar. the front door is not");
    println!("  forbidden -- it is expensive, and sometimes it is all there is.");
}

fn reset() {
    let (sv, _, _, _t) = load();
    store(sv, &Dig::default());
    println!("the dig is empty. every find can be made again.");
}

// ---------------------------------------------------------------- orbit ---

/// The Asteri reveal, computed. Nothing here is a stated fact — every
/// number falls out of `orbits.rs`, which is a real solver.
fn orbit() {
    let q = orbits::ASTERI_PERIHELION_AU;
    let claimed_p = orbits::ASTERI_RETURN_PREDICTED_YEARS;
    let true_p = orbits::ASTERI_RETURN_WINDOW_YEARS;
    let a_claimed = orbits::semi_major_au(claimed_p);
    let a_true = orbits::semi_major_au(true_p);
    let v_claimed = orbits::perihelion_speed(q, a_claimed);
    let v_true = orbits::perihelion_speed(q, a_true);
    let err = orbits::asteri_speed_error();

    println!("THE RETURN, RECOMPUTED");
    println!("two-body, heliocentric, AU and years. perihelion q = {q} AU.\n");

    println!("  their record   P = {claimed_p:.0} yr   a = {a_claimed:.1} AU   v_peri = {v_claimed:.5} AU/yr");
    println!("  the true orbit P = {true_p:.0} yr   a = {a_true:.1} AU   v_peri = {v_true:.5} AU/yr");
    println!("\n  the entire difference is {:.4}% of the perihelion speed.\n", err * 100.0);

    println!("  why so little buys so much: at perihelion the orbital energy is");
    println!("      1/a = 2/q - v^2/mu");
    println!("  and for a long orbit 1/a is the small remainder of a large");
    println!("  subtraction. 2/q here is {:.4}; 1/a for the true orbit is {:.5}.", 2.0/q, 1.0/a_true);
    println!("  the period is carried in the last digits of that difference.\n");

    println!("  what their arc actually allowed, at a range of measurement precisions:\n");
    for sigma in [0.0003, 0.001, 0.002, err, 0.005] {
        let w = orbits::period_band(q, v_true, sigma);
        let tag = if (sigma - err).abs() < 1e-9 { "  <- theirs" } else { "" };
        match w {
            orbits::Window::Bounded { min_years, max_years } => {
                println!("    +/- {:.3}%   P in [{:.0}, {:.0}] yr{tag}", sigma * 100.0, min_years, max_years);
            }
            orbits::Window::Open { min_years } => {
                println!("    +/- {:.3}%   P in [{:.0}, unbounded] yr{tag}", sigma * 100.0, min_years);
            }
        }
    }

    // run the question backwards: at what precision does 417 stop being
    // admissible as the floor? that is the instrument they would have
    // needed, and it is the honest measure of how close they came.
    let needed = orbits::precision_for_floor(q, v_true, claimed_p);
    let band = orbits::period_band(q, v_true, needed);
    println!();
    println!("  inverted: 417 stops being the floor once the speed is known to");
    println!("  better than {:.4}%. below that it is admissible, and so is", needed * 100.0);
    println!("  everything above it —");
    println!("      band admits {claimed_p:.0} yr : {}", band.admits(claimed_p));
    println!("      band admits {true_p:.0} yr: {}", band.admits(true_p));
    if matches!(band, orbits::Window::Open { .. }) {
        println!("\n  and at that precision the band has no ceiling at all. their data");
        println!("  could not distinguish 'returns in {claimed_p:.0} years' from 'does not");
        println!("  return'. they did not get a number wrong. they published the");
        println!("  optimistic end of an open band, and carved it in stone.");
    }
    println!("\n  they were not fools. they were a hundred and four years of good");
    println!("  measurements away from a question that needed a thousand.");
}

// ---------------------------------------------------------------- audit ---

/// Content checks the tests also make, but reported as a list a writer can
/// read while editing the tables rather than as a test failure.
fn audit() {
    let full: std::collections::BTreeSet<String> =
        FINDINGS.iter().map(|f| f.id.to_string()).collect();
    // deliberately anchor-free: the audit asks what the EVIDENCE supports,
    // which is a property of the tables, not of how the game is framed today
    let a = assay(CLAIMS, FINDINGS, &full);
    let mut problems = 0;

    println!("CONTENT AUDIT — {} claims, {} findings\n", CLAIMS.len(), FINDINGS.len());

    println!("if every finding in the game were recovered:");
    for layer in Stratum::all() {
        let rows: Vec<_> = CLAIMS.iter().filter(|c| c.layer == layer as u8).collect();
        if rows.is_empty() {
            continue;
        }
        println!("\n  LAYER {} — {}", layer as u8, layer.name());
        for c in rows {
            let asy = &a[c.id];
            println!("    {:<14} {:<30} {} for / {} against", 
                format!("[{}]", asy.standing().short()), c.id, asy.support(), asy.opposition());
            if !asy.standing().drawn() {
                println!("      ^ UNREACHABLE: no finding bears on this");
                problems += 1;
            }
        }
    }

    println!("\nsources (the independence keys):");
    let mut srcs: Vec<_> = FINDINGS.iter().map(|f| f.source).collect();
    srcs.sort_unstable();
    srcs.dedup();
    for s in &srcs {
        let n = FINDINGS.iter().filter(|f| f.source == *s).count();
        if n > 1 {
            println!("  {s:<22} {n} findings — these cannot corroborate each other");
        }
    }

    println!("\ngated findings:");
    for f in FINDINGS.iter().filter(|f| f.gate.is_some()) {
        println!("  {:<22} needs {}", f.id, f.gate.unwrap());
    }

    // a finding that moves nothing is content with no mechanical effect
    println!();
    for f in FINDINGS {
        if f.links.is_empty() {
            println!("  DEAD END: {} links to nothing", f.id);
            problems += 1;
        }
    }
    println!("{problems} problem(s).");
}

/// Wrap at `w` columns with `indent` leading spaces on continuation lines.
fn wrap(text: &str, w: usize, indent: usize) -> String {
    let pad = " ".repeat(indent);
    let mut out = String::new();
    let mut col = 0;
    for word in text.split_whitespace() {
        if col > 0 && col + 1 + word.len() > w {
            out.push('\n');
            out.push_str(&pad);
            col = 0;
        } else if col > 0 {
            out.push(' ');
            col += 1;
        }
        out.push_str(word);
        col += word.len();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_never_exceeds_its_width_and_loses_nothing() {
        let text = "the walls did not collapse into this shape they were cut and the cut is clean";
        let out = wrap(text, 30, 4);
        for line in out.lines() {
            assert!(line.trim_start().len() <= 30, "over-wide line: {line:?}");
        }
        let round: Vec<&str> = out.split_whitespace().collect();
        let orig: Vec<&str> = text.split_whitespace().collect();
        assert_eq!(round, orig, "wrapping dropped or reordered words");
    }

    #[test]
    fn wrap_handles_a_word_longer_than_the_width() {
        let out = wrap("supercalifragilistic x", 8, 2);
        assert!(out.contains("supercalifragilistic"), "a long word must survive intact");
    }

    #[test]
    fn every_standing_has_a_short_label() {
        for s in [
            Standing::Dark,
            Standing::Unknown,
            Standing::Possible,
            Standing::High,
            Standing::Contested,
        ] {
            assert!(!s.short().is_empty());
            assert!(!s.label().is_empty());
        }
    }
}
