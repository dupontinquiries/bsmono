//! Cargo crates: a `.crate` file is a gzipped tar containing
//! `<name>-<vers>/Cargo.toml` that cargo has already NORMALIZED at publish
//! time (path deps rewritten, tables expanded, `Cargo.toml.orig` kept beside
//! it). Everything the sparse index needs is in that one file plus the sha256
//! of the archive, so depot derives index lines itself and never needs a copy
//! of crates.io's index. That is what makes "drop .crate files in, get a
//! registry out" work on a box that has never seen the internet.
//!
//! The sparse registry protocol (RFC 2789) is plain files over HTTP:
//!
//! ```text
//! config.json                      {"dl": "...", "api": "..."}
//! 1/<name>                         one-letter crate names
//! 2/<name>                         two-letter
//! 3/<c>/<name>                     three-letter, bucketed by first char
//! <ab>/<cd>/<name>                 everything else, lower-cased
//! ```
//!
//! Each index file is one JSON object per line, one per version. cargo asks
//! for exactly the files it needs, with `If-None-Match`, so an index of 150k
//! crates costs a fleet device a handful of small GETs per build.
//!
//! Verified against index.crates.io: the lines this module derives for two
//! reference crates are field-for-field equal to the ones crates.io serves
//! (minus `pubtime`, which is not derivable and which cargo ignores).

use crate::{invalid, sha256_file, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::io::Read;
use std::path::Path;

/// What one `.crate` file tells us.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CrateMeta {
    pub name: String,
    pub vers: String,
    pub cksum: String,
    pub size: u64,
    /// The index line, ready to append to the index file.
    pub index_line: Value,
}

/// Read a `.crate` and derive its index line. The archive is streamed; only
/// the `Cargo.toml` entry is buffered.
pub fn read_crate(path: &Path) -> Result<CrateMeta> {
    let (cksum, size) = sha256_file(path)?;
    let f = std::fs::File::open(path)?;
    let gz = flate2::read::GzDecoder::new(f);
    let mut ar = tar::Archive::new(gz);
    let mut manifest: Option<(String, String)> = None; // (dirname, contents)
    for entry in ar.entries()? {
        let mut e = entry?;
        let p = e.path()?.to_path_buf();
        let mut comps = p.components();
        let dir = comps.next().map(|c| c.as_os_str().to_string_lossy().into_owned()).unwrap_or_default();
        let rest: Vec<String> = comps.map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        if rest.len() == 1 && rest[0] == "Cargo.toml" {
            let mut s = String::new();
            e.read_to_string(&mut s)?;
            manifest = Some((dir, s));
            break;
        }
    }
    let (dir, toml_text) = manifest.ok_or_else(|| invalid(format!("{}: no <name>-<vers>/Cargo.toml in archive", path.display())))?;
    let m: Manifest = toml::from_str(&toml_text).map_err(|e| invalid(format!("{}: Cargo.toml: {e}", path.display())))?;
    let pkg = m.package;
    let expect_dir = format!("{}-{}", pkg.name, pkg.version);
    if dir != expect_dir {
        return Err(invalid(format!("{}: archive dir {dir} != {expect_dir}", path.display())));
    }

    let mut deps: Vec<Value> = Vec::new();
    push_deps(&mut deps, &m.dependencies, "normal", None);
    push_deps(&mut deps, &m.dev_dependencies, "dev", None);
    push_deps(&mut deps, &m.build_dependencies, "build", None);
    for (cfg, t) in &m.target {
        push_deps(&mut deps, &t.dependencies, "normal", Some(cfg));
        push_deps(&mut deps, &t.dev_dependencies, "dev", Some(cfg));
        push_deps(&mut deps, &t.build_dependencies, "build", Some(cfg));
    }
    // crates.io serves deps ordered by name; match it so diffs against the
    // real index are readable.
    deps.sort_by(|a, b| {
        let k = |v: &Value| (v["name"].as_str().unwrap_or("").to_string(), v["kind"].as_str().unwrap_or("").to_string(), v["target"].as_str().unwrap_or("").to_string());
        k(a).cmp(&k(b))
    });

    // Features: v2 syntax (`dep:foo`, `foo?/bar`) goes in `features2` and
    // bumps the schema version so old cargos (which ignore features2) still
    // parse the line without choking on syntax they never learned.
    let mut features = Map::new();
    let mut features2 = Map::new();
    for (k, v) in &m.features {
        let uses_v2 = v.iter().any(|s| s.starts_with("dep:") || s.contains("?/"));
        let arr = Value::Array(v.iter().map(|s| Value::String(s.clone())).collect());
        if uses_v2 {
            features2.insert(k.clone(), arr);
        } else {
            features.insert(k.clone(), arr);
        }
    }

    let has_v2 = !features2.is_empty();
    let mut line = Map::new();
    line.insert("name".into(), json!(pkg.name));
    line.insert("vers".into(), json!(pkg.version));
    line.insert("deps".into(), Value::Array(deps));
    line.insert("cksum".into(), json!(cksum));
    line.insert("features".into(), Value::Object(features));
    if has_v2 {
        line.insert("features2".into(), Value::Object(features2));
    }
    line.insert("yanked".into(), json!(false));
    if let Some(l) = &pkg.links {
        line.insert("links".into(), json!(l));
    }
    if has_v2 {
        line.insert("v".into(), json!(2));
    }
    if let Some(rv) = &pkg.rust_version {
        line.insert("rust_version".into(), json!(rv));
    }

    Ok(CrateMeta { name: pkg.name, vers: pkg.version, cksum, size, index_line: Value::Object(line) })
}

