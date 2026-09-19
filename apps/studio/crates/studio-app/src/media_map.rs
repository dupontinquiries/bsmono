//! media_map — which file on disk is which clip, and what to do when it moves.
//!
//! ## The pieces existed; nothing joined them
//!
//! `studio_collab::assets::Manifest::build` already hashes an imported file
//! and keeps `Manifest.asset` as "the whole asset's hash, so a peer can tell
//! two takes apart at a glance". `library::Roots` and `library::search` can
//! already walk the sample folders. But a `Source` still refers to its media
//! by a raw `path: String`, so:
//!
//! * dropping the same file twice imports it twice, under two ids, with two
//!   copies of the audio in memory and two entries in the chunk store;
//! * moving a project or a sample folder leaves every clip pointing at a path
//!   that no longer exists, and the failure surfaces as a decode error at
//!   playback rather than as "this file moved";
//! * a *renamed* file is indistinguishable from a deleted one.
//!
//! This is the map between the two: content identity on one side, paths on
//! the other.
//!
//! ## Size and name first, hash only to confirm
//!
//! Relinking by hashing every candidate is correct and unusable — a project
//! with 200 clips relinking against a sample drive would hash gigabytes to
//! find files it could have ruled out on `len()`. So [`relink_candidates`]
//! filters by **byte length first, then name stem**, and the caller hashes
//! only what survives. Length is free from the directory entry, discriminates
//! almost perfectly between unrelated media, and cannot produce a false
//! negative: a file whose bytes are identical has identical length, so
//! nothing correct is ever filtered out.
//!
//! Name is a *ranking* signal, not a filter, because renaming is exactly the
//! case this has to survive.
//!
//! ## No filesystem in here
//!
//! The caller supplies [`Candidate`]s. That keeps the whole policy testable
//! without a temp directory, and it is the same split `library::scan_dir`
//! already uses — one function that walks, one that decides.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Content identity: `studio_collab::assets::Manifest.asset`.
///
/// A newtype rather than a bare integer so a path and a hash cannot be swapped
/// at a call site, which is the mistake this module exists to make impossible.
///
/// THE WIDTH IS THE APP'S, not a choice made here. I first wrote this as
/// `[u8; 32]` on the assumption that a content hash is a SHA-256, and wrote
/// "Manifest.asset in this app" directly above it without checking: that field
/// is a `ChunkId`, which is a `u64` from a 64-bit FNV-1a walk
/// (`assets::hash`). The doc named the right source and the type could not
/// hold it, so the module could never actually have been wired to the thing it
/// says it maps.
///
/// 64 bits of FNV is weak for adversarial collisions and fine for this: the
/// question is "have I already imported this file", the corpus is one
/// musician's sample folder, and a wrong answer costs a duplicate import
/// rather than anything unrecoverable. Widening it would mean changing the
/// wire format, which is a different and much larger decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetKey(pub u64);

impl AssetKey {
    /// Short form for logs and for the clip inspector.
    pub fn short(&self) -> String {
        format!("{:08x}", self.0 >> 32)
    }
}

/// What the map knows about one asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Where it was last seen.
    pub path: PathBuf,
    /// Byte length — the cheap relink filter.
    pub len: u64,
    /// Whether the last check found it there.
    pub present: bool,
}

/// Where an asset's media actually is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// At the recorded path.
    Here(PathBuf),
    /// Found somewhere else; the map has been updated.
    Relinked { from: PathBuf, to: PathBuf },
    /// Known asset, no file. Distinct from `Unknown` — the project remembers
    /// importing it, so the UI can say "missing" and offer to locate it,
    /// rather than behaving as though the clip was never there.
    Missing(PathBuf),
    /// Never imported.
    Unknown,
}

/// A file the caller found while looking for a missing asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub path: PathBuf,
    pub len: u64,
}

/// Content identity to filesystem location.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MediaMap {
    by_key: BTreeMap<AssetKey, Entry>,
}

impl MediaMap {
    pub fn new() -> MediaMap {
        MediaMap::default()
    }

    pub fn len(&self) -> usize {
        self.by_key.len()
    }
    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    /// Record an import.
    ///
    /// Re-recording a known key UPDATES the path rather than adding a second
    /// entry: importing the same audio from two folders is one asset that
    /// exists in two places, not two assets. Which of the two the map keeps is
    /// "the one seen most recently", because that is the one the user just
    /// pointed at.
    pub fn record(&mut self, key: AssetKey, path: impl Into<PathBuf>, len: u64) {
        self.by_key.insert(key, Entry { path: path.into(), len, present: true });
    }

    pub fn get(&self, key: &AssetKey) -> Option<&Entry> {
        self.by_key.get(key)
    }

    /// Is this content already in the project?
    ///
    /// The question a drop handler asks before importing. Answering it by
    /// PATH would miss the same file dropped from a different folder, and
    /// answering it by name would merge two different takes called
    /// `vocal.wav`.
    pub fn known(&self, key: &AssetKey) -> bool {
        self.by_key.contains_key(key)
    }

