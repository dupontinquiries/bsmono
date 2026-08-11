//! block — Audacity's other idea worth taking, and the one this crate's own
//! module docs named and left unbuilt: a `WaveTrack` is not one giant sample
//! array, it is a `Sequence` of blocks, so an edit deep inside a ten-hour
//! recording rewrites the handful of blocks near the edit and shares every
//! other block, byte-identical, with the sequence that existed a moment
//! ago. [`Peaks`](crate::Peaks) already borrows this crate's other half of
//! the same insight (summarise once, redraw cheaply); this is the write
//! side — edit once, cheaply, instead of copying the whole buffer.
//!
//! ## Why this earns its own module instead of changing `SourceAudio`
//!
//! [`SourceAudio`](crate::SourceAudio) — a flat `Vec<Vec<f32>>` per source —
//! stays exactly as it is. `mixdown` and `Peaks` already consume it, are
//! already tested against it, and neither needs to know a source was ever
//! block-backed: reading a whole source into memory once per render is the
//! right cost model for *playback*. Block-backing only pays for itself on
//! the *edit* path — inserting a punch-in take into the middle of an
//! existing hour-long recording, or cutting a bad take out of one, without
//! copying the untouched hour on either side. [`BlockSequence`] is that
//! path; [`to_flat`]/[`from_flat`] are the bridge, so a block-backed edit
//! and a flat in-memory source are interchangeable at the boundary rather
//! than forking the representation everywhere downstream cares about audio.
//!
//! ## The one property that makes this worth building at all
//!
//! Blocks are variable-length (Audacity's own scheme, not a simplification
//! of it) and held behind `Arc<[f32]>`, so a block an edit doesn't touch is
//! not just byte-identical afterward, it is the *same allocation* —
//! `Arc::ptr_eq` on it before and after an edit is `true`. That is the
//! claim this module's tests actually check, the same way `assets.rs`
//! checks a chunk survives an edit by identity, not just by value: a
//! block-backed sequence that happens to produce the right bytes by
//! rebuilding everything from scratch every time would pass a value
//! comparison and still not be the thing Audacity's design is for.
//!
//! `insert` rebuilds at most one existing block (the one straddling the
//! insertion point, split into a left remainder, the new material chunked
//! at `block_size`, and a right remainder) plus however many new blocks the
//! inserted material itself needs — never anything already in the
//! sequence on either side. `delete` rebuilds at most the two blocks
//! straddling the deleted range's edges (one, if both edges fall in the
//! same block) and drops whatever blocks were entirely inside it; every
//! block entirely outside the deleted range is untouched.

use std::sync::Arc;

/// One channel's samples, stored as a sequence of variable-length blocks.
/// The public surface is small on purpose: build one, read it back flat,
/// `insert`/`delete`. Random mid-sequence sample access is intentionally not
/// provided here — `to_flat` is how a caller gets samples for playback or
/// peak-building, both of which already want the whole span at once.
#[derive(Debug, Clone)]
pub struct BlockSequence {
    /// Target size for a newly-created block. Existing blocks may be
    /// shorter (the last block of a sequence, or a remainder left by an
    /// edit) — never rebuilt back up to this size just to tidy them, since
    /// that would mean touching a block an edit didn't actually need to
    /// touch, undoing the whole point.
    block_size: usize,
    blocks: Vec<Arc<[f32]>>,
}

impl BlockSequence {
    /// Split `samples` into `block_size`-sized blocks (the last one shorter
    /// if `samples.len()` doesn't divide evenly). `block_size` of 0 is
    /// treated as 1 — a zero-sized block is nonsensical and would loop
    /// forever chunking, not a case worth an `Option`/panic over when
    /// clamping is unambiguous and harmless.
    pub fn from_slice(samples: &[f32], block_size: usize) -> BlockSequence {
        let block_size = block_size.max(1);
        let blocks = samples.chunks(block_size).map(Arc::<[f32]>::from).collect();
        BlockSequence { block_size, blocks }
    }

