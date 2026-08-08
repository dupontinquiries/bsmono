//! sync — what media each peer has, and what is still coming.
//!
//! `studio_collab::assets` had no consumer anywhere. Its header states why it
//! exists: *"a session where everyone can see the same timeline but only one
//! person has the takes is not collaboration, it is screen-sharing with extra
//! steps."* `Manifest::progress` and `Manifest::need` are written for a UI, and
//! there was none — the same shape as the compressor's `reduction_db` being
//! documented "for the meter" with no meter.
//!
//! ## Progress is per ASSET, never a single bar
//!
//! One combined percentage across a session is the number that makes people
//! wait for the wrong thing. Ninety percent overall can mean every clip is
//! nearly there, or that one four-gigabyte take has not started — and those call
//! for different decisions: wait, or start editing with what has landed.
//!
//! ## "Have it" and "can edit it" are the same question, deliberately
//!
//! An asset that is 99% transferred cannot be scrubbed, cut, or rendered. So
//! `Readiness` has no "nearly ready" state that a caller might treat as usable;
//! it is `Complete` or it is not, and the percentage is decoration on the
//! waiting. Anything else invites a timeline that plays silence where a clip
//! should be.
//!
//! ## Content addressing means most of it is already here
//!
//! Chunks are addressed by content, so two editors working from the same shoot
//! share rushes for free, a re-render changes a few seconds of a ten-minute
//! file, and someone rejoining has yesterday's work. The view should say that —
//! "3 of 210 chunks needed" is reassuring in a way "1.4 GB remaining" is not,
//! and it is the honest number.

use std::collections::BTreeSet;
use studio_collab::assets::{ChunkId, Manifest};

/// Whether an asset can be used yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// Every chunk is present: scrub, cut and render are all safe.
    Complete,
    /// Transfer under way.
    Partial,
    /// Nothing local at all.
    Missing,
}

/// One asset's transfer state.
#[derive(Debug, Clone, PartialEq)]
pub struct AssetStatus {
    pub name: String,
    pub total_chunks: usize,
    pub needed_chunks: usize,
    /// 0..=1.
    pub progress: f32,
    pub readiness: Readiness,
}

impl AssetStatus {
    /// Build from a manifest and what is on disk.
    pub fn of(name: &str, manifest: &Manifest, have: &BTreeSet<ChunkId>) -> AssetStatus {
        let needed = manifest.need(have);
        let total = manifest.chunks.len();
        let readiness = if needed.is_empty() && total > 0 {
            Readiness::Complete
        } else if needed.len() >= total {
            Readiness::Missing
        } else {
            Readiness::Partial
        };
        AssetStatus {
            name: name.to_string(),
            total_chunks: total,
            needed_chunks: needed.len(),
            progress: manifest.progress(have),
            readiness,
        }
    }

    /// Can this asset be edited?
    ///
    /// Only when complete. An asset at 99% cannot be scrubbed, cut or rendered,
    /// so there is deliberately no "nearly" a caller could mistake for usable —
    /// that is how a timeline ends up playing silence where a clip should be.
    pub fn is_usable(&self) -> bool { self.readiness == Readiness::Complete }

    /// What the row says.
    ///
    /// Chunks rather than bytes: content addressing means most of a file is
    /// usually already here, and "3 of 210 chunks needed" is both reassuring and
    /// honest in a way "1.4 GB remaining" is not.
    pub fn label(&self) -> String {
        match self.readiness {
            Readiness::Complete => "ready".into(),
            Readiness::Missing if self.total_chunks == 0 => "empty".into(),
            _ => format!("{} of {} chunks needed", self.needed_chunks, self.total_chunks),
        }
    }
}

/// The session's media state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SyncView {
    assets: Vec<AssetStatus>,
}

impl SyncView {
    pub fn set(&mut self, assets: Vec<AssetStatus>) { self.assets = assets; }
    pub fn assets(&self) -> &[AssetStatus] { &self.assets }
    pub fn is_empty(&self) -> bool { self.assets.is_empty() }

    /// Assets that cannot be edited yet, worst first.
    ///
    /// Worst first because this list is read to answer "what am I waiting for",
    /// and the answer is the thing furthest away — putting the nearly-done ones
    /// on top buries it.
    pub fn blocking(&self) -> Vec<&AssetStatus> {
        let mut v: Vec<&AssetStatus> = self.assets.iter()
            .filter(|a| !a.is_usable()).collect();
        v.sort_by(|a, b| a.progress.partial_cmp(&b.progress)
                  .unwrap_or(std::cmp::Ordering::Equal)
                  .then_with(|| a.name.cmp(&b.name)));
        v
    }

