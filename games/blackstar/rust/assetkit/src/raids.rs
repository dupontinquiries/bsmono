//! Raid generation — port of gen_raids.py.
//!
//! Turns the boss fiction in raids-and-relics.md into playable content, and closes
//! the defect that raid_plan.h referenced maps ("kuiper", "unicron") which did not
//! exist in maps.json, so two of three phases loaded an undefined theme.
//!
//! The design point: each boss's lore already implies how its fight should work,
//! so the mechanic is DERIVED rather than assigned. Every raid records the premise
//! line its mechanic came from, so the reasoning stays inspectable.

use crate::json::{self, J};
use std::collections::HashMap;
use std::path::Path;

/// mech codes match the engine's raid_enc_t.mech:
/// 0 PURGE, 1 GROUND (anti-air), 2 AIR (elevated), 3 SYNC (min_units on plates)
pub const BOSSES: &[(&str, &str, i32, &str, &str)] = &[
 ("Astro Armor Titan Speakerman","The Last Resonance",3,"resonance",
  "sings across half a galaxy; fractures planets by matching tectonic resonance"),
 ("Devastator Jhiaxus","Prototype Omega",0,"rebuild",
  "destroying one only caused it to rebuild itself"),
 ("Plasma's Mechagodzilla","The Planet Burner",1,"burn",
  "its spinal plasma spine could ignite gas giants"),
 ("Cherno Alpha","The Walking Reactor",1,"march",
  "if it ever stops walking, the reactor reaches criticality"),
 ("Gigadrill","The Core Piercer",2,"undercut",
  "it drilled through moons, worlds, dwarf planets"),
 ("Hardboil","The Pressure King",0,"pressure",
  "built for depths that crush ordinary alloys"),
 ("Hussar Tank","The Charge",1,"charge",
  "it does not manoeuvre; it commits"),
 ("Judgementor","Executor of Empires",3,"sentence",
  "it did not fight wars, it ended them"),
 ("Primitive Mantis Mech","The First Hunter",0,"ambush",
  "the oldest predator pattern Cybertron ever built"),
 ("Pyrachnid","Forge Widow",3,"web",
  "it spins its own foundry around its prey"),
 ("Scorpion Commander","The Frontier Marshal",1,"frontier",
  "held a border that no longer exists"),
 ("Starlord","Twin Hammers",2,"sentry",
  "two hammers that are sentry guns"),
 ("Strider Tank","The Horizon Walker",2,"horizon",
  "it walks toward the horizon and the horizon moves"),
 ("Tremor","Planetbreaker",1,"quake",
  "the ground is its weapon"),
 ("Trenchmaster","The Ocean King",3,"flood",
  "the ocean king; the water rises to meet it"),
 ("Wuthering Waves Sigillum","The Seal",0,"seal",
  "a seal is only a door that has not been opened"),
 ("Titan Cameraman 3.0 Armor","Archivist",2,"archive",
  "it records everything; it has never stopped"),
 ("Sphere Robot","The Observer",0,"observe",
  "it watches, and being watched changes what you do"),
 ("Titan Cameraman 3.0 (What If...)","The Divergence",3,"diverge",
  "the recording and the event no longer agree"),
];

/// Maps the old header referenced but that were never defined.
pub const NEW_MAPS: &[(&str, &str, bool, &[&str], &str)] = &[
    ("kuiper", "void_edge", false, &["ring", "spire"],
     "shepherd moons + a frozen debris braid"),
    ("unicron", "devourer_red", true, &["arch", "spire", "tower"],
     "the maw; horizon curves the wrong way"),
];

pub struct Rng(u64);
impl Rng {
    pub fn new(seed: u64) -> Self { Rng(seed.wrapping_mul(6364136223846793005).wrapping_add(1)) }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13; self.0 ^= self.0 >> 7; self.0 ^= self.0 << 17; self.0
    }
    pub fn f(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (self.next() % 1_000_000) as f32 / 1_000_000.0 * (hi - lo)
    }
    pub fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T { &v[(self.next() % v.len() as u64) as usize] }
}

