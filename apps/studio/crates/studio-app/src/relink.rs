//! relink — pointing a cut at different files: proxies to camera originals.
//!
//! A film is cut on proxies because 4K AV1 does not scrub, and then finished
//! from the camera originals. Everything about the edit survives that swap —
//! the only thing that changes is which file each source reads, which is
//! exactly what `studio_core::Op::SetSourcePath` moves and nothing else.
//!
//! ## Matching is by name, and says what it could not do
//!
//! Proxies are named after their originals (`C0057.mkv` for `C0057.MXF`), so
//! the stem is the match, extension ignored and case folded — a card writes
//! `.MXF` where a proxy script writes `.mkv`. That is a convention, not a
//! guarantee, so this never guesses: a source with no match keeps its path
//! and is listed, and a stem that matches TWO files in the folder is listed
//! as ambiguous rather than resolved by picking the first one. Content
//! hashing ([`crate::media_map`]) is the stronger answer and the right next
//! step for a card that was renamed; it is also minutes of I/O, which is why
//! the cheap check comes first.
//!
//! ## What it deliberately does not check
//!
//! Length. A relink keeps the source's length, rate and channels, because
//! every clip's trim was validated against them; if the "original" is really
//! a different cut of the media, that is an import, not a relink. The
//! report gives the operator enough to notice.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use studio_core::{Op, Project, SourceId};

/// What a relink would do, before it does it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    /// The ops, ready to apply as one `Op::Group`.
    pub ops: Vec<Op>,
    /// Source path -> the file it would point at.
    pub matched: Vec<(String, PathBuf)>,
    /// Sources whose name was not in the folder at all.
    pub missing: Vec<String>,
    /// Sources whose name matched more than one file: not touched.
    pub ambiguous: Vec<(String, Vec<PathBuf>)>,
    /// Sources already pointing inside the folder: nothing to do.
    pub already: usize,
}

impl Plan {
    /// One line for the status bar.
    pub fn summary(&self) -> String {
        let mut s = format!("{} source(s) relinked", self.matched.len());
        if self.already > 0 { s.push_str(&format!(", {} already there", self.already)); }
        if !self.missing.is_empty() { s.push_str(&format!(", {} not found", self.missing.len())); }
        if !self.ambiguous.is_empty() { s.push_str(&format!(", {} ambiguous", self.ambiguous.len())); }
        s
    }
}

fn stem(p: &Path) -> Option<String> {
    p.file_stem().map(|s| s.to_string_lossy().to_lowercase())
}

