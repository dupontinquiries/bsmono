//! Handwriting corpus tooling — the layer bot-arena's handwriting challenges
//! and the finetuning tools consume.
//!
//! On-disk layout (a *corpus directory*):
//!
//! ```text
//! corpus/
//!   manifest.tsv          # id \t label   (labels are the ground-truth text)
//!   ink/<id>.ink          # vector strokes (io module format)
//!   img/<id>.pgm          # optional rasterized export (export_images)
//! ```
//!
//! Design points:
//! * **Vector-first.** The `.ink` files are the ground truth; images are a
//!   derived artifact regenerated at whatever resolution the model wants.
//!   Stroke order + timing survive, which sequence models (the better
//!   handwriting recognizers) need and pixels destroy.
//! * **Deterministic split.** Train/val assignment is a pure function of the
//!   sample id (FNV-1a hash), so the split is stable across machines and
//!   re-exports — no RNG state to version.
//! * Labels are raw UTF-8 text with tabs/newlines escaped; one line each.

use crate::io as inkio;
use crate::raster::rasterize;
use crate::stroke::Stroke;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampleMeta {
    /// Filename-safe id (enforced on write): `[A-Za-z0-9_-]+`.
    pub id: String,
    /// Ground-truth transcription (or class label).
    pub label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Split {
    Train,
    Val,
}

/// Stable id→split assignment. `val_permille` of 100 ≈ 10% validation.
pub fn split_of(id: &str, val_permille: u32) -> Split {
    // FNV-1a 64
    let mut h: u64 = 0xcbf29ce484222325;
    for b in id.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    if (h % 1000) < val_permille as u64 {
        Split::Val
    } else {
        Split::Train
    }
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n")
}
fn unesc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some('\\') => out.push('\\'),
                Some(o) => {
                    out.push('\\');
                    out.push(o);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

pub struct Corpus {
    pub root: PathBuf,
}

impl Corpus {
    pub fn open(root: impl Into<PathBuf>) -> Self {
        Corpus { root: root.into() }
    }

    fn ink_dir(&self) -> PathBuf {
        self.root.join("ink")
    }
    fn img_dir(&self) -> PathBuf {
        self.root.join("img")
    }
    fn manifest_path(&self) -> PathBuf {
        self.root.join("manifest.tsv")
    }

    /// Append one sample: writes `ink/<id>.ink` and a manifest line.
    pub fn add_sample(&self, meta: &SampleMeta, strokes: &[Stroke]) -> std::io::Result<()> {
        if !valid_id(&meta.id) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid sample id {:?}", meta.id),
            ));
        }
        fs::create_dir_all(self.ink_dir())?;
        fs::write(
            self.ink_dir().join(format!("{}.ink", meta.id)),
            inkio::emit(strokes),
        )?;
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.manifest_path())?;
        writeln!(f, "{}\t{}", meta.id, esc(&meta.label))?;
        Ok(())
    }

    /// Read the manifest. Later duplicate ids win (append-to-relabel).
    pub fn manifest(&self) -> std::io::Result<Vec<SampleMeta>> {
        let text = fs::read_to_string(self.manifest_path())?;
        let mut out: Vec<SampleMeta> = Vec::new();
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Some((id, label)) = line.split_once('\t') {
                let meta = SampleMeta {
                    id: id.to_string(),
                    label: unesc(label),
                };
                if let Some(existing) = out.iter_mut().find(|m| m.id == meta.id) {
                    *existing = meta;
                } else {
                    out.push(meta);
                }
            }
        }
        Ok(out)
    }

    pub fn load_strokes(&self, id: &str) -> std::io::Result<Vec<Stroke>> {
        let text = fs::read_to_string(self.ink_dir().join(format!("{}.ink", id)))?;
        inkio::parse(&text)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
    }

    /// Rasterize every sample to `img/<id>.pgm` at `size`². Returns the count.
    pub fn export_images(&self, size: usize) -> std::io::Result<usize> {
        fs::create_dir_all(self.img_dir())?;
        let mut n = 0;
        for m in self.manifest()? {
            let strokes = self.load_strokes(&m.id)?;
            let img = rasterize(&strokes, size, 0.08, size as f64 / 96.0);
            fs::write(self.img_dir().join(format!("{}.pgm", m.id)), img.to_pgm())?;
            n += 1;
        }
        Ok(n)
    }

    /// (train, val) manifests under the stable split.
    pub fn split(&self, val_permille: u32) -> std::io::Result<(Vec<SampleMeta>, Vec<SampleMeta>)> {
        let all = self.manifest()?;
        Ok(all
            .into_iter()
            .partition(|m| split_of(&m.id, val_permille) == Split::Train))
    }
}