pub fn slug(s: &str) -> String {
    let mut o = String::new();
    let mut last_us = true;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() { o.push(c.to_ascii_lowercase()); last_us = false; }
        else if !last_us { o.push('_'); last_us = true; }
    }
    o.trim_matches('_').to_string()
}

pub struct Enc { pub x: f32, pub z: f32, pub kind: i32, pub mech: i32, pub min_units: i32 }
pub struct Phase { pub name: String, pub map: String, pub enc: Vec<Enc> }
pub struct Raid { pub id: String, pub name: String, pub title: String, pub arena: String,
                  pub premise: String, pub mech: i32, pub phases: Vec<Phase> }

/// The boss's own mechanic is guaranteed on at least one encounter -- a phase
/// where every encounter is mech 0 plays identically to having no mechanic.
fn encounters(rng: &mut Rng, mech: i32, n: usize) -> Vec<Enc> {
    (0..n).map(|i| {
        let m = if i == n - 1 { mech } else { *rng.pick(&[0, 0, mech]) };
        Enc { x: (rng.f(-95.0, 95.0) * 10.0).round() / 10.0,
              z: (rng.f(-95.0, 95.0) * 10.0).round() / 10.0,
              kind: if i == 0 { 1 } else { 0 },
              mech: m,
              min_units: if m == 3 { 3 } else { 1 } }
    }).collect()
}

pub fn generate(map_names: &[String], phases: usize, seed: u64) -> Vec<Raid> {
    let mut rng = Rng::new(seed);
    let pool: Vec<String> = map_names.iter().filter(|m| *m != "beach").cloned().collect();
    BOSSES.iter().map(|(name, title, mech, arena, line)| {
        let finale = if *mech == 3 { "unicron".to_string() } else { rng.pick(&pool).clone() };
        let mut chosen: Vec<String> = Vec::new();
        while chosen.len() + 1 < phases {
            let c = rng.pick(&pool).clone();
            if c != finale && !chosen.contains(&c) { chosen.push(c); }
        }
        chosen.push(finale);
        let last = title.split_whitespace().last().unwrap_or("End").to_string();
        let names = ["The Approach".to_string(), "The Gauntlet".to_string(),
                     format!("The {}", last)];
        Raid {
            id: slug(name), name: name.to_string(), title: title.to_string(),
            arena: arena.to_string(), premise: line.to_string(), mech: *mech,
            phases: chosen.iter().enumerate().map(|(i, mp)| Phase {
                name: names.get(i).cloned().unwrap_or_else(|| format!("Phase {}", i + 1)),
                map: mp.clone(),
                enc: encounters(&mut rng, *mech, 4),
            }).collect(),
        }
    }).collect()
}

/// Adds the missing maps if absent. Returns the names it added.
pub fn patch_maps(path: &Path) -> Vec<String> {
    let raw = match std::fs::read(path) { Ok(r) => r, Err(_) => return vec![] };
    let mut j = match json::parse(&raw) { Some(j) => j, None => return vec![] };
    let mut added = Vec::new();
    if let J::Obj(root) = &mut j {
        if let Some(J::Obj(maps)) = root.get_mut("maps") {
            for (id, atmo, energon, structs, landmark) in NEW_MAPS {
                if maps.contains_key(*id) { continue; }
                let mut m: HashMap<String, J> = HashMap::new();
                m.insert("atmo".into(), J::Str(atmo.to_string()));
                m.insert("energon".into(), J::Bool(*energon));
                m.insert("structures".into(),
                         J::Arr(structs.iter().map(|s| J::Str(s.to_string())).collect()));
                m.insert("landmark".into(), J::Str(landmark.to_string()));
                maps.insert(id.to_string(), J::Obj(m));
                added.push(id.to_string());
            }
        }
    }
    if !added.is_empty() { let _ = std::fs::write(path, json::write(&j, 0)); }
    added
}

pub fn map_names(path: &Path) -> Vec<String> {
    let raw = match std::fs::read(path) { Ok(r) => r, Err(_) => return vec![] };
    let j = match json::parse(&raw) { Some(j) => j, None => return vec![] };
    match j.get("maps") {
        Some(J::Obj(m)) => { let mut v: Vec<String> = m.keys().cloned().collect(); v.sort(); v }
        _ => vec![],
    }
}