/// Walk `dir` (one level, plus its immediate subfolders — a card is usually
/// `CARD/CLIP/` deep) and plan a relink of every file source in `project`.
pub fn plan(project: &Project, dir: &Path) -> Plan {
    let mut by_stem: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    let mut dirs = vec![dir.to_path_buf()];
    let mut depth = 0;
    while depth < 2 {
        let mut next = Vec::new();
        for d in &dirs {
            let Ok(rd) = std::fs::read_dir(d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() { next.push(p); }
                else if let Some(s) = stem(&p) { by_stem.entry(s).or_default().push(p); }
            }
        }
        dirs = next;
        depth += 1;
    }

    let mut plan = Plan::default();
    for (id, src) in &project.sources {
        if src.kind != studio_core::SourceKind::File || src.path.is_empty() { continue; }
        let here = PathBuf::from(&src.path);
        let Some(s) = stem(&here) else { continue };
        match by_stem.get(&s) {
            None => plan.missing.push(src.path.clone()),
            Some(found) if found.len() > 1 => plan.ambiguous.push((src.path.clone(), found.clone())),
            Some(found) => {
                let to = &found[0];
                if *to == here { plan.already += 1; continue; }
                plan.ops.push(Op::SetSourcePath { id: SourceId(id.0), path: to.display().to_string() });
                plan.matched.push((src.path.clone(), to.clone()));
            }
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::{Source, Ticks};

    fn project(paths: &[&str]) -> Project {
        let mut p = Project::default();
        for (i, path) in paths.iter().enumerate() {
            p.apply(&Op::AddSource { source: Source::file(SourceId(i as u64 + 1), *path,
                Ticks::from_seconds(12.0), 48_000, 2, true) }).unwrap();
        }
        p
    }

    fn folder(files: &[&str]) -> tempdir::Dir {
        let d = tempdir::Dir::new();
        for f in files {
            let p = d.path.join(f);
            if let Some(parent) = p.parent() { std::fs::create_dir_all(parent).unwrap(); }
            std::fs::write(p, b"x").unwrap();
        }
        d
    }

    /// A temp directory that cleans up after itself, so the tests need no crate.
    mod tempdir {
        pub struct Dir { pub path: std::path::PathBuf }
        impl Dir {
            pub fn new() -> Dir {
                let p = std::env::temp_dir().join(format!("relink-{}-{:?}", std::process::id(),
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
                std::fs::create_dir_all(&p).unwrap();
                Dir { path: p }
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.path); }
        }
    }

    #[test]
    fn proxies_relink_to_the_originals_by_name_whatever_the_extension() {
        let p = project(&["/proxies/C0057.mkv", "/proxies/C0058.mkv"]);
        let card = folder(&["A001/C0057.MXF", "A001/C0058.MXF"]);
        let plan = plan(&p, &card.path);
        assert_eq!(plan.ops.len(), 2, "{plan:?}");
        assert!(plan.missing.is_empty() && plan.ambiguous.is_empty());
        let mut after = p.clone();
        after.apply(&Op::Group { ops: plan.ops.clone() }).expect("applies");
        for (id, s) in &after.sources {
            assert!(s.path.ends_with(".MXF"), "{s:?}");
            // everything the edit depends on is untouched
            assert_eq!((s.len, s.sample_rate, s.channels, s.has_video),
                       (p.sources[id].len, p.sources[id].sample_rate,
                        p.sources[id].channels, p.sources[id].has_video));
        }
    }

    #[test]
    fn what_it_cannot_match_it_reports_rather_than_guesses() {
        let p = project(&["/proxies/C0057.mkv", "/proxies/C0099.mkv"]);
        // C0057 exists twice on the card (two cards, same numbering), C0099 not at all
        let card = folder(&["A001/C0057.MXF", "A002/C0057.MXF"]);
        let plan = plan(&p, &card.path);
        assert!(plan.ops.is_empty(), "nothing is guessed: {plan:?}");
        assert_eq!(plan.missing, vec!["/proxies/C0099.mkv".to_string()]);
        assert_eq!(plan.ambiguous.len(), 1);
        assert_eq!(plan.ambiguous[0].1.len(), 2);
        assert!(plan.summary().contains("not found") && plan.summary().contains("ambiguous"),
                "{}", plan.summary());
    }

    #[test]
    fn relinking_to_where_it_already_points_is_not_an_edit() {
        let card = folder(&["C0057.MXF"]);
        let here = card.path.join("C0057.MXF");
        let p = project(&[&here.display().to_string()]);
        let plan = plan(&p, &card.path);
        assert!(plan.ops.is_empty(), "no op for a source that is already right");
        assert_eq!(plan.already, 1);
        assert_eq!(plan.summary(), "0 source(s) relinked, 1 already there");
    }

    #[test]
    fn a_generator_source_has_no_file_to_relink() {
        let mut p = project(&["/proxies/C0057.mkv"]);
        p.apply(&Op::AddSource { source: Source::pattern(SourceId(9), 0, "tone", Ticks::from_seconds(4.0), 48_000, 2) }).unwrap();
        let card = folder(&["C0057.MXF"]);
        let plan = plan(&p, &card.path);
        assert_eq!(plan.ops.len(), 1, "only the file source: {plan:?}");
        assert!(plan.missing.is_empty(), "a generator is not a missing file: {plan:?}");
    }
}
