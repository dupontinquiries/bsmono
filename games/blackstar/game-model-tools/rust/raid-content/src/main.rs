//! raid-content -- parse raid boss lore markdown into structured JSON.
//! Rust port of raid_content.py (lore-parsing core). Pure std.

use std::env;
use std::fs;

struct Boss {
    name: String,
    epithet: String,
    classification: String,
    backstory: Vec<String>,
    lore_source: &'static str,
}

/// name keyword -> (short description, class, tags) for stub generation.
fn theme_for(name: &str) -> (&'static str, &'static str, Vec<&'static str>) {
    let low = name.to_lowercase();
    let table: &[(&str, &str, &str, &[&str])] = &[
        ("titan", "colossal war-form", "Titan-class", &["siege", "colossus"]),
        ("drill", "planetary excavation engine", "Core-Breaker", &["burrow", "armor_pierce"]),
        ("reactor", "mobile fission core", "Walking Reactor", &["radiation", "meltdown"]),
        ("cherno", "nuclear brawler", "Walking Reactor", &["radiation", "melee"]),
        ("godzilla", "kaiju-scale destroyer", "Leviathan-class", &["colossus", "beam"]),
        ("scorpion", "subterranean ambush-form", "Stinger-class", &["burrow", "venom"]),
        ("mantis", "primitive blade-predator", "Reaper-form", &["blades", "leap"]),
        ("spider", "web-laying swarm-host", "Brood-form", &["swarm", "web"]),
        ("arachnid", "magma-brooding swarm-host", "Ember-Brood", &["fire", "swarm"]),
        ("tank", "siege-artillery relic", "Fortress-class", &["armor", "artillery"]),
        ("tremor", "seismic destroyer", "Planetbreaker", &["quake", "aoe"]),
        ("devastator", "combiner colossus", "Gestalt-class", &["combiner", "colossus"]),
    ];
    for (k, d, c, t) in table {
        if low.contains(k) {
            return (d, c, t.to_vec());
        }
    }
    ("ancient war-relic", "Relic-class", vec!["unknown"])
}

/// Parse entries: a Name line, then a "epithet" line, then paragraphs until the
/// next name/epithet pair. Mirrors the Python anchor logic.
fn parse_lore(text: &str) -> Vec<Boss> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();
        let nxt = lines.get(i + 1).map(|s| s.trim()).unwrap_or("");
        let is_epithet = nxt.starts_with('"') && nxt.ends_with('"') && nxt.len() >= 2;
        let name_ok = !line.is_empty()
            && !line.starts_with(['#', '-', '*', '>']);
        if name_ok && is_epithet {
            let name = line.to_string();
            let epithet = nxt.trim_matches('"').to_string();
            let mut body = Vec::new();
            let mut cls: Option<String> = None;
            let mut j = i + 2;
            while j < lines.len() {
                let s = lines[j].trim();
                let nx = lines.get(j + 1).map(|x| x.trim()).unwrap_or("");
                if !s.is_empty() && nx.starts_with('"') && nx.ends_with('"') && nx.len() >= 2 {
                    break; // next entry starts
                }
                if s.to_lowercase().starts_with("classification:") {
                    cls = Some(s[s.find(':').unwrap() + 1..].trim().to_string());
                }
                if !s.is_empty() {
                    body.push(s.to_string());
                }
                j += 1;
            }
            out.push(Boss {
                classification: cls.unwrap_or_else(|| "Relic-class".into()),
                name,
                epithet,
                backstory: body,
                lore_source: "authored",
            });
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

fn json_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            _ => o.push(c),
        }
    }
    o
}

fn to_json(bosses: &[Boss]) -> String {
    let authored = bosses.iter().filter(|b| b.lore_source == "authored").count();
    let generated = bosses.len() - authored;
    let mut out = String::from("{\n \"raid_bosses\": [\n");
    for (i, b) in bosses.iter().enumerate() {
        let (_d, _c, tags) = theme_for(&b.name);
        let tag_str: Vec<String> = tags.iter().map(|t| format!("\"{}\"", t)).collect();
        let body: Vec<String> = b
            .backstory
            .iter()
            .map(|p| format!("\"{}\"", json_escape(p)))
            .collect();
        out.push_str("  {\n");
        out.push_str(&format!("   \"name\": \"{}\",\n", json_escape(&b.name)));
        out.push_str(&format!("   \"epithet\": \"{}\",\n", json_escape(&b.epithet)));
        out.push_str(&format!(
            "   \"classification\": \"{}\",\n",
            json_escape(&b.classification)
        ));
        out.push_str(&format!("   \"backstory\": [{}],\n", body.join(", ")));
        out.push_str(&format!("   \"tags\": [{}],\n", tag_str.join(", ")));
        out.push_str(&format!("   \"lore_source\": \"{}\"\n", b.lore_source));
        out.push_str(if i + 1 < bosses.len() { "  },\n" } else { "  }\n" });
    }
    out.push_str(" ],\n");
    out.push_str(&format!(
        " \"counts\": {{ \"total\": {}, \"authored\": {}, \"generated\": {} }}\n}}\n",
        bosses.len(),
        authored,
        generated
    ));
    out
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut lore: Option<String> = None;
    let mut out = "raid_content.json".to_string();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--lore" => { i += 1; lore = args.get(i).cloned(); }
            "--out" => { i += 1; out = args.get(i).cloned().unwrap_or(out); }
            _ => {}
        }
        i += 1;
    }
    let lore = match lore {
        Some(l) => l,
        None => {
            eprintln!("usage: raid-content --lore raids-and-relics.md [--out raid_content.json]");
            std::process::exit(1);
        }
    };
    let text = fs::read_to_string(&lore).expect("read lore");
    let bosses = parse_lore(&text);
    fs::write(&out, to_json(&bosses)).expect("write json");
    let authored = bosses.iter().filter(|b| b.lore_source == "authored").count();
    println!("{} raid bosses -> {}", bosses.len(), out);
    println!("  {} authored, {} generated", authored, bosses.len() - authored);
    for b in bosses.iter().take(6) {
        println!("  - {:<30.30} \"{}\"", b.name, b.epithet);
    }
}
