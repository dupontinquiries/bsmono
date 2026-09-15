//! find — search the timeline for a sound, cheaply first.
//!
//! "Where did I say 'chorus'?" over a session of takes is the question a
//! recognizer answers correctly and far too slowly to type into: whisper
//! on every clip is minutes. voiceterm solved the same problem for
//! utterances and its notes name the lever exactly — the fingerprint DB
//! short-circuits whisper on most inputs — so this reuses that machinery
//! rather than inventing a second one:
//!
//! 1. **Name match** first, because it is free and usually right: a clip
//!    whose source file is called `Vox_Dry_Chorus` needs no acoustics.
//! 2. **Fingerprint prescreen** (`voiceprint-kit`, lifted from voiceterm)
//!    over every candidate clip: 39 floats and a cosine each, microseconds,
//!    which ORDERS the field even though it cannot decide it.
//! 3. **Verification** on the survivors only, by whatever [`Verifier`] the
//!    host supplies — voiceterm's whisper pool being the intended one.
//!    Absent a verifier the prescreen's ranking is returned AS a ranking,
//!    labelled unverified, never as a claim about words.
//!
//! The gate matters as much as the search: [`Gate`] can restrict the hunt
//! to tracks that actually carry audio, or to the ones that DON'T — "which
//! tracks are silent" is a real question when a session has forty of them
//! and something is missing.

use std::collections::BTreeMap;

use studio_audio::SourceAudio;
use studio_core::{Clip, ClipId, Project, TrackId};

/// What to search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Gate {
    /// Every clip on every track.
    #[default]
    Everything,
    /// Only clips whose audio actually has signal — skip the takes that
    /// are room tone, and skip tracks whose media never arrived.
    WithAudio,
    /// Only the SILENT ones: no clips at all, or clips whose audio is
    /// below the floor. The "why can I not hear track twelve" question.
    Silent,
}

/// Anything that can turn audio into words. voiceterm's whisper pool is
/// the intended implementation; the trait exists so this module can be
/// tested, and shipped, without one.
pub trait Verifier {
    /// Words heard in this audio, or `None` if the verifier declines
    /// (too short, too quiet, busy). Never a guess.
    fn transcribe(&self, audio: &SourceAudio) -> Option<String>;
}

