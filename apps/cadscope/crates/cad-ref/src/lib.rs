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
pub mod obj;
pub mod png;

/// The unit a file's numbers are in. Deliberately defaults to `Unknown` and is never inferred:
/// nothing in an OBJ or an IQM says whether a coordinate is a millimetre or a metre, and guessing
/// is how a bracket ends up a thousand times too big. Units are *declared*, in `ref.toml`, by a
/// person who knows.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Units {
    #[default]
    Unknown,
    Mm,
    Cm,
    M,
    In,
}

impl Units {
    /// Length multiplier into millimetres, CADscope's document unit (see `cad-forge`).
    pub fn to_mm(self) -> Option<f32> {
        Some(match self {
            Units::Mm => 1.0,
            Units::Cm => 10.0,
            Units::M => 1000.0,
            Units::In => 25.4,
            Units::Unknown => return None,
        })
    }
    pub fn parse(s: &str) -> Units {
        match s.trim().to_ascii_lowercase().as_str() {
            "mm" | "millimetre" | "millimeter" => Units::Mm,
            "cm" | "centimetre" | "centimeter" => Units::Cm,
            "m" | "metre" | "meter" => Units::M,
            "in" | "inch" | "inches" => Units::In,
            _ => Units::Unknown,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Units::Unknown => "unknown",
            Units::Mm => "mm",
            Units::Cm => "cm",
            Units::M => "m",
            Units::In => "in",
        }
    }
}

/// Why a reference could not be turned into a number. Every variant names what was actually
/// wrong, because §17.5's acceptance criterion is that a refusal says what was searched rather
/// than inventing something plausible.
#[derive(Clone, Debug, PartialEq)]
pub enum RefError {
    /// Nothing indexed under that name. Carries the corpus size so the message can say what was
    /// searched, and the nearest matches so the answer is useful rather than merely correct.
    NotIndexed { name: String, searched: usize, nearest: Vec<String> },
    NotAModel { name: String },
    /// Indexed, but only a header was read — no vertices, so nothing to measure.
    NoGeometry { name: String },
    /// Measured, but the surface does not enclose anything.
    NotClosed { name: String, open_edges: u32 },
    /// Closed and measured, but nobody has declared what unit the file is in, so the number cannot
    /// be compared with a document in millimetres.
    UnknownUnits { name: String },
    /// Several distinct models answer to that name; picking one silently is not an option.
    Ambiguous { name: String, candidates: Vec<String> },
}

impl std::fmt::Display for RefError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RefError::NotIndexed { name, searched, nearest } => {
                write!(f, "'{name}' is not in the corpus ({searched} entries searched)")?;
                if !nearest.is_empty() {
                    write!(f, "; nearest: {}", nearest.join(", "))?;
                }
                Ok(())
            }
            RefError::NotAModel { name } => write!(f, "'{name}' is not a model"),
            RefError::NoGeometry { name } => {
                write!(f, "'{name}' was indexed from a header only; no vertices to measure")
            }
            RefError::NotClosed { name, open_edges } => write!(
                f,
                "'{name}' is an open surface ({open_edges} boundary edges), so it encloses no volume"
            ),
            RefError::UnknownUnits { name } => write!(
                f,
                "'{name}' has no declared units, so its size cannot be compared with a document in mm — declare them in ref.toml"
            ),
            RefError::Ambiguous { name, candidates } => {
                write!(f, "'{name}' is ambiguous between {}", candidates.join(", "))
            }
        }
    }
}

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
    /// Axis-aligned bounds, when the file carries them (IQM stores them per animation frame) or
    /// when we read enough geometry to compute them.
    pub bounds: Option<(Vec3, Vec3)>,
    /// Enclosed volume, measured from vertex data with the same divergence-theorem sum the mesher
    /// uses — so a reference volume and a modelled volume are the same quantity, not two things
    /// that share a name. `None` when only a header was read, **and `None` when the surface is not
    /// closed**: the sum returns a confident number for an open shell, and that number is not a
    /// volume. Withholding it is the whole difference between a fact and a plausible figure.
    pub volume: Option<f32>,
    pub area: Option<f32>,
    /// Surface topology, when geometry was read. `closed` is the gate on `volume`.
    pub closed: bool,
    pub open_edges: u32,
    pub non_manifold_edges: u32,
    /// Declared, never inferred. See `Units`.
    pub units: Units,
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
    /// Declared units per subtree, relative to the scan root. Longest matching prefix wins.
    pub unit_hints: Vec<(PathBuf, Units)>,
}

