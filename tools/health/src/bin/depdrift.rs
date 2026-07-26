//! depdrift — dependency version fragmentation across every Cargo.toml in a tree.
//!
//! The disease: one monorepo, many workspaces, and the same external crate pinned at
//! different versions in different corners — here, egui 0.31 vs 0.34 and wgpu 22 vs 29
//! literally decided which apps could compile under which toolchain. Nobody chose that
//! state; it accreted. This tool makes it visible in one screen: every external
//! dependency requested at ≥2 distinct version requirements, with the crates on each
//! side, most-fragmented first. Path-only internal deps are excluded (they have no
//! version to drift). A second section is the Cargo.lock census: which lockfiles exist
//! and their `version = N` format (v4 requires cargo ≥1.78 — the exact line that
//! decided in-sandbox buildability all session).
//!
//! TOML reading is a purpose-built line scanner for the dependency subset: `[deps*]`
//! sections, `name = "req"`, `name = { version = "req", … }`, and the multi-line
//! `[dependencies.name]` table form. `workspace = true` deps resolve to the
//! `[workspace.dependencies]` entry when it's in the same file; cross-file workspace
//! inheritance is attributed to the workspace root's toml (where the req lives).
//! Not handled (rare in this tree, noted when seen): dotted keys `foo.version = "…"`.
//!
//! Usage: depdrift [--all] DIR...      --all also prints single-version deps
//! Exit: always 0 (a report, not a gate) unless --fail-on-drift is given.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn collect_tomls(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_file() {
        out.push(path.to_path_buf());
        return;
    }
    let rd = match std::fs::read_dir(path) {
        Ok(r) => r,
        Err(_) => return,
    };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if p.is_dir() {
            if name == "target" || name == ".git" || name == "node_modules" || name == "build" || name == "vendor" {
                continue;
            }
            collect_tomls(&p, out);
        } else if name == "Cargo.toml" {
            out.push(p);
        } else if name == "Cargo.lock" {
            out.push(p);
        }
    }
}

/// Extract `key = "value"` from a line (first string value after the first `=`).
fn quoted_value(line: &str) -> Option<String> {
    let eq = line.find('=')?;
    let rest = &line[eq + 1..];
    let q1 = rest.find('"')?;
    let q2 = rest[q1 + 1..].find('"')?;
    Some(rest[q1 + 1..q1 + 1 + q2].to_string())
}

/// From an inline table `{ version = "x", … }` on this line, the version value.
fn inline_version(line: &str) -> Option<String> {
    let b = line.find('{')?;
    let body = &line[b..];
    let v = body.find("version")?;
    quoted_value(&body[v..])
}

struct Dep {
    name: String,
    req: String, // version requirement, or "path-only", or "workspace"
}

