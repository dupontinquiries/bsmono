# Layered corpus model: vocabulary vs identity vs provisional users

Status: **DESIGN ONLY (v0.1.497).** No code yet — this names a real
architectural VOID the user identified and specs the layering before
anything calcifies a wrong abstraction. Confirmed by reading the
code: the layer described here is largely nonexistent (not just
unwired).

## The insight (user, verbatim intent)

- **Vocabulary is shareable; it is NOT identity.** Two different
  people can share the same vocabulary (both say "deploy staging")
  without being the same person. The current model conflates these.
- Properly handling this needs **an additional abstraction layer**:
  temporary/provisional user buffering, dynamic handling of "little
  flakes" and new corpus entries, and an **intermediate organizing
  layer** that cross-references with MORE scrutiny in some ways and
  DIFFERENTLY from a big pass over the whole corpus.
- **The corpus as a whole** has potential to be updated/flagged in
  ways that refine BOTH general language AND individual users over
  time. Is that code present / unwired / a void? → Mostly a VOID.

## What exists today (ground truth, v0.1.496)

- `phonetic::FP_DB`: one FLAT global `Vec<Fingerprint>`. Each
  Fingerprint = {label (text/vocabulary), mfcc, profile (tonal),
  polarity, match_count, …}. **No owner/speaker field.** A
  fingerprint is "this audio → this text", speaker-agnostic in
  storage but identity-entangled at match time (matching weighs the
  tonal profile).
- `speakers::Speaker`: CONFIRMED named users (profile, permissions,
  cadence). The "individual" layer — but ONLY for registered people.
- `speaker_claim`: lets a user OVERRIDE identity by voice when
  acoustic ID is uncertain. Not a storage layer.

### The three things that are VOID

1. **General / shared vocabulary layer.** Vocabulary can't be "owned
   by everyone." A phrase lives on whatever fingerprint(s) carry it,
   so a shared phrase gets attributed to the first speaker.
2. **Provisional / temporary user layer.** An unattributed utterance
   has nowhere to live except forced into a confirmed speaker or
   dropped. No holding buffer, no scrutinized promotion path.
3. **Corpus-wide refinement pass.** Everything is per-utterance,
   online, greedy. Nothing periodically re-examines the whole pool to
   refine general language or split/merge/reattribute as evidence
   accumulates.

## Proposed layering

Three SCOPES for a corpus entry, plus a periodic reconciler:

### Scope 1 — General language (shared, speaker-agnostic)
Vocabulary that belongs to no one: command phrases, common
intents. Keyed by normalized text. May aggregate MANY speakers'
renditions of the same phrase (improves recognition for everyone,
including brand-new speakers). This is where "two people share the
same vocabulary" is represented correctly: the PHRASE is shared; the
voice renditions are tagged but the phrase isn't owned.

### Scope 2 — Individual (confirmed speaker)
A registered Speaker's personal patterns: their renditions, their
idiosyncratic phrasings, their permission context. Promoted FROM the
provisional layer once identity is confident. Refined over time.

### Scope 3 — Provisional / temporary user (the missing buffer)
A short-lived holding area for utterances we can't yet confidently
attribute. Key properties:
  - **Loose assumptions, heavy scrutiny on PROMOTION.** Cheap to
    create a temp user; expensive (high bar) to promote one to a
    confirmed Speaker or merge into an existing one.
  - **"Little flakes":** one-off utterances, noisy captures, a
    maybe-new-person. They accumulate here, cross-referenced against
    each other and against confirmed speakers, WITHOUT polluting
    Scope 2.
  - **TTL / decay:** a temp user that never gathers enough
    corroborating evidence ages out (so the buffer doesn't grow
    unbounded). This is the "dynamic handling" the user described.
  - Promotion paths: temp → confirmed (enough corroborating
    evidence + a name), temp → merged-into-existing (high-confidence
    fingerprint match, NOT just tonal — see v0.1.496's documented
    limit), temp → discarded (decayed).

### The reconciler — two DIFFERENT cadences (the user's key point)
The user explicitly distinguished intermediate cross-referencing
from "a huge pass over the corpus as a whole." So TWO mechanisms:

  - **Intermediate (frequent, narrow, high-scrutiny):** runs as temp
    users accumulate. Cross-references the provisional buffer against
    itself and confirmed speakers. Decides: corroborate, promote,
    merge, or hold. Scoped, cheap, conservative — bias toward holding
    over wrong promotion (mirrors v0.1.496's "refuse to guess").
  - **Corpus-wide (infrequent, broad, offline-ish):** a slow careful
    pass over EVERYTHING. Refines general-language models, recomputes
    speaker profiles from accumulated evidence, can SPLIT a speaker
    that was wrongly merged (the v0.1.496 bug's cleanup path) or
    merge true duplicates. Different from the intermediate pass:
    operates on the whole, can make structural changes, runs rarely.

## Why this is the right fix for the v0.1.496 limit

v0.1.496 reduced accidental merges (sharper similarity + margin
guard) but documented that pitch alone can't separate same-pitch
speakers, and that the real fix is fingerprint-based identify. This
layered model is the HOME for that fix: provisional buffering means
an uncertain utterance doesn't have to be attributed immediately —
it waits in Scope 3 until the fingerprint evidence (or a speaker
claim, or corroboration) is strong enough. The greedy per-utterance
attribution that caused merges is replaced by accumulate-then-promote.

## Sequencing (when built — NOT now)

1. Add an `owner` discriminant to corpus entries:
   `General | Confirmed(speaker_id) | Provisional(temp_id)`. Purely
   additive to Fingerprint; existing entries default to a scope.
2. Provisional buffer: temp-user creation, TTL/decay, the
   accumulate path. Pure, testable.
3. Intermediate reconciler: the cross-reference/promote/hold logic.
   Pure decision function, heavily testable (this is where the
   scrutiny lives).
4. Corpus-wide pass: the slow structural reconcile (split/merge,
   general-language refinement).
5. Rewire identify() to consult scopes (general as pre-filter,
   provisional as holding, confirmed as the answer) + fingerprint
   match per the v0.1.496 note.

Each stage is independently testable as pure logic. None should be
rushed; building stage 1's discriminant wrong would calcify the
whole model. Co-design the promotion thresholds with real usage.

## Open questions for the user

- Should General vocabulary auto-aggregate across ALL speakers, or
  only across CONFIRMED ones (privacy: a guest's phrasing entering
  everyone's shared model)?
- Promotion bar for temp → confirmed: evidence count? explicit name
  claim required? time window?
- Decay policy for provisional entries: how long is a "flake" kept
  before it ages out?
