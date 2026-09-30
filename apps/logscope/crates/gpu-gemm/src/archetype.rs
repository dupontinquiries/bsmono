//! How a model's context / state / prompt relate — and whether a carry-forward between two turns
//! is position-valid.
//!
//! Every turn this engine runs is `generate(prompt)` on a growing prompt, and a naïve cache
//! re-uses would assume "previous prompt is a token-prefix of this prompt ⇒ everything carried
//! from it is valid here." That assumption is *true for one kind of cache and false for another*,
//! and the old backend folded both into a single `prev_toks.iter().eq(prompt_ids[..prev_toks.len()])`
//! check — which is why the carry fired or refused on the wrong signal, and why no log could tell
//! you which invariant had actually been violated.
//!
//! This module is the model-intrinsic answer to "what does my cache mean":
//!
//! * **KV cache of a Transformer** — the rows at token position `i` are *only* valid to re-use at
//!   the same absolute token position `i`. The cache corresponds to a **prefix of the prompt** and
//!   is position-absolute in *token* space.
//! * **Recurrent (Gated DeltaNet) state of a Hybrid** — the `{ ring, s, next_pos }` checkpoint is
//!   position-absolute in *recurrent-stream* space: it folds the stream over `0..next_pos` and can
//!   never seek to an earlier position. What it corresponds to is the **recurrent stream** it was
//!   folded over, not the whole prompt. Injected context that lands *after* the stream (a suffix)
//!   is fine; context that shifts the stream earlier (a prefix) invalidates the checkpoint, because
//!   the state would then be declared at the wrong absolute position and
//!   `block_prefill_deltanet_gpu`'s `assert np == pos` fires.
//!
//! The old code was *accidentally correct* for the one hybrid the engine ships today because, in
//! that narrow linear-agent case, the recurrent stream equals the whole prompt and so
//! `token position == state position`. That coincidence is exactly what this makes explicit: the two
//! invariants are now separate, and `plan_context` returns *which* one each turn relies on.

use crate::Model;

/// What kind of model this is, by how its context and state are structured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Archetype {
    /// A pure transformer: KV cache, no recurrent layers.
    Transformer,
    /// Transformer blocks PLUS at least one Gated DeltaNet layer. The two caches have *different*
    /// correspondence to the prompt — K/V to a token prefix, delta state to the recurrent stream —
    /// and must be planned separately. The current engine ships only hybrids (deltas ride
    /// alongside attention blocks, never alone); `Recurrent` is a slot for a future pure-recurrent
    /// checkpoint.
    Hybrid,
    /// A model that carries recurrent state with no attention blocks — a slot for a pure-recurrent
    /// checkpoint (none exists today).
    Recurrent,
}

impl Archetype {
    /// A hybrid (attention blocks plus Gated DeltaNet layers) is `Hybrid`; a pure transformer is
    /// `Transformer`. The current engine only constructs hybrids (deltas ride alongside attention,
    /// never standing alone).
    pub fn classify(m: &Model) -> Archetype {
        if m.deltanet_layers() > 0 { Archetype::Hybrid } else { Archetype::Transformer }
    }
}

/// What the model's KV cache *corresponds to* (in the reference frame of a turn's prompt).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextSemantics {
    /// Rows are keyed by their absolute token position: re-use is valid iff this prompt embeds the
    /// carried prefix at identical token positions. Transformers.
    AbsolutePrefix,
    /// The state is keyed by its own recurrent stream, not the prompt's token positions. Hybrids.
    RecurrentStream,
}

/// What the on-device cache of this model holds — the invariant the caller must respect when
/// seeding a fresh session. The numeric layer count is a model field (`Model::deltanet_layers()`);
/// this only names the *shape*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateSemantics {
    /// A plain KV cache — a transformer. Re-use is by token prefix.
    KvCache,
    /// Gated DeltaNet hidden-state checkpoints — a pure recurrent model. Re-use is by recurrent
    /// stream at a position-absolute position.
    DeltaNet { layers: usize },
    /// Both: attention blocks write a KV cache, delta layers hold recurrent checkpoints. Re-use
    /// splits across the two — a token prefix *and* a recurrent stream.
    Hybrid { deltanet_layers: usize },
}

/// How a prompt's tokens relate across consecutive turns — what re-use is *allowed* to assume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptSemantics {
    /// Prompt N is Prompt N-1 with tokens appended. The linear-agent loop; the carry-forward case.
    PrefixStable,
    /// Each turn re-renders the prompt (a chat template, re-injected context). Only a shared
    /// *prefix* may be re-used.
    ReRendered,
    /// Only the tail of the prompt is kept (a sliding window). Prefix reuse is limited to what
    /// survived.
    Windowed,
}

