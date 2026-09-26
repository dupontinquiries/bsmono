//! pattern_pack — one drum loop as one file, throwable anywhere.
//!
//! A pattern is only meaningful with the channels it fires and the samples
//! those channels play, so the shareable unit is all three: pattern +
//! channel subset + the sample FILES (the compressed originals — the same
//! "the compact representation of a sample IS the file" principle
//! `import_audio` states). The container follows the archive's refusal
//! discipline — magic checked, every length checked, any malformation
//! refuses the WHOLE file — but is its own small format rather than a
//! bent `.studioarch`: an archive answers "reopen this session anywhere",
//! a pack answers "drop this loop into a DIFFERENT session", and forcing
//! one container to mean both would leave every reader guessing which.
//!
//! ## Where packs live
//!
//! Publishing writes to the per-user cache (`$XDG_CACHE_HOME/studio-live`,
//! falling back to `~/.cache/studio-live` — the Linux convention the ask
//! named), which every project on the machine lists from. A pack is also
//! just a file: copy it to a phone, a peer, a sample-pack zip — the format
//! carries everything, the cache is merely the rendezvous. "Publishing" IS
//! the placement: a pack in the cache is available to other projects, one
//! outside it is not, and there is no second flag to disagree with where
//! the file actually is.
//!
//! ## Import remapping
//!
//! A `Sampler` channel references a `SourceId` from the EXPORTING project;
//! import mints fresh sources in the importing one and rewrites channels.
//! Pattern rows/mods are indexed by channel POSITION, so they shift by the
//! importing rack's existing channel count — pinned by test, because an
//! off-by-one here plays the hat pattern on the kick.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use studio_live::{Channel, Pattern};

/// [`Pattern`] decomposed for JSON: its `mods` map is keyed by a
/// `(usize, u32)` tuple, which JSON objects cannot express (the wire's
/// hand-rolled binary never met this; serde_json does). Vectors of pairs
/// carry the same information and rebuild losslessly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PatternFlat {
    pub name: String,
    pub steps: u32,
    pub rows: Vec<u64>,
    pub mods: Vec<((usize, u32), studio_live::StepMod)>,
    pub rolls: Vec<(usize, Vec<studio_live::Note>)>,
}

impl PatternFlat {
    pub fn from_pattern(p: &Pattern) -> PatternFlat {
        PatternFlat { name: p.name.clone(), steps: p.steps, rows: p.rows.clone(),
                      mods: p.mods.iter().map(|(k, v)| (*k, v.clone())).collect(),
                      rolls: p.rolls.iter().map(|(k, v)| (*k, v.clone())).collect() }
    }
    pub fn into_pattern(self) -> Pattern {
        Pattern { name: self.name, steps: self.steps, rows: self.rows,
                  mods: self.mods.into_iter().collect(),
                  rolls: self.rolls.into_iter().collect() }
    }
}

pub const MAGIC: &[u8; 8] = b"STPACK\x00\x01";

/// The JSON half: everything but the media bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackMeta {
    pub name: String,
    pub tempo_bpm: f32,
    pub swing: f32,
    /// The channels the pattern fires, in pattern-row order. `Sampler`
    /// sources here are the EXPORTER's ids; [`media`](Self::media) carries
    /// the bytes under the same ids, and import remaps both together.
    pub channels: Vec<Channel>,
    pub pattern: PatternFlat,
    /// (exporter SourceId, file-name hint for the decoder) per embedded
    /// media entry, in the order the byte sections follow the JSON.
    pub media: Vec<(u64, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PatternPack {
    pub meta: PackMeta,
    /// Raw file bytes per `meta.media` entry, same order.
    pub media: Vec<Vec<u8>>,
}

impl PatternPack {
    pub fn encode(&self) -> Vec<u8> {
        let json = serde_json::to_vec(&self.meta).expect("meta is plain data");
        let mut o = Vec::with_capacity(json.len() + 64);
        o.extend_from_slice(MAGIC);
        o.extend_from_slice(&(json.len() as u32).to_le_bytes());
        o.extend_from_slice(&json);
        for m in &self.media {
            o.extend_from_slice(&(m.len() as u32).to_le_bytes());
            o.extend_from_slice(m);
        }
        o
    }

    /// Whole-file refusal: any malformation is `None`, never a partial
    /// pack — a loop with a silently missing kick is worse than no loop.
    pub fn decode(b: &[u8]) -> Option<PatternPack> {
        if b.len() < MAGIC.len() + 4 || &b[..MAGIC.len()] != MAGIC { return None; }
        let mut at = MAGIC.len();
        let jlen = u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?) as usize;
        at += 4;
        let meta: PackMeta = serde_json::from_slice(b.get(at..at + jlen)?).ok()?;
        at += jlen;
        let mut media = Vec::with_capacity(meta.media.len());
        for _ in 0..meta.media.len() {
            let len = u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?) as usize;
            at += 4;
            media.push(b.get(at..at + len)?.to_vec());
            at += len;
        }
        if at != b.len() { return None; } // trailing garbage: refuse whole
        Some(PatternPack { meta, media })
    }
}