    /// Resolve an asset, given a way to test whether a path exists.
    ///
    /// The existence check is a closure so this stays testable; in the app it
    /// is `|p| p.exists()`.
    pub fn resolve<F: Fn(&Path) -> bool>(&mut self, key: &AssetKey, exists: F) -> Resolved {
        let Some(e) = self.by_key.get_mut(key) else {
            return Resolved::Unknown;
        };
        if exists(&e.path) {
            e.present = true;
            Resolved::Here(e.path.clone())
        } else {
            e.present = false;
            Resolved::Missing(e.path.clone())
        }
    }

    /// Point a known asset at a new file after a successful relink.
    pub fn relink(&mut self, key: &AssetKey, to: impl Into<PathBuf>) -> Option<Resolved> {
        let e = self.by_key.get_mut(key)?;
        let from = std::mem::replace(&mut e.path, to.into());
        e.present = true;
        Some(Resolved::Relinked { from, to: e.path.clone() })
    }

    /// Every asset whose file was missing at the last resolve.
    ///
    /// What a "relink missing media" dialog lists. Sorted by key so the
    /// dialog's order is stable between runs — a list that reshuffles is one
    /// you cannot work down.
    pub fn missing(&self) -> Vec<(AssetKey, &Entry)> {
        self.by_key.iter().filter(|(_, e)| !e.present).map(|(k, e)| (*k, e)).collect()
    }
}

/// Rank candidates for a missing asset, best first.
///
/// **Length is a hard filter.** Identical bytes have identical length, so
/// nothing correct is excluded, and it removes almost everything for free.
/// Name is a ranking signal only — a renamed file must still be findable,
/// which is half the point.
///
/// The caller hashes the survivors in order and stops at the first match.
pub fn relink_candidates(want: &Entry, candidates: &[Candidate]) -> Vec<PathBuf> {
    let want_stem = stem_of(&want.path);
    let mut scored: Vec<(u8, PathBuf)> = candidates
        .iter()
        .filter(|c| c.len == want.len)
        .map(|c| {
            let stem = stem_of(&c.path);
            // 0 sorts first.
            let rank = if stem == want_stem {
                0
            } else if !want_stem.is_empty()
                && (stem.starts_with(&want_stem) || want_stem.starts_with(&stem))
            {
                // "vocal" vs "vocal-2", the shape a copy or a bounce takes.
                1
            } else {
                2
            };
            (rank, c.path.clone())
        })
        .collect();
    // Stable within a rank so the caller's own ordering (nearest root first)
    // survives — otherwise a relink could prefer a backup drive over the
    // project folder for no reason the user could see.
    scored.sort_by_key(|(r, _)| *r);
    scored.into_iter().map(|(_, p)| p).collect()
}

