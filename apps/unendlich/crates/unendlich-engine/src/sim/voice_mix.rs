//! `sim::voice_mix` — pick which sounds actually play.
//!
//! A scene emits far more sounds than the mixer has voices for. This resolves
//! every candidate through [`sound_resolve`](crate::sim::sound_resolve) (distance,
//! occlusion, Doppler, reverb) and keeps the most *audible* ones within a voice
//! budget — louder, nearer, and higher-priority sources win; the inaudible and
//! the over-budget are dropped (voice-stealing). It composes the resolve step; it
//! adds only the scoring + selection. Pure and deterministic.

use crate::sim::sound_resolve::{self, Acoustics, Emitter, Env, Listener, ResolvedSound};

/// One candidate sound: where/how it's emitted, the medium between it and the
/// listener, and how important it is (a UI cue or a dragon roar outranks a
/// distant footstep at equal loudness).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Source {
    pub emitter: Emitter,
    pub env: Env,
    /// Importance weight `>= 0` (1.0 = nominal).
    pub priority: f32,
}

/// A sound that won a voice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActiveVoice {
    /// Index into the input `sources`.
    pub index: usize,
    /// The resolved sound to feed the mixer.
    pub sound: ResolvedSound,
    /// Audibility score used for selection (`gain × priority`).
    pub score: f32,
}

/// Resolve and select up to `max_voices` audible sounds, most-audible first.
/// Score is `resolved gain × priority`; sources with non-positive score (silent,
/// fully attenuated) never take a voice. Ties break by input order, so the result
/// is deterministic. Composes `sound_resolve::resolve`.
pub fn mix(
    sources: &[Source],
    listener: &Listener,
    ac: &Acoustics,
    max_voices: usize,
) -> Vec<ActiveVoice> {
    let mut scored: Vec<ActiveVoice> = sources
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            let sound = sound_resolve::resolve(&s.emitter, listener, &s.env, ac);
            let score = sound.gain * s.priority.max(0.0);
            if score > 0.0 {
                Some(ActiveVoice { index: i, sound, score })
            } else {
                None
            }
        })
        .collect();

    // Most audible first; ties keep input order (stable sort + index tiebreak).
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.index.cmp(&b.index))
    });
    scored.truncate(max_voices);
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ac() -> Acoustics {
        Acoustics { reference: 4.0, rolloff: 1.0, speed_of_sound: 343.0 }
    }
    fn open_env() -> Env {
        Env { solid_blocks_between: 0, per_block_db: 3.0, enclosure: 0.0, room_size: 0.5 }
    }
    fn listener() -> Listener {
        Listener { pos: [0.0; 3], vel: [0.0; 3] }
    }
    fn src(x: f32, volume: f32, priority: f32, env: Env) -> Source {
        Source {
            emitter: Emitter { pos: [x, 0.0, 0.0], vel: [0.0; 3], volume },
            env,
            priority,
        }
    }

    #[test]
    fn budget_is_respected() {
        let sources: Vec<Source> = (0..10).map(|i| src(2.0 + i as f32, 1.0, 1.0, open_env())).collect();
        let out = mix(&sources, &listener(), &ac(), 4);
        assert_eq!(out.len(), 4, "never exceeds the voice budget");
    }

    #[test]
    fn nearest_loudest_win_and_are_ordered() {
        let sources = vec![
            src(50.0, 1.0, 1.0, open_env()), // far
            src(3.0, 1.0, 1.0, open_env()),  // near (loudest)
            src(20.0, 1.0, 1.0, open_env()), // mid
        ];
        let out = mix(&sources, &listener(), &ac(), 3);
        assert_eq!(out[0].index, 1, "nearest is the most audible");
        assert!(out[0].score >= out[1].score && out[1].score >= out[2].score, "sorted by audibility");
    }

    #[test]
    fn priority_can_outrank_loudness() {
        // a slightly farther but high-priority cue beats a near low-priority one.
        let sources = vec![
            src(3.0, 1.0, 1.0, open_env()),   // near, normal priority
            src(6.0, 1.0, 5.0, open_env()),   // farther, high priority
        ];
        let out = mix(&sources, &listener(), &ac(), 1);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].index, 1, "high-priority cue wins the single voice");
    }

    #[test]
    fn inaudible_sources_take_no_voice() {
        let mut sealed_far = open_env();
        sealed_far.solid_blocks_between = 50; // heavily occluded
        let sources = vec![
            src(3.0, 1.0, 1.0, open_env()),
            src(900.0, 0.0, 1.0, open_env()), // zero volume
        ];
        let out = mix(&sources, &listener(), &ac(), 8);
        assert_eq!(out.len(), 1, "the silent source never takes a voice");
        assert_eq!(out[0].index, 0);
    }

    #[test]
    fn zero_budget_is_empty_and_deterministic() {
        let sources = vec![src(3.0, 1.0, 1.0, open_env()), src(5.0, 1.0, 1.0, open_env())];
        assert!(mix(&sources, &listener(), &ac(), 0).is_empty());
        assert_eq!(
            mix(&sources, &listener(), &ac(), 2),
            mix(&sources, &listener(), &ac(), 2)
        );
    }
}
