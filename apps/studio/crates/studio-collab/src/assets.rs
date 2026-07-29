//! assets — moving the media, not just the edits.
//!
//! `wire` and `carrier` move *ops*, which are tiny. Media is not: a session
//! where everyone can see the same timeline but only one person has the takes
//! is not collaboration, it is screen-sharing with extra steps.
//!
//! The design is content-addressed chunking, for one reason that matters more
//! than the others: **peers usually already have most of it**. Two editors
//! working from the same shoot have the same rushes; a re-render of a bounce
//! changes a few seconds of a ten-minute file; someone rejoining a session has
//! everything from yesterday. Addressing chunks by their content rather than
//! their position means all of those cost nothing to discover and nothing to
//! transfer.
//!
//! ## Content-defined boundaries, not fixed offsets
//!
//! Chunks are split where a rolling hash of the last few bytes hits a pattern,
//! not every N bytes. The difference shows up the moment anything is inserted:
//! with fixed offsets, adding one frame at the head of a file shifts every
//! subsequent boundary and **nothing** dedupes; with content-defined
//! boundaries the split points move with the content, so only the chunks that
//! actually changed are new. That property is the whole reason to do this
//! rather than just gzip the file, and there is a test for it.
//!
//! ## What this deliberately does not do
//!
//! No transfer scheduling, no rarest-first, no peer selection. `need` answers
//! "what am I missing" and the carrier moves bytes; anything cleverer belongs
//! with whoever is actually doing the moving, and guessing at it here would be
//! inventing policy for a network this crate cannot see.

use std::collections::{BTreeMap, BTreeSet};

/// A chunk's content hash. FNV-1a 64: not cryptographic, and not pretending to
/// be — this is a dedupe and integrity check between peers who already trust
/// each other enough to be editing the same project. A malicious peer is a
/// problem for the transport's signing, not for this.
pub type ChunkId = u64;

pub fn hash(bytes: &[u8]) -> ChunkId {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// Target chunk size. Small enough that a changed region costs little to
/// re-send, large enough that a two-hour take does not produce a manifest
/// bigger than the edit history.
pub const TARGET_CHUNK: usize = 64 * 1024;
pub const MIN_CHUNK: usize = 16 * 1024;
pub const MAX_CHUNK: usize = 256 * 1024;

/// Window the rolling hash looks at. Only these many trailing bytes may
/// influence a boundary decision — that is what makes the split points a
/// property of the CONTENT rather than of where the previous split happened.
pub const WINDOW: usize = 48;

/// Buzhash table. Content-defined chunking needs a hash that can have a byte
/// removed from the back as cheaply as one is added to the front; a cyclic
/// polynomial over a random table does exactly that.
fn table() -> &'static [u64; 256] {
    use std::sync::OnceLock;
    static T: OnceLock<[u64; 256]> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = [0u64; 256];
        let mut s: u64 = 0x243F_6A88_85A3_08D3;
        for v in t.iter_mut() {
            s ^= s << 13; s ^= s >> 7; s ^= s << 17;
            *v = s;
        }
        t
    })
}

/// Split into content-defined chunks.
///
/// A chunk ends where a rolling hash of the last [`WINDOW`] bytes hits a
/// pattern. The first version accumulated over the whole chunk instead of a
/// window and reset at each boundary, which quietly defeats the entire idea:
/// boundaries then depend on where the PREVIOUS boundary fell, so the same
/// bytes at a different offset split differently and nothing dedupes. Two tests
/// caught it — a file containing the same block twice shared no chunks, and a
/// run of zeros did not either.
pub fn split(data: &[u8]) -> Vec<(usize, usize)> {
    let t = table();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut roll: u64 = 0;
    let mask = (TARGET_CHUNK as u64).next_power_of_two() - 1;
    for i in 0..data.len() {
        // add the incoming byte, remove the one leaving the window
        roll = roll.rotate_left(1) ^ t[data[i] as usize];
        if i >= WINDOW {
            roll ^= t[data[i - WINDOW] as usize].rotate_left(WINDOW as u32);
        }
        let len = i - start + 1;
        if (len >= MIN_CHUNK && (roll & mask) == 0) || len >= MAX_CHUNK {
            out.push((start, i + 1));
            start = i + 1;
            // NOTE: the hash is deliberately NOT reset. Carrying it across the
            // boundary is what keeps the window purely positional — resetting
            // would reintroduce the dependence on the previous split.
        }
    }
    if start < data.len() {
        out.push((start, data.len()));
    }
    out
}

