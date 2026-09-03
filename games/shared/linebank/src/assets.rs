//! ASSETS — what the bank costs to vendor, and in what shape.
//!
//! WAV is a studio format, not a shipping one. 48 kHz 16-bit mono is 768 kbps,
//! so an hour of recorded VO is ~330 MB in the repository, and a bank the size
//! this game wants is several hours across several actors. Opus at speech
//! bitrates is 20–40x smaller and this tree already vendors it.
//!
//! ## Measured, from this repository
//!
//! `apps/bo-suite/docs/scenes/clips_spatial` holds 84 Ogg-Opus files, 8.52 MB
//! for 1037 seconds — mean 65.8 kbps, but that average is stereo ambience.
//! The mono clips in the same folder sit at **24–25 kbps**, which is the
//! relevant figure for a voice bank: `bird_chirp_1.opus` is 4.18 s of mono in
//! 13081 bytes. Every file in the set declares `pre_skip = 312`.
//!
//! Those are the numbers [`Budget`] uses. They are measurements from the
//! existing assets rather than published guidance, so they already include
//! whatever encoder settings this project actually uses.
//!
//! ## Encode LAST, not first
//!
//! The pipeline is: record lossless, slice, verify, THEN encode. Encoding the
//! session first and cutting the Opus means decode–recut–reencode, which is
//! generation loss on every take, and Opus frames are 2.5–60 ms (20 ms by
//! default) so the cut points quantise to frame boundaries anyway. The masters
//! do not get vendored at all — only the encoded takes do, which is what makes
//! the repository size argument work without the studio losing anything.
//!
//! ## The pre-skip trap, which matters more here than anywhere else
//!
//! Every Opus stream carries encoder delay declared in its header —
//! `pre_skip`, 312 samples at 48 kHz (6.5 ms) in every file measured above —
//! and the decoder is required to discard it. A player that ignores pre-skip
//! puts 6.5 ms of garbage at the head of each file.
//!
//! On a music track that is inaudible. On a bank of 300 ms blips fired
//! constantly it is a click on every single one, and it will be diagnosed as a
//! recording problem rather than a decoder one. [`GAPLESS_NOTE`] exists so
//! whoever writes the playback path reads this before hearing it.
//!
//! ## One file per take, and why not a pack
//!
//! Per-file container overhead is real: an OggOpus header page plus a comment
//! page is roughly 150–250 bytes before any audio. On a 0.3 s blip at 24 kbps
//! (~900 bytes of audio) that is a fifth of the file.
//!
//! A pack — raw Opus packets concatenated with an external index — removes
//! nearly all of it, and is the right answer eventually. It is not the right
//! answer first, because it needs a custom demuxer on the playback side, and
//! `shared/opus-kit` DOES NOT EXIST YET: `audiomix-kit` defers codec work to
//! it in a doc comment, and there is no such crate in the tree. Adding a
//! bespoke container before there is a decoder is building the second floor
//! first. [`Budget::packed`] reports what the pack would save so the decision
//! can be made on a number.

/// Measured from this repository's own mono Opus assets. Not a published
/// figure — `bird_chirp_1.opus` is 4.18 s of mono in 13081 bytes.
pub const MEASURED_MONO_KBPS: f32 = 25.0;

/// Bytes of Ogg container per file, before audio: an `OpusHead` page and an
/// `OpusTags` page. Conservative; comment blocks vary with tooling.
pub const CONTAINER_BYTES: usize = 200;

/// Encoder delay every Opus stream declares and every decoder must discard.
/// 312 samples at 48 kHz in every file measured in this tree.
pub const PRE_SKIP_SAMPLES: u32 = 312;

/// Left where a playback implementer will find it.
pub const GAPLESS_NOTE: &str = "\
Opus declares pre_skip (312 samples, 6.5 ms) in its header and the decoder \
MUST discard that many samples from the start of the stream. Skipping this \
step is inaudible on long clips and is a click on every short blip.";

/// What a take should be encoded at. Mono throughout — VO is positioned by the
/// mixer, not baked, and `audiomix-kit`'s RouteMatrix already does that.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Quality {
    /// Blips and chatter. At this length nobody is listening for detail.
    Chatter,
    /// Ordinary spoken lines.
    Speech,
    /// Lines that carry a scene. The prologue monologue is this.
    Hero,
}

impl Quality {
    pub fn kbps(self) -> f32 {
        match self {
            // below the measured 25 because blips are short and noisy-edged;
            // the artefacts that show at 16 kbps need a sustained vowel
            Quality::Chatter => 16.0,
            Quality::Speech => MEASURED_MONO_KBPS,
            Quality::Hero => 48.0,
        }
    }
    /// The quality a unit kind wants by default.
    pub fn for_kind(k: crate::Kind) -> Quality {
        match k {
            crate::Kind::Blip => Quality::Chatter,
            crate::Kind::Slot => Quality::Speech,
            crate::Kind::Phrase => Quality::Speech,
        }
    }
}

