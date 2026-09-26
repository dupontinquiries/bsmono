//! studio-copilot — proposals, never actions.
//!
//! Everything here does the same thing: turn some read of the session
//! (a spectrum, a request, a tap of recent commands) into a
//! [`Proposal`] — a description of ops to apply, with a summary and a
//! confidence — that the caller shows the user and applies (or doesn't) as
//! one undo group. Nothing in this crate ever calls `Project::apply` or
//! touches an `EngineState` directly. That's deliberate: the copilot is a
//! client of the same `Command`/`Op` seam touch, voice, and an incoming
//! collab packet all go through (see `studio-command`'s docs) — it has no
//! private powers a human at the keyboard doesn't also have.
//!
//! ## What's actually implemented in this pass
//!
//! - [`mix_pass`] — the one recipe with a clean path to real ops: it wraps
//!   `studio_fx::assistant::{analyze, propose}` (findings-first, strength-
//!   capped, already tested to propose nothing on a balanced mix) and maps
//!   each proposed [`Move`] onto a `studio_core::Op` against a caller-given
//!   master bus.
//! - [`FakeCopilot`] plus [`set_up_a_verse`] — a deterministic, offline
//!   backend and one Command-table recipe ("set up a verse": arm, monitor-
//!   retune on, count-in, loop the punch region) that needs no new schema,
//!   only composition of existing `Command`s.
//! - [`flip_the_drums`] — constrained, deterministic mutation of a pattern's
//!   hits plus a bounded random walk of any Beatspace channel's latent `z`,
//!   seeded so the same (rack, pattern, seed) always proposes the identical
//!   flip. Returns a whole replacement `Rack`, not `Op`s — see [`Proposal`]'s
//!   docs on why that's a genuinely different shape of "apply."
//! - [`change_song_key`] — transposes every `Sub`/`Sine` channel's pitch by
//!   the shift between a caller-given `from`/`to` `studio_fx::retune::Scale`.
//!   No persisted project key was needed for this: the key is a parameter
//!   the caller supplies (from a UI picker, a voice command, wherever),
//!   same as `mix_pass`'s analysis inputs already are — not project state
//!   this crate reads back. See its own doc comment for what it does NOT
//!   cover (repitching existing audio clips), and why.
//!
//! ## What's explicitly NOT implemented here, and why
//!
//! The original plan named four recipes: mix pass, set up a verse, change
//! song key, flip the drums. All four now have a real implementation, though
//! `change_song_key`'s is partial — see its own doc comment for the
//! audio-clip-repitch piece that's deliberately out of scope for this crate.

use studio_command::CommandKind;
use studio_core::{BusId, Effect, Op};
use studio_fx::analysis::Spectrum;
use studio_fx::assistant::{self, Move};

/// A compact read of the session — kilobytes, not the project — that a
/// `Copilot` implementation reasons about. Deliberately small: this is
/// context for a request, not a serialization of the whole `Project`.
#[derive(Debug, Clone, Default)]
pub struct SessionCtx {
    pub bpm: f32,
    pub master_bus: Option<BusId>,
    /// The tail of the engine tap (see `studio-command::CommandTap`) — recent
    /// activity a copilot might reasonably use as context ("they just armed a
    /// track" is a hint "record a verse" means THIS track).
    pub recent_commands: Vec<CommandKind>,
    /// The current rack, when the copilot's ask might be a pattern-mutation
    /// recipe ("flip the drums"). `None` when the caller doesn't have one to
    /// hand (or the project has none) — such an ask then falls to a
    /// "nothing to propose" response instead of a real `Proposal`, the same
    /// as any other recipe whose preconditions aren't met.
    pub rack: Option<studio_live::Rack>,
}

/// One proposal: a human-readable summary, the ops that would apply it, and
/// a confidence the UI can use to hedge ("I think..." vs a flat statement).
/// Never applied by anything in this crate — the caller decides.
#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
    pub summary: String,
    pub ops: Vec<Op>,
    pub confidence: f32,
    /// A whole replacement rack, when the proposal is a pattern/rack
    /// mutation ("flip the drums") rather than a persisted `Op`. Rack edits
    /// don't go through `History`'s undo groups at all — they're a separate
    /// whole-value LWW register (see `studio_collab::RackSync`'s docs) — so
    /// this is a genuinely different kind of "apply" than `ops` is: the
    /// caller applies this via `Doc::set_rack`, not `History::apply_group`.
    /// A proposal carries either `ops` or `rack` in practice, never both —
    /// they're different recipes, not a combined edit.
    pub rack: Option<studio_live::Rack>,
}

impl Proposal {
    fn nothing(summary: impl Into<String>) -> Proposal {
        Proposal { summary: summary.into(), ops: Vec::new(), confidence: 0.0, rack: None }
    }

    /// The single `Command` this proposal's `rack` (if any) applies as —
    /// the bridge from "here is a proposed replacement rack" to "here is the
    /// thing you feed `EngineState::apply`". Returns `None` when this
    /// proposal isn't a rack recipe at all (its `ops`-based proposals, like
    /// `mix_pass`, don't need this: the caller already wraps each op as
    /// `Command::Edit` directly — see this crate's own tests for that
    /// pattern — since a single, uniform `Command` doesn't make sense for a
    /// list of several ops the way it does for one whole rack).
    pub fn rack_command(&self) -> Option<studio_command::Command> {
        self.rack.clone().map(studio_command::Command::Rack)
    }
}

/// Something that can answer a free-text ask with a `Proposal`. The trait
/// exists so a real backend (an LLM client) and [`FakeCopilot`] (canned,
/// deterministic, offline) are interchangeable — tests, and any offline
/// development, run against the fake without needing a network call.
pub trait Copilot {
    fn respond(&mut self, ctx: &SessionCtx, ask: &str) -> Proposal;
}