impl Archetype {
    /// What the model's KV cache corresponds to — token prefix (Transformer) or recurrent stream
    /// (Hybrid, per [`Archetype::Hybrid`]).
    pub fn context_semantics(&self) -> ContextSemantics {
        match self {
            Archetype::Transformer => ContextSemantics::AbsolutePrefix,
            Archetype::Hybrid | Archetype::Recurrent => ContextSemantics::RecurrentStream,
        }
    }

    /// What the model's on-device cache holds.
    pub fn state_semantics(&self) -> StateSemantics {
        match self {
            Archetype::Transformer => StateSemantics::KvCache,
            Archetype::Recurrent => StateSemantics::DeltaNet { layers: 0 },
            // Only hybrid is currently constructible; the layer count is read off the model.
            Archetype::Hybrid => StateSemantics::Hybrid { deltanet_layers: 0 },
        }
    }

    /// How a prompt's tokens relate across turns — prefix-stable for the linear agent, but this is
    /// where a future non-linear driver (a re-rendered prompt, a windowed tail) would pick a
    /// different semantics.
    pub fn prompt_semantics(&self) -> PromptSemantics {
        PromptSemantics::PrefixStable
    }
}

/// A turn's prompt, seen by the planner — plus, for the turn being carried *from*, the recurrent
/// stream and state generation that state was actually folded over.
///
/// The current turn always has an empty `carried_stream` and a `None` generation — its state has
/// not been built yet. A turn carried *from* has `carried_stream` = the tokens the delta-net layers
/// were folded over (in the engine's linear agent this is the whole previous transcript,
/// `prompt_ids + generated`) and `generation` = the `next_pos` that session's cache ends at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextTurn {
    /// Every token in this turn's prompt.
    pub prompt_ids: Vec<u32>,
    /// For a turn carried *from*: the recurrent stream the delta-net checkpoints were folded over.
    /// Empty on the current turn.
    pub carried_stream: Vec<u32>,
    /// For a turn carried *from*: the `next_pos` the carried state sits at (0 if never folded).
    /// `None` on the current turn.
    pub carried_generation: Option<usize>,
}

impl ContextTurn {
    /// The current turn of generation, before its reply has been folded through the recurrent path.
    pub fn current(prompt_ids: Vec<u32>) -> Self {
        Self { prompt_ids, carried_stream: Vec::new(), carried_generation: None }
    }
    /// A turn carried forward, with the stream + generation its checkpoints were built at.
    pub fn carried(prompt_ids: Vec<u32>, carried_stream: Vec<u32>, generation: usize) -> Self {
        Self { prompt_ids, carried_stream, carried_generation: Some(generation) }
    }
}

/// Where a carried reference frame lives, and how far it reaches.
///
/// * [`PrefixReference::AbsolutePrefix`] — a KV token prefix of length `prefix_len`, valid at
///   identical token positions.
/// * [`PrefixReference::RecurrentStream`] — a recurrent checkpoint folded over `state_generation`
///   tokens of `stream_tokens`, valid iff the new stream embeds those tokens at the same absolute
///   positions (position-absolute). `state_generation` may be < `stream_tokens.len()` when the
///   state was checkpointed mid-stream, which is why the two fields are kept separate rather than
///   implied by a single length.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefixReference {
    AbsolutePrefix(usize),
    RecurrentStream(RecurrentPosition),
}

/// A recurrent checkpoint: the tokens it was folded over, and how many were folded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrentPosition {
    /// The recurrent stream the checkpoints fold over. For a carry this is the previous turn's
    /// carried transcript; the valid region is `0..state_generation`.
    pub stream_tokens: Vec<u32>,
    /// How many tokens of `stream_tokens` the state actually folds over — the `next_pos`.
    pub state_generation: usize,
}

/// Why a carry-forward was refused, so the planner (and its caller's diagnostics) can say *why*
/// rather than just "no".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// No turn was carried forward (first turn, or cleared).
    NoCarriedTurn,
    /// The prompt is shorter than the state's generation — a branch / the fan-out case, where the
    /// shared prefix is too short to serve the checkpoint at its real position.
    PromptTooShort { prompt_len: usize, needed: usize },
    /// The prompt is long enough but diverges from the carried stream before the state ends — the
    /// state would be declared at the wrong absolute position.
    StreamDiverges { at: usize },
}