fn stem_of(p: &Path) -> String {
    p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u8) -> AssetKey {
        AssetKey(n as u64)
    }

    /// THE DUPLICATE IMPORT. Dropping the same file from two folders is one
    /// asset in two places, not two assets — answering by path would miss it.
    #[test]
    fn the_same_content_from_a_different_folder_is_already_known() {
        let mut m = MediaMap::new();
        m.record(key(1), "/a/vocal.wav", 4096);
        assert!(m.known(&key(1)));
        m.record(key(1), "/b/vocal.wav", 4096);
        assert_eq!(m.len(), 1, "one asset, not two");
        assert_eq!(m.get(&key(1)).unwrap().path, PathBuf::from("/b/vocal.wav"),
            "and it points at the one just seen");
    }

    /// Two different takes called vocal.wav must stay two assets — which is
    /// what keying on content rather than name buys.
    #[test]
    fn two_takes_with_the_same_name_stay_separate() {
        let mut m = MediaMap::new();
        m.record(key(1), "/take1/vocal.wav", 4096);
        m.record(key(2), "/take2/vocal.wav", 5000);
        assert_eq!(m.len(), 2);
    }

    /// Missing is not Unknown. The project remembers importing it, so the UI
    /// can offer to locate it rather than acting as if the clip never existed.
    #[test]
    fn a_missing_file_is_distinguished_from_one_never_imported() {
        let mut m = MediaMap::new();
        m.record(key(1), "/gone/vocal.wav", 4096);
        assert_eq!(m.resolve(&key(1), |_| false), Resolved::Missing("/gone/vocal.wav".into()));
        assert_eq!(m.resolve(&key(9), |_| true), Resolved::Unknown);
    }

    #[test]
    fn a_present_file_resolves_where_it_was_recorded() {
        let mut m = MediaMap::new();
        m.record(key(1), "/here/vocal.wav", 4096);
        assert_eq!(m.resolve(&key(1), |_| true), Resolved::Here("/here/vocal.wav".into()));
        assert!(m.missing().is_empty());
    }

    /// A resolve that fails then succeeds must clear the missing flag, or the
    /// relink dialog keeps listing a file that came back.
    #[test]
    fn finding_a_file_again_clears_its_missing_state() {
        let mut m = MediaMap::new();
        m.record(key(1), "/v.wav", 10);
        m.resolve(&key(1), |_| false);
        assert_eq!(m.missing().len(), 1);
        m.resolve(&key(1), |_| true);
        assert!(m.missing().is_empty());
    }

    #[test]
    fn relinking_moves_the_asset_and_reports_both_ends() {
        let mut m = MediaMap::new();
        m.record(key(1), "/old/v.wav", 10);
        m.resolve(&key(1), |_| false);
        let r = m.relink(&key(1), "/new/v.wav").unwrap();
        assert_eq!(r, Resolved::Relinked { from: "/old/v.wav".into(), to: "/new/v.wav".into() });
        assert!(m.missing().is_empty(), "and it is no longer missing");
        assert_eq!(m.relink(&key(9), "/x"), None, "an unknown key relinks nothing");
    }

    /// LENGTH IS A HARD FILTER, and it cannot produce a false negative:
    /// identical bytes have identical length.
    #[test]
    fn candidates_of_the_wrong_length_are_excluded_outright() {
        let want = Entry { path: "/old/vocal.wav".into(), len: 4096, present: false };
        let got = relink_candidates(&want, &[
            Candidate { path: "/new/vocal.wav".into(), len: 9999 },
            Candidate { path: "/new/other.wav".into(), len: 4096 },
        ]);
        assert_eq!(got, vec![PathBuf::from("/new/other.wav")],
            "the same name at the wrong size is the wrong file");
    }

    /// Name RANKS, it does not filter — a renamed file must still be found,
    /// which is half of what relinking is for.
    #[test]
    fn a_renamed_file_is_still_a_candidate_just_a_later_one() {
        let want = Entry { path: "/old/vocal.wav".into(), len: 4096, present: false };
        let got = relink_candidates(&want, &[
            Candidate { path: "/new/untitled-3.wav".into(), len: 4096 },
            Candidate { path: "/new/vocal.wav".into(), len: 4096 },
        ]);
        assert_eq!(got[0], PathBuf::from("/new/vocal.wav"), "exact name first");
        assert_eq!(got.len(), 2, "but the renamed one is still offered");
    }

    /// "vocal" vs "vocal-2" is what a copy or a bounce looks like, and should
    /// beat an unrelated name of the same size.
    #[test]
    fn a_near_name_outranks_an_unrelated_one() {
        let want = Entry { path: "/old/vocal.wav".into(), len: 4096, present: false };
        let got = relink_candidates(&want, &[
            Candidate { path: "/n/zzz.wav".into(), len: 4096 },
            Candidate { path: "/n/vocal-2.wav".into(), len: 4096 },
        ]);
        assert_eq!(got[0], PathBuf::from("/n/vocal-2.wav"));
    }

    /// Case must not decide it: the same file on a case-insensitive volume
    /// comes back as VOCAL.WAV and is still the exact-name match.
    #[test]
    fn name_matching_ignores_case() {
        let want = Entry { path: "/old/Vocal.wav".into(), len: 10, present: false };
        let got = relink_candidates(&want, &[
            Candidate { path: "/n/other.wav".into(), len: 10 },
            Candidate { path: "/n/VOCAL.WAV".into(), len: 10 },
        ]);
        assert_eq!(got[0], PathBuf::from("/n/VOCAL.WAV"));
    }

    /// Within a rank the caller's order survives, so a relink cannot prefer a
    /// backup drive over the project folder for no visible reason.
    #[test]
    fn equal_candidates_keep_the_callers_order() {
        let want = Entry { path: "/old/v.wav".into(), len: 10, present: false };
        let got = relink_candidates(&want, &[
            Candidate { path: "/project/a.wav".into(), len: 10 },
            Candidate { path: "/backup/b.wav".into(), len: 10 },
        ]);
        assert_eq!(got, vec![PathBuf::from("/project/a.wav"), PathBuf::from("/backup/b.wav")]);
    }

    /// The relink dialog's order must be stable between runs.
    #[test]
    fn the_missing_list_is_in_a_stable_order() {
        let mut m = MediaMap::new();
        for n in [3u8, 1, 2] {
            m.record(key(n), format!("/{n}.wav"), 10);
            m.resolve(&key(n), |_| false);
        }
        let got: Vec<AssetKey> = m.missing().into_iter().map(|(k, _)| k).collect();
        assert_eq!(got, vec![key(1), key(2), key(3)]);
    }

    #[test]
    fn a_short_key_is_readable_in_a_log() {
        // The TOP 32 bits: FNV-1a's low bits move most per input byte, so the
        // high half is the more distinctive half to show in a log line.
        assert_eq!(AssetKey(0xdead_beef_0000_0000).short(), "deadbeef");
    }

    /// The key is whatever `assets::hash` produced, so the same bytes give the
    /// same key and different bytes almost never do. Pinned because the whole
    /// dedupe rests on it.
    #[test]
    fn identical_bytes_give_an_identical_key() {
        let a = AssetKey(studio_collab::assets::hash(b"the same take"));
        let b = AssetKey(studio_collab::assets::hash(b"the same take"));
        let c = AssetKey(studio_collab::assets::hash(b"a different take"));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    /// Nothing to search is not a crash.
    #[test]
    fn no_candidates_yields_nothing() {
        let want = Entry { path: "/old/v.wav".into(), len: 10, present: false };
        assert!(relink_candidates(&want, &[]).is_empty());
    }
}