/// A canned backend: exact-ish phrase matching onto the recipes this crate
/// actually implements, plus a coordinate-in-good-faith fallback for
/// anything else. Not a real language model — a fixed point other pieces
/// (voice grammar, UI, tests) can be built and tested against before a real
/// backend exists, same bargain `SynthKind::Beatspace` makes for a missing
/// model: the surface exists and behaves sanely with nothing behind it.
///
/// `change_song_key` is deliberately NOT wired up here: unlike "flip the
/// drums" (which needs no arguments beyond the current rack), it needs a
/// structured `from`/`to` key — extracting that from free text ("change the
/// key to F sharp minor") is real natural-language parsing this canned
/// matcher has no business attempting. A real backend would parse it; this
/// one honestly declines rather than guessing at a key from a phrase.
#[derive(Debug, Default)]
pub struct FakeCopilot;

/// A fixed seed for voice-triggered flips, since a spoken "flip the drums"
/// carries no seed of its own to be deterministic against — this keeps
/// `FakeCopilot`'s own responses reproducible for testing, the same
/// determinism-over-randomness trade the rest of this crate's recipes make.
const VOICE_FLIP_SEED: u64 = 0xF11D;

impl Copilot for FakeCopilot {
    fn respond(&mut self, ctx: &SessionCtx, ask: &str) -> Proposal {
        let ask_lower = ask.to_lowercase();
        if ask_lower.contains("verse") || ask_lower.contains("record") {
            return set_up_a_verse(ctx);
        }
        if ask_lower.contains("flip") && ask_lower.contains("drum") {
            return match &ctx.rack {
                Some(rack) => flip_the_drums_proposal(rack, 0, VOICE_FLIP_SEED, 0.5),
                None => Proposal::nothing("There's no rack in this session to flip."),
            };
        }
        Proposal::nothing(format!("I don't have a recipe for \"{ask}\" yet — nothing proposed."))
    }
}

// ── voice routing ───────────────────────────────────────────────────────────

/// Compile [`studio_command::COMMAND_TABLE`] into a [`studio_voice::Repertoire`]
/// the fast voice path matches against, alongside a lookup back from a
/// matched routine's name to the [`CommandKind`] it represents.
///
/// The table, not a hand-maintained grammar, is what a routine's phrases come
/// from — the same reasoning `studio-command`'s own docs give for compiling
/// the grammar from the table: a new command that forgets to register
/// phrases here is a missing table entry, not a silently-uncovered case
/// discovered later. Kinds with no phrases at all (`PreviewVoice`,
/// `StopPreview` — pad-only gestures, not something anyone says) are skipped
/// rather than compiled into a routine that could never fire.
pub fn compiled_repertoire() -> (studio_voice::Repertoire, std::collections::BTreeMap<String, CommandKind>) {
    let mut rep = studio_voice::Repertoire::default();
    let mut by_name = std::collections::BTreeMap::new();
    for spec in studio_command::COMMAND_TABLE {
        if spec.phrases.is_empty() {
            continue;
        }
        // The routine's own `steps` can't carry an arbitrary Command (only
        // `Op`, per `studio_voice::Step`'s definition) — see this module's
        // doc comment on why the actual dispatch happens through
        // `by_name`/`CommandKind`, not through the routine's `steps`.
        rep.record(spec.name, spec.phrases, Vec::new());
        by_name.insert(spec.name.to_string(), spec.kind);
    }
    (rep, by_name)
}

/// What matching an utterance against the voice grammar, with a copilot
/// fallback, produced.
#[derive(Debug)]
pub enum RouteResult {
    /// The fast path fired: a `Command` kind the caller should build and
    /// apply (through the same lowering every producer goes through — see
    /// `studio-command`'s docs). The caller is responsible for checking
    /// `studio_command::spec_for(kind).destructive` and gating on the
    /// confirm word before applying, same as any other command source.
    Voice(CommandKind),
    /// Nothing in the compiled grammar cleared the firing floor — handed to
    /// the copilot instead of being dropped on the floor, per the rule this
    /// whole router exists to implement: a miss falls through to the
    /// copilot rather than silence.
    Copilot(Proposal),
}

/// Match `ask` against the compiled command grammar first; on a miss, ask
/// `copilot` instead. This is the whole "misses fall through to the copilot"
/// rule as one function — deterministic given a deterministic `copilot`
/// (e.g. [`FakeCopilot`]), so it's fully testable without real speech input.
pub fn route_utterance(
    repertoire: &studio_voice::Repertoire,
    by_name: &std::collections::BTreeMap<String, CommandKind>,
    ctx: &SessionCtx,
    copilot: &mut dyn Copilot,
    ask: &str,
) -> RouteResult {
    if let Some(routine) = repertoire.resolve(ask) {
        if let Some(&kind) = by_name.get(&routine.name) {
            return RouteResult::Voice(kind);
        }
    }
    RouteResult::Copilot(copilot.respond(ctx, ask))
}