pub fn to_json(raids: &[Raid]) -> String {
    let mut out = String::from("{\n \"_doc\": \"Raid content generated from raids-and-relics.md by assetkit raids. 'mech' is the engine's raid_enc_t.mech (0 purge, 1 ground, 2 air, 3 sync) chosen from each boss's fiction, not assigned at random -- see 'premise'.\",\n \"raids\": {\n");
    for (i, r) in raids.iter().enumerate() {
        out.push_str(&format!("  \"{}\": {{\n   \"name\": \"{}\",\n   \"title\": \"{}\",\n   \"arena\": \"{}\",\n   \"premise\": \"{}\",\n   \"mech\": {},\n   \"phases\": [\n",
            r.id, r.name.replace('"', "'"), r.title, r.arena, r.premise, r.mech));
        for (k, p) in r.phases.iter().enumerate() {
            out.push_str(&format!("    {{ \"name\": \"{}\", \"map\": \"{}\", \"encounters\": [\n", p.name, p.map));
            for (j2, e) in p.enc.iter().enumerate() {
                out.push_str(&format!("     {{ \"x\": {:.1}, \"z\": {:.1}, \"kind\": {}, \"mech\": {}, \"min_units\": {} }}{}\n",
                    e.x, e.z, e.kind, e.mech, e.min_units,
                    if j2 + 1 < p.enc.len() { "," } else { "" }));
            }
            out.push_str(&format!("    ] }}{}\n", if k + 1 < r.phases.len() { "," } else { "" }));
        }
        out.push_str(&format!("   ]\n  }}{}\n", if i + 1 < raids.len() { "," } else { "" }));
    }
    out.push_str(" }\n}\n");
    out
}

pub fn to_header(r: &Raid) -> String {
    let mut l = vec![
        "// AUTO-GENERATED by assetkit raids from raids-and-relics.md. Do not edit.".to_string(),
        "#ifndef RAID_PLAN_H".into(), "#define RAID_PLAN_H".into(), String::new(),
        "// mech: 0 PURGE  1 GROUND(anti-air, flyers forced down)  2 AIR(elevated, fly/grapple)  3 SYNC(min_units on plates)".into(),
        "typedef struct { float x, z; int kind; int mech; int min_units; } raid_enc_t;".into(),
        "typedef struct { const char *name; const char *map; int n_enc; const raid_enc_t *enc; } raid_phase_t;".into(),
        String::new(),
        format!("// {} -- \"{}\"", r.name, r.title),
        format!("// {}", r.premise), String::new(),
    ];
    for (i, p) in r.phases.iter().enumerate() {
        l.push(format!("static const raid_enc_t g_raid_enc_{}[] = {{", i));
        for e in &p.enc {
            l.push(format!("  {{ {:.1}f, {:.1}f, {}, {}, {} }},",
                           e.x, e.z, e.kind, e.mech, e.min_units));
        }
        l.push("};".into());
    }
    l.push(String::new());
    l.push(format!("#define NUM_RAID_PHASES {}", r.phases.len()));
    l.push("static const raid_phase_t g_raid_phases[NUM_RAID_PHASES] = {".into());
    for (i, p) in r.phases.iter().enumerate() {
        l.push(format!("  {{ \"{}\", \"{}\", {}, g_raid_enc_{} }},", p.name, p.map, p.enc.len(), i));
    }
    l.push("};".into());
    l.push(String::new());
    l.push("#endif".into());
    l.join("\n") + "\n"
}

/// Every map a phase references must exist, or that phase loads nothing. This is
/// the check the hand-written header lacked.
pub fn validate(raids: &[Raid], maps: &[String]) -> Vec<String> {
    let mut missing: Vec<String> = Vec::new();
    for r in raids {
        for p in &r.phases {
            if !maps.contains(&p.map) && !missing.contains(&p.map) {
                missing.push(p.map.clone());
            }
        }
    }
    missing
}