    pub fn len(&self) -> usize {
        self.blocks.iter().map(|b| b.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// Flatten back to one contiguous buffer — for playback, peak-building,
    /// or handing to code (like `mixdown`) that only knows flat audio.
    pub fn to_flat(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.len());
        for b in &self.blocks {
            out.extend_from_slice(b);
        }
        out
    }

    /// Byte-for-byte identity of every block, in order — what the "did this
    /// edit avoid touching block N" tests below actually compare. Two
    /// sequences with identical *samples* but rebuilt from scratch would
    /// still fail this, which is the point: value equality is not the
    /// property this module exists to provide, `Arc` identity is.
    #[cfg(test)]
    fn block_ptrs(&self) -> Vec<*const f32> {
        self.blocks.iter().map(|b| b.as_ptr()).collect()
    }

    /// Which block index (and offset within it) sample position `pos`
    /// falls in. Returns `(index, offset)` where `index == blocks.len()`
    /// and `offset == 0` means "past the end" (a valid, `push`-style
    /// insertion point at the sequence's own length).
    fn locate(&self, pos: usize) -> (usize, usize) {
        let mut remaining = pos;
        for (i, b) in self.blocks.iter().enumerate() {
            if remaining < b.len() {
                return (i, remaining);
            }
            remaining -= b.len();
        }
        (self.blocks.len(), 0)
    }

    /// Insert `samples` at position `pos` (clamped to `0..=len()`).
    /// Rebuilds only the one existing block `pos` falls inside (split
    /// around the insertion point) plus fresh blocks for `samples` itself;
    /// every other existing block keeps its `Arc` unchanged. Inserting at
    /// exactly a block boundary (including the very start or the very end
    /// of the sequence) needs no split at all — the new blocks are simply
    /// spliced in, and every existing block, including the ones now on
    /// either side of the insertion, is untouched.
    pub fn insert(&mut self, pos: usize, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        let pos = pos.min(self.len());
        let (idx, offset) = self.locate(pos);
        let new_blocks: Vec<Arc<[f32]>> = samples.chunks(self.block_size).map(Arc::<[f32]>::from).collect();

        if idx >= self.blocks.len() {
            // Past the end (including an empty sequence) — pure append, no
            // existing block to split.
            self.blocks.extend(new_blocks);
            return;
        }
        if offset == 0 {
            // Landed exactly on a block boundary — splice in, nothing to split.
            self.blocks.splice(idx..idx, new_blocks);
            return;
        }

        // Split the one straddling block into left/right remainders around
        // `offset`, and replace it with left + new_blocks + right. This is
        // the only existing block this call ever reads from or rebuilds.
        let straddling = &self.blocks[idx];
        let left: Arc<[f32]> = Arc::from(&straddling[..offset]);
        let right: Arc<[f32]> = Arc::from(&straddling[offset..]);
        let mut replacement = Vec::with_capacity(2 + new_blocks.len());
        replacement.push(left);
        replacement.extend(new_blocks);
        replacement.push(right);
        self.blocks.splice(idx..idx + 1, replacement);
    }

    /// Delete `[start, end)` (clamped to the sequence's own bounds; a
    /// backwards or empty range is a no-op). Blocks entirely inside the
    /// range are dropped outright; the (at most two — one, if `start` and
    /// `end` fall in the same block) blocks straddling either edge are
    /// rebuilt with the deleted portion excised; every block entirely
    /// outside `[start, end)` is untouched.
    pub fn delete(&mut self, start: usize, end: usize) {
        let len = self.len();
        let start = start.min(len);
        let end = end.min(len);
        if start >= end {
            return;
        }

        let (start_idx, start_off) = self.locate(start);
        // `end` is exclusive; locate the last sample actually being
        // removed (`end - 1`) so a range ending exactly on a block
        // boundary doesn't pull the following, untouched block into the
        // straddle handling below.
        let (end_idx, end_off_inclusive) = self.locate(end - 1);

        if start_idx == end_idx {
            // The whole deleted range sits inside one block — rebuild that
            // one block with the middle excised, touch nothing else.
            let b = &self.blocks[start_idx];
            let mut rebuilt = Vec::with_capacity(b.len() - (end_off_inclusive + 1 - start_off));
            rebuilt.extend_from_slice(&b[..start_off]);
            rebuilt.extend_from_slice(&b[end_off_inclusive + 1..]);
            if rebuilt.is_empty() {
                self.blocks.remove(start_idx);
            } else {
                self.blocks[start_idx] = Arc::from(rebuilt.as_slice());
            }
            return;
        }

        // Straddles two or more blocks: keep the untouched prefix of the
        // first straddling block and the untouched suffix of the last one,
        // drop everything strictly between them (fully-interior blocks),
        // and splice the (up to two) surviving remainders back in — every
        // block before `start_idx` or after `end_idx` never gets read.
        let mut replacement: Vec<Arc<[f32]>> = Vec::with_capacity(2);
        if start_off > 0 {
            replacement.push(Arc::from(&self.blocks[start_idx][..start_off]));
        }
        let end_block = &self.blocks[end_idx];
        if end_off_inclusive + 1 < end_block.len() {
            replacement.push(Arc::from(&end_block[end_off_inclusive + 1..]));
        }
        self.blocks.splice(start_idx..=end_idx, replacement);
    }
}

/// One channel's worth of block sequences to a flat plane, for handing to
/// [`crate::SourceAudio`]. Sample rate is not this module's concern — the
/// caller already has it, same as `SourceAudio` itself takes it separately
/// from `channels`.
pub fn to_flat_channels(seqs: &[BlockSequence]) -> Vec<Vec<f32>> {
    seqs.iter().map(BlockSequence::to_flat).collect()
}

/// The inverse: wrap each of a [`crate::SourceAudio`]'s planes as a
/// block-backed sequence, ready for editing.
pub fn from_flat_channels(channels: &[Vec<f32>], block_size: usize) -> Vec<BlockSequence> {
    channels.iter().map(|c| BlockSequence::from_slice(c, block_size)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(n: usize) -> Vec<f32> {
        (0..n).map(|i| i as f32).collect()
    }

    #[test]
    fn from_slice_round_trips_exactly() {
        let s = ramp(1000);
        let seq = BlockSequence::from_slice(&s, 64);
        assert_eq!(seq.len(), 1000);
        assert_eq!(seq.to_flat(), s);
        assert!(seq.block_count() > 1, "1000 samples at a block size of 64 must actually split");
    }

    #[test]
    fn a_block_size_that_divides_evenly_leaves_no_short_tail() {
        let s = ramp(256);
        let seq = BlockSequence::from_slice(&s, 64);
        assert_eq!(seq.block_count(), 4);
    }

    #[test]
    fn an_uneven_length_leaves_a_short_final_block_not_padding() {
        let s = ramp(100);
        let seq = BlockSequence::from_slice(&s, 64);
        assert_eq!(seq.block_count(), 2);
        assert_eq!(seq.to_flat(), s, "no padding samples leaked into the flattened output");
    }

    #[test]
    fn empty_input_is_an_empty_sequence_not_one_empty_block() {
        let seq = BlockSequence::from_slice(&[], 64);
        assert_eq!(seq.len(), 0);
        assert_eq!(seq.block_count(), 0);
        assert!(seq.is_empty());
    }

    #[test]
    fn insert_at_a_block_boundary_touches_no_existing_block() {
        let seq = BlockSequence::from_slice(&ramp(256), 64); // 4 blocks
        let before = seq.block_ptrs();
        let mut edited = seq.clone();
        edited.insert(128, &[9.0, 9.0, 9.0]); // exactly the boundary between block 1 and 2
        let after_existing = &edited.block_ptrs()[..2]; // blocks 0,1 (before the insertion point)
        assert_eq!(&before[..2], after_existing, "blocks entirely before a boundary insertion must be untouched");
        let after_tail = &edited.block_ptrs()[edited.block_ptrs().len() - 2..];
        assert_eq!(&before[2..], after_tail, "blocks entirely after a boundary insertion must also be untouched");
    }

    #[test]
    fn insert_mid_block_only_rebuilds_that_one_block() {
        let original = ramp(1000);
        let seq = BlockSequence::from_slice(&original, 100); // 10 blocks, edit lands in block 5
        let before = seq.block_ptrs();

        let mut edited = seq.clone();
        edited.insert(550, &[-1.0, -2.0, -3.0]);

        // Correctness first: the flattened result must match a naive Vec insert.
        let mut expected = original.clone();
        expected.splice(550..550, [-1.0, -2.0, -3.0]);
        assert_eq!(edited.to_flat(), expected);

        // Then the property this module exists for: blocks 0..5 and 6..10
        // (everything not straddling the insertion point) must be the SAME
        // allocation as before the edit, not just equal-valued.
        let after = edited.block_ptrs();
        // Block 5 (index 5, the one containing sample 550) is replaced by
        // [left_remainder, new_material, right_remainder] — 3 blocks — so
        // everything from index 8 onward in `after` corresponds to the old
        // blocks 6..10.
        assert_eq!(&before[..5], &after[..5], "blocks entirely before the edit must be the same allocation");
        assert_eq!(&before[6..], &after[8..], "blocks entirely after the edit must be the same allocation");
    }

    #[test]
    fn insert_at_the_very_start_touches_nothing_existing() {
        let seq = BlockSequence::from_slice(&ramp(300), 100);
        let before = seq.block_ptrs();
        let mut edited = seq.clone();
        edited.insert(0, &[7.0, 7.0]);
        assert_eq!(before, &edited.block_ptrs()[1..], "every original block must survive, shifted by one new block at the front");
        let mut expected = vec![7.0, 7.0];
        expected.extend(ramp(300));
        assert_eq!(edited.to_flat(), expected);
    }

    #[test]
    fn insert_at_the_very_end_is_a_pure_append() {
        let seq = BlockSequence::from_slice(&ramp(300), 100);
        let before = seq.block_ptrs();
        let mut edited = seq.clone();
        edited.insert(300, &[7.0, 7.0]);
        assert_eq!(&before[..], &edited.block_ptrs()[..3], "existing blocks must be untouched by an append");
        let mut expected = ramp(300);
        expected.extend([7.0, 7.0]);
        assert_eq!(edited.to_flat(), expected);
    }

    #[test]
    fn insert_past_the_end_clamps_rather_than_panics() {
        let mut seq = BlockSequence::from_slice(&ramp(10), 4);
        seq.insert(9999, &[1.0]);
        let mut expected = ramp(10);
        expected.push(1.0);
        assert_eq!(seq.to_flat(), expected);
    }

    #[test]
    fn insert_into_an_empty_sequence_just_creates_blocks() {
        let mut seq = BlockSequence::from_slice(&[], 64);
        seq.insert(0, &ramp(10));
        assert_eq!(seq.to_flat(), ramp(10));
    }

    #[test]
    fn delete_within_one_block_only_rebuilds_that_block() {
        let original = ramp(1000);
        let seq = BlockSequence::from_slice(&original, 100);
        let before = seq.block_ptrs();

        let mut edited = seq.clone();
        edited.delete(520, 540); // entirely inside block 5

        let mut expected = original.clone();
        expected.drain(520..540);
        assert_eq!(edited.to_flat(), expected);

        let after = edited.block_ptrs();
        assert_eq!(&before[..5], &after[..5], "blocks before the deletion must be untouched");
        assert_eq!(&before[6..], &after[6..], "blocks after the deletion must be untouched — only block 5 itself changed");
    }

    #[test]
    fn delete_spanning_several_whole_blocks_drops_them_and_touches_only_the_edges() {
        let original = ramp(1000);
        let seq = BlockSequence::from_slice(&original, 100); // blocks 0..10
        let before = seq.block_ptrs();

        let mut edited = seq.clone();
        edited.delete(150, 650); // straddles block 1 (edge) through block 6 (edge), blocks 2..6 fully interior

        let mut expected = original.clone();
        expected.drain(150..650);
        assert_eq!(edited.to_flat(), expected);

        let after = edited.block_ptrs();
        assert_eq!(before[0], after[0], "block 0, entirely before the deletion, must be untouched");
        // Blocks 1..=6 (6 originals) collapsed into 2 replacement blocks
        // (the left remainder of old block 1, the right remainder of old
        // block 6), so the untouched tail shifts down by 6 - 2 = 4: old
        // blocks 7,8,9 now live at new indices 3,4,5.
        assert_eq!(&before[7..], &after[3..], "blocks 7..10, entirely after the deletion, must be untouched");
        // Blocks 2..6 (fully inside the deleted range) must be gone
        // entirely, not merely absent from the pointer comparison above —
        // the surviving sequence should have far fewer blocks than it
        // started with, not the same count with holes patched.
        assert!(edited.block_count() < seq.block_count(), "fully-interior blocks must actually be dropped, not rebuilt as empty");
    }

    #[test]
    fn delete_exactly_a_whole_block_and_nothing_else_leaves_neighbours_untouched() {
        let original = ramp(400);
        let seq = BlockSequence::from_slice(&original, 100); // 4 blocks
        let before = seq.block_ptrs();
        let mut edited = seq.clone();
        edited.delete(100, 200); // exactly block 1, boundary-aligned both edges
        assert_eq!(edited.to_flat().len(), 300);
        let mut expected = original.clone();
        expected.drain(100..200);
        assert_eq!(edited.to_flat(), expected);
        let after = edited.block_ptrs();
        assert_eq!(before[0], after[0]);
        assert_eq!(&before[2..], &after[1..], "the two blocks after the deleted one must be untouched, just shifted down an index");
    }

    #[test]
    fn delete_backwards_or_empty_range_is_a_no_op() {
        let mut seq = BlockSequence::from_slice(&ramp(100), 32);
        let before = seq.to_flat();
        seq.delete(50, 50);
        assert_eq!(seq.to_flat(), before);
        seq.delete(80, 20);
        assert_eq!(seq.to_flat(), before);
    }

    #[test]
    fn delete_past_the_end_clamps_rather_than_panics() {
        let mut seq = BlockSequence::from_slice(&ramp(10), 4);
        seq.delete(5, 9999);
        assert_eq!(seq.to_flat(), &ramp(10)[..5]);
    }

    #[test]
    fn deleting_everything_leaves_an_empty_sequence() {
        let mut seq = BlockSequence::from_slice(&ramp(300), 64);
        seq.delete(0, 300);
        assert!(seq.is_empty());
        assert_eq!(seq.to_flat(), Vec::<f32>::new());
    }

    #[test]
    fn a_long_edit_session_only_ever_touches_a_bounded_number_of_blocks_per_edit() {
        // The end-to-end version of the property every test above checks
        // locally: on a sequence with many blocks, one edit deep in the
        // middle must leave the overwhelming majority of blocks — not just
        // "some" — as the same allocation.
        let n_blocks_before = 200;
        let block_size = 500;
        let original = ramp(n_blocks_before * block_size);
        let seq = BlockSequence::from_slice(&original, block_size);
        assert_eq!(seq.block_count(), n_blocks_before);
        let before = seq.block_ptrs();

        let mut edited = seq.clone();
        edited.insert(100 * block_size + 17, &ramp(30)); // deep in block 100, small insert

        let after = edited.block_ptrs();
        let mut untouched = 0usize;
        for p in &before {
            if after.contains(p) {
                untouched += 1;
            }
        }
        assert!(
            untouched >= n_blocks_before - 2,
            "at most the one straddling block should have been rebuilt; {} of {} blocks survived",
            untouched, n_blocks_before
        );
    }

    #[test]
    fn to_flat_channels_and_from_flat_channels_round_trip_through_source_audio_shape() {
        let ch0 = ramp(500);
        let ch1: Vec<f32> = ramp(500).into_iter().map(|s| -s).collect();
        let seqs = from_flat_channels(&[ch0.clone(), ch1.clone()], 64);
        assert_eq!(seqs.len(), 2);
        let flat = to_flat_channels(&seqs);
        assert_eq!(flat, vec![ch0, ch1]);
    }
}