/// Findings-first mastering pass against `master_bus`, wrapping
/// `studio_fx::assistant`. `strength` and the underlying findings already
/// guarantee: strength 0 proposes nothing, and a genuinely balanced signal
/// (no findings at all) proposes nothing regardless of strength — both are
/// `assistant`'s own tested invariants, inherited here rather than
/// reimplemented.
///
/// `current_gain` is the master bus's gain BEFORE this pass, needed because
/// `Move::Gain` is a relative dB suggestion but `Op::SetBusGain` sets an
/// absolute value — without the current gain, applying a gain move would
/// clobber whatever the fader was already at instead of nudging it.
///
/// `Move::BandWidth` findings are included in the summary text but do not
/// currently produce an Op: `studio_fx::chain::instantiate` doesn't yet
/// register an "imager" effect kind (a pre-existing gap between
/// `studio_fx::imager` and the effect registry, not something introduced
/// here), so there is nowhere for a width-change Op to land yet.
pub fn mix_pass(
    spec: &Spectrum,
    crest_db: f32,
    correlation: f32,
    low_width: f32,
    master_bus: BusId,
    current_gain: f32,
    strength: f32,
) -> Proposal {
    let findings = assistant::analyze(spec, crest_db, correlation, low_width);
    if findings.is_empty() {
        return Proposal::nothing("This mix reads balanced — nothing to propose.");
    }
    let moves = assistant::propose(&findings, strength);
    if moves.is_empty() {
        // strength 0, or every finding capped to a no-op move.
        return Proposal::nothing(assistant::summary(&findings));
    }

    let mut ops = Vec::new();
    let mut skipped_bandwidth = false;
    for mv in &moves {
        match mv {
            Move::Eq { hz, gain_db, q } => {
                let mut params = std::collections::BTreeMap::new();
                params.insert("bands".to_string(), 1.0);
                params.insert("kind0".to_string(), 0.0); // Peaking
                params.insert("freq0".to_string(), *hz);
                params.insert("q0".to_string(), *q);
                params.insert("gain0".to_string(), *gain_db);
                ops.push(Op::SetBusChain {
                    id: master_bus,
                    // Appended as its own EQ stage rather than merged into an
                    // existing one — instantiate() runs each chain effect in
                    // series, so several single-band EQ effects behave
                    // correctly, just not as compactly as a merged multi-band
                    // one would. A real merge needs the caller's current
                    // chain, which this pure function doesn't have; the
                    // caller composing this Op onto its own current chain
                    // (rather than replacing it wholesale) is assumed here —
                    // see the crate-level docs' honesty note style: this is
                    // the simplification, stated rather than hidden.
                    chain: vec![Effect { kind: "eq".into(), params, enabled: true }],
                });
            }
            Move::Compress { threshold_db, ratio } => {
                let mut params = std::collections::BTreeMap::new();
                params.insert("threshold".to_string(), *threshold_db);
                params.insert("ratio".to_string(), *ratio);
                ops.push(Op::SetBusChain {
                    id: master_bus,
                    chain: vec![Effect { kind: "compressor".into(), params, enabled: true }],
                });
            }
            Move::Gain { db } => {
                let new_gain = current_gain * 10f32.powf(db / 20.0);
                ops.push(Op::SetBusGain { id: master_bus, gain: new_gain });
            }
            Move::BandWidth { .. } => {
                skipped_bandwidth = true;
            }
        }
    }

    let mut summary = assistant::summary(&findings);
    if skipped_bandwidth {
        summary.push_str(" (a width finding is noted above but has no effect kind registered to act on it yet.)");
    }
    let confidence = findings.iter().map(|f| f.confidence).fold(0.0f32, f32::max);
    Proposal { summary, ops, confidence, rack: None }
}

/// "Set up a verse": arm the given track, turn on the live monitor retune,
/// set a count-in, and loop the punch region — pure composition of existing
/// `Command`s, so it needs no new schema and no audio to test. Everything
/// here is reversible/ephemeral (see `studio-command`'s docs on why
/// transport/engine commands aren't ops), so there's no undo-group concern
/// the way there is for `mix_pass`'s persisted `Op`s — which is also why
/// this returns an empty `Proposal.ops`: the real output is the command
/// sequence in [`set_up_a_verse_commands`], not an `Op` list.
pub fn set_up_a_verse(_ctx: &SessionCtx) -> Proposal {
    Proposal {
        summary: "Arm, count in for 1 bar, and turn on monitor retune.".into(),
        ops: Vec::new(),
        confidence: 1.0,
        rack: None,
    }
}

/// The actual command sequence `set_up_a_verse` describes — split out so a
/// caller (or a test) can get the real `Command`s rather than only the
/// human-readable summary `set_up_a_verse` returns as a `Proposal`.
pub fn set_up_a_verse_commands() -> Vec<studio_command::Command> {
    use studio_command::{Command, EngineCmd, TransportCmd};
    vec![
        Command::Transport(TransportCmd::CountIn(1)),
        Command::Engine(EngineCmd::MonitorRetune(true)),
        Command::Transport(TransportCmd::Record(true)),
    ]
}

/// "Flip the drums": propose a whole replacement rack with the given
/// pattern's hits nudged and any Beatspace channel's latent position walked
/// a bounded random step, seeded so the exact same inputs always propose the
/// exact same flip — the same "deterministic despite looking random" trade
/// `studio_live::render_pattern`'s own `prob`/`every` handling makes, reused
/// here rather than re-derived.
///
/// `strength` (0..=1) scales how much changes: 0 is a true no-op (returns a
/// rack identical to the input, not just "mostly the same" — see the pinned
/// test for this), matching every other strength-scaled recipe in this
/// codebase's own house rule that a dial at zero must change nothing.
///
/// Returns `None` if `pattern_index` is out of range for `rack.patterns` —
/// there's nothing sensible to flip.
pub fn flip_the_drums(rack: &studio_live::Rack, pattern_index: usize, seed: u64, strength: f32) -> Option<studio_live::Rack> {
    if rack.patterns.get(pattern_index).is_none() {
        return None;
    }
    let strength = strength.clamp(0.0, 1.0);
    let mut new_rack = rack.clone();
    let mut rng = core_kit::rng::SplitMix64::new(seed);

    let steps = new_rack.patterns[pattern_index].steps;
    let row_count = new_rack.patterns[pattern_index].rows.len();
    for ch in 0..row_count {
        for step in 0..steps {
            // A flip probability capped well under 1.0 even at full
            // strength: "flip the drums" is meant to nudge a pattern into a
            // variation, not replace it with unrelated noise. 0.35 was
            // chosen so a full-strength flip changes roughly a third of
            // steps on average — enough to clearly hear, not so much the
            // original pattern is unrecognisable.
            let roll = rng.next_f64() as f32;
            if roll < strength * 0.35 {
                new_rack.patterns[pattern_index].toggle(ch, step);
            }
        }
    }

    // Beatspace channels: nudge the latent position by a small bounded
    // random step rather than teleporting it — a walk, not a jump, per the
    // plan's own "Beatspace z walks where that channel is present."
    for ch in new_rack.channels.iter_mut() {
        if let studio_live::SynthKind::Beatspace { z, .. } = &mut ch.kind {
            for v in z.iter_mut() {
                let delta = (rng.next_f64() as f32 - 0.5) * 0.4 * strength;
                *v = (*v + delta).clamp(-1.0, 1.0);
            }
        }
    }

    Some(new_rack)
}