/// What a peer needs to know about an asset to fetch it.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    /// The whole asset's hash, so a peer can tell two takes apart at a glance.
    pub asset: ChunkId,
    pub total_len: usize,
    /// Chunks in order. Duplicates are expected and are the point — a file with
    /// a repeated section stores it once.
    pub chunks: Vec<ChunkId>,
}

impl Manifest {
    pub fn build(data: &[u8]) -> (Manifest, BTreeMap<ChunkId, Vec<u8>>) {
        let mut store = BTreeMap::new();
        let mut chunks = Vec::new();
        for (a, b) in split(data) {
            let piece = &data[a..b];
            let id = hash(piece);
            chunks.push(id);
            store.entry(id).or_insert_with(|| piece.to_vec());
        }
        (Manifest { asset: hash(data), total_len: data.len(), chunks }, store)
    }

    /// Which chunks this manifest needs that `have` does not contain. Deduped
    /// and in first-appearance order, so a caller fetching them sequentially
    /// gets the head of the file first — which is what lets playback start
    /// before the transfer finishes.
    pub fn need(&self, have: &BTreeSet<ChunkId>) -> Vec<ChunkId> {
        let mut seen = BTreeSet::new();
        self.chunks.iter().copied()
            .filter(|c| !have.contains(c) && seen.insert(*c))
            .collect()
    }

    /// Rebuild the asset, verifying as it goes.
    ///
    /// Every chunk is re-hashed rather than trusted: a corrupt or substituted
    /// chunk must fail loudly here, because the alternative is a take that is
    /// subtly wrong in one place and gets discovered at the mix.
    pub fn assemble(&self, store: &BTreeMap<ChunkId, Vec<u8>>) -> Result<Vec<u8>, AssetError> {
        let mut out = Vec::with_capacity(self.total_len);
        for id in &self.chunks {
            let piece = store.get(id).ok_or(AssetError::Missing(*id))?;
            if hash(piece) != *id {
                return Err(AssetError::Corrupt(*id));
            }
            out.extend_from_slice(piece);
        }
        if out.len() != self.total_len {
            return Err(AssetError::WrongLength { expected: self.total_len, got: out.len() });
        }
        if hash(&out) != self.asset {
            return Err(AssetError::Corrupt(self.asset));
        }
        Ok(out)
    }

