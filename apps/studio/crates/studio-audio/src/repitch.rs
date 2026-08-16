//! repitch — the "change song key" piece [`studio_copilot::change_song_key`]
//! deliberately left out: repitching EXISTING recorded audio, as opposed to
//! transposing a synth channel's note.
//!
//! This is the real, separate work that module's doc comment named:
//! `studio_fx::autotune::AutotuneChannel::process_buffer` needs a whole
//! buffer up front (epoch marking has to run unbroken across it — see
//! `psola`'s own docs), so it cannot be a per-sample `Effect` in
//! `Bus.chain`/`Clip.effects`. It has to be an explicit **offline bounce**
//! instead: render the region once, run it through autotune once, and land
//! the result as a genuinely new source and clip. The original clip and its
//! source are never touched — this is non-destructive by construction,
//! not by a flag, since nothing here ever calls `RemoveClip` or mutates
//! anything that already existed.
//!
//! Same shape as [`crate::export`]'s "here are the ops, you own storing the
//! audio" split, and the same shape `studio_live::bounce_ops` and
//! `studio_live::Recorder::commit_ops` already use for exactly this reason:
//! three instances of one house pattern, not three separate ones.

use studio_core::{Clip, ClipId, Op, Source, SourceId, Ticks, TrackId};
use studio_fx::autotune::AutotuneChannel;
use studio_fx::retune::Scale;

