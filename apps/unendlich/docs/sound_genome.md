# Sound genome & the mimic horror — design notes

Captured from playtest musings. Both depend on the **sound physics** layer
maturing first (we have `sim/sound.rs` hearing + masking + localization; what's
missing is actual *voice synthesis* and the audio pipeline). Sketched here as
pure-testable models so the synthesis/codec layer is a thin shell, the way the
creature viewer sits on a tested model.

## 1. Voice genome — diverse but always recognizable

Goal: every creature of a species sounds like that species, but no two
individuals sound identical — and a parent's voice is heritable. Mirror the
diploid [`genetics`] model: a **species base voice** plus a per-individual
**voice genome** that perturbs it.

Proposed (pure, testable):
- `VoiceGene` traits, each a diploid [`Gene`] like the body genome: `pitch`
  (fundamental), `formant` (timbre / "vowel" colour), `roughness`/`saturation`
  (clean → growly — maps to a waveshaper/drive amount), `breathiness` (noise
  mix), `tempo` (call rate), `sustain`. Each expresses to `[0,1]` via the same
  dominance/activator/epigenetic machinery, so breeding blends voices and
  environment (e.g. fear, exhaustion) can shift them.
- `SpeciesVoice` — the base preset per `CreatureKind` (a dragon's register vs a
  fauna's), giving the recognizable identity. `VoiceProfile = SpeciesVoice
  perturbed by the individual's VoiceGene`, bounded so it never leaves the
  species' envelope (recognizable).
- A deterministic `voice_signature(VoiceProfile) -> [params]` the synth consumes.
  The Opus/codec + waveshaper ("saturation and speaker characteristics") and the
  actual DSP are the engine shell; the gene→param mapping is the tested part.
- Tests: same genome → same voice; two genomes differ but stay within the
  species envelope; breeding blends parents; an env switch (fear) shifts
  roughness up.

## 2. The mimic — a horror predator that wears its prey's voices

Goal (future): a horror creature — a new one, or an evolved Lauscher — that, as
it eats animals, **ingests their voice genes** and, while hunting the player,
**recombines them in a twisted way**: fragments of familiar animal calls,
pitch-shifted/time-smeared/cross-bred into something almost-recognizable and
wrong, to unsettle the player and mask its approach.

Proposed (pure, testable, once the voice genome exists):
- A `MimicRepertoire`: a bounded collection of `VoiceGene` fragments harvested
  from prey (eat a creature → absorb a sample of its voice genome).
- `twisted_mix(repertoire, seed) -> VoiceProfile`: cross + mutate stored
  fragments with deliberate "uncanny" operators — partial pitch inversion,
  formant mismatch, stutter — producing a call that's a recognizable-but-warped
  chimera of things the player has heard. Bounded so it's eerie, not noise.
- Hunting behaviour ties to the existing model: it stays quiet (low footstep
  loudness / stance), uses **localization** to track the player by sound, and
  emits mimic calls from the *wrong* direction to misdirect — exploiting the
  noisy-bearing model so the player mis-locates it too.
- Tests: a fresh mimic has no voice (nothing eaten); after absorbing prey it can
  emit; the mix is deterministic per seed; the output stays within an "uncanny"
  band (recognizable fragments present, but distorted beyond the source).

## Dependency order

1. **Sound physics / synthesis pipeline** (engine) — voices actually audible.
2. **Voice genome** (1 above) — the tested gene→param model.
3. **The mimic** (2 above) — builds on both.

The voice genome is the next pure brick whenever we turn to audio; the mimic is
the payoff after.
