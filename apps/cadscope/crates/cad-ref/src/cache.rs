//! cache.rs — so a corpus is scanned once rather than once per run.
//!
//! A full scan of the monorepo takes about 50 seconds, nearly all of it in the OBJ reader (reading
//! multi-megabyte text, welding vertices, censusing edges) and the document scanner. That is fine
//! for a diagnostic and impossible for anything the app does at startup, which is what currently
//! keeps every reference verb stuck behind `--ref-index`.
//!
//! Freshness is judged on **size and mtime**, not a content hash. Hashing four thousand files
//! means reading four thousand files, which is most of the cost the cache exists to avoid.
//!
//! The mtime is taken at **nanosecond** resolution, and that detail is not incidental. With
//! second granularity, an edit that preserves a file's length — changing `120 mm` to `200 mm` in a
//! spec, say — is invisible to the cache, and the first end-to-end test written here landed on
//! exactly that case and served stale facts. Nanoseconds cost nothing and reduce the stale window
//! to a rewrite in the same nanosecond at the same length, which is not a case worth engineering
//! against. `--no-cache` remains for anyone who wants none of this.
//!
//! The format is one escaped, tab-separated line per entry, with a version tag on the first line.
//! A cache that fails to parse — truncated, from an older version, corrupt — is discarded whole and
//! the scan simply runs. It is a cache: it may never be the reason something breaks.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use glam::Vec3;

use crate::{Dimension, DocFacts, Entry, Facts, Kind, ModelFacts, NiftyFacts, TextureFacts, TextureRole, Units};

const VERSION: &str = "cadref-cache v2";
/// Separator inside list fields. Escaped like the others, so it can never appear in a value.
const LIST_SEP: char = '\x1f';

/// What the cache remembers about a file, alongside the entry itself.
#[derive(Clone, Debug)]
pub struct Cached {
    pub bytes: u64,
    pub mtime: u64,
    pub entry: Entry,
}

/// Modification time in nanoseconds since the epoch. Saturates rather than wrapping, and returns
/// 0 for a filesystem that reports nothing — a file with no usable timestamp is simply always
/// re-read, which is the safe direction.
pub fn mtime_of(md: &fs::Metadata) -> u64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            LIST_SEP => out.push_str("\\u"),
            _ => out.push(c),
        }
    }
    out
}

fn unesc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('\\') => out.push('\\'),
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('u') => out.push(LIST_SEP),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

fn join(items: &[String]) -> String {
    items.iter().map(|s| esc(s)).collect::<Vec<_>>().join(&LIST_SEP.to_string())
}

fn split(s: &str) -> Vec<String> {
    if s.is_empty() {
        return Vec::new();
    }
    s.split(LIST_SEP).map(unesc).collect()
}

fn opt_f32(v: Option<f32>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| "-".into())
}

fn parse_opt_f32(s: &str) -> Option<f32> {
    if s == "-" { None } else { s.parse().ok() }
}

fn kind_tag(k: Kind) -> &'static str {
    match k {
        Kind::Model => "model",
        Kind::Texture => "texture",
        Kind::Niftyplot => "nifty",
        Kind::Doc => "doc",
    }
}

fn parse_kind(s: &str) -> Option<Kind> {
    Some(match s {
        "model" => Kind::Model,
        "texture" => Kind::Texture,
        "nifty" => Kind::Niftyplot,
        "doc" => Kind::Doc,
        _ => return None,
    })
}

fn role_tag(r: TextureRole) -> &'static str {
    match r {
        TextureRole::Base => "base",
        TextureRole::Normal => "norm",
        TextureRole::Specular => "spec",
    }
}

fn units_tag(u: Units) -> &'static str {
    u.label()
}

/// Serialize one entry to a line (without the trailing newline).
pub fn encode(c: &Cached) -> String {
    let e = &c.entry;
    let mut f = vec![
        esc(&e.name),
        kind_tag(e.kind).to_string(),
        esc(&e.path.to_string_lossy()),
        c.bytes.to_string(),
        c.mtime.to_string(),
    ];
    match &e.facts {
        Facts::Model(m) => {
            f.push("M".into());
            f.push(m.triangles.to_string());
            f.push(m.vertices.to_string());
            f.push(m.meshes.to_string());
            f.push(join(&m.mesh_names));
            f.push(join(&m.materials));
            match m.bounds {
                Some((lo, hi)) => f.push(format!(
                    "{} {} {} {} {} {}",
                    lo.x, lo.y, lo.z, hi.x, hi.y, hi.z
                )),
                None => f.push("-".into()),
            }
            f.push(opt_f32(m.volume));
            f.push(opt_f32(m.area));
            f.push(if m.closed { "1" } else { "0" }.into());
            f.push(m.open_edges.to_string());
            f.push(m.non_manifold_edges.to_string());
            f.push(units_tag(m.units).into());
        }
        Facts::Texture(t) => {
            f.push("T".into());
            f.push(t.width.to_string());
            f.push(t.height.to_string());
            f.push(role_tag(t.role).into());
        }
        Facts::Doc(d) => {
            f.push("D".into());
            f.push(esc(d.title.as_deref().unwrap_or("")));
            f.push(join(&d.headings));
            // Dimensions ride as triples flattened into one list.
            let flat: Vec<String> = d
                .dimensions
                .iter()
                .flat_map(|dim| {
                    [dim.value.to_string(), dim.unit.clone(), dim.context.clone()]
                })
                .collect();
            f.push(join(&flat));
        }
        Facts::Nifty(n) => {
            f.push("N".into());
            f.push(esc(&n.kind));
            f.push(esc(n.title.as_deref().unwrap_or("")));
            f.push(n.components.to_string());
            f.push(n.pins.to_string());
            f.push(n.traces.to_string());
            match n.extent_mm {
                Some((lo, hi)) => f.push(format!("{} {} {} {}", lo.x, lo.y, hi.x, hi.y)),
                None => f.push("-".into()),
            }
            let flat: Vec<String> = n
                .keepouts
                .iter()
                .flat_map(|(a, b)| [a.x.to_string(), a.y.to_string(), b.x.to_string(), b.y.to_string()])
                .collect();
            f.push(join(&flat));
            f.push(n.warnings.to_string());
        }
        Facts::Unparsed => f.push("U".into()),
    }
    f.join("\t")
}