/// Estimated shipped size.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Budget {
    /// One Ogg-Opus file per take, container overhead included.
    pub per_file: usize,
    /// Raw packets in a pack with an external index — no per-take container.
    pub packed: usize,
    /// The same audio as 48 kHz 16-bit mono WAV, for the comparison that
    /// motivated the format choice.
    pub as_wav: usize,
    pub takes: usize,
    pub seconds: f32,
}

impl Budget {
    /// What the pack would save, as a fraction of the per-file size. Small
    /// numbers mean the custom container is not worth a bespoke demuxer yet.
    pub fn pack_saving(&self) -> f32 {
        if self.per_file == 0 {
            return 0.0;
        }
        1.0 - self.packed as f32 / self.per_file as f32
    }
}

/// Estimate a bank's shipped size.
///
/// `takes` is (seconds, quality) per recorded take — durations from
/// `Span::seconds`, so this can be run against a real session rather than
/// guessed at.
pub fn budget(takes: &[(f32, Quality)]) -> Budget {
    let mut audio = 0.0f64;
    let mut seconds = 0.0f32;
    for (s, q) in takes {
        let s = s.max(0.0);
        audio += (s * q.kbps() * 1000.0 / 8.0) as f64;
        seconds += s;
    }
    let packed = audio as usize;
    Budget {
        per_file: packed + takes.len() * CONTAINER_BYTES,
        packed,
        as_wav: (seconds as f64 * 48_000.0 * 2.0) as usize,
        takes: takes.len(),
        seconds,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Kind;

    /// A bank shaped like this game's: mostly short blips, some phrases.
    fn realistic() -> Vec<(f32, Quality)> {
        let mut v = Vec::new();
        for _ in 0..1200 {
            v.push((0.35, Quality::Chatter));
        }
        for _ in 0..400 {
            v.push((2.5, Quality::Speech));
        }
        for _ in 0..40 {
            v.push((9.0, Quality::Hero));
        }
        v
    }

    #[test]
    fn the_format_choice_is_worth_what_it_claims() {
        // The argument that started this: WAV cannot be vendored at bank scale.
        let b = budget(&realistic());
        let ratio = b.as_wav as f32 / b.per_file as f32;
        assert!(ratio > 15.0, "Opus is only {ratio:.1}x smaller than WAV; check the bitrates");
        assert!(b.per_file < 12_000_000, "{} bytes is too much to vendor", b.per_file);
    }

    #[test]
    fn container_overhead_bites_on_blips_and_not_on_lines() {
        // Why the pack question is about the BLIPS. A 0.35 s chatter take is
        // ~700 bytes of audio against 200 bytes of container; a 9 s hero line
        // is ~54 kB against the same 200.
        let blips = budget(&vec![(0.35, Quality::Chatter); 1000]);
        let heroes = budget(&vec![(9.0, Quality::Hero); 1000]);
        assert!(blips.pack_saving() > 0.15, "blip overhead {:.3}", blips.pack_saving());
        assert!(heroes.pack_saving() < 0.01, "hero overhead {:.3}", heroes.pack_saving());
    }

    #[test]
    fn the_pack_is_not_yet_worth_a_bespoke_demuxer() {
        // The decision this exists to inform, on a number rather than a
        // feeling. If a future bank shifts hard toward blips this flips, and
        // the test says so by failing.
        let b = budget(&realistic());
        assert!(
            b.pack_saving() < 0.10,
            "a pack would now save {:.1}% — worth revisiting opus-kit's container",
            b.pack_saving() * 100.0
        );
    }

    #[test]
    fn quality_follows_the_kind_a_unit_already_declares() {
        assert_eq!(Quality::for_kind(Kind::Blip), Quality::Chatter);
        assert_eq!(Quality::for_kind(Kind::Phrase), Quality::Speech);
        assert!(Quality::Hero.kbps() > Quality::Speech.kbps());
        assert!(Quality::Chatter.kbps() < Quality::Speech.kbps());
    }

    #[test]
    fn the_speech_rate_is_the_one_measured_in_this_tree() {
        // Not a published guideline: bird_chirp_1.opus is 4.18 s of mono in
        // 13081 bytes, which is 25.0 kbps with this project's own encoder
        // settings. If someone retunes the encoder, this is the anchor.
        assert_eq!(Quality::Speech.kbps(), MEASURED_MONO_KBPS);
        let measured = 13081.0 * 8.0 / 4.18 / 1000.0;
        assert!((measured - MEASURED_MONO_KBPS).abs() < 1.0, "measured {measured:.1} kbps");
    }

    #[test]
    fn an_empty_bank_costs_nothing_and_does_not_divide_by_zero() {
        let b = budget(&[]);
        assert_eq!((b.per_file, b.packed, b.as_wav, b.takes), (0, 0, 0, 0));
        assert_eq!(b.pack_saving(), 0.0);
    }

    #[test]
    fn negative_durations_do_not_shrink_the_estimate() {
        // Span::seconds cannot go negative, but a hand-built manifest can.
        let b = budget(&[(1.0, Quality::Speech), (-5.0, Quality::Speech)]);
        assert!(b.packed > 0);
        assert_eq!(b.seconds, 1.0);
    }
}