    /// Fraction already held, for the progress the UI shows next to a clip whose
    /// media is still arriving.
    pub fn progress(&self, have: &BTreeSet<ChunkId>) -> f32 {
        if self.chunks.is_empty() { return 1.0; }
        let unique: BTreeSet<ChunkId> = self.chunks.iter().copied().collect();
        let got = unique.iter().filter(|c| have.contains(c)).count();
        got as f32 / unique.len() as f32
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AssetError {
    Missing(ChunkId),
    Corrupt(ChunkId),
    WrongLength { expected: usize, got: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-media: varied enough that chunk boundaries land in
    /// different places, which is what the dedupe test needs to mean anything.
    fn media(seed: u64, len: usize) -> Vec<u8> {
        // NOT `seed | 1` — that maps 42 and 43 to the same value, so a test
        // comparing two "different" files was comparing one file with itself.
        let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        (0..len).map(|_| {
            s ^= s << 13; s ^= s >> 7; s ^= s << 17;
            (s & 0xFF) as u8
        }).collect()
    }

    #[test]
    fn the_same_file_always_produces_the_same_manifest() {
        // Two peers must agree on chunk boundaries without talking, or nothing
        // dedupes and every transfer is a full copy.
        let data = media(42, 900_000);
        let (a, _) = Manifest::build(&data);
        let (b, _) = Manifest::build(&data);
        assert_eq!(a, b);
        assert!(a.chunks.len() > 4, "a 900 KB file should split into several chunks");
        let (c, _) = Manifest::build(&media(43, 900_000));
        assert_ne!(a.asset, c.asset, "different files are told apart");
    }

    #[test]
    fn an_insertion_at_the_head_only_costs_the_chunks_it_touched() {
        // THE property, and the whole reason for content-defined boundaries.
        // With fixed offsets, inserting one frame at the head shifts every
        // subsequent boundary and NOTHING dedupes — a ten-minute file would
        // re-transfer in full because a second was added at the front.
        let original = media(7, 800_000);
        let mut edited = vec![0u8; 5000]; // a new head
        edited.extend_from_slice(&original);

        let (m1, s1) = Manifest::build(&original);
        let (m2, _) = Manifest::build(&edited);
        let have: BTreeSet<ChunkId> = s1.keys().copied().collect();
        let missing = m2.need(&have);

        let shared = m2.chunks.iter().filter(|c| have.contains(c)).count();
        assert!(shared > m2.chunks.len() / 2,
                "most chunks should survive an insertion: {shared} of {}", m2.chunks.len());
        assert!(missing.len() <= 3,
                "only the disturbed region should be new, got {} chunks", missing.len());
    }

    #[test]
    fn a_peer_that_already_has_everything_needs_nothing() {
        let data = media(11, 400_000);
        let (m, store) = Manifest::build(&data);
        let have: BTreeSet<ChunkId> = store.keys().copied().collect();
        assert!(m.need(&have).is_empty(), "nothing to fetch");
        assert_eq!(m.progress(&have), 1.0);
        // and a peer with nothing needs every distinct chunk, once each
        let none = BTreeSet::new();
        let need = m.need(&none);
        let unique: BTreeSet<ChunkId> = m.chunks.iter().copied().collect();
        assert_eq!(need.len(), unique.len(), "deduped");
        assert_eq!(m.progress(&none), 0.0);
    }

    #[test]
    fn needed_chunks_come_head_first_so_playback_can_start_early() {
        let data = media(5, 600_000);
        let (m, _) = Manifest::build(&data);
        let need = m.need(&BTreeSet::new());
        // the first thing asked for is the first chunk of the file
        assert_eq!(need[0], m.chunks[0]);
        // and the order follows first appearance
        let mut pos = 0usize;
        for id in &need {
            let at = m.chunks.iter().position(|c| c == id).unwrap();
            assert!(at >= pos, "fetch order should follow the file");
            pos = at;
        }
    }

    #[test]
    fn a_repeated_section_is_stored_once() {
        // Silence, colour bars, a repeated bounce — media has more repetition
        // than people expect, and content addressing gets it for free.
        let block = media(3, 300_000);
        let mut doubled = block.clone();
        doubled.extend_from_slice(&block);
        let (m, store) = Manifest::build(&doubled);
        let unique: BTreeSet<ChunkId> = m.chunks.iter().copied().collect();
        assert!(unique.len() < m.chunks.len(), "the repeat should share chunks");
        assert_eq!(store.len(), unique.len(), "and be stored once");
        // it still rebuilds correctly
        assert_eq!(m.assemble(&store).unwrap(), doubled);
    }

    #[test]
    fn assembly_verifies_every_chunk_rather_than_trusting_it() {
        // A take that is subtly wrong in one place gets discovered at the mix,
        // which is far too late.
        let data = media(9, 500_000);
        let (m, mut store) = Manifest::build(&data);
        assert_eq!(m.assemble(&store).unwrap(), data);

        // a substituted chunk fails loudly
        let victim = m.chunks[1];
        store.insert(victim, vec![0u8; 32]);
        assert_eq!(m.assemble(&store), Err(AssetError::Corrupt(victim)));

        // a missing chunk names itself
        let (m2, mut s2) = Manifest::build(&media(10, 400_000));
        let gone = m2.chunks[0];
        s2.remove(&gone);
        assert_eq!(m2.assemble(&s2), Err(AssetError::Missing(gone)));
    }

    #[test]
    fn chunking_survives_awkward_input() {
        // Empty files, tiny files, and a pathological all-zero file that would
        // otherwise produce either one chunk or a million.
        let (m, s) = Manifest::build(&[]);
        assert!(m.chunks.is_empty());
        assert_eq!(m.assemble(&s).unwrap(), Vec::<u8>::new());
        assert_eq!(m.progress(&BTreeSet::new()), 1.0, "an empty asset is complete");

        let tiny = vec![7u8; 100];
        let (m, s) = Manifest::build(&tiny);
        assert_eq!(m.chunks.len(), 1);
        assert_eq!(m.assemble(&s).unwrap(), tiny);

        let zeros = vec![0u8; 700_000];
        let (m, s) = Manifest::build(&zeros);
        assert!(m.chunks.len() >= 2, "the max bound splits it");
        // every FULL chunk is identical; the tail is shorter and so differs
        let full = &m.chunks[..m.chunks.len() - 1];
        assert!(full.iter().all(|c| *c == full[0]), "every full chunk of zeros is identical");
        assert!(s.len() <= 2, "so it stores one, plus the short tail: {}", s.len());
        assert_eq!(m.assemble(&s).unwrap(), zeros);
    }
}