/// Run `original_audio` (mono) through print-side autotune targeting
/// `scale`, and return `(ops, repitched_audio)`: the two ops that add the
/// result as a new source and clip at `track`/`at`, and the actual samples
/// the caller stores under `new_source`'s id (this function does not know
/// about a project's audio store, the same reason `bounce_ops` doesn't
/// either).
///
/// `retune_speed_ms` is exposed directly (0.0 = hard snap, the genre-
/// defining default) rather than requiring the caller to reach into
/// `AutotuneChannel::retune` themselves — everything else stays at
/// `AutotuneChannel::new`'s own defaults.
#[allow(clippy::too_many_arguments)]
pub fn repitch_clip_ops(
    original_audio: &[f32],
    sample_rate: u32,
    scale: Scale,
    retune_speed_ms: f32,
    seed: u64,
    new_source: SourceId,
    new_clip: ClipId,
    track: TrackId,
    at: Ticks,
    path: &str,
) -> (Vec<Op>, Vec<f32>) {
    let mut channel = AutotuneChannel::new(scale, sample_rate as f32, seed);
    channel.retune.retune_speed_ms = retune_speed_ms;
    let repitched = channel.process_buffer(original_audio);

    let len = Ticks::from_samples(repitched.len() as i64, sample_rate);
    let ops = vec![
        Op::AddSource {
            source: Source { id: new_source, path: path.into(), len, sample_rate, channels: 1, has_video: false },
        },
        Op::AddClip {
            clip: Clip { id: new_clip, source: new_source, track, pos: at, src_in: Ticks::ZERO, len, gain: 1.0, effects: Vec::new() },
        },
    ];
    (ops, repitched)
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::{Project, TrackKind};
    use studio_fx::retune::{yin_pitch, Mode, DEFAULT_MAX_HZ, DEFAULT_MIN_HZ};

    const SR: u32 = 48_000;

    fn detuned_take(hz: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| (2.0 * std::f32::consts::PI * hz * i as f32 / SR as f32).sin() * 0.6).collect()
    }

    #[test]
    fn repitch_ops_declare_a_length_matching_the_actual_repitched_audio() {
        let audio = detuned_take(440.0, (0.5 * SR as f32) as usize);
        let (ops, repitched) = repitch_clip_ops(
            &audio, SR, Scale { root: 9, mode: Mode::Minor }, 0.0, 1,
            SourceId(9), ClipId(9), TrackId(1), Ticks::ZERO, "repitch.wav",
        );
        let Op::AddSource { source } = &ops[0] else { panic!("expected AddSource first") };
        assert_eq!(source.len.to_samples(SR) as usize, repitched.len(),
            "the declared source length must match the actual repitched sample count, or a later mixdown could read out of bounds or truncate");
        assert_eq!(source.channels, 1);
    }

    #[test]
    fn repitch_ops_apply_cleanly_alongside_an_existing_untouched_clip() {
        // The non-destructiveness claim, checked directly: build a project
        // with an EXISTING clip, apply the repitch ops, and confirm the
        // original clip and source are byte-identical afterward — nothing
        // here should have removed or mutated them.
        let mut project = Project { name: "t".into(), ..Default::default() };
        project.apply(&Op::AddTrack { track: studio_core::Track {
            id: TrackId(1), kind: TrackKind::Audio, name: "Vox".into(), muted: false, locked: false, gain: 1.0, route: None,
        }, at: 0 }).unwrap();
        project.apply(&Op::AddSource { source: Source {
            id: SourceId(1), path: "original.wav".into(), len: Ticks::from_seconds(2.0),
            sample_rate: SR, channels: 1, has_video: false,
        } }).unwrap();
        project.apply(&Op::AddClip { clip: Clip {
            id: ClipId(1), source: SourceId(1), track: TrackId(1), pos: Ticks::ZERO,
            src_in: Ticks::ZERO, len: Ticks::from_seconds(2.0), gain: 1.0, effects: Vec::new(),
        } }).unwrap();
        let original_source = project.sources[&SourceId(1)].clone();
        let original_clip = project.clips[&ClipId(1)].clone();

        let audio = detuned_take(300.0, (0.5 * SR as f32) as usize);
        let (ops, _repitched) = repitch_clip_ops(
            &audio, SR, Scale { root: 0, mode: Mode::Minor }, 0.0, 3,
            SourceId(2), ClipId(2), TrackId(1), Ticks::from_seconds(2.0), "repitch.wav",
        );
        for op in ops {
            project.apply(&op).expect("repitch ops should apply cleanly");
        }

        assert_eq!(project.sources[&SourceId(1)], original_source, "the original source must be untouched");
        assert_eq!(project.clips[&ClipId(1)], original_clip, "the original clip must be untouched");
        assert_eq!(project.clips.len(), 2, "the repitched result lands as a genuinely NEW clip, not a replacement");
        assert_eq!(project.sources.len(), 2);
    }

    #[test]
    fn the_repitched_audio_actually_reads_closer_to_the_target_scale() {
        // The end-to-end proof: a detuned tone, run through repitch_clip_ops
        // at hard-snap speed, should read (via an independent detection
        // pass) close to the scale degree it targeted — not just "some ops
        // got produced."
        let hz = 440.0 * 2f32.powf(0.4 / 12.0); // ~40 cents sharp of A4
        let audio = detuned_take(hz, (0.5 * SR as f32) as usize);
        let (_ops, repitched) = repitch_clip_ops(
            &audio, SR, Scale { root: 9, mode: Mode::Minor }, 0.0, 1,
            SourceId(1), ClipId(1), TrackId(1), Ticks::ZERO, "repitch.wav",
        );
        let p = yin_pitch(&repitched[4000..20_000], SR as f32, DEFAULT_MIN_HZ, DEFAULT_MAX_HZ, 0.15)
            .expect("the repitched audio should still read as pitched");
        assert!((p.hz - 440.0).abs() < 440.0 * 0.02, "should land within 2% of A4 (440Hz), got {}Hz", p.hz);
    }

    #[test]
    fn an_empty_clip_does_not_panic_and_produces_an_empty_result() {
        let (ops, repitched) = repitch_clip_ops(
            &[], SR, Scale { root: 0, mode: Mode::Minor }, 0.0, 1,
            SourceId(1), ClipId(1), TrackId(1), Ticks::ZERO, "repitch.wav",
        );
        assert!(repitched.is_empty());
        let Op::AddSource { source } = &ops[0] else { panic!() };
        assert_eq!(source.len, Ticks::ZERO);
    }
}