/// The honest default: no recognizer available.
pub struct NoVerifier;
impl Verifier for NoVerifier {
    fn transcribe(&self, _: &SourceAudio) -> Option<String> { None }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub clip: ClipId,
    pub track: TrackId,
    pub at: studio_core::Ticks,
    pub label: String,
    /// Prescreen distance, lower is nearer. `None` for a name-only match,
    /// which needs no acoustics and should not pretend to have any.
    pub distance: Option<f32>,
    /// What a verifier heard, when one ran. The distinction between "the
    /// cheap pass liked this" and "a recognizer read it" is the whole
    /// reason both fields exist.
    pub verified: Option<String>,
    pub why: Why,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why { Name, Sound, Silence }

/// Below this RMS a clip counts as silent for [`Gate`]. About -60 dBFS:
/// quiet enough that room tone passes as sound, loud enough that a truly
/// empty take does not.
pub const SILENCE_RMS: f32 = 0.001;

fn rms(a: &SourceAudio) -> f32 {
    let ch = match a.channels.first() { Some(c) if !c.is_empty() => c, _ => return 0.0 };
    (ch.iter().map(|s| s * s).sum::<f32>() / ch.len() as f32).sqrt()
}

/// The clip's own audio window, at the kit's rate.
fn clip_audio(clip: &Clip, audio: &BTreeMap<studio_core::SourceId, SourceAudio>)
              -> Option<Vec<f32>> {
    let a = audio.get(&clip.source)?;
    let from = (clip.src_in.to_seconds().max(0.0) * a.sample_rate as f64) as usize;
    let len = (clip.len.to_seconds().max(0.0) * a.sample_rate as f64) as usize;
    let cut: Vec<Vec<f32>> = a.channels.iter()
        .map(|c| c[from.min(c.len())..(from + len).min(c.len())].to_vec())
        .collect();
    if cut.first().map(Vec::len).unwrap_or(0) == 0 { return None; }
    Some(voiceprint_kit::to_16k_mono(&cut, a.sample_rate))
}

/// Search. `spoken` is a rendering of the keyword to match acoustically —
/// a TTS of it, or a previous recording of the person saying it. Without
/// one the search is name-only, which is stated in the result rather than
/// silently degraded.
pub fn find(
    project: &Project,
    audio: &BTreeMap<studio_core::SourceId, SourceAudio>,
    keyword: &str,
    spoken: Option<&SourceAudio>,
    gate: Gate,
    verifier: &dyn Verifier,
    keep: usize,
) -> Vec<Hit> {
    let needle = keyword.trim().to_lowercase();
    let mut hits: Vec<Hit> = Vec::new();

    // The silence gate answers a different question and answers it alone.
    if gate == Gate::Silent {
        for t in &project.tracks {
            let clips: Vec<&Clip> = project.clips.values().filter(|c| c.track == t.id).collect();
            if clips.is_empty() {
                hits.push(Hit { clip: ClipId(0), track: t.id, at: studio_core::Ticks::ZERO,
                    label: format!("{} — no clips at all", t.name),
                    distance: None, verified: None, why: Why::Silence });
                continue;
            }
            for c in clips {
                let quiet = match audio.get(&c.source) {
                    Some(a) => rms(a) < SILENCE_RMS,
                    // Media that never arrived is not silence — it is a
                    // different problem, and saying so is more useful
                    // than calling the clip empty.
                    None => false,
                };
                if quiet {
                    hits.push(Hit { clip: c.id, track: t.id, at: c.pos,
                        label: format!("{} — silent clip", t.name),
                        distance: None, verified: None, why: Why::Silence });
                }
            }
        }
        return hits;
    }

    let query = spoken.and_then(|s| {
        let mono = voiceprint_kit::to_16k_mono(&s.channels, s.sample_rate);
        let fp = voiceprint_kit::fingerprint(&mono);
        if fp.is_empty() { None } else { Some(fp) }
    });

    for c in project.clips.values() {
        let name = project.sources.get(&c.source).map(|s| s.path.clone()).unwrap_or_default();
        let named = !needle.is_empty() && name.to_lowercase().contains(&needle);
        let sound = clip_audio(c, audio);
        if gate == Gate::WithAudio {
            let has = audio.get(&c.source).map(|a| rms(a) >= SILENCE_RMS).unwrap_or(false);
            if !has { continue; }
        }
        // A name match is free and certain; it needs no distance and must
        // not be outranked by an acoustic maybe.
        if named {
            hits.push(Hit { clip: c.id, track: c.track, at: c.pos, label: name,
                            distance: None, verified: None, why: Why::Name });
            continue;
        }
        let (Some(q), Some(mono)) = (&query, sound) else { continue };
        let fp = voiceprint_kit::fingerprint(&mono);
        if fp.is_empty() { continue; }
        hits.push(Hit { clip: c.id, track: c.track, at: c.pos, label: name,
                        distance: Some(voiceprint_kit::distance(q, &fp)),
                        verified: None, why: Why::Sound });
    }

    // Name matches first, then nearest sound. Ties by position, so the
    // list reads down the timeline rather than by hash order.
    hits.sort_by(|a, b| {
        let ka = (a.why != Why::Name, a.distance.unwrap_or(0.0));
        let kb = (b.why != Why::Name, b.distance.unwrap_or(0.0));
        ka.0.cmp(&kb.0)
            .then(ka.1.partial_cmp(&kb.1).unwrap_or(std::cmp::Ordering::Equal))
            .then(a.at.cmp(&b.at))
    });
    hits.truncate(keep);

    // Verify only the survivors — the entire point of the prescreen. A
    // verifier that declines leaves `verified` None, and the hit stays in
    // the list as what it is: a cheap guess, labelled.
    for h in &mut hits {
        if h.why != Why::Sound { continue; }
        let Some(clip) = project.clips.get(&h.clip) else { continue };
        let Some(mono) = clip_audio(clip, audio) else { continue };
        let as_audio = SourceAudio { sample_rate: voiceprint_kit::RATE,
                                     channels: vec![mono] };
        if let Some(text) = verifier.transcribe(&as_audio) {
            h.verified = Some(text.clone());
        }
    }
    // A verifier that ran and disagreed demotes: heard-but-wrong is worse
    // evidence than never-heard, because something DID listen.
    if !needle.is_empty() {
        hits.sort_by_key(|h| match &h.verified {
            Some(t) if t.to_lowercase().contains(&needle) => 0,
            None => 1,
            Some(_) => 2,
        });
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::{Op, Source, SourceId, Ticks, Track, TrackKind};

    struct Says(&'static str);
    impl Verifier for Says {
        fn transcribe(&self, _: &SourceAudio) -> Option<String> { Some(self.0.into()) }
    }

    fn word(f: (f32, f32), secs: f32, rate: u32) -> SourceAudio {
        let n = (rate as f32 * secs) as usize;
        let mut p = 0.0f32;
        let ch: Vec<f32> = (0..n).map(|i| {
            let t = i as f32 / n as f32;
            p += std::f32::consts::TAU * (f.0 + (f.1 - f.0) * t) / rate as f32;
            p.sin() * 0.4 * (t * std::f32::consts::PI).sin()
        }).collect();
        SourceAudio { sample_rate: rate, channels: vec![ch] }
    }

    fn session() -> (Project, BTreeMap<SourceId, SourceAudio>) {
        let mut p = Project::default();
        let mut audio = BTreeMap::new();
        for (i, name) in ["takes/verse.wav", "takes/chorus take.wav",
                          "takes/room.wav"].iter().enumerate() {
            let id = SourceId(i as u64 + 1);
            let a = match i {
                0 => word((300.0, 500.0), 0.5, 48_000),
                1 => word((900.0, 600.0), 0.5, 48_000),
                _ => SourceAudio { sample_rate: 48_000, channels: vec![vec![0.0; 24_000]] },
            };
            p.apply(&Op::AddSource { source: Source { id, path: (*name).into(),
                len: Ticks::from_seconds(0.5), sample_rate: 48_000, channels: 1,
                has_video: false, kind: studio_core::SourceKind::File } }).unwrap();
            audio.insert(id, a);
        }
        for t in 0..2u64 {
            p.apply(&Op::AddTrack { track: Track { id: TrackId(t + 1),
                kind: TrackKind::Audio, name: format!("t{t}"), muted: false,
                locked: false, gain: 1.0, route: None }, at: t as usize }).unwrap();
        }
        for (i, sid) in [1u64, 2, 3].iter().enumerate() {
            p.apply(&Op::AddClip { clip: Clip { id: ClipId(i as u64 + 1),
                source: SourceId(*sid), track: TrackId(1),
                pos: Ticks::from_seconds(i as f64), src_in: Ticks::ZERO,
                len: Ticks::from_seconds(0.5), gain: 1.0, effects: vec![] } }).unwrap();
        }
        (p, audio)
    }

    #[test]
    fn a_name_match_needs_no_acoustics_and_outranks_a_sonic_guess() {
        let (p, audio) = session();
        let hits = find(&p, &audio, "chorus", None, Gate::Everything, &NoVerifier, 10);
        assert_eq!(hits.len(), 1, "name-only search finds exactly the named clip");
        assert_eq!(hits[0].why, Why::Name);
        assert!(hits[0].distance.is_none(),
                "a name match must not invent a distance it never measured");
        assert!(hits[0].label.contains("chorus"));
    }

    #[test]
    fn the_prescreen_orders_by_sound_and_the_nearest_is_the_same_word() {
        let (p, audio) = session();
        // A "TTS" of the keyword: the same word-shape as the verse take.
        let spoken = word((305.0, 495.0), 0.5, 22_050);
        let hits = find(&p, &audio, "nothing-matches-by-name", Some(&spoken),
                        Gate::Everything, &NoVerifier, 10);
        assert!(hits.len() >= 2, "every candidate is ranked, not filtered");
        assert!(hits.iter().all(|h| h.why == Why::Sound));
        assert!(hits[0].distance.unwrap() <= hits[1].distance.unwrap(), "sorted by nearness");
        let nearest = p.sources[&p.clips[&hits[0].clip].source].path.clone();
        assert!(nearest.contains("verse"), "the same word is nearest, got {nearest}");
        assert!(hits.iter().all(|h| h.verified.is_none()),
                "with no verifier, nothing claims to have been heard");
    }

    #[test]
    fn verification_runs_only_on_survivors_and_disagreement_demotes() {
        let (p, audio) = session();
        let spoken = word((305.0, 495.0), 0.5, 22_050);
        let hits = find(&p, &audio, "verse", Some(&spoken), Gate::Everything,
                        &Says("this is the verse"), 2);
        assert!(hits.len() <= 2, "the expensive pass only ever sees `keep` clips");
        assert!(hits.iter().any(|h| h.verified.as_deref() == Some("this is the verse")
                                    || h.why == Why::Name));
        // A verifier that heard something else pushes that hit down.
        let wrong = find(&p, &audio, "chorus", Some(&spoken), Gate::Everything,
                         &Says("completely different words"), 5);
        assert_eq!(wrong[0].why, Why::Name,
                   "the certain name match still leads once the recognizer disagrees");
    }

    #[test]
    fn the_silence_gate_answers_which_tracks_are_empty() {
        let (p, audio) = session();
        let quiet = find(&p, &audio, "", None, Gate::Silent, &NoVerifier, 10);
        // Track 2 has no clips at all; the room take is below the floor.
        assert!(quiet.iter().any(|h| h.label.contains("no clips at all")));
        assert!(quiet.iter().any(|h| h.label.contains("silent clip")));
        assert!(quiet.iter().all(|h| h.why == Why::Silence));
        // And the opposite gate excludes exactly those.
        let loud = find(&p, &audio, "", Some(&word((300.0, 500.0), 0.4, 48_000)),
                        Gate::WithAudio, &NoVerifier, 10);
        assert!(loud.iter().all(|h| {
            let c = &p.clips[&h.clip];
            rms(&audio[&c.source]) >= SILENCE_RMS
        }), "the silent take is gated out");
    }
}
