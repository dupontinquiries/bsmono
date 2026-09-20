//! bsweb — the blackstar parts web, as a library.
//!
//! Container layout, read off `games/blackstar/tools/build_web.py` and
//! verified against the real `catalog/parts_web.bin.zst` (1,790 parts,
//! 1,683 geometry blobs — the 107 folds the docs claim):
//!
//! ```text
//!   b"BSWEB\x01"            6 bytes
//!   <u32 idx_len><u32 pay_len>
//!   idx_len  bytes          JSON: {version, parts:[…], geom:[{v,i}], …}
//!   pay_len  bytes          per geom entry: vertex blob then index blob
//!   ── the whole thing wrapped in gzip or zstd ──
//! ```
//!
//! Two properties of that layout drive the whole design here:
//!
//! * **Index and payload are separate.** Almost every operation — listing,
//!   filtering, stats, removal bookkeeping — touches only the JSON index.
//!   [`Web::open_index_only`] reads 637 KB instead of 7.9 MB, which is the
//!   difference between an instant `ls` and a visible pause on a Fairphone.
//! * **Several parts may share one geom blob.** Removing a part must not
//!   remove its geometry unless it was the last referrer. Getting this wrong
//!   silently corrupts every other part pointing at the same blob, so
//!   [`Web::remove`] never touches the payload and [`Web::gc`] is the only
//!   thing that does.
//!
//! Version 1 indexes carry no `bits`/`delta` keys. They are optional here
//! with the writer's defaults, so a v1 file round-trips unchanged.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Write};
use std::path::Path;

pub const MAGIC: &[u8; 6] = b"BSWEB\x01";

/// One part: an annotation record plus a pointer into the payload.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub id: String,
    #[serde(default)]
    pub catalog: Option<String>,
    /// Index into [`Index::geom`]. Not unique — dedup folds share blobs.
    pub geom: usize,
    #[serde(default)]
    pub nv: usize,
    #[serde(default)]
    pub nt: usize,
    #[serde(default)]
    pub isize: u8,
    #[serde(default)]
    pub lo: Vec<f32>,
    #[serde(default)]
    pub span: Vec<f32>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub sig: Option<String>,
    #[serde(default)]
    pub instances: Option<u32>,
    #[serde(default)]
    pub tris_src: Option<u32>,
    #[serde(default)]
    pub size: Option<f64>,
    #[serde(default)]
    pub residual_mean: Option<f64>,
    #[serde(default)]
    pub residual_max: Option<f64>,
    /// Anything a newer writer added. Preserved so we never lose fields.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GeomLen {
    pub v: usize,
    pub i: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Index {
    pub version: u32,
    pub parts: Vec<Part>,
    pub geom: Vec<GeomLen>,
    /// Absent in v1 containers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bits: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Gzip,
    Zstd,
    Raw,
}

impl Codec {
    /// Sniff by magic, because a `.bin` may be either and a mis-named file
    /// is common when these get moved around by hand.
    pub fn sniff(head: &[u8]) -> Codec {
        if head.starts_with(&[0x1f, 0x8b]) {
            Codec::Gzip
        } else if head.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
            Codec::Zstd
        } else {
            Codec::Raw
        }
    }
    pub fn for_path(p: &Path) -> Codec {
        match p.extension().and_then(|e| e.to_str()) {
            Some("zst") => Codec::Zstd,
            Some("gz") => Codec::Gzip,
            _ => Codec::Raw,
        }
    }
}

pub struct Web {
    pub index: Index,
    /// `None` when opened index-only. Operations needing it say so.
    payload: Option<Vec<u8>>,
    pub codec: Codec,
}

fn decompress(bytes: &[u8]) -> Result<(Vec<u8>, Codec)> {
    let c = Codec::sniff(bytes);
    let out = match c {
        Codec::Gzip => {
            let mut d = flate2::read::GzDecoder::new(bytes);
            let mut v = Vec::new();
            d.read_to_end(&mut v)?;
            v
        }
        Codec::Zstd => zstd::decode_all(bytes)?,
        Codec::Raw => bytes.to_vec(),
    };
    Ok((out, c))
}