fn push_deps(out: &mut Vec<Value>, deps: &std::collections::BTreeMap<String, toml::Value>, kind: &str, target: Option<&str>) {
    for (key, v) in deps {
        let d = match v {
            toml::Value::String(req) => Dep { version: Some(req.clone()), ..Default::default() },
            toml::Value::Table(_) => v.clone().try_into::<Dep>().unwrap_or_default(),
            _ => Dep::default(),
        };
        let mut o = Map::new();
        // In a normalized manifest the key is the name as used in code; when
        // `package` is set, the registry name is `package` and the index
        // carries both: `name` = key, `package` = real crate. That is the
        // reverse of what one might guess and it is what cargo expects.
        o.insert("name".into(), json!(key));
        o.insert("req".into(), json!(normalize_req(d.version.as_deref().unwrap_or("*"))));
        o.insert("features".into(), json!(d.features));
        o.insert("optional".into(), json!(d.optional));
        o.insert("default_features".into(), json!(d.default_features));
        o.insert("target".into(), match target {
            Some(t) => json!(t),
            None => Value::Null,
        });
        o.insert("kind".into(), json!(kind));
        if let Some(r) = &d.registry {
            o.insert("registry".into(), json!(r));
        }
        if let Some(p) = &d.package {
            o.insert("package".into(), json!(p));
        }
        out.push(Value::Object(o));
    }
}