/// Parse the dependency subset of one Cargo.toml.
fn parse_deps(src: &str) -> Vec<Dep> {
    let mut out = Vec::new();
    let mut section = String::new();
    let mut in_table = false; // are we inside a [*dependencies.NAME] multi-line table?
    for raw in src.lines() {
        let line = raw.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            section = line.trim_matches(|c| c == '[' || c == ']').to_string();
            in_table = false;
            // multi-line dep table: [dependencies.foo] / [workspace.dependencies.foo] /
            // [target.'cfg(...)'.dependencies.foo]
            let name = ["dependencies.", "dev-dependencies.", "build-dependencies.", "workspace.dependencies."]
                .iter()
                .find_map(|pfx| section.strip_prefix(pfx))
                .or_else(|| section.find(".dependencies.").map(|ix| &section[ix + ".dependencies.".len()..]));
            if let Some(n) = name {
                if !n.is_empty() && !n.contains('.') {
                    out.push(Dep { name: n.to_string(), req: String::new() });
                    in_table = true;
                }
            }
            continue;
        }
        let in_flat_section = !in_table
            && (section == "dependencies"
                || section == "dev-dependencies"
                || section == "build-dependencies"
                || section == "workspace.dependencies"
                || section.ends_with(".dependencies"));
        // inside a multi-line dep table, a bare `version = "x"` fills the last pushed dep
        if in_table {
            if line.starts_with("version") {
                if let (Some(v), Some(last)) = (quoted_value(line), out.last_mut()) {
                    if last.req.is_empty() {
                        last.req = v;
                    }
                }
            }
            continue;
        }
        if !in_flat_section {
            continue;
        }
        // `name = …` forms (only in the flat dep sections)
        {
            let Some(eq) = line.find('=') else { continue };
            let name = line[..eq].trim().trim_matches('"').to_string();
            if name.is_empty() || name == "version" || name == "features" || name == "path" || name == "package" {
                continue;
            }
            let rhs = line[eq + 1..].trim();
            let req = if rhs.starts_with('{') {
                if let Some(v) = inline_version(line) {
                    v
                } else if rhs.contains("workspace") {
                    "workspace".to_string()
                } else if rhs.contains("path") {
                    "path-only".to_string()
                } else {
                    continue;
                }
            } else if rhs.starts_with('"') {
                match quoted_value(line) {
                    Some(v) => v,
                    None => continue,
                }
            } else {
                continue;
            };
            out.push(Dep { name, req });
        }
    }
    // multi-line tables that stayed req-less were path-only/workspace tables
    for d in out.iter_mut() {
        if d.req.is_empty() {
            d.req = "path-only".to_string();
        }
    }
    out
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let all = if let Some(p) = args.iter().position(|a| a == "--all") {
        args.remove(p);
        true
    } else {
        false
    };
    let fail_on_drift = if let Some(p) = args.iter().position(|a| a == "--fail-on-drift") {
        args.remove(p);
        true
    } else {
        false
    };
    if args.is_empty() {
        eprintln!("usage: depdrift [--all] [--fail-on-drift] DIR...");
        std::process::exit(2);
    }
    let mut files = Vec::new();
    for a in &args {
        collect_tomls(Path::new(a), &mut files);
    }

    // name -> req -> [who]
    let mut deps: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    let mut locks: Vec<(String, String)> = Vec::new();

    for f in &files {
        let fname = f.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let src = match std::fs::read_to_string(f) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let who = f.display().to_string();
        if fname == "Cargo.lock" {
            let ver = src
                .lines()
                .take(8)
                .find_map(|l| l.trim().strip_prefix("version = ").map(|v| v.trim().to_string()))
                .unwrap_or_else(|| "?".to_string());
            locks.push((who, ver));
            continue;
        }
        for d in parse_deps(&src) {
            if d.req == "path-only" || d.req == "workspace" {
                continue; // internal / inherited — no independent version to drift
            }
            deps.entry(d.name).or_default().entry(d.req).or_default().push(who.clone());
        }
    }

    let mut drifted = 0usize;
    println!("── dependency version drift ─────────────────────────────");
    let mut by_frag: Vec<(&String, &BTreeMap<String, Vec<String>>)> = deps.iter().collect();
    by_frag.sort_by_key(|(_, reqs)| std::cmp::Reverse(reqs.len()));
    for (name, reqs) in by_frag {
        if reqs.len() < 2 {
            if all {
                let (req, users) = reqs.iter().next().unwrap();
                println!("  {name} = {req}  ({} user(s))", users.len());
            }
            continue;
        }
        drifted += 1;
        println!("  {name} — {} distinct requirements:", reqs.len());
        for (req, users) in reqs {
            println!("      \"{req}\"  ← {}", users.join(", "));
        }
    }
    if drifted == 0 {
        println!("  (no drift — every external dep has a single requirement)");
    }

    println!("── Cargo.lock census ────────────────────────────────────");
    for (who, ver) in &locks {
        println!("  format v{ver}  {who}");
    }
    if locks.is_empty() {
        println!("  (no lockfiles found)");
    }

    std::process::exit(if fail_on_drift && drifted > 0 { 1 } else { 0 });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_dep_forms_parse() {
        let toml = r#"
[dependencies]
egui = "0.34"
wgpu = { version = "29", features = ["glsl"] }
uikit-egui = { path = "../uikit" }
serde = { workspace = true }

[dependencies.image]
version = "0.25.1"
default-features = false

[dev-dependencies]
tempfile = "3"
"#;
        let d = parse_deps(toml);
        let get = |n: &str| d.iter().find(|x| x.name == n).map(|x| x.req.clone());
        assert_eq!(get("egui").as_deref(), Some("0.34"));
        assert_eq!(get("wgpu").as_deref(), Some("29"));
        assert_eq!(get("uikit-egui").as_deref(), Some("path-only"));
        assert_eq!(get("serde").as_deref(), Some("workspace"));
        assert_eq!(get("image").as_deref(), Some("0.25.1"));
        assert_eq!(get("tempfile").as_deref(), Some("3"));
    }

    #[test]
    fn non_dep_sections_and_comments_are_ignored() {
        let toml = "[package]\nname = \"x\"\nversion = \"9.9\"\n# egui = \"0.1\"\n[features]\ndefault = []\n";
        assert!(parse_deps(toml).is_empty());
    }
}