impl Default for ScanOpts {
    fn default() -> Self {
        ScanOpts {
            exclude_dirs: vec![".git".into(), "target".into(), "node_modules".into(), "build".into()],
            max_depth: 12,
            max_entries: 100_000,
            unit_hints: Vec::new(),
        }
    }
}

impl ScanOpts {
    /// Read unit declarations from a `ref.toml`. Only the shape this needs is parsed — a table
    /// array of roots — rather than pulling in a TOML dependency for six lines of config.
    ///
    /// ```toml
    /// [[root]]
    /// path = "games/clawbound/assets"
    /// units = "m"
    /// ```
    pub fn with_ref_toml(mut self, text: &str) -> ScanOpts {
        let mut path: Option<String> = None;
        let mut units = Units::Unknown;
        let flush = |path: &mut Option<String>, units: &mut Units, out: &mut Vec<(PathBuf, Units)>| {
            if let Some(p) = path.take() {
                out.push((PathBuf::from(p), *units));
            }
            *units = Units::Unknown;
        };
        for line in text.lines() {
            let t = line.split('#').next().unwrap_or("").trim();
            if t == "[[root]]" {
                flush(&mut path, &mut units, &mut self.unit_hints);
            } else if let Some((k, v)) = t.split_once('=') {
                let v = v.trim().trim_matches('"').to_string();
                match k.trim() {
                    "path" => path = Some(v),
                    "units" => units = Units::parse(&v),
                    _ => {}
                }
            }
        }
        flush(&mut path, &mut units, &mut self.unit_hints);
        // Longest prefix first, so a specific subtree overrides the root it sits in.
        self.unit_hints.sort_by_key(|(p, _)| std::cmp::Reverse(p.as_os_str().len()));
        self
    }