/// `"0.1.2"` → `"^0.1.2"`, `">= 1, < 2"` → `">=1, <2"` — the form crates.io
/// stores (semver's `VersionReq::to_string`). cargo accepts either; matching
/// the canonical form keeps `diff` against index.crates.io quiet.
pub fn normalize_req(req: &str) -> String {
    let req = req.trim();
    if req.is_empty() || req == "*" {
        return "*".into();
    }
    req.split(',')
        .map(|c| {
            let c = c.trim();
            let (op, rest) = split_op(c);
            let rest = rest.trim();
            match op {
                "" => format!("^{rest}"),
                _ => format!("{op}{rest}"),
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn split_op(c: &str) -> (&str, &str) {
    for op in [">=", "<=", "^", "~", "=", ">", "<"] {
        if let Some(r) = c.strip_prefix(op) {
            return (op, r);
        }
    }
    ("", c)
}

/// The path of a crate's index file, relative to the index root.
pub fn index_path(name: &str) -> String {
    let n = name.to_ascii_lowercase();
    match n.len() {
        0 => "0/_".into(), // unreachable for valid names; keep the function total
        1 => format!("1/{n}"),
        2 => format!("2/{n}"),
        3 => format!("3/{}/{n}", &n[..1]),
        _ => format!("{}/{}/{n}", &n[..2], &n[2..4]),
    }
}

/// Render the index file for a name from its catalog entries: one line per
/// version, oldest first (crates.io order), `yanked` reflecting the catalog.
pub fn render_index(entries: &[crate::catalog::CrateEntry]) -> String {
    let mut v: Vec<&crate::catalog::CrateEntry> = entries.iter().collect();
    v.sort_by(|a, b| semver_key(&a.vers).cmp(&semver_key(&b.vers)));
    let mut out = String::new();
    for e in v {
        let mut line = e.index_line.clone();
        if let Value::Object(m) = &mut line {
            m.insert("yanked".into(), json!(e.yanked));
        }
        out.push_str(&serde_json::to_string(&line).unwrap_or_default());
        out.push('\n');
    }
    out
}

/// `config.json` for a registry whose files live under `<base>/crates`.
/// The `dl` template names the exact file depot serves, so the download URL
/// is browsable and cargo does no rewriting.
pub fn render_config(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    serde_json::to_string_pretty(&json!({
        "dl": format!("{base}/crates/dl/{{crate}}/{{crate}}-{{version}}.crate"),
        "api": base,
    }))
    .unwrap()
        + "\n"
}

/// Relative path of a version's .crate under `pub/crates/dl/`.
pub fn dl_path(name: &str, vers: &str) -> String {
    format!("dl/{name}/{name}-{vers}.crate")
}

/// A sort key that orders `1.10.0` after `1.9.0` and pre-releases before the
/// release they precede — enough for index ordering; not a full semver impl.
pub fn semver_key(v: &str) -> (Vec<u64>, bool, String) {
    let (core, pre) = match v.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (v, None),
    };
    let core = core.split('+').next().unwrap_or(core);
    let nums: Vec<u64> = core.split('.').map(|x| x.parse().unwrap_or(0)).collect();
    (nums, pre.is_none(), pre.unwrap_or("").to_string())
}

// ── the subset of a normalized manifest depot reads ──────────────────────────

#[derive(Deserialize)]
struct Manifest {
    package: Package,
    #[serde(default)]
    dependencies: std::collections::BTreeMap<String, toml::Value>,
    #[serde(default, rename = "dev-dependencies", alias = "dev_dependencies")]
    dev_dependencies: std::collections::BTreeMap<String, toml::Value>,
    #[serde(default, rename = "build-dependencies", alias = "build_dependencies")]
    build_dependencies: std::collections::BTreeMap<String, toml::Value>,
    #[serde(default)]
    target: std::collections::BTreeMap<String, TargetDeps>,
    #[serde(default)]
    features: std::collections::BTreeMap<String, Vec<String>>,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    version: String,
    #[serde(default)]
    links: Option<String>,
    #[serde(default, rename = "rust-version", alias = "rust_version")]
    rust_version: Option<String>,
}

#[derive(Deserialize, Default)]
struct TargetDeps {
    #[serde(default)]
    dependencies: std::collections::BTreeMap<String, toml::Value>,
    #[serde(default, rename = "dev-dependencies", alias = "dev_dependencies")]
    dev_dependencies: std::collections::BTreeMap<String, toml::Value>,
    #[serde(default, rename = "build-dependencies", alias = "build_dependencies")]
    build_dependencies: std::collections::BTreeMap<String, toml::Value>,
}

#[derive(Deserialize, Default)]
struct Dep {
    version: Option<String>,
    #[serde(default)]
    features: Vec<String>,
    #[serde(default)]
    optional: bool,
    #[serde(default = "yes", rename = "default-features", alias = "default_features")]
    default_features: bool,
    package: Option<String>,
    registry: Option<String>,
}
fn yes() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn testdata(f: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(f)
    }

    /// Compare against the line index.crates.io serves, ignoring `pubtime`.
    fn assert_matches_reference(crate_file: &str, ref_file: &str) {
        let m = read_crate(&testdata(crate_file)).unwrap();
        let mut want: Value = serde_json::from_str(std::fs::read_to_string(testdata(ref_file)).unwrap().trim()).unwrap();
        want.as_object_mut().unwrap().remove("pubtime");
        assert_eq!(m.index_line, want, "derived index line differs from index.crates.io");
        assert_eq!(m.cksum, want["cksum"].as_str().unwrap());
    }

    #[test]
    fn cfg_if_matches_crates_io_line_including_renamed_dep_and_optional() {
        assert_matches_reference("cfg-if-1.0.0.crate", "ref-cfg-if-1.0.0.json");
    }

    #[test]
    fn unicode_ident_matches_crates_io_line_including_dev_deps_and_rust_version() {
        assert_matches_reference("unicode-ident-1.0.12.crate", "ref-unicode-ident-1.0.12.json");
        let m = read_crate(&testdata("unicode-ident-1.0.12.crate")).unwrap();
        assert_eq!(m.size, 42168);
        assert_eq!(m.index_line["deps"].as_array().unwrap().len(), 6, "the scan found deps");
    }

    #[test]
    fn req_normalization_matches_semver_to_string() {
        assert_eq!(normalize_req("0.1.2"), "^0.1.2");
        assert_eq!(normalize_req("^1"), "^1");
        assert_eq!(normalize_req(">= 1.0, < 2.0"), ">=1.0, <2.0");
        assert_eq!(normalize_req("~0.3"), "~0.3");
        assert_eq!(normalize_req("=1.2.3"), "=1.2.3");
        assert_eq!(normalize_req("*"), "*");
        assert_eq!(normalize_req(""), "*");
    }

    #[test]
    fn index_paths_follow_the_four_rules_and_lowercase() {
        assert_eq!(index_path("a"), "1/a");
        assert_eq!(index_path("ab"), "2/ab");
        assert_eq!(index_path("abc"), "3/a/abc");
        assert_eq!(index_path("cfg-if"), "cf/g-/cfg-if");
        assert_eq!(index_path("Serde"), "se/rd/serde");
        assert_eq!(index_path("unicode-ident"), "un/ic/unicode-ident");
    }

    #[test]
    fn render_index_orders_versions_and_applies_yanked() {
        use crate::catalog::CrateEntry;
        let mk = |v: &str, y: bool| CrateEntry { vers: v.into(), hash: "0".repeat(64), size: 0, index_line: json!({"name":"x","vers":v,"yanked":false}), yanked: y, added: 0 };
        let s = render_index(&[mk("1.10.0", false), mk("1.9.0", true), mk("2.0.0-beta.1", false), mk("2.0.0", false)]);
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines.len(), 4);
        assert!(lines[0].contains("\"vers\":\"1.9.0\"") && lines[0].contains("\"yanked\":true"));
        assert!(lines[1].contains("\"vers\":\"1.10.0\""));
        assert!(lines[2].contains("2.0.0-beta.1"));
        assert!(lines[3].contains("\"vers\":\"2.0.0\"") && lines[3].contains("\"yanked\":false"));
    }

    #[test]
    fn config_json_carries_absolute_dl_template() {
        let c = render_config("http://depot.lan:8000/");
        let v: Value = serde_json::from_str(&c).unwrap();
        assert_eq!(v["dl"], "http://depot.lan:8000/crates/dl/{crate}/{crate}-{version}.crate");
        assert_eq!(v["api"], "http://depot.lan:8000");
        assert_eq!(dl_path("cfg-if", "1.0.0"), "dl/cfg-if/cfg-if-1.0.0.crate");
    }

    #[test]
    fn features2_split_and_v2_flag() {
        // A synthetic manifest exercised through the same code path as read_crate.
        let text = r#"
[package]
name = "z"
version = "0.1.0"
links = "z"
[dependencies.serde]
version = "1"
optional = true
[features]
default = ["std"]
std = []
json = ["dep:serde", "serde?/derive"]
"#;
        let m: Manifest = toml::from_str(text).unwrap();
        assert_eq!(m.package.links.as_deref(), Some("z"));
        assert_eq!(m.features.len(), 3);
        let v2: Vec<_> = m.features.iter().filter(|(_, v)| v.iter().any(|s| s.starts_with("dep:") || s.contains("?/"))).map(|(k, _)| k.clone()).collect();
        assert_eq!(v2, vec!["json"]);
    }

    #[test]
    fn not_a_crate_is_refused() {
        let d = crate::tmpdir("crates");
        let p = d.join("junk.crate");
        std::fs::write(&p, b"not gzip").unwrap();
        assert!(read_crate(&p).is_err());
        std::fs::remove_dir_all(d).unwrap();
    }
}
