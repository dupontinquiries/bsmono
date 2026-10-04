//! The training data on disk: every frame the trainer kept, as a feature vector with the dot the person was looking at.
//! A small binary file per session under `~/.local/share/gaze-trainer/`, owner-only (faces, in effect), little-endian:
//! `GZT1`, dim u32, count u32, then per sample: session u32, dot u32, target x f32, target y f32, `dim` floats.

use std::io::{Read, Write};
use std::path::Path;

const MAGIC: &[u8; 4] = b"GZT1";

#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub f: Vec<f32>,
    /// The dot, in screen fractions.
    pub target: [f32; 2],
    /// Which dot of the session (samples of one dot share it: cross-validation holds a whole dot out).
    pub dot: u32,
    pub session: u32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Dataset {
    pub dim: usize,
    pub samples: Vec<Sample>,
}

impl Dataset {
    pub fn new(dim: usize) -> Dataset { Dataset { dim, samples: Vec::new() } }

    pub fn push(&mut self, s: Sample) -> Result<(), String> {
        if s.f.len() != self.dim { return Err(format!("sample has {} features, the set has {}", s.f.len(), self.dim)); }
        self.samples.push(s);
        Ok(())
    }

    pub fn extend(&mut self, other: Dataset) -> Result<(), String> {
        if !self.samples.is_empty() && other.dim != self.dim { return Err(format!("dimension {} does not match {}", other.dim, self.dim)); }
        if self.samples.is_empty() { self.dim = other.dim; }
        self.samples.extend(other.samples);
        Ok(())
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(12 + self.samples.len() * (16 + self.dim * 4));
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(self.dim as u32).to_le_bytes());
        out.extend_from_slice(&(self.samples.len() as u32).to_le_bytes());
        for s in &self.samples {
            out.extend_from_slice(&s.session.to_le_bytes());
            out.extend_from_slice(&s.dot.to_le_bytes());
            for t in s.target { out.extend_from_slice(&t.to_le_bytes()); }
            for v in &s.f { out.extend_from_slice(&v.to_le_bytes()); }
        }
        out
    }

    pub fn decode(b: &[u8]) -> Result<Dataset, String> {
        fn take<'a>(b: &'a [u8], pos: &mut usize, n: usize) -> Result<&'a [u8], String> {
            let end = pos.checked_add(n).filter(|&e| e <= b.len()).ok_or("the file ends early")?;
            let s = &b[*pos..end];
            *pos = end;
            Ok(s)
        }
        let u32_of = |s: &[u8]| u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
        let f32_of = |s: &[u8]| f32::from_le_bytes([s[0], s[1], s[2], s[3]]);
        let mut pos = 0usize;
        if take(b, &mut pos, 4)? != MAGIC { return Err("not a gaze-trainer dataset".into()); }
        let dim = u32_of(take(b, &mut pos, 4)?) as usize;
        let count = u32_of(take(b, &mut pos, 4)?) as usize;
        if dim == 0 || dim > 100_000 { return Err(format!("implausible dimension {dim}")); }
        let per = 16usize.checked_add(dim.checked_mul(4).ok_or("overflow")?).ok_or("overflow")?;
        let want = count.checked_mul(per).ok_or("overflow")?;
        if b.len() - pos != want { return Err(format!("expected {want} bytes of samples, found {}", b.len() - pos)); }
        let mut samples = Vec::with_capacity(count);
        for _ in 0..count {
            let session = u32_of(take(b, &mut pos, 4)?);
            let dot = u32_of(take(b, &mut pos, 4)?);
            let target = [f32_of(take(b, &mut pos, 4)?), f32_of(take(b, &mut pos, 4)?)];
            let f: Vec<f32> = take(b, &mut pos, dim * 4)?.chunks_exact(4).map(f32_of).collect();
            samples.push(Sample { f, target, dot, session });
        }
        Ok(Dataset { dim, samples })
    }

    /// Write atomically (a temp file, then a rename) and owner-only: a crash mid-save never leaves half a session.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
        let tmp = path.with_extension("tmp");
        {
            #[cfg(unix)]
            let mut f = { use std::os::unix::fs::OpenOptionsExt; std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)? };
            #[cfg(not(unix))]
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&self.encode())?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, path)
    }

    pub fn load(path: &Path) -> Result<Dataset, String> {
        let mut b = Vec::new();
        std::fs::File::open(path).and_then(|mut f| f.read_to_end(&mut b)).map_err(|e| format!("{}: {e}", path.display()))?;
        Dataset::decode(&b)
    }

    /// Every `*.gzt` in a directory, sorted, as one set.
    pub fn load_all(dir: &Path) -> Result<Dataset, String> {
        let mut files: Vec<_> = match std::fs::read_dir(dir) { Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "gzt")).collect(), Err(_) => return Ok(Dataset::default()) };
        files.sort();
        let mut all = Dataset::default();
        for f in files { all.extend(Dataset::load(&f)?)?; }
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(n: usize, dim: usize) -> Dataset {
        let mut d = Dataset::new(dim);
        for i in 0..n { d.push(Sample { f: (0..dim).map(|k| (i * 7 + k) as f32 * 0.25 - 3.0).collect(), target: [i as f32 / 10.0, 1.0 - i as f32 / 10.0], dot: (i / 3) as u32, session: 2 }).unwrap(); }
        d
    }

    #[test]
    fn a_dataset_survives_encode_and_decode_exactly() {
        let d = set(9, 5);
        assert_eq!(Dataset::decode(&d.encode()).unwrap(), d);
        assert_eq!(Dataset::decode(&Dataset::new(5).encode()).unwrap().samples.len(), 0, "an empty set round-trips");
    }

    #[test]
    fn damaged_files_are_errors_not_panics_or_garbage() {
        let good = set(4, 6).encode();
        assert!(Dataset::decode(&good[..good.len() - 1]).is_err(), "one byte short");
        let mut long = good.clone();
        long.push(0);
        assert!(Dataset::decode(&long).is_err(), "one byte long");
        assert!(Dataset::decode(b"NOPE\0\0\0\0\0\0\0\0").is_err());
        assert!(Dataset::decode(&[]).is_err());
        let mut huge = good.clone();
        huge[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Dataset::decode(&huge).is_err(), "an absurd dimension is refused before any allocation");
        let mut many = good;
        many[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Dataset::decode(&many).is_err(), "an absurd count too");
    }

    #[test]
    fn a_sample_of_the_wrong_size_is_refused_and_sets_of_different_dimensions_do_not_merge() {
        let mut d = Dataset::new(3);
        assert!(d.push(Sample { f: vec![0.0; 4], target: [0.0; 2], dot: 0, session: 0 }).is_err());
        let mut a = set(2, 3);
        assert!(a.extend(set(2, 4)).is_err());
        assert!(a.extend(set(2, 3)).is_ok() && a.samples.len() == 4);
        let mut empty = Dataset::default();
        assert!(empty.extend(set(2, 7)).is_ok() && empty.dim == 7, "an empty set takes the first dimension it is given");
    }

    #[test]
    fn saving_is_atomic_owner_only_and_load_all_reads_every_session_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (set(3, 4), set(2, 4));
        a.save(&dir.path().join("s1.gzt")).unwrap();
        b.save(&dir.path().join("s2.gzt")).unwrap();
        std::fs::write(dir.path().join("notes.txt"), "ignored").unwrap();
        assert!(!dir.path().join("s1.tmp").exists(), "no temp file is left behind");
        #[cfg(unix)]
        { use std::os::unix::fs::PermissionsExt; assert_eq!(std::fs::metadata(dir.path().join("s1.gzt")).unwrap().permissions().mode() & 0o777, 0o600); }
        let all = Dataset::load_all(dir.path()).unwrap();
        assert_eq!(all.samples.len(), 5);
        assert_eq!(&all.samples[..3], &a.samples[..], "sorted by file name");
        assert_eq!(Dataset::load_all(&dir.path().join("missing")).unwrap().samples.len(), 0, "no directory yet is an empty set");
    }
}