fn split(raw: &[u8]) -> Result<(Index, &[u8])> {
    if raw.len() < 14 || &raw[..6] != MAGIC {
        bail!("not a BSWEB container (bad magic)");
    }
    let il = u32::from_le_bytes(raw[6..10].try_into().unwrap()) as usize;
    let pl = u32::from_le_bytes(raw[10..14].try_into().unwrap()) as usize;
    if raw.len() < 14 + il + pl {
        bail!(
            "truncated: header claims {} index + {} payload, file has {}",
            il,
            pl,
            raw.len() - 14
        );
    }
    let index: Index =
        serde_json::from_slice(&raw[14..14 + il]).context("index JSON did not parse")?;
    Ok((index, &raw[14 + il..14 + il + pl]))
}

impl Web {
    pub fn open(path: &Path) -> Result<Web> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let (raw, codec) = decompress(&bytes)?;
        let (index, payload) = split(&raw)?;
        Ok(Web { index, payload: Some(payload.to_vec()), codec })
    }

    /// Index only. Enough for ls / show / stat / filtering, and on the real
    /// catalog that is 637 KB rather than 7.9 MB.
    pub fn open_index_only(path: &Path) -> Result<Web> {
        let bytes = std::fs::read(path)?;
        let (raw, codec) = decompress(&bytes)?;
        let (index, _) = split(&raw)?;
        Ok(Web { index, payload: None, codec })
    }

    pub fn payload(&self) -> Result<&[u8]> {
        self.payload
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("this web was opened index-only; reopen with open()"))
    }

    /// Byte offset of each geom blob, derived from the lengths in order.
    pub fn offsets(&self) -> Vec<usize> {
        let mut out = Vec::with_capacity(self.index.geom.len());
        let mut at = 0usize;
        for g in &self.index.geom {
            out.push(at);
            at += g.v + g.i;
        }
        out
    }

    /// `catalog/id`. The bare `id` is NOT unique: 195 ids in the real
    /// catalog appear in two catalogs with DIFFERENT geometry
    /// (`part_panel_35cc719e` is 233 tris in vehiclecat and 118 in
    /// greeblecat). Addressing by bare id silently picks whichever came
    /// first, which is the same class of failure PIPELINE.md records for
    /// truncated model names.
    pub fn key(p: &Part) -> String {
        match &p.catalog {
            Some(c) => format!("{c}/{}", p.id),
            None => p.id.clone(),
        }
    }

    /// Exact lookup by `catalog/id`, or by bare id when unambiguous.
    pub fn find(&self, spec: &str) -> Option<&Part> {
        if let Some(p) = self.index.parts.iter().find(|p| Web::key(p) == spec) {
            return Some(p);
        }
        let mut it = self.index.parts.iter().filter(|p| p.id == spec);
        let first = it.next()?;
        if it.next().is_some() { None } else { Some(first) }
    }

    /// Every part matching a bare id, so a caller can report the ambiguity.
    pub fn find_all(&self, spec: &str) -> Vec<&Part> {
        self.index.parts.iter()
            .filter(|p| Web::key(p) == spec || p.id == spec)
            .collect()
    }

    /// How many parts point at each geom blob.
    pub fn refcounts(&self) -> Vec<usize> {
        let mut rc = vec![0usize; self.index.geom.len()];
        for p in &self.index.parts {
            if let Some(c) = rc.get_mut(p.geom) {
                *c += 1;
            }
        }
        rc
    }

    /// Structural check. Returns every problem rather than the first, because
    /// a half-edited web usually has several and fixing them one round-trip
    /// at a time is miserable.
    pub fn verify(&self) -> Vec<String> {
        let mut errs = Vec::new();
        let ng = self.index.geom.len();
        for p in &self.index.parts {
            if p.geom >= ng {
                errs.push(format!("part {} points at geom {} of {}", p.id, p.geom, ng));
            }
        }
        let mut seen = HashSet::new();
        for p in &self.index.parts {
            let k = Web::key(p);
            if !seen.insert(k.clone()) {
                errs.push(format!("duplicate part key: {k}"));
            }
        }
        if let Some(pl) = &self.payload {
            let want: usize = self.index.geom.iter().map(|g| g.v + g.i).sum();
            if want != pl.len() {
                errs.push(format!("payload is {} bytes, index sums to {}", pl.len(), want));
            }
        }
        let orphans = self.refcounts().iter().filter(|c| **c == 0).count();
        if orphans > 0 {
            errs.push(format!("{orphans} geom blob(s) unreferenced — run `gc`"));
        }
        errs
    }

    /// Drop parts by id. Geometry is deliberately left alone: another part
    /// may share the blob, and a wrong free corrupts it silently.
    pub fn remove(&mut self, ids: &HashSet<String>) -> usize {
        let before = self.index.parts.len();
        self.index.parts.retain(|p| !ids.contains(&Web::key(p)));
        before - self.index.parts.len()
    }

    /// Repack: drop unreferenced geometry and renumber. Needs the payload.
    pub fn gc(&mut self) -> Result<(usize, usize)> {
        let pl = self.payload()?.to_vec();
        let offs = self.offsets();
        let rc = self.refcounts();

        let mut remap = HashMap::new();
        let mut new_geom = Vec::new();
        let mut new_pay = Vec::new();
        let mut freed = 0usize;
        for (old, g) in self.index.geom.iter().enumerate() {
            if rc[old] == 0 {
                freed += g.v + g.i;
                continue;
            }
            remap.insert(old, new_geom.len());
            new_geom.push(*g);
            new_pay.extend_from_slice(&pl[offs[old]..offs[old] + g.v + g.i]);
        }
        let dropped = self.index.geom.len() - new_geom.len();
        for p in &mut self.index.parts {
            if let Some(n) = remap.get(&p.geom) {
                p.geom = *n;
            }
        }
        self.index.geom = new_geom;
        self.payload = Some(new_pay);
        Ok((dropped, freed))
    }

    /// Append a part. If `blob` matches an existing blob byte-for-byte the
    /// geometry is shared — the same content-dedup `build_web.py` does, which
    /// is where 107 of the catalog's folds came from.
    pub fn add(&mut self, mut part: Part, vblob: &[u8], iblob: &[u8]) -> Result<bool> {
        let k = Web::key(&part);
        if self.index.parts.iter().any(|p| Web::key(p) == k) {
            bail!("part already present: {k}");
        }
        let pl = self.payload()?.to_vec();
        let offs = self.offsets();
        let mut blob = Vec::with_capacity(vblob.len() + iblob.len());
        blob.extend_from_slice(vblob);
        blob.extend_from_slice(iblob);

        for (i, g) in self.index.geom.iter().enumerate() {
            if g.v == vblob.len() && g.i == iblob.len()
                && &pl[offs[i]..offs[i] + g.v + g.i] == blob.as_slice()
            {
                part.geom = i;
                self.index.parts.push(part);
                return Ok(true); // shared
            }
        }
        part.geom = self.index.geom.len();
        self.index.geom.push(GeomLen { v: vblob.len(), i: iblob.len() });
        let mut np = pl;
        np.extend_from_slice(&blob);
        self.payload = Some(np);
        self.index.parts.push(part);
        Ok(false) // new blob
    }

    pub fn write(&self, path: &Path, codec: Codec) -> Result<u64> {
        let pl = self.payload()?;
        let idx = serde_json::to_vec(&self.index)?;
        let mut raw = Vec::with_capacity(14 + idx.len() + pl.len());
        raw.extend_from_slice(MAGIC);
        raw.extend_from_slice(&(idx.len() as u32).to_le_bytes());
        raw.extend_from_slice(&(pl.len() as u32).to_le_bytes());
        raw.extend_from_slice(&idx);
        raw.extend_from_slice(pl);

        // temp + rename: a half-written web is worse than no web, and these
        // get edited in place on a phone where the process can be killed.
        let tmp = path.with_extension("bsweb-tmp");
        {
            let f = std::fs::File::create(&tmp)?;
            match codec {
                Codec::Gzip => {
                    let mut e = flate2::write::GzEncoder::new(f, flate2::Compression::new(9));
                    e.write_all(&raw)?;
                    e.finish()?;
                }
                Codec::Zstd => {
                    let mut e = zstd::Encoder::new(f, 19)?;
                    e.write_all(&raw)?;
                    e.finish()?;
                }
                Codec::Raw => {
                    let mut f = f;
                    f.write_all(&raw)?;
                }
            }
        }
        std::fs::rename(&tmp, path)?;
        Ok(std::fs::metadata(path)?.len())
    }
}