/// The result of planning how to carry context from one turn to the next.
///
/// Exactly one of [`ContextPlan::kv_prefix`]/[`ContextPlan::recurrent_seed`] is set (both are `None`
/// when nothing may be carried); the other names what the caller should *not* try — for a Hybrid,
/// `kv_prefix` stays `None` because a delta checkpoint is not a KV token prefix, and for a
/// Transformer, `recurrent_seed` stays `None` because there is no recurrent state to seed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextPlan {
    /// Length of the current turn's prompt tokens — how far the prompt reaches in token space.
    pub prompt_position: usize,
    /// For a Transformer: the reused KV token prefix length, if the current prompt embeds a carried
    /// prefix at identical token positions. `None` for a Hybrid (its state is not a KV prefix).
    pub kv_prefix: Option<usize>,
    /// For a Hybrid: the delta-net carry position — fold this turn's state into a fresh session at
    /// `next_pos == seed` and prefill the appended suffix. `None` if the carry is not valid.
    pub recurrent_seed: Option<usize>,
    /// If `recurrent_seed` is `None`, why — for diagnostics.
    pub refused_reason: Option<Refusal>,
}

impl Archetype {
    /// Plan how to carry context from `previous` (the turn being carried from; `None` on a fresh
    /// turn) into `current`.
    ///
    /// This is the ONLY place the carry decision lives now — the backend used to fold it into a
    /// token-equality check and report it as a KV-tree hit. Here it is one question per archetype:
    ///
    /// * **Hybrid** — is the carried delta-net state a valid anchor for `current`? The state is
    ///   position-absolute over `state_generation` tokens of `carried_stream`, so `current` must
    ///   embed exactly those tokens at identical positions (`current.prompt_ids[..state_generation]
    ///   == carried_stream[..state_generation]`) *and* be long enough to reach them. If it does,
    ///   the valid seed is `next_pos == state_generation` and the suffix past it is prefilled fresh.
    ///   Refusal reports *which* invariant broke.
    /// * **Transformer** — the current turn does not carry recurrent state; returns the carried KV
    ///   prefix length if `current` embeds a carried token prefix at identical positions.
    pub fn plan_context(&self, previous: Option<&ContextTurn>, current: &ContextTurn) -> ContextPlan {
        let prompt_tokens = current.prompt_ids.clone();
        let prompt_position = prompt_tokens.len();
        let mut plan = ContextPlan {
            prompt_position,
            kv_prefix: None,
            recurrent_seed: None,
            refused_reason: None,
        };
        match self {
            Archetype::Hybrid | Archetype::Recurrent => plan.plan_hybrid(previous, current),
            Archetype::Transformer => plan.plan_transformer(previous, current),
        }
        plan
    }
}

impl ContextPlan {
    fn plan_hybrid(&mut self, previous: Option<&ContextTurn>, current: &ContextTurn) {
        let Some(prev) = previous else {
            self.recursed_no_carried();
            return;
        };
        // The state was folded over `prev.carried_stream` up to `generation`. Embedding that stream
        // at identical absolute positions is the whole invariant; what trails `generation` is a
        // fresh suffix the state has not seen.
        let generation = prev.carried_generation.unwrap_or(0);
        let stream = &prev.carried_stream;
        let fold = stream.len().min(generation); // guard a never-folded (generation == 0) anchor
        if generation == 0 || fold == 0 {
            // A zero-generation anchor carries no history — nothing to reuse, start fresh.
            self.recursed_no_carried();
            return;
        }
        if current.prompt_ids.len() < generation {
            self.recursed_too_short(generation);
            return;
        }
        let diverge = (0..fold).find(|&i| stream[i] != current.prompt_ids[i]).unwrap_or(fold);
        if diverge != fold {
            self.recursed_diverges(diverge);
            return;
        }
        // Valid: fold into a fresh session at next_pos == generation and prefill the suffix past it.
        self.recurrent_seed = Some(generation);
    }

    fn plan_transformer(&mut self, previous: Option<&ContextTurn>, current: &ContextTurn) {
        let Some(prev) = previous else { return };
        // The current turn embeds the previous prompt as a prefix at identical token positions —
        // the transformer KV cache's only valid reuse. (The actual pool/anchor pick happens in the
        // backend; this reports what the archetype *permits*, which PrefixStable is "whole prefix".)
        let fold = prev.prompt_ids.len().min(current.prompt_ids.len());
        let diverge = (0..fold).find(|&i| prev.prompt_ids[i] != current.prompt_ids[i]).unwrap_or(fold);
        if diverge != fold {
            self.recursed_diverges(diverge);
            return;
        }
        if current.prompt_ids.len() < fold {
            self.recursed_too_short(fold);
            return;
        }
        self.kv_prefix = Some(prev.prompt_ids.len());
    }