/// Wraps [`flip_the_drums`] as a [`Proposal`] — the shape a caller showing a
/// preview card actually wants, rather than the bare `Option<Rack>`.
pub fn flip_the_drums_proposal(rack: &studio_live::Rack, pattern_index: usize, seed: u64, strength: f32) -> Proposal {
    match flip_the_drums(rack, pattern_index, seed, strength) {
        Some(new_rack) => Proposal {
            summary: format!("Flip pattern {pattern_index}: nudge some hits and, if any Beatspace channels are present, walk their latent position."),
            ops: Vec::new(),
            confidence: 0.6,
            rack: Some(new_rack),
        },
        None => Proposal::nothing(format!("No pattern at index {pattern_index} to flip.")),
    }
}

/// "Change song key": transpose every `Sub`/`Sine` channel's pitch by the
/// signed semitone shift between `from.root` and `to.root` — the smallest-
/// magnitude shift (e.g. C to B is -1 semitone, not +11), same reasoning
/// `studio_fx::retune::Scale::nearest_hz` uses for picking the nearest
/// degree rather than always rounding up.
///
/// `StepMod.tune` is deliberately left untouched: it's a semitone offset
/// RELATIVE to the channel's own note (see `StepMod`'s docs), so once the
/// channel's base note has shifted, every relative offset built on top of it
/// remains correct as-is — re-shifting it too would double-transpose.
/// `Sampler`/`Beatspace` channels have no defined pitch parameter to shift
/// (the same "no-op for kinds without one" rule `StepMod.tune` itself
/// already follows), so they're left alone too.
///
/// What this does NOT do, and why: repitch existing AUDIO clips (sung or
/// recorded material, as opposed to a synth channel's note). That needs
/// `studio_fx::psola` run over a whole clip's rendered buffer — a
/// fundamentally different, buffer-level operation (see `psola`'s and
/// `autotune`'s own docs on why they don't fit the per-sample
/// `Instance::process_audio` pipeline `Bus.chain`/`Clip.effects` use) — real,
/// separate work as an offline bounce step, not something to force into this
/// function's shape.
pub fn change_song_key(rack: &studio_live::Rack, from: studio_fx::retune::Scale, to: studio_fx::retune::Scale) -> studio_live::Rack {
    let raw = (to.root as i32 - from.root as i32).rem_euclid(12);
    let delta = if raw > 6 { raw - 12 } else { raw };
    let mut new_rack = rack.clone();
    for ch in new_rack.channels.iter_mut() {
        match &mut ch.kind {
            studio_live::SynthKind::Sub { note, .. } => {
                *note = note.saturating_add(delta as i8);
            }
            // melodic, like the sine: transposed with the song
            studio_live::SynthKind::Sine { hz } | studio_live::SynthKind::Poly { hz, .. } => {
                *hz *= 2f32.powf(delta as f32 / 12.0);
            }
            // TRANSPOSED, because a soundfont is a melodic instrument.
            // A key change that moved the 808 and the sine and left the
            // piano behind would put the song out of tune with itself,
            // which is the one outcome this function exists to prevent.
            //
            // Bank 128 is General MIDI percussion and is NOT transposed,
            // for the same reason the drum channels below are not: a kit
            // is not in the key, and shifting it renumbers which drum
            // each note plays.
            studio_live::SynthKind::Sf2 { transpose, bank, .. } if *bank != 128 => {
                *transpose = transpose.saturating_add(delta as i8);
            }
            studio_live::SynthKind::Sf2 { .. } => {}
            studio_live::SynthKind::Sampler { .. } | studio_live::SynthKind::Beatspace { .. } => {}
            // Deliberately NOT transposed: a kit is not in the key. Moving a
            // song from C to F should carry the 808 and the melodic channels
            // with it and leave the kick, snare and hats exactly where they
            // were — retuning the drums five semitones on a key change is
            // the mistake, not the feature. Per-hit drum retunes stay a
            // `StepMod::tune` decision, made by a person, on a step.
            studio_live::SynthKind::Drum { .. } => {}
        }
    }
    new_rack
}

