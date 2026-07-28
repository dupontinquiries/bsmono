//! cad-ref — the reference corpus (plan §17). A designer says "same volume as `puma`" or "the
//! texture from `scorponok_body`", and something has to turn that name into a *measured fact*
//! rather than into a language model's recollection of a file it never opened. This crate is that
//! something, and it deliberately has no LLM dependency at all: it walks real files, parses real
//! headers, and answers by name.
//!
//! The division of labour that makes the whole design honest: `cad-liaison` (M3) is the only crate
//! that speaks natural language, and all it may emit is a constrained query — kind, name, verb.
//! Resolution and measurement happen here, in Rust. If a name is not in the index, the answer is
//! that it is not in the index; there is no path by which a plausible-sounding entry can be
//! invented, because invention is not one of the operations this crate offers.
//!
//! std only. Untrusted input: every parser here reads files that were produced by other tools, so
//! every offset is bounds-checked and a malformed file yields `None`, never a panic.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use glam::Vec3;

pub mod doc;
pub mod iqm;
pub mod png;

/// What sort of reference an entry is. The four kinds of §17.1.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    Model,
    Texture,
    Niftyplot,
    Doc,
}

impl Kind {
    pub fn of_path(p: &Path) -> Option<Kind> {
        let ext = p.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "iqm" | "glb" | "gltf" | "obj" => Kind::Model,
            "png" | "jpg" | "jpeg" | "tga" => Kind::Texture,
            "nifty" | "niftyplot" => Kind::Niftyplot,
            "md" | "txt" => Kind::Doc,
            _ => return None,
        })
    }
}

/// Facts derived from a model file. Everything optional: an OBJ tells us less than an IQM, and a
/// truncated file tells us less again. Absence is reported as absence.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelFacts {
    pub triangles: u32,
    pub vertices: u32,
    pub meshes: u32,
    /// Sub-mesh names, in file order — this is what makes "arms like velgorn" addressable.
    pub mesh_names: Vec<String>,
    /// Material names referenced by the meshes; the bridge to the texture corpus.
    pub materials: Vec<String>,
    /// Axis-aligned bounds, when the file carries them (IQM stores them per animation frame).
    pub bounds: Option<(Vec3, Vec3)>,
}