/// Convenience for tools: a path check that reports what's missing.
pub fn corpus_status(root: &Path) -> String {
    let c = Corpus::open(root);
    match c.manifest() {
        Err(e) => format!("no manifest at {:?}: {}", c.manifest_path(), e),
        Ok(m) => {
            let missing: Vec<&str> = m
                .iter()
                .filter(|s| !c.ink_dir().join(format!("{}.ink", s.id)).exists())
                .map(|s| s.id.as_str())
                .collect();
            format!(
                "{} samples, {} missing ink files{}",
                m.len(),
                missing.len(),
                if missing.is_empty() {
                    String::new()
                } else {
                    format!(": {}", missing.join(", "))
                }
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stroke::{InkPoint, StrokeId, StrokeStyle};

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("inkkit-test-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn sample_strokes() -> Vec<Stroke> {
        let mut s = Stroke::new(StrokeId(1), StrokeStyle::default());
        s.points.push(InkPoint::xy(0.0, 0.0));
        s.points.push(InkPoint::xy(40.0, 25.0));
        vec![s]
    }

    #[test]
    fn add_read_export_split() {
        let root = tmp("full");
        let c = Corpus::open(&root);
        for i in 0..20 {
            c.add_sample(
                &SampleMeta {
                    id: format!("s{:03}", i),
                    label: format!("word {}\twith tab", i),
                },
                &sample_strokes(),
            )
            .unwrap();
        }
        let m = c.manifest().unwrap();
        assert_eq!(m.len(), 20);
        assert!(m[3].label.contains('\t'), "tab survives escaping");

        let strokes = c.load_strokes("s000").unwrap();
        assert_eq!(strokes[0].points.len(), 2);

        assert_eq!(c.export_images(32).unwrap(), 20);
        let pgm = fs::read(root.join("img/s000.pgm")).unwrap();
        assert!(pgm.starts_with(b"P5\n32 32\n"));

        let (train, val) = c.split(200).unwrap(); // ~20%
        assert_eq!(train.len() + val.len(), 20);
        assert!(!train.is_empty());
        // determinism
        let (t2, _) = c.split(200).unwrap();
        assert_eq!(train, t2);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn relabel_by_append_wins() {
        let root = tmp("relabel");
        let c = Corpus::open(&root);
        let meta = SampleMeta {
            id: "a1".into(),
            label: "old".into(),
        };
        c.add_sample(&meta, &sample_strokes()).unwrap();
        c.add_sample(
            &SampleMeta {
                id: "a1".into(),
                label: "new".into(),
            },
            &sample_strokes(),
        )
        .unwrap();
        let m = c.manifest().unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].label, "new");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_path_hostile_ids() {
        let root = tmp("ids");
        let c = Corpus::open(&root);
        let bad = SampleMeta {
            id: "../evil".into(),
            label: "x".into(),
        };
        assert!(c.add_sample(&bad, &sample_strokes()).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn split_is_a_pure_function_of_id() {
        assert_eq!(split_of("abc", 100), split_of("abc", 100));
        // permille 0 → everything trains; 1000 → everything validates
        assert_eq!(split_of("anything", 0), Split::Train);
        assert_eq!(split_of("anything", 1000), Split::Val);
    }
}
