//! model-metadata -- classify a model library from zip PATHS + NAMES into JSON.
//! Rust port of model_metadata.py. Pure std (no external crates).

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::Path;

fn category_for(dir: &str) -> Option<&'static str> {
    match dir.to_lowercase().as_str() {
        "raid-bosses" | "raid_bosses" => Some("raid_boss"),
        "animals" | "spiders" | "falcons" | "dragons" => Some("animal"),
        "patrol-bots" => Some("patrol_bot"),
        "bots" => Some("bot"),
        "cars" | "tanks" | "flyers" | "boats" | "hoverers" | "spaceships" | "vehicles" => {
            Some("vehicle")
        }
        "missiles" | "weapons" => Some("weapon"),
        "cities" => Some("structure"),
        "terrains" => Some("terrain"),
        "references" => Some("reference"),
        _ => None,
    }
}

const SUBTYPE_PREFIXES: &[(&str, &str)] = &[
    ("animal-bot", "beast"),
    ("spiderbot", "spider"),
    ("spider", "spider"),
    ("hawk-flyer", "avian"),
    ("dragonflyer", "dragon"),
    ("raidboss", "raid_boss"),
    ("raid-boss", "raid_boss"),
    ("flyer", "flyer"),
];

#[derive(Default)]
struct Meta {
    raw: String,
    modes: Vec<String>,
    flags: Vec<String>,
    subtype: Option<String>,
    name: String,
    note: Option<String>,
    category: String,
    dir: Option<String>,
    path: String,
}

fn parse_name(fname: &str) -> Meta {
    let stem = fname.strip_suffix(".zip").unwrap_or(fname).to_string();
    let low = stem.to_lowercase();
    let mut m = Meta {
        raw: fname.to_string(),
        modes: vec!["robot".into()],
        ..Default::default()
    };

    if low.starts_with("both") || low.contains("--both") {
        m.modes = vec!["robot".into(), "alt".into()];
        m.flags.push("both_modes".into());
    }
    if low.contains("transform-animation") {
        m.flags.push("transform_anim".into());
    }
    if low.contains("animated idle") {
        m.flags.push("idle_anim".into());
    } else if low.contains("animated") {
        m.flags.push("animated".into());
    }
    if low.contains("detailed") {
        m.flags.push("detailed".into());
    }
    if low.contains("weapon") {
        m.flags.push("has_weapon".into());
    }
    if low.contains("low-poly") || low.contains("low poly") {
        m.flags.push("low_poly".into());
    }
    if low.contains("pbr") {
        m.flags.push("pbr".into());
    }
    if low.contains("set of ") {
        m.flags.push("set".into());
    }

    for (pref, st) in SUBTYPE_PREFIXES {
        if low.starts_with(pref) {
            m.subtype = Some((*st).into());
            break;
        }
    }

    // note after 'aka'
    let mut core = stem.clone();
    if let Some(idx) = find_aka(&stem) {
        m.note = Some(stem[idx.1..].trim().to_string());
        core = stem[..idx.0].to_string();
    }

    // strip known prefixes
    let prefixes = [
        "both--", "both-", "both+weapon-", "animated idle ", "animated - ",
        "animated-", "detailed-", "animal-bot--", "spiderbot--", "spider-",
        "hawk-flyer--", "dragonflyer--", "raidboss--", "raidboss-", "raid-boss--",
        "raid-boss-", "flyer--", "flyer",
    ];
    let low_core = core.to_lowercase();
    for p in prefixes {
        if low_core.starts_with(p) {
            core = core[p.len()..].to_string();
            break;
        }
    }

    // a '--name' segment wins
    if let Some(pos) = core.find("--") {
        let tail: String = core[pos + 2..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ' ')
            .collect();
        m.name = tail.trim().to_string();
    }
    if m.name.is_empty() {
        // strip franchise boilerplate
        let mut c = core.to_lowercase();
        for boiler in [
            "transformers_", "transformers", "fall of cybertron", "war for cybertron",
            "war of cybertron", "decepticon", "autobot", "cybertron",
        ] {
            c = c.replace(boiler, " ");
        }
        let cleaned: String = c
            .chars()
            .map(|ch| if ch == '_' || ch == '-' { ' ' } else { ch })
            .collect();
        let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
        m.name = if cleaned.is_empty() {
            core.trim().to_string()
        } else {
            cleaned
        };
    }
    m
}

/// Find the "aka" separator in `s` (case-insensitive), returning (start,end) byte
/// offsets valid for slicing `s` itself. Char-safe.
fn find_aka(s: &str) -> Option<(usize, usize)> {
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let n = chars.len();
    let lower: Vec<char> = s.chars().map(|c| c.to_ascii_lowercase()).collect();
    let mut ci = 0;
    while ci + 3 <= n {
        if lower[ci] == 'a' && lower[ci + 1] == 'k' && lower[ci + 2] == 'a' {
            let before_ok = ci == 0 || matches!(lower[ci - 1], ' ' | '-');
            if before_ok {
                // end char index = after 'aka' + following separators
                let mut je = ci + 3;
                while je < n && matches!(lower[je], ' ' | '-') {
                    je += 1;
                }
                // start char index = back over separators before 'aka'
                let mut se = ci;
                while se > 0 && matches!(lower[se - 1], ' ' | '-') {
                    se -= 1;
                }
                let start = chars[se].0;
                let end = if je < n { chars[je].0 } else { s.len() };
                return Some((start, end));
            }
        }
        ci += 1;
    }
    None
}