    fn recursed_no_carried(&mut self) { self.refused_reason = Some(Refusal::NoCarriedTurn); }
    fn recursed_too_short(&mut self, needed: usize) {
        self.refused_reason = Some(Refusal::PromptTooShort { prompt_len: self.prompt_position, needed });
    }
    fn recursed_diverges(&mut self, at: usize) {
        self.refused_reason = Some(Refusal::StreamDiverges { at });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The linear-agent carry-forward, the case this whole module exists to serve: the current
    /// turn's prompt embeds the previous turn's carried transcript at identical absolute positions
    /// up to the carried `generation` (the delta state's true `next_pos`), with a fresh suffix past
    /// it. `plan_context` must accept it and return the seed at exactly that generation, because the
    /// delta state can be seeded only position-absolute.
    #[test]
    fn hybrid_carries_at_the_carried_generation_and_prefills_the_suffix() {
        // previous turn folded the transcript to next_pos == 10 (its delta state sits there).
        let prev = ContextTurn::carried(vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10], vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10], 10);
        // current turn = the same 10 tokens (identical absolute positions) + an appended suffix.
        let current = ContextTurn::current(vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
        let plan = Archetype::Hybrid.plan_context(Some(&prev), &current);
        assert_eq!(plan.recurrent_seed, Some(10), "the delta state folds 0..10, so it must be seeded at next_pos == 10, not a shorter prefix");
        assert!(plan.recurrent_seed.is_some() && plan.kv_prefix.is_none(), "a Hybrid carries state, not a KV token prefix");
        assert!(plan.refused_reason.is_none());
    }