/// Where published packs live. `$STUDIO_LIVE_CACHE` overrides (tests and
/// unusual setups), else the XDG convention the ask named.
pub fn cache_dir() -> PathBuf {
    if let Ok(p) = std::env::var("STUDIO_LIVE_CACHE") {
        return PathBuf::from(p);
    }
    if let Ok(x) = std::env::var("XDG_CACHE_HOME") {
        if !x.is_empty() { return Path::new(&x).join("studio-live"); }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    Path::new(&home).join(".cache").join("studio-live")
}

/// Publish = place in the cache. Returns the path the pack now lives at.
pub fn publish(pack: &PatternPack, dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let file = dir.join(format!("{}.stpack", crate::export::sanitize(&pack.meta.name)));
    std::fs::write(&file, pack.encode())?;
    Ok(file)
}

/// Every pack in the cache, (display name, path), name-sorted. Unreadable
/// or malformed files are skipped — one corrupt pack must not hide the
/// rest of the shelf.
pub fn list(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("stpack") { continue; }
        if let Some(pack) = std::fs::read(&p).ok().and_then(|b| PatternPack::decode(&b)) {
            out.push((pack.meta.name, p));
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use studio_live::SynthKind;

    fn pack() -> PatternPack {
        let mut pattern = Pattern { name: "boom".into(), steps: 16,
                                    rows: vec![0b1001, 0b0100],
                                    mods: BTreeMap::new(),
                                    rolls: Default::default() };
        pattern.mods.insert((1, 2), studio_live::StepMod {
            vel: 0.5, ..Default::default() });
        PatternPack {
            meta: PackMeta {
                name: "trap 140".into(), tempo_bpm: 140.0, swing: 0.11,
                channels: vec![
                    Channel { name: "kick".into(),
                        kind: SynthKind::Sine { hz: 55.0 },
                        gain: 1.0, pan: 0.0, muted: false, choke_group: None, one_shot: false },
                    Channel { name: "snap".into(),
                        kind: SynthKind::Sampler { source: studio_core::SourceId(7) },
                        gain: 0.9, pan: 0.1, muted: false, choke_group: Some(1), one_shot: false },
                ],
                pattern: PatternFlat::from_pattern(&pattern),
                media: vec![(7, "snap.opus".into())],
            },
            media: vec![vec![1, 2, 3, 4, 5]],
        }
    }

    #[test]
    fn a_pack_round_trips_and_refuses_any_malformation() {
        let p = pack();
        let bytes = p.encode();
        assert_eq!(PatternPack::decode(&bytes), Some(p.clone()));
        // Truncation anywhere refuses the whole file.
        for cut in [4, MAGIC.len() + 2, bytes.len() - 1] {
            assert_eq!(PatternPack::decode(&bytes[..cut]), None, "cut at {cut}");
        }
        // Trailing garbage refuses too — a pack is exactly its bytes.
        let mut long = bytes.clone();
        long.push(0);
        assert_eq!(PatternPack::decode(&long), None);
        // Wrong magic is someone else's file.
        let mut wrong = bytes;
        wrong[0] = b'X';
        assert_eq!(PatternPack::decode(&wrong), None);
    }

    #[test]
    fn the_cache_is_the_shelf_and_a_corrupt_pack_hides_only_itself() {
        let dir = std::env::temp_dir()
            .join(format!("stpack-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = pack();
        let at = publish(&p, &dir).expect("writes");
        assert_eq!(at.extension().and_then(|x| x.to_str()), Some("stpack"));
        std::fs::write(dir.join("broken.stpack"), b"not a pack").unwrap();
        std::fs::write(dir.join("readme.txt"), b"ignored").unwrap();
        let shelf = list(&dir);
        assert_eq!(shelf.len(), 1, "the corrupt pack and the txt are skipped");
        assert_eq!(shelf[0].0, "trap 140");
        let back = PatternPack::decode(&std::fs::read(&shelf[0].1).unwrap()).unwrap();
        assert_eq!(back, p, "what the shelf lists is what was published");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