fn walk_paths(paths: &[String]) -> Vec<Meta> {
    let mut out = Vec::new();
    for p in paths {
        let parts: Vec<&str> = p.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
        if parts.is_empty() {
            continue;
        }
        let fname = *parts.last().unwrap();
        if !fname.to_lowercase().ends_with(".zip") {
            continue;
        }
        let mut cat: Option<&str> = None;
        let mut sub: Option<String> = None;
        for seg in &parts[..parts.len() - 1] {
            if let Some(c) = category_for(seg) {
                if cat.is_none() {
                    cat = Some(c);
                }
                sub = Some(seg.to_lowercase());
            }
        }
        let mut m = parse_name(fname);
        m.category = cat.unwrap_or("unknown").to_string();
        m.dir = sub;
        m.path = p.clone();
        out.push(m);
    }
    out
}

/// Reconstruct full paths from an indented tree listing.
fn paths_from_tree(text: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut stack: BTreeMap<usize, String> = BTreeMap::new();
    for line in text.lines() {
        let cleaned: String = line
            .chars()
            .map(|c| if "│├└─┬┤┌┐┘┴".contains(c) { ' ' } else { c })
            .collect();
        let name = cleaned.trim().to_string();
        if name.is_empty()
            || name.starts_with('#')
            || name.starts_with('`')
            || name.starts_with('|')
        {
            continue;
        }
        let depth = cleaned.len() - cleaned.trim_start().len();
        stack.insert(depth, name.clone());
        let deeper: Vec<usize> = stack.keys().cloned().filter(|k| *k > depth).collect();
        for k in deeper {
            stack.remove(&k);
        }
        if name.to_lowercase().ends_with(".zip") {
            let joined: Vec<String> = stack.values().cloned().collect();
            paths.push(joined.join("/"));
        }
    }
    paths
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

fn arr(items: &[String]) -> String {
    let inner: Vec<String> = items.iter().map(|s| format!("\"{}\"", json_escape(s))).collect();
    format!("[{}]", inner.join(", "))
}

fn to_json(recs: &[Meta]) -> String {
    let mut out = String::from("[\n");
    for (i, m) in recs.iter().enumerate() {
        out.push_str(" {\n");
        out.push_str(&format!("  \"raw\": \"{}\",\n", json_escape(&m.raw)));
        out.push_str(&format!("  \"name\": \"{}\",\n", json_escape(&m.name)));
        out.push_str(&format!("  \"category\": \"{}\",\n", json_escape(&m.category)));
        out.push_str(&format!(
            "  \"subtype\": {},\n",
            m.subtype
                .as_ref()
                .map(|s| format!("\"{}\"", json_escape(s)))
                .unwrap_or("null".into())
        ));
        out.push_str(&format!("  \"modes\": {},\n", arr(&m.modes)));
        out.push_str(&format!("  \"flags\": {},\n", arr(&m.flags)));
        out.push_str(&format!(
            "  \"note\": {},\n",
            m.note
                .as_ref()
                .map(|s| format!("\"{}\"", json_escape(s)))
                .unwrap_or("null".into())
        ));
        out.push_str(&format!(
            "  \"dir\": {},\n",
            m.dir
                .as_ref()
                .map(|s| format!("\"{}\"", json_escape(s)))
                .unwrap_or("null".into())
        ));
        out.push_str(&format!("  \"path\": \"{}\"\n", json_escape(&m.path)));
        out.push_str(if i + 1 < recs.len() { " },\n" } else { " }\n" });
    }
    out.push_str("]\n");
    out
}

fn collect_zip_paths(root: &Path, out: &mut Vec<String>) {
    if let Ok(entries) = fs::read_dir(root) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect_zip_paths(&p, out);
            } else if p.extension().map(|x| x.eq_ignore_ascii_case("zip")).unwrap_or(false) {
                out.push(p.to_string_lossy().into_owned());
            }
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut root: Option<String> = None;
    let mut tree: Option<String> = None;
    let mut out = "metadata.json".to_string();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--tree" => { i += 1; tree = args.get(i).cloned(); }
            "--out" => { i += 1; out = args.get(i).cloned().unwrap_or(out); }
            s if !s.starts_with("--") => root = Some(s.to_string()),
            _ => {}
        }
        i += 1;
    }

    let paths: Vec<String> = if let Some(t) = tree {
        let text = fs::read_to_string(&t).unwrap_or_default();
        paths_from_tree(&text)
    } else if let Some(r) = root {
        let mut v = Vec::new();
        collect_zip_paths(Path::new(&r), &mut v);
        v
    } else {
        eprintln!("give a root dir or --tree file");
        std::process::exit(1);
    };

    let recs = walk_paths(&paths);
    fs::write(&out, to_json(&recs)).expect("write json");

    let mut cats: BTreeMap<String, usize> = BTreeMap::new();
    let mut both = 0;
    for m in &recs {
        *cats.entry(m.category.clone()).or_insert(0) += 1;
        if m.flags.iter().any(|f| f == "both_modes") {
            both += 1;
        }
    }
    println!("{} models -> {}", recs.len(), out);
    let cat_str: Vec<String> = cats.iter().map(|(k, v)| format!("{}: {}", k, v)).collect();
    println!("by category: {{{}}}", cat_str.join(", "));
    println!("both-modes (robot+alt): {}", both);
}