    fn units_for(&self, root: &Path, path: &Path) -> Units {
        for (prefix, units) in &self.unit_hints {
            if path.starts_with(root.join(prefix)) {
                return *units;
            }
        }
        Units::Unknown
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
                    if let Some(mut e) = Self::entry_of(&path) {
                        if let Facts::Model(m) = &mut e.facts {
                            m.units = opts.units_for(root, &e.path);
                        }
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
            Kind::Model => {
                // IQM gives headers; OBJ gives real geometry, and therefore a measured volume.
                let facts = iqm::facts(path).or_else(|| obj::facts(path)).unwrap_or_default();
                (stem, Facts::Model(facts))
            }
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

    /// Build an index directly from entries — for tests, and for loading a cache without a scan.
    pub fn from_entries(entries: Vec<Entry>) -> Index {
        let mut idx = Index { entries, by_name: HashMap::new() };
        idx.entries.sort_by(|a, b| a.name.cmp(&b.name).then(a.path.cmp(&b.path)));
        idx.reindex();
        idx
    }

    /// The `volume of <model>` verb (plan §17.2), in CADscope's own unit.
    ///
    /// Every way this can fail is a distinct, named error, because the alternative — returning
    /// `None`, or worse a plausible number — is how a design gets anchored to something nobody
    /// checked. A caller that cannot proceed can at least say precisely why.
    pub fn volume_mm3(&self, name: &str) -> Result<f32, RefError> {
        let models: Vec<&Entry> = self.exact(name).into_iter().filter(|e| e.kind == Kind::Model).collect();
        if models.is_empty() {
            if self.exact(name).is_empty() {
                let nearest = self
                    .resolve(name, Some(Kind::Model), 3)
                    .into_iter()
                    .map(|(e, _)| e.name.clone())
                    .collect();
                return Err(RefError::NotIndexed { name: name.into(), searched: self.len(), nearest });
            }
            return Err(RefError::NotAModel { name: name.into() });
        }
        let facts: Vec<&ModelFacts> = models
            .iter()
            .filter_map(|e| match &e.facts {
                Facts::Model(m) => Some(m),
                _ => None,
            })
            .collect();

        // The same part is routinely present in two formats — the greeble catalogue pairs an .iqm
        // and an .obj for every entry — and only one of them carries vertices. That is not a
        // conflict, it is one model described twice, so take the candidate that can actually answer.
        // Ambiguity is refused only when two candidates give *different* answers.
        let measurable: Vec<&ModelFacts> = facts.iter().copied().filter(|f| f.volume.is_some()).collect();
        let f = match measurable.len() {
            0 => {
                // Report the most informative failure available across the candidates.
                let open = facts.iter().find(|f| f.area.is_some());
                return Err(match open {
                    Some(f) => RefError::NotClosed { name: name.into(), open_edges: f.open_edges },
                    None => RefError::NoGeometry { name: name.into() },
                });
            }
            1 => measurable[0],
            _ => {
                let v0 = measurable[0].volume.unwrap_or_default();
                let differ = measurable
                    .iter()
                    .any(|f| (f.volume.unwrap_or_default() - v0).abs() / v0.abs().max(1e-6) > 1e-3);
                if differ {
                    return Err(RefError::Ambiguous {
                        name: name.into(),
                        candidates: models.iter().map(|e| e.path.display().to_string()).collect(),
                    });
                }
                measurable[0]
            }
        };
        let v = f.volume.unwrap_or_default();
        let Some(mm) = f.units.to_mm() else {
            return Err(RefError::UnknownUnits { name: name.into() });
        };
        Ok(v * mm * mm * mm)
    }

    /// The `texture from <model>` verb (plan §17.2): resolve a model's material names to the
    /// texture entries that implement them, grouped by role.
    ///
    /// This is a *join*, not a guess. A material the corpus has no texture for is reported as
    /// present-but-unresolved rather than dropped, because "this model references a map nobody has"
    /// is exactly the kind of fact a designer needs to hear before relying on it.
    pub fn textures_of_model(&self, model: &str) -> Vec<MaterialMaps<'_>> {
        let Some(entry) = self.exact(model).into_iter().find(|e| e.kind == Kind::Model) else {
            return Vec::new();
        };
        let Facts::Model(m) = &entry.facts else { return Vec::new() };
        m.materials
            .iter()
            .map(|mat| {
                // Materials are written as bare names, as paths, and with extensions; the corpus is
                // keyed on the stem either way.
                let stem = Path::new(mat)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(mat.as_str());
                let (stem, _) = split_texture_role(stem);
                let mut maps = MaterialMaps { material: mat.clone(), name: stem.clone(), base: None, normal: None, specular: None };
                for e in self.material_set(&stem) {
                    if let Facts::Texture(t) = &e.facts {
                        match t.role {
                            TextureRole::Base => maps.base = Some(e),
                            TextureRole::Normal => maps.normal = Some(e),
                            TextureRole::Specular => maps.specular = Some(e),
                        }
                    }
                }
                maps
            })
            .collect()
    }

    /// All maps belonging to one material name — the `texture from <model>` verb's payload.
    pub fn material_set(&self, name: &str) -> Vec<&Entry> {
        self.exact(name).into_iter().filter(|e| e.kind == Kind::Texture).collect()
    }
}

/// A material as the corpus can supply it: the name a model asked for, and whichever of the
/// engine's three maps actually exist for it.
#[derive(Clone, Debug)]
pub struct MaterialMaps<'a> {
    /// Exactly as the model file spelled it.
    pub material: String,
    /// The corpus key it resolved to.
    pub name: String,
    pub base: Option<&'a Entry>,
    pub normal: Option<&'a Entry>,
    pub specular: Option<&'a Entry>,
}