    /// Position-correctness guard: if the current turn's prompt diverges from the carried stream
    /// *before* the state ends, the state would be installed at the wrong absolute position — the
    /// exact corruption `block_prefill_deltanet_gpu`'s `assert np == pos` exists to prevent. The
    /// planner must refuse and say *where*.
    #[test]
    fn hybrid_refuses_when_the_stream_diverges_before_the_state_ends() {
        let prev = ContextTurn::carried(vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10], vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10], 10);
        // position 3 (0-indexed) differs from the carried transcript — the injected block lives here.
        let current = ContextTurn::current(vec![1, 2, 3, 99, 5, 6, 7, 8, 9, 10, 11, 12]);
        let plan = Archetype::Hybrid.plan_context(Some(&prev), &current);
        assert!(plan.recurrent_seed.is_none());
        assert_eq!(plan.refused_reason, Some(Refusal::StreamDiverges { at: 3 }));
    }

    /// A branch (fan-out) shortens the transcript below the state's generation: the state is
    /// position-absolute at next_pos == 10, but there aren't 10 tokens to reach it, so the state
    /// can't be seeded without a seek. Refuse.
    #[test]
    fn hybrid_refuses_when_the_prompt_is_shorter_than_the_generation() {
        let prev = ContextTurn::carried(vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10], vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10], 10);
        let current = ContextTurn::current(vec![1, 2, 3, 4]);
        let plan = Archetype::Hybrid.plan_context(Some(&prev), &current);
        assert!(plan.recurrent_seed.is_none());
        assert!(matches!(plan.refused_reason, Some(Refusal::PromptTooShort { .. })));
    }

    /// Position-absolute is the point: seeding the delta state at a *shorter* shared prefix (the KV
    /// tree's habit — pick the longest common prefix, which is < the full transcript) is wrong for
    /// recurrent state, which cannot seek. This asserts the archetype seeds at the full generation,
    /// not at the length of whatever prefix the transformer KV tree would have picked.
    #[test]
    fn hybrid_seeds_at_full_generation_not_a_shared_prefix() {
        let prev = ContextTurn::carried(vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10], vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10], 10);
        // The shared prefix is the whole transcript (10 tokens) — a Transformer's pick_anchor would
        // grab exactly this, but here it must be read as the delta *generation*, not a KV length.
        let current = ContextTurn::current(vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        let plan = Archetype::Hybrid.plan_context(Some(&prev), &current);
        assert_eq!(plan.recurrent_seed, Some(10));
    }

    /// A Transformer's KV cache *is* a token prefix, so reuse reports the reused length. This is
    /// the axis the Hybrid carries differently — the two archetypes answer the same question with
    /// different semantics, and the planner must not conflate them.
    #[test]
    fn transformer_reports_the_kv_prefix_length() {
        let prev = ContextTurn::current(vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        let current = ContextTurn::current(vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
        let plan = Archetype::Transformer.plan_context(Some(&prev), &current);
        assert_eq!(plan.kv_prefix, Some(10));
        assert!(plan.recurrent_seed.is_none(), "a Transformer has no recurrent state to carry");
    }

    /// The exact contract the production carry path requires, constructed as the linear-continue
    /// driver would build it and independent of any chat template: the carried stream is `prompt
    /// [..generation]` where `generation == |prefix| + |generated|` is the delta state's true
    /// `next_pos`, and turn 2's prompt is `prefix + generated + suffix` at identical absolute
    /// positions. The success branch is the actual performance feature, so this pins the seed to the
    /// EXACT position — a position-0 regression (the old `mem::replace` mistake where the snapshot
    /// read the swapped-in session's `pos`, not the carried one) would make the seed `Some(0)`, which
    /// `plan_hybrid` then rejects as a zero-generation anchor.
    #[test]
    fn hybrid_carries_when_the_next_turn_repeats_the_carried_stream_exactly() {
        let prefix: Vec<u32> = vec![100, 101, 102, 103, 104]; // 5 tokens
        let generated: Vec<u32> = vec![1, 2, 3, 4, 5, 6, 7, 8]; // 8 tokens
        let generation = prefix.len() + generated.len(); // 13 == the delta state's next_pos
        let carried_stream: Vec<u32> = prefix.iter().chain(generated.iter()).copied().collect();

        // The planner is fed the carried turn (stream + generation) and the next turn, whose prompt
        // is `prefix + generated + suffix` — the identical token stream over 0..generation.
        let previous = ContextTurn::carried(carried_stream.clone(), carried_stream, generation);
        let suffix = vec![9, 10, 11];
        let current = ContextTurn::current(prefix.iter().chain(generated.iter()).chain(suffix.iter()).copied().collect());
        let plan = Archetype::Hybrid.plan_context(Some(&previous), &current);

        // No refusal: the state is reusable.
        assert!(plan.refused_reason.is_none(), "an identical carried stream must not be refused");
        // The seed is the exact position the state was folded to, not any shorter shared prefix —
        // this is the position-0 regression guard.
        assert_eq!(
            plan.recurrent_seed,
            Some(generation),
            "recurrent_seed must equal |prefix| + |generated| (the delta state's next_pos), not a shared-prefix length"
        );
        // Defensive: the carry is at a position that actually reaches the end of the carried stream,
        // i.e. the whole transcript is present and identical.
        assert!(plan.recurrent_seed.map_or(false, |p| p == generated.len() + prefix.len()));
    }

    /// The companion negative case for the same linear-continue invariant: turn 2's prompt shares
    /// `prefix` but its `generated` slot has been tampered with so it diverges at position `N`.
    /// The planner must refuse and name the first divergent token — refusing is what stops a
    /// position-absolute delta state from being installed at the wrong absolute position (the
    /// `assert np == pos` the delta kernel guards against).
    #[test]
    fn hybrid_refuses_when_the_carried_stream_diverges_in_the_mutable_tail() {
        let prefix: Vec<u32> = vec![100, 101, 102, 103, 104];
        let generated: Vec<u32> = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let generation = prefix.len() + generated.len();
        let carried_stream: Vec<u32> = prefix.iter().chain(generated.iter()).copied().collect();

        // Turn 2 repeats prefix exactly, but injects a divergent token at position 6 inside the
        // previously-generated region — exactly where an injected block would land, and it is inside
        // 0..generation, so the carried state would be seeded at the wrong absolute position.
        let mut tampered = carried_stream.clone();
        let diverge_at = 6;
        let original = tampered[diverge_at];
        tampered[diverge_at] = if original == 9 { 8 } else { original + 1 };
        assert_ne!(tampered[diverge_at], original, "test setup: the injected token must actually differ");

        let previous = ContextTurn::carried(carried_stream.clone(), carried_stream, generation);
        let suffix = vec![9, 10];
        let current = ContextTurn::current(
            tampered.iter().chain(suffix.iter()).copied().collect(),
        );
        let plan = Archetype::Hybrid.plan_context(Some(&previous), &current);

        assert!(plan.recurrent_seed.is_none(), "a divergent carried stream must not seed a carry");
        assert_eq!(
            plan.refused_reason,
            Some(Refusal::StreamDiverges { at: diverge_at }),
            "the refusal must name the first divergent token position"
        );
    }
}