impl ModelFacts {
    /// Bounding-box volume. Not the solid's volume — that needs the mesh itself, which the
    /// `volume of` verb (M5) computes with the same divergence-theorem sum the mesher uses. This
    /// is the cheap proxy available from headers alone, and it is named so nobody confuses them.
    pub fn bounds_volume(&self) -> Option<f32> {
        let (lo, hi) = self.bounds?;
        let d = hi - lo;
        Some((d.x * d.y * d.z).abs())
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextureFacts {
    pub width: u32,
    pub height: u32,
    /// The engine convention: `<name>.png`, `norm_<name>.png`, `spec_<name>.png`. This is the
    /// *role* this file plays, and `Entry::name` is stripped of the prefix so all three group.
    pub role: TextureRole,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TextureRole {
    #[default]
    Base,
    Normal,
    Specular,
}

/// A dimensioned statement lifted out of prose — the raw material of the `fit <doc>` verb, which
/// turns a spec into a checklist the model is measured against.
#[derive(Clone, Debug, PartialEq)]
pub struct Dimension {
    pub value: f32,
    pub unit: String,
    /// The line it came from, kept so the checklist can quote its source rather than a bare number.
    pub context: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DocFacts {
    pub title: Option<String>,
    pub headings: Vec<String>,
    pub dimensions: Vec<Dimension>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Facts {
    Model(ModelFacts),
    Texture(TextureFacts),
    Doc(DocFacts),
    /// Indexed but not yet parsed — niftyplot lands at M6 with the hardware-kind implementation.
    Unparsed,
}

/// One addressable reference.
#[derive(Clone, Debug)]
pub struct Entry {
    /// The spoken name: file stem, with a texture role prefix stripped so a material's three maps
    /// share one name.
    pub name: String,
    pub kind: Kind,
    pub path: PathBuf,
    pub bytes: u64,
    pub facts: Facts,
}

/// Roots to scan, and what to leave out.
#[derive(Clone, Debug)]
pub struct ScanOpts {
    /// Directory names skipped wholesale, at any depth.
    pub exclude_dirs: Vec<String>,
    pub max_depth: usize,
    /// Stop after this many entries, so a mis-pointed root cannot run away.
    pub max_entries: usize,
}

impl Default for ScanOpts {
    fn default() -> Self {
        ScanOpts {
            exclude_dirs: vec![".git".into(), "target".into(), "node_modules".into(), "build".into()],
            max_depth: 12,
            max_entries: 100_000,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Index {
    pub entries: Vec<Entry>,
    by_name: HashMap<String, Vec<usize>>,
}

impl Index {
    /// Walk `root`, classify by extension, and parse what is cheap to parse. Unreadable files and
    /// unrecognised extensions are skipped silently — a corpus is a pile of other people's output
    /// and must never be able to fail the scan.
    pub fn scan(root: &Path, opts: &ScanOpts) -> Index {
        let mut idx = Index::default();
        let mut stack = vec![(root.to_path_buf(), 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            if depth > opts.max_depth || idx.entries.len() >= opts.max_entries {
                continue;
            }
            let Ok(rd) = fs::read_dir(&dir) else { continue };
            for ent in rd.flatten() {
                let path = ent.path();
                let Ok(ft) = ent.file_type() else { continue };
                if ft.is_dir() {
                    let skip = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| opts.exclude_dirs.iter().any(|e| e == n))
                        .unwrap_or(true);
                    if !skip {
                        stack.push((path, depth + 1));
                    }
                } else if ft.is_file() {
                    if let Some(e) = Self::entry_of(&path) {
                        idx.entries.push(e);
                    }
                }
            }
        }
        idx.entries.sort_by(|a, b| a.name.cmp(&b.name).then(a.path.cmp(&b.path)));
        idx.reindex();
        idx
    }

    fn entry_of(path: &Path) -> Option<Entry> {
        let kind = Kind::of_path(path)?;
        let stem = path.file_stem()?.to_str()?.to_string();
        let bytes = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let (name, facts) = match kind {
            Kind::Model => (stem, Facts::Model(iqm::facts(path).unwrap_or_default())),
            Kind::Texture => {
                let (name, role) = split_texture_role(&stem);
                let mut f = png::facts(path).unwrap_or_default();
                f.role = role;
                (name, Facts::Texture(f))
            }
            Kind::Doc => (stem, Facts::Doc(doc::facts(path).unwrap_or_default())),
            Kind::Niftyplot => (stem, Facts::Unparsed),
        };
        Some(Entry { name, kind, path: path.to_path_buf(), bytes, facts })
    }

    fn reindex(&mut self) {
        self.by_name.clear();
        for (i, e) in self.entries.iter().enumerate() {
            self.by_name.entry(e.name.to_ascii_lowercase()).or_default().push(i);
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn count(&self, kind: Kind) -> usize {
        self.entries.iter().filter(|e| e.kind == kind).count()
    }

    /// Exact, case-insensitive lookup. Several entries can share a name (a texture's three maps,
    /// or the same model vendored twice), so this returns all of them.
    pub fn exact(&self, name: &str) -> Vec<&Entry> {
        self.by_name
            .get(&name.to_ascii_lowercase())
            .map(|v| v.iter().map(|&i| &self.entries[i]).collect())
            .unwrap_or_default()
    }

    /// Fuzzy resolution in the fzf-lite spirit of `logscope-fan`: subsequence match, scored so
    /// that tighter and earlier matches win. Returns best-first.
    ///
    /// It deliberately does **not** collapse to a single answer. An ambiguous reference should
    /// produce a disambiguation prompt, never a silent pick — picking silently is how you end up
    /// scaling a part to the volume of the wrong `puma`.
    pub fn resolve(&self, query: &str, kind: Option<Kind>, limit: usize) -> Vec<(&Entry, i32)> {
        let mut hits: Vec<(&Entry, i32)> = self
            .entries
            .iter()
            .filter(|e| kind.map_or(true, |k| e.kind == k))
            .filter_map(|e| fuzzy_score(query, &e.name).map(|s| (e, s)))
            .collect();
        hits.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.name.len().cmp(&b.0.name.len())));
        hits.truncate(limit);
        hits
    }

    /// All maps belonging to one material name — the `texture from <model>` verb's payload.
    pub fn material_set(&self, name: &str) -> Vec<&Entry> {
        self.exact(name).into_iter().filter(|e| e.kind == Kind::Texture).collect()
    }
}

/// Strip the engine's role prefix so `norm_obsidian` and `obsidian` share the name `obsidian`.
pub fn split_texture_role(stem: &str) -> (String, TextureRole) {
    if let Some(rest) = stem.strip_prefix("norm_") {
        (rest.to_string(), TextureRole::Normal)
    } else if let Some(rest) = stem.strip_prefix("spec_") {
        (rest.to_string(), TextureRole::Specular)
    } else {
        (stem.to_string(), TextureRole::Base)
    }
}

/// Subsequence score, or `None` when `needle` is not a subsequence of `haystack`.
/// Contiguous runs and an early start both score higher; case-insensitive.
pub fn fuzzy_score(needle: &str, haystack: &str) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = haystack.to_ascii_lowercase().chars().collect();
    let mut score = 0i32;
    let mut hi = 0usize;
    let mut last: Option<usize> = None;
    for nc in needle.to_ascii_lowercase().chars() {
        let found = hay[hi..].iter().position(|&c| c == nc)? + hi;
        score += match last {
            Some(prev) if found == prev + 1 => 8, // contiguous
            Some(_) => 1,
            None => 10 - (found.min(9) as i32), // reward matching near the start
        };
        last = Some(found);
        hi = found + 1;
    }
    // Prefer a name that is mostly the needle over one that merely contains it.
    Some(score + (20 - (hay.len().min(20) as i32)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texture_roles_group_under_one_name() {
        assert_eq!(split_texture_role("obsidian"), ("obsidian".into(), TextureRole::Base));
        assert_eq!(split_texture_role("norm_obsidian"), ("obsidian".into(), TextureRole::Normal));
        assert_eq!(split_texture_role("spec_obsidian"), ("obsidian".into(), TextureRole::Specular));
    }

    #[test]
    fn fuzzy_prefers_the_tighter_match() {
        let exact = fuzzy_score("puma", "puma").unwrap();
        let loose = fuzzy_score("puma", "plasma_uniform_matrix_assembly").unwrap();
        assert!(exact > loose, "{exact} vs {loose}");
        assert!(fuzzy_score("zzz", "puma").is_none());
    }

    #[test]
    fn kind_classification() {
        assert_eq!(Kind::of_path(Path::new("a/b/puma.iqm")), Some(Kind::Model));
        assert_eq!(Kind::of_path(Path::new("norm_x.PNG")), Some(Kind::Texture));
        assert_eq!(Kind::of_path(Path::new("spec.md")), Some(Kind::Doc));
        assert_eq!(Kind::of_path(Path::new("Cargo.lock")), None);
    }

    #[test]
    fn bounds_volume_is_named_honestly() {
        let f = ModelFacts { bounds: Some((Vec3::ZERO, Vec3::new(2.0, 3.0, 4.0))), ..Default::default() };
        assert_eq!(f.bounds_volume(), Some(24.0));
        assert_eq!(ModelFacts::default().bounds_volume(), None);
    }
}