    /// Can the session be edited at all?
    ///
    /// True when every asset has landed. Not a percentage: the question a
    /// person actually asks is "can I start", which is yes or no.
    pub fn all_ready(&self) -> bool { self.assets.iter().all(|a| a.is_usable()) }

    /// How many assets are usable, for a "12 of 14 takes" line.
    pub fn ready_count(&self) -> usize {
        self.assets.iter().filter(|a| a.is_usable()).count()
    }
}

/// Draw the media panel.
pub fn sync_ui(ui: &mut egui::Ui, v: &SyncView, c: &uikit_egui::PaletteColors) {
    if v.is_empty() {
        ui.colored_label(c.text_weak, "No media in this session.");
        return;
    }
    ui.horizontal(|ui| {
        ui.colored_label(c.text_weak,
            format!("{} of {} takes ready", v.ready_count(), v.assets().len()));
        if v.all_ready() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.colored_label(c.success, "all media here");
            });
        }
    });
    ui.separator();

    for a in v.assets() {
        ui.horizontal(|ui| {
            // Colour carries readiness; the bar alone cannot distinguish
            // "almost there" from "done", and only one of those is editable.
            let col = match a.readiness {
                Readiness::Complete => c.success,
                Readiness::Partial => c.warning,
                Readiness::Missing => c.text_weak,
            };
            ui.colored_label(col, &a.name);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.colored_label(c.text_weak, egui::RichText::new(a.label()).small());
                if !a.is_usable() {
                    ui.add(egui::ProgressBar::new(a.progress).desired_width(90.0));
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_collab::assets;

    fn manifest_of(data: &[u8]) -> (Manifest, Vec<ChunkId>) {
        let (m, store) = Manifest::build(data);
        (m, store.keys().copied().collect())
    }

    /// Pseudo-random rather than periodic, and the first version of this was
    /// the latter — which made content-defined chunking find almost no
    /// boundaries, so a 400KB file became two chunks and a "99% transferred"
    /// test could only ever reach 50%. The fixture has to exercise the real
    /// chunker, or the numbers the view reports are tested against a shape no
    /// media file has.
    fn payload(n: usize, seed: u8) -> Vec<u8> {
        let mut x = 0x2545_F491_u32 ^ (seed as u32).wrapping_mul(0x9E37_79B9);
        (0..n).map(|_| {
            x ^= x << 13; x ^= x >> 17; x ^= x << 5;
            (x >> 24) as u8
        }).collect()
    }

    #[test]
    fn a_fully_transferred_asset_is_complete_and_usable() {
        let (m, ids) = manifest_of(&payload(300_000, 1));
        let have: BTreeSet<ChunkId> = ids.into_iter().collect();
        let s = AssetStatus::of("take1.mov", &m, &have);
        assert_eq!(s.readiness, Readiness::Complete);
        assert!(s.is_usable());
        assert_eq!(s.label(), "ready");
    }

    #[test]
    fn an_asset_at_ninety_nine_percent_is_not_usable() {
        // It cannot be scrubbed, cut or rendered. There is deliberately no
        // "nearly" state a caller could treat as usable — that is how a
        // timeline ends up playing silence where a clip should be.
        let (m, ids) = manifest_of(&payload(400_000, 2));
        let mut have: BTreeSet<ChunkId> = ids.iter().copied().collect();
        let dropped = *ids.last().expect("chunks exist");
        have.remove(&dropped);
        let s = AssetStatus::of("take2.mov", &m, &have);
        assert_eq!(s.readiness, Readiness::Partial);
        assert!(!s.is_usable());
        assert!(m.chunks.len() > 3, "the fixture must produce real chunking: {}",
                m.chunks.len());
        assert!(s.progress > 0.5, "it really is nearly there: {}", s.progress);
    }

    #[test]
    fn an_asset_with_nothing_local_reads_as_missing_not_partial() {
        // "Started and stalled" and "not started" are different things to a
        // user deciding whether to wait.
        let (m, _) = manifest_of(&payload(200_000, 3));
        let s = AssetStatus::of("take3.mov", &m, &BTreeSet::new());
        assert_eq!(s.readiness, Readiness::Missing);
        assert!(!s.is_usable());
    }

    #[test]
    fn the_label_counts_chunks_rather_than_bytes() {
        // Content addressing means most of a file is usually already here, and
        // "3 of 210 chunks needed" is reassuring in a way "1.4 GB remaining" is
        // not — and it is the honest number.
        let (m, ids) = manifest_of(&payload(400_000, 4));
        let mut have: BTreeSet<ChunkId> = ids.iter().copied().collect();
        have.remove(&ids[0]);
        let s = AssetStatus::of("t.mov", &m, &have);
        assert!(s.label().contains("chunks needed"), "{}", s.label());
        assert!(s.label().starts_with("1 of "));
    }

    #[test]
    fn shared_content_costs_nothing_to_discover() {
        // Two editors from the same shoot have the same rushes. The identical
        // payload produces identical chunk ids, so the second asset needs
        // nothing.
        let data = payload(300_000, 5);
        let (m1, ids) = manifest_of(&data);
        let (m2, _) = manifest_of(&data);
        let have: BTreeSet<ChunkId> = ids.into_iter().collect();
        assert!(AssetStatus::of("a", &m1, &have).is_usable());
        assert!(AssetStatus::of("b", &m2, &have).is_usable(),
                "the same content is already satisfied");
    }

    #[test]
    fn a_re_render_only_needs_what_changed() {
        // The design's own claim: a re-render of a bounce changes a few seconds
        // of a ten-minute file.
        let original = payload(500_000, 6);
        let mut edited = original.clone();
        for b in edited.iter_mut().take(2_000) { *b = b.wrapping_add(7); }
        let (m1, ids) = manifest_of(&original);
        let (m2, _) = manifest_of(&edited);
        let have: BTreeSet<ChunkId> = ids.into_iter().collect();
        let s = AssetStatus::of("bounce", &m2, &have);
        assert!(s.needed_chunks < m1.chunks.len(),
                "needed {} of {}", s.needed_chunks, m1.chunks.len());
    }

    #[test]
    fn blocking_assets_are_listed_worst_first() {
        // This list answers "what am I waiting for", and the answer is the
        // thing furthest away — putting the nearly-done ones on top buries it.
        let mut v = SyncView::default();
        v.set(vec![
            AssetStatus { name: "nearly".into(), total_chunks: 10, needed_chunks: 1,
                          progress: 0.9, readiness: Readiness::Partial },
            AssetStatus { name: "barely".into(), total_chunks: 10, needed_chunks: 9,
                          progress: 0.1, readiness: Readiness::Partial },
            AssetStatus { name: "done".into(), total_chunks: 10, needed_chunks: 0,
                          progress: 1.0, readiness: Readiness::Complete },
        ]);
        let b = v.blocking();
        assert_eq!(b.len(), 2, "the complete one is not blocking");
        assert_eq!(b[0].name, "barely");
    }

    #[test]
    fn readiness_is_a_count_not_one_combined_percentage() {
        // Ninety percent overall can mean every clip is nearly there, or that
        // one four-gigabyte take has not started, and those call for different
        // decisions.
        let mut v = SyncView::default();
        v.set(vec![
            AssetStatus { name: "a".into(), total_chunks: 10, needed_chunks: 0,
                          progress: 1.0, readiness: Readiness::Complete },
            AssetStatus { name: "b".into(), total_chunks: 10, needed_chunks: 10,
                          progress: 0.0, readiness: Readiness::Missing },
        ]);
        assert_eq!(v.ready_count(), 1);
        assert!(!v.all_ready());
    }

    #[test]
    fn an_empty_session_is_ready_rather_than_blocked() {
        // Nothing to wait for is not the same as waiting, and a session with no
        // media should not present as stuck.
        let v = SyncView::default();
        assert!(v.all_ready());
        assert!(v.blocking().is_empty());
        assert!(v.is_empty());
    }

    #[test]
    fn an_empty_asset_does_not_read_as_complete() {
        // A manifest with no chunks is a failed import, not a finished
        // transfer, and calling it ready would put a clip on the timeline that
        // plays nothing.
        let (m, _) = manifest_of(&[]);
        let s = AssetStatus::of("broken.mov", &m, &BTreeSet::new());
        assert!(!s.is_usable());
        assert_eq!(s.label(), "empty");
    }

    #[test]
    fn the_blocking_order_is_stable_when_progress_ties() {
        // A list that reshuffles between frames is unreadable.
        let mut v = SyncView::default();
        v.set(vec![
            AssetStatus { name: "zebra".into(), total_chunks: 4, needed_chunks: 2,
                          progress: 0.5, readiness: Readiness::Partial },
            AssetStatus { name: "alpha".into(), total_chunks: 4, needed_chunks: 2,
                          progress: 0.5, readiness: Readiness::Partial },
        ]);
        assert_eq!(v.blocking()[0].name, "alpha");
        assert_eq!(v.blocking()[0].name, "alpha");
    }

    #[test]
    fn chunking_respects_its_own_bounds() {
        // The view reports chunk counts, so those counts have to mean
        // something — a splitter emitting one-byte chunks would make "3 of 4000
        // needed" meaningless.
        let data = payload(1_000_000, 8);
        for (start, end) in assets::split(&data) {
            let len = end - start;
            assert!(len <= assets::MAX_CHUNK, "chunk of {len}");
        }
    }
}