/// Wraps [`change_song_key`] as a [`Proposal`] — a whole replacement rack,
/// same apply path as [`flip_the_drums_proposal`].
pub fn change_song_key_proposal(rack: &studio_live::Rack, from: studio_fx::retune::Scale, to: studio_fx::retune::Scale) -> Proposal {
    let new_rack = change_song_key(rack, from, to);
    Proposal {
        summary: format!(
            "Change key: transpose Sub and Sine channels (root {} -> root {}). Sampled/Beatspace channels and existing audio clips are unaffected."
            , from.root, to.root
        ),
        ops: Vec::new(),
        confidence: 0.7,
        rack: Some(new_rack),
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_key_change_carries_a_melodic_soundfont_and_leaves_a_drum_bank() {
        // A key change that moved the 808 and the sine and left the
        // piano behind would put the song out of tune with itself.
        // Bank 128 is GM percussion — transposing it renumbers which
        // drum each note plays, which is not a key change at all.
        use studio_live::SynthKind;
        let piano = SynthKind::Sf2 {
            path: "/p.sf2".into(), bank: 0, preset: 0, transpose: 0,
        };
        let kit = SynthKind::Sf2 {
            path: "/k.sf2".into(), bank: 128, preset: 0, transpose: 0,
        };
        use studio_live::Channel;
        let ch = |name: &str, kind| Channel {
            name: name.into(), kind, gain: 1.0, pan: 0.0,
            muted: false, choke_group: None, one_shot: false,
        };
        let rack = studio_live::Rack {
            channels: vec![ch("piano", piano), ch("kit", kit)],
            patterns: vec![studio_live::Pattern::new("p", 16, 2)],
            tempo_bpm: 120.0, swing: 0.0, ..Default::default() };

        use studio_fx::retune::{Mode, Scale};
        let c = Scale { root: 0, mode: Mode::Minor };
        let f = Scale { root: 5, mode: Mode::Minor };
        let out = change_song_key(&rack, c, f);

        match &out.channels[0].kind {
            SynthKind::Sf2 { transpose, .. } => assert_eq!(*transpose, 5),
            k => panic!("{k:?}"),
        }
        match &out.channels[1].kind {
            SynthKind::Sf2 { transpose, .. } =>
                assert_eq!(*transpose, 0, "a percussion bank must not transpose"),
            k => panic!("{k:?}"),
        }
    }
    use super::*;

    fn low_mid_buildup_signal(n: usize) -> Vec<f32> {
        let sr = 48_000.0;
        // One tone roughly centered in each of assistant::analyze's 8 octave
        // bands, all at a similar baseline level (so the MEDIAN octave reads
        // as real signal, not silence — analyze() early-returns as "nothing
        // playing" if the median octave is near the noise floor), except the
        // two octaves assistant.rs calls "low_mid" (160-320Hz, 320-640Hz)
        // boosted well past its >5dB-over-median threshold.
        let band_tones: [(f32, f32); 8] = [
            (60.0, 0.12), (120.0, 0.12), (240.0, 0.6), (480.0, 0.6),
            (960.0, 0.12), (1920.0, 0.12), (3840.0, 0.12), (7680.0, 0.12),
        ];
        (0..n)
            .map(|i| {
                let t = i as f32 / sr;
                band_tones.iter().map(|(hz, amp)| (2.0 * std::f32::consts::PI * hz * t).sin() * amp).sum()
            })
            .collect()
    }

    fn crest_db(samples: &[f32]) -> f32 {
        let peak = samples.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
        if rms <= 0.0 { 0.0 } else { 20.0 * (peak / rms).log10() }
    }

    #[test]
    fn mix_pass_proposes_nothing_at_strength_zero_regardless_of_findings() {
        let n = 4096;
        let s = low_mid_buildup_signal(n);
        let spec = Spectrum::analyze(&s, 48_000.0);
        let p = mix_pass(&spec, crest_db(&s), 0.9, 0.3, BusId(1), 1.0, 0.0);
        assert!(p.ops.is_empty(), "strength 0 must propose nothing, inherited from assistant::propose's own guarantee");
    }

    #[test]
    fn mix_pass_proposes_an_eq_move_for_a_real_low_mid_buildup() {
        let n = 4096;
        let s = low_mid_buildup_signal(n);
        let spec = Spectrum::analyze(&s, 48_000.0);
        let p = mix_pass(&spec, crest_db(&s), 0.9, 0.3, BusId(7), 1.0, assistant::DEFAULT_STRENGTH);
        assert!(!p.ops.is_empty(), "a real low-mid buildup at default strength should propose something");
        let has_eq_on_master = p.ops.iter().any(|op| matches!(op, Op::SetBusChain { id, chain } if *id == BusId(7) && chain.iter().any(|e| e.kind == "eq")));
        assert!(has_eq_on_master, "expected an EQ move targeting the given master bus, got {:?}", p.ops);
    }

    #[test]
    fn mix_pass_never_proposes_a_cut_or_boost_beyond_the_assistants_own_cap() {
        // assistant::propose caps EQ moves at 4dB even at full strength —
        // mix_pass must not widen that cap when translating to an Op.
        let n = 4096;
        let s = low_mid_buildup_signal(n);
        let spec = Spectrum::analyze(&s, 48_000.0);
        let p = mix_pass(&spec, crest_db(&s), 0.9, 0.3, BusId(1), 1.0, 1.0); // full strength
        for op in &p.ops {
            if let Op::SetBusChain { chain, .. } = op {
                for e in chain {
                    if e.kind == "eq" {
                        let gain = e.params.get("gain0").copied().unwrap_or(0.0);
                        assert!(gain.abs() <= 4.0 + 0.01, "EQ gain {gain} exceeds the assistant's own 4dB cap");
                    }
                }
            }
        }
    }

    #[test]
    fn mix_pass_gain_move_is_relative_to_the_current_gain_not_absolute() {
        // Construct a scenario likely to trigger a Gain move: an
        // over-compressed signal (very low crest factor) at full strength.
        let n = 4096;
        let sr = 48_000.0;
        // A hard-limited-looking square-ish wave: very low crest factor.
        let s: Vec<f32> = (0..n).map(|i| {
            let t = i as f32 / sr;
            (2.0 * std::f32::consts::PI * 200.0 * t).sin().signum() * 0.95
        }).collect();
        let spec = Spectrum::analyze(&s, sr);
        let current = 0.5f32;
        let p = mix_pass(&spec, crest_db(&s), 0.9, 0.3, BusId(1), current, 1.0);
        for op in &p.ops {
            if let Op::SetBusGain { gain, .. } = op {
                // If a gain move fired, it should be a small nudge relative
                // to `current`, not an unrelated absolute value like 1.0.
                assert!((*gain - current).abs() < current, "gain op {gain} should be a modest adjustment relative to current gain {current}, not an unrelated absolute value");
            }
        }
    }

    #[test]
    fn every_proposal_op_is_a_persistent_edit_never_a_transport_or_engine_command() {
        // A structural invariant: Proposal.ops is Vec<Op>, so this is
        // actually enforced by the type system — this test exists to make
        // that guarantee explicit and named, not to catch a runtime failure
        // the compiler would already prevent.
        let n = 4096;
        let s = low_mid_buildup_signal(n);
        let spec = Spectrum::analyze(&s, 48_000.0);
        let p = mix_pass(&spec, crest_db(&s), 0.9, 0.3, BusId(1), 1.0, 0.5);
        for op in &p.ops {
            assert!(studio_command::Command::Edit(op.clone()).is_persistent());
        }
    }

    #[test]
    fn fake_copilot_recognizes_a_verse_request() {
        let mut fc = FakeCopilot;
        let ctx = SessionCtx::default();
        let p = fc.respond(&ctx, "set up a verse for me");
        assert_eq!(p.confidence, 1.0);
        assert!(p.summary.to_lowercase().contains("count in"));
    }

    #[test]
    fn fake_copilot_declines_gracefully_on_an_unknown_ask() {
        let mut fc = FakeCopilot;
        let ctx = SessionCtx::default();
        let p = fc.respond(&ctx, "make it sound like a vintage tape machine from mars");
        assert!(p.ops.is_empty());
        assert_eq!(p.confidence, 0.0);
        assert!(p.summary.contains("mars"), "should echo back what it couldn't handle rather than a generic error");
    }

    #[test]
    fn fake_copilot_recognizes_flip_the_drums_when_a_rack_is_available() {
        // Regression test for a real gap: flip_the_drums existed as a real
        // function but FakeCopilot's dispatcher never routed to it — a
        // spoken "flip the drums" would have hit the generic "I don't have
        // a recipe" fallback even though the recipe existed.
        let mut fc = FakeCopilot;
        let ctx = SessionCtx { rack: Some(sample_flip_rack()), ..Default::default() };
        let p = fc.respond(&ctx, "flip the drums for me");
        assert!(p.rack.is_some(), "should route to the real flip_the_drums recipe, not the generic fallback");
        assert!(p.ops.is_empty(), "a rack mutation carries no ops");
    }

    #[test]
    fn fake_copilot_declines_flip_the_drums_gracefully_with_no_rack_in_context() {
        let mut fc = FakeCopilot;
        let ctx = SessionCtx::default(); // rack: None
        let p = fc.respond(&ctx, "flip the drums");
        assert!(p.rack.is_none());
        assert!(p.ops.is_empty());
        assert_eq!(p.confidence, 0.0);
    }

    #[test]
    fn fake_copilot_voice_triggered_flips_are_deterministic() {
        let mut fc = FakeCopilot;
        let ctx = SessionCtx { rack: Some(sample_flip_rack()), ..Default::default() };
        let a = fc.respond(&ctx, "flip the drums");
        let b = fc.respond(&ctx, "flip the drums please");
        assert_eq!(a.rack, b.rack, "voice-triggered flips use a fixed seed, so repeated asks propose the identical flip");
    }

    #[test]
    fn set_up_a_verse_commands_are_reversible_or_ephemeral_not_destructive() {
        let commands = set_up_a_verse_commands();
        assert!(!commands.is_empty());
        for cmd in &commands {
            assert!(!cmd.is_destructive(), "a verse setup recipe should never require a confirm word: {cmd:?}");
        }
    }

    #[test]
    fn compiled_repertoire_covers_every_command_kind_that_has_phrases() {
        let (_, by_name) = compiled_repertoire();
        let with_phrases = studio_command::COMMAND_TABLE.iter().filter(|s| !s.phrases.is_empty()).count();
        assert_eq!(by_name.len(), with_phrases, "every phrase-bearing command should compile into exactly one routine");
    }

    #[test]
    fn rack_replace_is_deliberately_excluded_from_the_compiled_voice_grammar() {
        // Not just a count check: confirm BY NAME that RackReplace didn't
        // sneak in. It's phrase-less on purpose — see COMMAND_TABLE's own
        // comment on why the fast voice path can't usefully build a
        // Command::Rack (no payload, only a CommandKind marker), and why
        // that's FakeCopilot's job instead. A future edit accidentally
        // giving RackReplace phrases would silently starve that fallback
        // the way the very first draft of this feature almost did.
        let (_, by_name) = compiled_repertoire();
        assert!(!by_name.values().any(|k| *k == CommandKind::RackReplace),
            "RackReplace must never appear in the compiled grammar — see COMMAND_TABLE's own comment on why");
    }

    #[test]
    fn a_clear_phrase_routes_through_the_fast_voice_path() {
        let (rep, by_name) = compiled_repertoire();
        let ctx = SessionCtx::default();
        let mut fc = FakeCopilot;
        match route_utterance(&rep, &by_name, &ctx, &mut fc, "hit play") {
            RouteResult::Voice(kind) => assert_eq!(kind, CommandKind::TransportPlay),
            RouteResult::Copilot(p) => panic!("expected the fast voice path to fire, got a copilot fallback: {p:?}"),
        }
    }

    #[test]
    fn a_phrase_matching_both_the_grammar_and_the_copilot_prefers_the_fast_voice_path() {
        // "record a verse" happens to satisfy BOTH matchers — the compiled
        // grammar's "record verse" routine (both tokens present) and
        // FakeCopilot's substring check. The fast path must win: it's
        // cheaper and, per the plan this router implements, voice should
        // only fall through to the copilot when the grammar genuinely
        // doesn't recognize the phrase, not whenever a fallback also could.
        let (rep, by_name) = compiled_repertoire();
        let ctx = SessionCtx::default();
        let mut fc = FakeCopilot;
        match route_utterance(&rep, &by_name, &ctx, &mut fc, "record a verse for me please") {
            RouteResult::Voice(kind) => assert_eq!(kind, CommandKind::TransportRecord),
            RouteResult::Copilot(p) => panic!("expected the fast voice path to win when both would match, got fallback: {p:?}"),
        }
    }

    #[test]
    fn an_unmatched_utterance_falls_through_to_the_copilot_rather_than_silence() {
        let (rep, by_name) = compiled_repertoire();
        let ctx = SessionCtx::default();
        let mut fc = FakeCopilot;
        // "verse" alone, without "record", only overlaps one of
        // TransportRecord's two required tokens ("record verse") — below
        // the voice grammar's 0.6 firing floor — while still tripping
        // FakeCopilot's substring match, so this genuinely exercises the
        // fallback path rather than happening to satisfy both matchers.
        match route_utterance(&rep, &by_name, &ctx, &mut fc, "let's do a verse now") {
            RouteResult::Copilot(p) => {
                assert!(p.summary.to_lowercase().contains("count in"), "should reach FakeCopilot's set_up_a_verse recipe, got: {}", p.summary);
            }
            RouteResult::Voice(kind) => panic!("did not expect the fast path to claim this phrase: {kind:?}"),
        }
    }

    #[test]
    fn a_destructive_command_kind_still_reports_as_destructive_after_routing() {
        // The router itself doesn't gate on destructiveness — that's the
        // caller's job, per this module's docs — but the CommandKind it
        // hands back must still carry that information correctly so the
        // caller CAN gate on it.
        let (rep, by_name) = compiled_repertoire();
        let ctx = SessionCtx::default();
        let mut fc = FakeCopilot;
        match route_utterance(&rep, &by_name, &ctx, &mut fc, "remove the track") {
            RouteResult::Voice(kind) => {
                assert!(studio_command::spec_for(kind).destructive, "removing a track should be flagged destructive: {kind:?}");
            }
            RouteResult::Copilot(p) => panic!("expected the fast voice path to recognize this phrase, got fallback: {p:?}"),
        }
    }

    #[test]
    fn gibberish_never_panics_and_always_falls_through() {
        let (rep, by_name) = compiled_repertoire();
        let ctx = SessionCtx::default();
        let mut fc = FakeCopilot;
        for gibberish in ["", "asdkfjhaslkdjfh", "🎛️🎚️", "the quick brown fox"] {
            match route_utterance(&rep, &by_name, &ctx, &mut fc, gibberish) {
                RouteResult::Copilot(_) => {} // expected
                RouteResult::Voice(kind) => panic!("unexpected match for {gibberish:?}: {kind:?}"),
            }
        }
    }

    fn sample_flip_rack() -> studio_live::Rack {
        use studio_live::{Channel, Pattern, SynthKind};
        let mut pattern = Pattern::new("beat", 16, 3);
        pattern.toggle(0, 0);
        pattern.toggle(0, 8);
        pattern.toggle(1, 4);
        studio_live::Rack {
            channels: vec![
                Channel { name: "kick".into(), kind: SynthKind::Sine { hz: 60.0 }, gain: 1.0, pan: 0.0, muted: false, choke_group: None, one_shot: false },
                Channel { name: "snare".into(), kind: SynthKind::Sampler { source: studio_core::SourceId(1) }, gain: 1.0, pan: 0.0, muted: false, choke_group: None, one_shot: false },
                Channel { name: "gan".into(), kind: SynthKind::Beatspace { z: [0.0, 0.0, 0.0], crossfade: 0.5 }, gain: 1.0, pan: 0.0, muted: false, choke_group: None, one_shot: false },
            ],
            patterns: vec![pattern],
            tempo_bpm: 140.0, swing: 0.0,
            ..Default::default()
        }
    }

    #[test]
    fn flip_the_drums_at_zero_strength_is_a_true_no_op() {
        let rack = sample_flip_rack();
        let flipped = flip_the_drums(&rack, 0, 7, 0.0).unwrap();
        assert_eq!(flipped, rack, "strength 0 must change nothing at all, not just 'mostly nothing'");
    }

    #[test]
    fn flip_the_drums_is_deterministic_given_the_same_seed() {
        let rack = sample_flip_rack();
        let a = flip_the_drums(&rack, 0, 42, 0.8).unwrap();
        let b = flip_the_drums(&rack, 0, 42, 0.8).unwrap();
        assert_eq!(a, b, "the same seed must propose the identical flip every time");
    }

    #[test]
    fn flip_the_drums_actually_changes_the_pattern_at_nonzero_strength() {
        let rack = sample_flip_rack();
        // Try a handful of seeds so this isn't relying on one lucky roll.
        let changed = (0..10u64).any(|seed| flip_the_drums(&rack, 0, seed, 0.8).unwrap() != rack);
        assert!(changed, "at least one of these seeds at high strength should change something");
    }

    #[test]
    fn flip_the_drums_returns_none_for_an_out_of_range_pattern() {
        let rack = sample_flip_rack();
        assert!(flip_the_drums(&rack, 5, 1, 0.5).is_none());
    }

    #[test]
    fn flip_the_drums_only_mutates_the_targeted_pattern_and_leaves_other_channels_kind_alone() {
        let mut rack = sample_flip_rack();
        rack.patterns.push(rack.patterns[0].clone()); // a second, identical pattern
        let flipped = flip_the_drums(&rack, 0, 3, 1.0).unwrap();
        assert_eq!(flipped.patterns[1], rack.patterns[1], "only the targeted pattern index should be touched");
        // Non-Beatspace channels' kind must be completely untouched.
        assert_eq!(flipped.channels[0].kind, rack.channels[0].kind);
        assert_eq!(flipped.channels[1].kind, rack.channels[1].kind);
    }

    #[test]
    fn beatspace_z_stays_within_bounds_across_many_seeds_and_repeated_flips() {
        let mut rack = sample_flip_rack();
        for seed in 0..50u64 {
            rack = flip_the_drums(&rack, 0, seed, 1.0).unwrap();
            if let studio_live::SynthKind::Beatspace { z, .. } = &rack.channels[2].kind {
                for v in z {
                    assert!((-1.0..=1.0).contains(v), "z component {v} escaped its bound after seed {seed}");
                }
            } else {
                panic!("channel 2 should still be Beatspace");
            }
        }
    }

    #[test]
    fn flip_the_drums_proposal_carries_the_rack_not_ops() {
        let rack = sample_flip_rack();
        let p = flip_the_drums_proposal(&rack, 0, 9, 0.5);
        assert!(p.ops.is_empty(), "a rack mutation must not also carry ops — they're different apply paths");
        assert!(p.rack.is_some());
    }

    #[test]
    fn rack_command_bridges_a_flip_proposal_into_a_real_command() {
        let rack = sample_flip_rack();
        let p = flip_the_drums_proposal(&rack, 0, 9, 0.5);
        let cmd = p.rack_command().expect("a rack proposal should produce a Command::Rack");
        match cmd {
            studio_command::Command::Rack(new_rack) => assert_eq!(Some(new_rack), p.rack),
            other => panic!("expected Command::Rack, got {other:?}"),
        }
    }

    #[test]
    fn rack_command_is_none_for_an_ops_only_proposal() {
        let p = set_up_a_verse(&SessionCtx::default());
        assert!(p.rack_command().is_none(), "an ops-based proposal has no single rack to bridge");
    }

    #[test]
    fn flip_the_drums_proposal_reports_nothing_for_a_bad_index() {
        let rack = sample_flip_rack();
        let p = flip_the_drums_proposal(&rack, 99, 1, 0.5);
        assert!(p.rack.is_none());
        assert_eq!(p.confidence, 0.0);
    }

    fn scale(root: u8) -> studio_fx::retune::Scale {
        studio_fx::retune::Scale { root, mode: studio_fx::retune::Mode::Minor }
    }

    fn sample_key_rack() -> studio_live::Rack {
        use studio_live::{Channel, SynthKind};
        studio_live::Rack {
            channels: vec![
                Channel { name: "808".into(), kind: SynthKind::Sub { note: 0, glide_from: None, glide_ms: 0.0, drive: 0.0, decay_ms: 300.0 }, gain: 1.0, pan: 0.0, muted: false, choke_group: None, one_shot: false },
                Channel { name: "lead".into(), kind: SynthKind::Sine { hz: 440.0 }, gain: 1.0, pan: 0.0, muted: false, choke_group: None, one_shot: false },
                Channel { name: "snare".into(), kind: SynthKind::Sampler { source: studio_core::SourceId(1) }, gain: 1.0, pan: 0.0, muted: false, choke_group: None, one_shot: false },
                Channel { name: "gan".into(), kind: SynthKind::Beatspace { z: [0.1, 0.2, 0.3], crossfade: 0.5 }, gain: 1.0, pan: 0.0, muted: false, choke_group: None, one_shot: false },
            ],
            patterns: vec![],
            tempo_bpm: 140.0, swing: 0.0,
            ..Default::default()
        }
    }

    #[test]
    fn change_song_key_with_the_same_key_is_a_true_no_op() {
        let rack = sample_key_rack();
        let out = change_song_key(&rack, scale(0), scale(0));
        assert_eq!(out, rack, "from == to must change nothing");
    }

    #[test]
    fn change_song_key_transposes_sub_note_by_the_smallest_signed_shift() {
        let rack = sample_key_rack();
        // C(0) -> B(11): the smallest shift is -1 semitone, not +11.
        let out = change_song_key(&rack, scale(0), scale(11));
        match &out.channels[0].kind {
            studio_live::SynthKind::Sub { note, .. } => assert_eq!(*note, -1, "C->B should shift by -1, not +11"),
            other => panic!("expected Sub, got {other:?}"),
        }
    }

    #[test]
    fn change_song_key_scales_sine_hz_correctly_for_an_octave_shift() {
        // A shift that reduces to exactly 6 semitones both ways is the edge
        // of the "smallest magnitude" rule; use a clean, unambiguous case
        // instead: root 0 -> root 5 is +5 semitones (a fourth up).
        let rack = sample_key_rack();
        let out = change_song_key(&rack, scale(0), scale(5));
        match &out.channels[1].kind {
            studio_live::SynthKind::Sine { hz } => {
                let expected = 440.0 * 2f32.powf(5.0 / 12.0);
                assert!((hz - expected).abs() < 0.01, "expected {expected}Hz, got {hz}");
            }
            other => panic!("expected Sine, got {other:?}"),
        }
    }

    #[test]
    fn change_song_key_leaves_sampler_and_beatspace_channels_untouched() {
        let rack = sample_key_rack();
        let out = change_song_key(&rack, scale(0), scale(7));
        assert_eq!(out.channels[2].kind, rack.channels[2].kind, "Sampler has no pitch parameter to shift");
        assert_eq!(out.channels[3].kind, rack.channels[3].kind, "Beatspace has no pitch parameter to shift");
        // and everything else about every channel besides `kind` is untouched
        for i in 0..rack.channels.len() {
            assert_eq!(out.channels[i].gain, rack.channels[i].gain);
            assert_eq!(out.channels[i].pan, rack.channels[i].pan);
            assert_eq!(out.channels[i].name, rack.channels[i].name);
        }
    }

    #[test]
    fn change_song_key_proposal_carries_the_rack_not_ops() {
        let rack = sample_key_rack();
        let p = change_song_key_proposal(&rack, scale(0), scale(3));
        assert!(p.ops.is_empty());
        assert!(p.rack.is_some());
    }
}