/// Parse one line back. `None` for anything malformed — the caller discards the whole cache.
pub fn decode(line: &str) -> Option<Cached> {
    let f: Vec<&str> = line.split('\t').collect();
    if f.len() < 6 {
        return None;
    }
    let name = unesc(f[0]);
    let kind = parse_kind(f[1])?;
    let path = PathBuf::from(unesc(f[2]));
    let bytes: u64 = f[3].parse().ok()?;
    let mtime: u64 = f[4].parse().ok()?;
    let facts = match f[5] {
        "M" => {
            if f.len() < 18 {
                return None;
            }
            let bounds = if f[11] == "-" {
                None
            } else {
                let v: Vec<f32> = f[11].split(' ').filter_map(|x| x.parse().ok()).collect();
                (v.len() == 6).then(|| (Vec3::new(v[0], v[1], v[2]), Vec3::new(v[3], v[4], v[5])))
            };
            Facts::Model(ModelFacts {
                triangles: f[6].parse().ok()?,
                vertices: f[7].parse().ok()?,
                meshes: f[8].parse().ok()?,
                mesh_names: split(f[9]),
                materials: split(f[10]),
                bounds,
                volume: parse_opt_f32(f[12]),
                area: parse_opt_f32(f[13]),
                closed: f[14] == "1",
                open_edges: f[15].parse().ok()?,
                non_manifold_edges: f[16].parse().ok()?,
                units: Units::parse(f[17]),
            })
        }
        "T" => {
            if f.len() < 9 {
                return None;
            }
            Facts::Texture(TextureFacts {
                width: f[6].parse().ok()?,
                height: f[7].parse().ok()?,
                role: match f[8] {
                    "norm" => TextureRole::Normal,
                    "spec" => TextureRole::Specular,
                    _ => TextureRole::Base,
                },
            })
        }
        "D" => {
            if f.len() < 9 {
                return None;
            }
            let title = unesc(f[6]);
            let flat = split(f[8]);
            let dimensions = flat
                .chunks_exact(3)
                .filter_map(|c| {
                    Some(Dimension {
                        value: c[0].parse().ok()?,
                        unit: c[1].clone(),
                        context: c[2].clone(),
                    })
                })
                .collect();
            Facts::Doc(DocFacts {
                title: (!title.is_empty()).then_some(title),
                headings: split(f[7]),
                dimensions,
            })
        }
        "N" => {
            if f.len() < 14 {
                return None;
            }
            let title = unesc(f[7]);
            let extent = if f[11] == "-" {
                None
            } else {
                let v: Vec<f32> = f[11].split(' ').filter_map(|x| x.parse().ok()).collect();
                (v.len() == 4).then(|| (glam::Vec2::new(v[0], v[1]), glam::Vec2::new(v[2], v[3])))
            };
            let flat = split(f[12]);
            let keepouts = flat
                .chunks_exact(4)
                .filter_map(|c| {
                    Some((
                        glam::Vec2::new(c[0].parse().ok()?, c[1].parse().ok()?),
                        glam::Vec2::new(c[2].parse().ok()?, c[3].parse().ok()?),
                    ))
                })
                .collect();
            Facts::Nifty(NiftyFacts {
                kind: unesc(f[6]),
                title: (!title.is_empty()).then_some(title),
                components: f[8].parse().ok()?,
                pins: f[9].parse().ok()?,
                traces: f[10].parse().ok()?,
                extent_mm: extent,
                keepouts,
                warnings: f[13].parse().ok()?,
            })
        }
        "U" => Facts::Unparsed,
        _ => return None,
    };
    Some(Cached { bytes, mtime, entry: Entry { name, kind, path, bytes, facts } })
}