impl MaterialMaps<'_> {
    /// True when nothing in the corpus implements this material.
    pub fn unresolved(&self) -> bool {
        self.base.is_none() && self.normal.is_none() && self.specular.is_none()
    }
    pub fn found(&self) -> usize {
        self.base.is_some() as usize + self.normal.is_some() as usize + self.specular.is_some() as usize
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

    fn tex(path: &str) -> Entry {
        let stem = Path::new(path).file_stem().unwrap().to_str().unwrap();
        let (name, role) = split_texture_role(stem);
        Entry {
            name,
            kind: Kind::Texture,
            path: path.into(),
            bytes: 0,
            facts: Facts::Texture(TextureFacts { width: 512, height: 512, role }),
        }
    }

    fn model_entry(name: &str, path: &str, facts: ModelFacts) -> Entry {
        Entry { name: name.into(), kind: Kind::Model, path: path.into(), bytes: 0, facts: Facts::Model(facts) }
    }

    #[test]
    fn ref_toml_declares_units_longest_prefix_first() {
        let opts = ScanOpts::default().with_ref_toml(
            "# corpus roots\n[[root]]\npath = \"games\"\nunits = \"m\"\n\n[[root]]\npath = \"games/blackstar/parts\"\nunits = \"mm\"\n",
        );
        assert_eq!(opts.unit_hints.len(), 2);
        let root = Path::new("/repo");
        // The deeper declaration wins over the shallower one it sits inside.
        assert_eq!(opts.units_for(root, Path::new("/repo/games/blackstar/parts/a.obj")), Units::Mm);
        assert_eq!(opts.units_for(root, Path::new("/repo/games/clawbound/b.obj")), Units::M);
        // Anything undeclared stays unknown; nothing is inferred.
        assert_eq!(opts.units_for(root, Path::new("/repo/apps/x.obj")), Units::Unknown);
    }

    #[test]
    fn volume_in_mm3_converts_by_the_cube_of_the_length_factor() {
        let facts = ModelFacts { volume: Some(2.0), area: Some(1.0), closed: true, units: Units::Cm, ..Default::default() };
        let idx = Index::from_entries(vec![model_entry("cog", "cog.obj", facts)]);
        // 2 cm^3 is 2000 mm^3.
        assert!((idx.volume_mm3("cog").unwrap() - 2000.0).abs() < 1e-3);
    }

    #[test]
    fn every_refusal_names_its_reason() {
        let closed_no_units = ModelFacts { volume: Some(2.0), area: Some(1.0), closed: true, ..Default::default() };
        let open = ModelFacts { area: Some(1.0), closed: false, open_edges: 12, units: Units::Mm, ..Default::default() };
        let header_only = ModelFacts { triangles: 100, units: Units::Mm, ..Default::default() };
        let idx = Index::from_entries(vec![
            model_entry("nounits", "a.obj", closed_no_units),
            model_entry("shell", "b.obj", open),
            model_entry("headeronly", "c.iqm", header_only),
            tex("plating.png"),
        ]);
        assert!(matches!(idx.volume_mm3("nounits"), Err(RefError::UnknownUnits { .. })));
        assert!(matches!(idx.volume_mm3("shell"), Err(RefError::NotClosed { open_edges: 12, .. })));
        assert!(matches!(idx.volume_mm3("headeronly"), Err(RefError::NoGeometry { .. })));
        assert!(matches!(idx.volume_mm3("plating"), Err(RefError::NotAModel { .. })));
        // A miss reports the corpus size and points at the nearest names rather than inventing one.
        match idx.volume_mm3("shel") {
            Err(RefError::NotIndexed { searched, nearest, .. }) => {
                assert_eq!(searched, 4);
                assert_eq!(nearest.first().map(String::as_str), Some("shell"));
            }
            other => panic!("expected NotIndexed, got {other:?}"),
        }
        // The messages are for humans, so they must actually say the thing.
        assert!(idx.volume_mm3("shell").unwrap_err().to_string().contains("open surface"));
        assert!(idx.volume_mm3("nounits").unwrap_err().to_string().contains("ref.toml"));
    }

    #[test]
    fn one_model_in_two_formats_resolves_to_the_measurable_one() {
        // The greeble catalogue ships an .iqm and an .obj per part; only the OBJ has vertices.
        // That is one model described twice, not two models in conflict.
        let header_only = ModelFacts { triangles: 400, units: Units::Mm, ..Default::default() };
        let measured = ModelFacts { volume: Some(3.0), area: Some(9.0), closed: true, units: Units::Mm, ..Default::default() };
        let idx = Index::from_entries(vec![
            model_entry("barrel", "cat/barrel.iqm", header_only),
            model_entry("barrel", "cat/barrel.obj", measured),
        ]);
        assert!((idx.volume_mm3("barrel").unwrap() - 3.0).abs() < 1e-4);
    }

    #[test]
    fn two_different_models_under_one_name_refuse_to_be_picked_between() {
        let a = ModelFacts { volume: Some(2.0), area: Some(1.0), closed: true, units: Units::Mm, ..Default::default() };
        let b = ModelFacts { volume: Some(90.0), area: Some(1.0), closed: true, units: Units::Mm, ..Default::default() };
        let idx = Index::from_entries(vec![model_entry("wheel", "x/wheel.obj", a), model_entry("wheel", "y/wheel.obj", b)]);
        match idx.volume_mm3("wheel") {
            Err(RefError::Ambiguous { candidates, .. }) => assert_eq!(candidates.len(), 2),
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn texture_from_model_is_a_join_and_reports_gaps() {
        let model = Entry {
            name: "velgorn".into(),
            kind: Kind::Model,
            path: "velgorn.iqm".into(),
            bytes: 0,
            facts: Facts::Model(ModelFacts {
                materials: vec!["obsidian".into(), "textures/plating.png".into(), "ghost".into()],
                ..Default::default()
            }),
        };
        let idx = Index::from_entries(vec![
            model,
            tex("obsidian.png"),
            tex("norm_obsidian.png"),
            tex("spec_obsidian.png"),
            tex("plating.png"),
        ]);
        let maps = idx.textures_of_model("velgorn");
        assert_eq!(maps.len(), 3);
        // A full set resolves all three roles.
        assert_eq!(maps[0].name, "obsidian");
        assert_eq!(maps[0].found(), 3);
        // A material written as a path still resolves by stem, here with only a base map.
        assert_eq!(maps[1].name, "plating");
        assert_eq!(maps[1].found(), 1);
        assert!(maps[1].normal.is_none());
        // And a material nobody implements is reported, not silently dropped.
        assert!(maps[2].unresolved(), "missing materials must stay visible");
    }

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
    fn measured_volume_is_distinct_from_the_box_proxy() {
        let f = ModelFacts {
            bounds: Some((Vec3::ZERO, Vec3::new(2.0, 2.0, 2.0))),
            volume: Some(4.19), // a sphere inscribed in that box
            ..Default::default()
        };
        assert_eq!(f.bounds_volume(), Some(8.0));
        assert!(f.volume.unwrap() < f.bounds_volume().unwrap(), "the solid fills less than its box");
    }

    #[test]
    fn bounds_volume_is_named_honestly() {
        let f = ModelFacts { bounds: Some((Vec3::ZERO, Vec3::new(2.0, 3.0, 4.0))), ..Default::default() };
        assert_eq!(f.bounds_volume(), Some(24.0));
        assert_eq!(ModelFacts::default().bounds_volume(), None);
    }
}