/// Glob-ish filter: `*` only, which covers `car-*` and `*wheel*` and keeps
/// the surface small enough to not need a regex dependency.
pub fn matches(pattern: &str, s: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or("");
    if !s.starts_with(first) {
        return false;
    }
    let mut at = first.len();
    let rest: Vec<&str> = parts.collect();
    if rest.is_empty() {
        return s.len() == first.len();
    }
    for (n, seg) in rest.iter().enumerate() {
        if seg.is_empty() {
            continue;
        }
        match s[at..].find(seg) {
            Some(i) => at += i + seg.len(),
            None => return false,
        }
        if n == rest.len() - 1 && !pattern.ends_with('*') && at != s.len() {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(n: u8, len: usize) -> Vec<u8> {
        vec![n; len]
    }

    fn part(id: &str, geom: usize, name: &str, from: &str) -> Part {
        Part {
            id: id.into(),
            catalog: Some("vehiclecat".into()),
            geom,
            nv: 10,
            nt: 4,
            isize: 2,
            lo: vec![0.0; 3],
            span: vec![1.0; 3],
            name: Some(name.into()),
            from: Some(from.into()),
            sig: None,
            instances: Some(3),
            tris_src: None,
            size: None,
            residual_mean: None,
            residual_max: None,
            extra: Default::default(),
        }
    }

    fn web() -> Web {
        // two parts sharing geom 0 — the dedup case that removal must respect
        let index = Index {
            version: 1,
            parts: vec![
                part("a", 0, "roadwheel", "car-alfa"),
                part("b", 0, "roadwheel", "car-bmw"),
                part("c", 1, "barrel", "tank-t35"),
            ],
            geom: vec![GeomLen { v: 4, i: 2 }, GeomLen { v: 3, i: 1 }],
            bits: None,
            delta: None,
            extra: Default::default(),
        };
        let mut pay = blob(1, 4);
        pay.extend(blob(2, 2));
        pay.extend(blob(3, 3));
        pay.extend(blob(4, 1));
        Web { index, payload: Some(pay), codec: Codec::Raw }
    }

    #[test]
    fn offsets_follow_the_lengths() {
        assert_eq!(web().offsets(), vec![0, 6]);
    }

    #[test]
    fn refcounts_see_shared_blobs() {
        assert_eq!(web().refcounts(), vec![2, 1]);
    }

    #[test]
    fn a_clean_web_verifies() {
        assert!(web().verify().is_empty());
    }

    #[test]
    fn removing_one_sharer_does_not_free_the_blob() {
        // The bug this guards: freeing geometry on the first removal
        // corrupts every other part pointing at it, invisibly.
        let mut w = web();
        w.remove(&["vehiclecat/a".to_string()].into_iter().collect());
        assert_eq!(w.refcounts(), vec![1, 1]);
        assert_eq!(w.payload().unwrap().len(), 10, "payload must not shrink");
        assert!(w.verify().is_empty());
        assert_eq!(w.find("vehiclecat/b").unwrap().geom, 0, "b still resolves");
    }

    #[test]
    fn gc_frees_only_after_the_last_referrer_goes() {
        let mut w = web();
        w.remove(&["vehiclecat/a".into(), "vehiclecat/b".into()].into_iter().collect());
        let errs = w.verify();
        assert_eq!(errs.len(), 1, "orphan should be reported: {errs:?}");
        let (dropped, freed) = w.gc().unwrap();
        assert_eq!((dropped, freed), (1, 6));
        assert_eq!(w.find("vehiclecat/c").unwrap().geom, 0, "c renumbered");
        assert_eq!(w.payload().unwrap().len(), 4);
        assert!(w.verify().is_empty());
    }

    #[test]
    fn add_shares_identical_geometry() {
        let mut w = web();
        let shared = w.add(part("d", 0, "roadwheel", "car-x"), &blob(1, 4), &blob(2, 2)).unwrap();
        assert!(shared, "byte-identical blob must fold");
        assert_eq!(w.index.geom.len(), 2, "no new blob");
        assert_eq!(w.find("vehiclecat/d").unwrap().geom, 0);
        assert_eq!(w.payload().unwrap().len(), 10);
    }

    #[test]
    fn add_appends_novel_geometry() {
        let mut w = web();
        let shared = w.add(part("e", 0, "strut", "car-y"), &blob(9, 5), &blob(8, 2)).unwrap();
        assert!(!shared);
        assert_eq!(w.index.geom.len(), 3);
        assert_eq!(w.payload().unwrap().len(), 17);
        assert!(w.verify().is_empty());
    }

    #[test]
    fn add_refuses_a_duplicate_id() {
        let mut w = web();
        assert!(w.add(part("a", 0, "x", "y"), &blob(7, 1), &blob(7, 1)).is_err());
    }

    #[test]
    fn verify_catches_a_dangling_geom_pointer() {
        let mut w = web();
        w.index.parts[0].geom = 99;
        assert!(w.verify().iter().any(|e| e.contains("points at geom 99")));
    }

    #[test]
    fn verify_catches_payload_length_drift() {
        let mut w = web();
        w.payload = Some(vec![0; 3]);
        assert!(w.verify().iter().any(|e| e.contains("payload is 3 bytes")));
    }

    #[test]
    fn round_trips_through_every_codec() {
        let dir = std::env::temp_dir().join(format!("bsweb_rt_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (c, ext) in [(Codec::Raw, "bin"), (Codec::Gzip, "bin.gz"), (Codec::Zstd, "bin.zst")] {
            let p = dir.join(format!("w.{ext}"));
            let w = web();
            w.write(&p, c).unwrap();
            let r = Web::open(&p).unwrap();
            assert_eq!(r.codec, c, "codec sniff for {ext}");
            assert_eq!(r.index.parts.len(), 3);
            assert_eq!(r.payload().unwrap(), w.payload().unwrap());
            assert!(r.verify().is_empty());
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unknown_index_fields_survive_a_round_trip() {
        // A newer build_web.py will add keys. Dropping them on save would
        // quietly degrade the file every time this tool touched it.
        let mut w = web();
        w.index.extra.insert("future_key".into(), serde_json::json!(42));
        w.index.parts[0].extra.insert("lod".into(), serde_json::json!("high"));
        let p = std::env::temp_dir().join(format!("bsweb_fwd_{}.bin", std::process::id()));
        w.write(&p, Codec::Raw).unwrap();
        let r = Web::open(&p).unwrap();
        assert_eq!(r.index.extra["future_key"], 42);
        assert_eq!(r.index.parts[0].extra["lod"], "high");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn bad_magic_is_rejected() {
        assert!(split(b"NOPE\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00").is_err());
    }

    #[test]
    fn truncation_is_reported_not_panicked() {
        let mut raw = MAGIC.to_vec();
        raw.extend_from_slice(&(100u32).to_le_bytes());
        raw.extend_from_slice(&(100u32).to_le_bytes());
        let e = split(&raw).unwrap_err().to_string();
        assert!(e.contains("truncated"), "{e}");
    }

    #[test]
    fn same_id_in_two_catalogs_is_two_parts() {
        // The real catalog has 195 of these: part_panel_35cc719e is 233 tris
        // in vehiclecat and 118 in greeblecat. Bare-id addressing picks one
        // at random; the composite key is what makes them distinct.
        let mut w = web();
        let mut clash = part("a", 1, "panel", "other");
        clash.catalog = Some("greeblecat".into());
        w.index.parts.push(clash);
        assert!(w.verify().is_empty(), "same id, different catalog is legal");
        assert!(w.find("a").is_none(), "bare id must refuse to guess");
        assert_eq!(w.find_all("a").len(), 2);
        assert_eq!(w.find("vehiclecat/a").unwrap().geom, 0);
        assert_eq!(w.find("greeblecat/a").unwrap().geom, 1);
    }

    #[test]
    fn glob_matching() {
        assert!(matches("car-*", "car-alfa"));
        assert!(!matches("car-*", "tank-t35"));
        assert!(matches("*wheel*", "roadwheel_front"));
        assert!(matches("barrel", "barrel"));
        assert!(!matches("barrel", "barrel_2"));
        assert!(matches("*", "anything"));
    }
}

// ---------------------------------------------------------------- journal

/// An edit, as a value.
///
/// Same bargain `aperture-core::op` makes: every mutation carries enough to
/// invert itself, so undo is "apply the inverse", a macro is a `Vec<Op>`, and
/// a test is a fold of ops over a starting web. One mechanism, no command
/// classes.
///
/// This exists because the GUI edits on a machine that travels. A desktop
/// with a keyboard is as reliable as any desktop — `write` is already
/// temp-then-rename — but a nomad edits in places where they cannot re-run
/// the pipeline, and "I removed the wrong 500 parts and saved" needs an
/// answer that is not "restore from the copy you did not make".
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// Parts removed, with their original index so undo restores order.
    Removed(Vec<(usize, Part)>),
    /// A part appended. Undo pops it. `shared` records whether `add` folded
    /// onto existing geometry — if it did NOT, undo must also drop the blob.
    Added { key: String, shared: bool, geom: usize },
}

#[derive(Default)]
pub struct Journal {
    done: Vec<Op>,
    undone: Vec<Op>,
}

impl Journal {
    pub fn new() -> Self { Self::default() }
    pub fn can_undo(&self) -> bool { !self.done.is_empty() }
    pub fn can_redo(&self) -> bool { !self.undone.is_empty() }
    pub fn depth(&self) -> usize { self.done.len() }
    /// A short label for the UI, newest first.
    pub fn describe(&self) -> Option<String> {
        self.done.last().map(|op| match op {
            Op::Removed(v) => format!("remove {} part(s)", v.len()),
            Op::Added { key, .. } => format!("add {key}"),
        })
    }
    fn push(&mut self, op: Op) {
        self.done.push(op);
        self.undone.clear();   // a new edit forks the future
    }
}

impl Web {
    /// Remove, recording enough to put it all back.
    pub fn remove_journaled(&mut self, ids: &HashSet<String>, j: &mut Journal) -> usize {
        let mut taken = Vec::new();
        let mut i = 0;
        self.index.parts.retain(|p| {
            let hit = ids.contains(&Web::key(p));
            if hit { taken.push((i, p.clone())); }
            i += 1;
            !hit
        });
        let n = taken.len();
        if n > 0 { j.push(Op::Removed(taken)); }
        n
    }

    pub fn add_journaled(&mut self, part: Part, v: &[u8], idx: &[u8], j: &mut Journal)
        -> Result<bool>
    {
        let key = Web::key(&part);
        let shared = self.add(part, v, idx)?;
        let geom = self.index.parts.last().map(|p| p.geom).unwrap_or(0);
        j.push(Op::Added { key, shared, geom });
        Ok(shared)
    }

    pub fn undo(&mut self, j: &mut Journal) -> Option<String> {
        let op = j.done.pop()?;
        let label = self.apply_inverse(&op);
        j.undone.push(op);
        Some(label)
    }

    pub fn redo(&mut self, j: &mut Journal) -> Option<String> {
        let op = j.undone.pop()?;
        let label = self.apply_forward(&op);
        j.done.push(op);
        Some(label)
    }

    fn apply_inverse(&mut self, op: &Op) -> String {
        match op {
            Op::Removed(v) => {
                // ascending index, so each insert lands where it came from
                for (i, p) in v.iter() {
                    let at = (*i).min(self.index.parts.len());
                    self.index.parts.insert(at, p.clone());
                }
                format!("restored {} part(s)", v.len())
            }
            Op::Added { key, shared, .. } => {
                self.index.parts.retain(|p| &Web::key(p) != key);
                // A fold added no blob, so there is none to drop. A novel
                // blob is left in place and reclaimed by `gc` — removing it
                // here would have to renumber every later geom pointer,
                // which is exactly the corruption `gc` exists to do safely.
                let _ = shared;
                format!("removed {key}")
            }
        }
    }

    fn apply_forward(&mut self, op: &Op) -> String {
        match op {
            Op::Removed(v) => {
                let keys: HashSet<String> = v.iter().map(|(_, p)| Web::key(p)).collect();
                let n = self.remove(&keys);
                format!("re-removed {n} part(s)")
            }
            Op::Added { key, geom, .. } => {
                let _ = (key, geom);
                format!("redo add is not reconstructible without the blob")
            }
        }
    }
}

#[cfg(test)]
mod journal_tests {
    use super::*;

    fn w() -> Web {
        let index = Index {
            version: 1,
            parts: vec![
                Part { id: "a".into(), catalog: Some("vc".into()), geom: 0, nv: 1, nt: 1,
                       isize: 2, lo: vec![], span: vec![], name: Some("wheel".into()),
                       from: None, sig: None, instances: None, tris_src: None, size: None,
                       residual_mean: None, residual_max: None, extra: Default::default() },
                Part { id: "b".into(), catalog: Some("vc".into()), geom: 0, nv: 1, nt: 1,
                       isize: 2, lo: vec![], span: vec![], name: Some("wheel".into()),
                       from: None, sig: None, instances: None, tris_src: None, size: None,
                       residual_mean: None, residual_max: None, extra: Default::default() },
                Part { id: "c".into(), catalog: Some("vc".into()), geom: 1, nv: 1, nt: 1,
                       isize: 2, lo: vec![], span: vec![], name: Some("strut".into()),
                       from: None, sig: None, instances: None, tris_src: None, size: None,
                       residual_mean: None, residual_max: None, extra: Default::default() },
            ],
            geom: vec![GeomLen { v: 4, i: 2 }, GeomLen { v: 3, i: 1 }],
            bits: None, delta: None, extra: Default::default(),
        };
        Web { index, payload: Some(vec![7u8; 10]), codec: Codec::Raw }
    }

    #[test]
    fn undo_restores_content_and_order() {
        let mut web = w();
        let mut j = Journal::new();
        let before: Vec<String> = web.index.parts.iter().map(Web::key).collect();
        web.remove_journaled(&["vc/a".into(), "vc/c".into()].into_iter().collect(), &mut j);
        assert_eq!(web.index.parts.len(), 1);
        assert!(j.can_undo());
        web.undo(&mut j);
        let after: Vec<String> = web.index.parts.iter().map(Web::key).collect();
        assert_eq!(before, after, "order must be restored, not just membership");
        assert!(web.verify().is_empty());
    }

    #[test]
    fn undo_then_redo_round_trips() {
        let mut web = w();
        let mut j = Journal::new();
        web.remove_journaled(&["vc/b".into()].into_iter().collect(), &mut j);
        web.undo(&mut j);
        assert_eq!(web.index.parts.len(), 3);
        assert!(j.can_redo());
        web.redo(&mut j);
        assert_eq!(web.index.parts.len(), 2);
        assert!(web.find("vc/b").is_none());
    }

    #[test]
    fn a_new_edit_forks_the_future() {
        let mut web = w();
        let mut j = Journal::new();
        web.remove_journaled(&["vc/a".into()].into_iter().collect(), &mut j);
        web.undo(&mut j);
        assert!(j.can_redo());
        web.remove_journaled(&["vc/c".into()].into_iter().collect(), &mut j);
        assert!(!j.can_redo(), "redo stack must be cleared by a new edit");
    }

    #[test]
    fn undo_never_touches_the_payload() {
        // The invariant that matters: geometry is shared, so an index-level
        // undo must not free or reorder blobs.
        let mut web = w();
        let mut j = Journal::new();
        let pay = web.payload().unwrap().to_vec();
        web.remove_journaled(&["vc/a".into(), "vc/b".into()].into_iter().collect(), &mut j);
        web.undo(&mut j);
        assert_eq!(web.payload().unwrap(), pay.as_slice());
        assert!(web.verify().is_empty());
    }

    #[test]
    fn deep_stack_unwinds_completely() {
        let mut web = w();
        let mut j = Journal::new();
        let before: Vec<String> = web.index.parts.iter().map(Web::key).collect();
        for k in ["vc/a", "vc/b", "vc/c"] {
            web.remove_journaled(&[k.to_string()].into_iter().collect(), &mut j);
        }
        assert_eq!(web.index.parts.len(), 0);
        assert_eq!(j.depth(), 3);
        while j.can_undo() { web.undo(&mut j); }
        assert_eq!(before, web.index.parts.iter().map(Web::key).collect::<Vec<_>>());
    }

    #[test]
    fn describe_labels_the_top_of_the_stack() {
        let mut web = w();
        let mut j = Journal::new();
        assert_eq!(j.describe(), None);
        web.remove_journaled(&["vc/a".into(), "vc/b".into()].into_iter().collect(), &mut j);
        assert_eq!(j.describe().as_deref(), Some("remove 2 part(s)"));
    }
}