/// Write the cache. Failure is reported but is never fatal to a scan.
pub fn save(path: &Path, items: &[Cached]) -> std::io::Result<()> {
    let mut out = String::with_capacity(items.len() * 128);
    out.push_str(VERSION);
    out.push('\n');
    for c in items {
        out.push_str(&encode(c));
        out.push('\n');
    }
    let mut f = fs::File::create(path)?;
    f.write_all(out.as_bytes())
}

/// Load a cache into a path-keyed map. Any parse failure discards the whole file: a partially
/// understood cache is more dangerous than none.
pub fn load(path: &Path) -> Option<HashMap<PathBuf, Cached>> {
    let text = fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    if lines.next()? != VERSION {
        return None;
    }
    let mut map = HashMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let c = decode(line)?;
        map.insert(c.entry.path.clone(), c);
    }
    Some(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Cached> {
        vec![
            Cached {
                bytes: 1234,
                mtime: 99,
                entry: Entry {
                    name: "puma".into(),
                    kind: Kind::Model,
                    path: "games/puma.obj".into(),
                    bytes: 1234,
                    facts: Facts::Model(ModelFacts {
                        triangles: 12,
                        vertices: 8,
                        meshes: 1,
                        mesh_names: vec!["arm_L".into(), "arm R".into()],
                        materials: vec!["steel".into()],
                        bounds: Some((Vec3::new(-1.0, -2.0, -3.0), Vec3::new(1.0, 2.0, 3.0))),
                        volume: Some(1.5),
                        area: None,
                        closed: true,
                        open_edges: 0,
                        non_manifold_edges: 0,
                        units: Units::Mm,
                    }),
                },
            },
            Cached {
                bytes: 10,
                mtime: 1,
                entry: Entry {
                    name: "spec".into(),
                    kind: Kind::Doc,
                    path: "docs/spec.md".into(),
                    bytes: 10,
                    // Deliberately nasty: tabs, newlines and the list separator inside a value.
                    facts: Facts::Doc(DocFacts {
                        title: Some("A\ttitle".into()),
                        headings: vec!["one\nline".into(), "two".into()],
                        dimensions: vec![Dimension {
                            value: 120.0,
                            unit: "mm".into(),
                            context: format!("cavity\tis 120 mm{LIST_SEP}wide"),
                        }],
                    }),
                },
            },
        ]
    }

    #[test]
    fn nifty_facts_survive_the_cache() {
        let c = Cached {
            bytes: 42,
            mtime: 7,
            entry: Entry {
                name: "breakout".into(),
                kind: Kind::Niftyplot,
                path: "refs/breakout.nifty".into(),
                bytes: 42,
                facts: Facts::Nifty(NiftyFacts {
                    kind: "hardware".into(),
                    title: Some("I2C breakout v1".into()),
                    components: 2,
                    pins: 8,
                    traces: 4,
                    extent_mm: Some((glam::Vec2::new(10.0, 20.0), glam::Vec2::new(60.0, 30.0))),
                    keepouts: vec![(glam::Vec2::new(40.0, 10.0), glam::Vec2::new(50.0, 30.0))],
                    warnings: 0,
                }),
            },
        };
        let back = decode(&encode(&c)).expect("decodes");
        assert_eq!(back.entry.facts, c.entry.facts, "board geometry must survive exactly");
    }

    #[test]
    fn round_trips_through_the_line_format() {
        for c in sample() {
            let line = encode(&c);
            assert!(!line.contains('\n'), "a record must stay on one line");
            let back = decode(&line).expect("decodes");
            assert_eq!(back.entry.name, c.entry.name);
            assert_eq!(back.entry.path, c.entry.path);
            assert_eq!(back.mtime, c.mtime);
            assert_eq!(back.entry.facts, c.entry.facts, "facts must survive exactly");
        }
    }

    #[test]
    fn separators_inside_values_survive() {
        let c = &sample()[1];
        let back = decode(&encode(c)).unwrap();
        let Facts::Doc(d) = &back.entry.facts else { panic!("wrong kind") };
        assert_eq!(d.title.as_deref(), Some("A\ttitle"));
        assert_eq!(d.headings[0], "one\nline");
        assert!(d.dimensions[0].context.contains(LIST_SEP), "unit separator preserved");
    }

    #[test]
    fn saves_and_loads_a_whole_file() {
        let dir = std::env::temp_dir().join(format!("cadref-cache-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("index.cache");
        save(&path, &sample()).unwrap();
        let map = load(&path).expect("loads");
        assert_eq!(map.len(), 2);
        assert!(map.contains_key(Path::new("games/puma.obj")));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_corrupt_or_foreign_cache_is_discarded_whole() {
        let dir = std::env::temp_dir().join(format!("cadref-bad-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.cache");
        // Wrong version tag.
        fs::write(&path, "cadref-cache v0\nwhatever\n").unwrap();
        assert!(load(&path).is_none());
        // Right version, damaged record: still all-or-nothing.
        fs::write(&path, format!("{VERSION}\nnot\ta\treal\trecord\n")).unwrap();
        assert!(load(&path).is_none());
        fs::remove_dir_all(&dir).ok();
    }
}
