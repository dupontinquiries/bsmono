# Perception overhaul — design notes

> **STATUS (built & tested):** all four bricks below are now implemented as pure, tested models.
> (1) localization → `sim/localize.rs`; (2) crouch → `sim/sound.rs` Stance; (3) ambient masking →
> `sim/sound.rs` biome_ambient_loudness/noticed_over_ambient; (4) vision profiles → `sim/eyes.rs`
> EyeProfile/sees + eyesight-gene scaling. What REMAINS is bin/cognition wiring: feed the player's
> crouch state and biome floor into the hearing check, route brains toward the localize() estimate
> instead of the true position, assign EyeProfiles by archetype+genome and run sees() each tick.

Captured from playtest musings. The current hearing model (`sim/sound.rs`) and
sight (`sim/perception_field.rs`, sentinel agents) are a solid base, but several
gaps make creatures feel "telepathic". Each item below is shaped as a *pure,
testable* brick so we can verify it the way we did fall-damage and the
forest-hearing check, then wire it to the bin/world.

## 1. Hearing is emitter-based, not sound-based

**Observed:** animals know *exactly* where the player is rather than estimating
from what they hear; no tolerance, perfect source isolation.

**Cause:** the AI reads the `SoundSource.pos` directly. Real hearing gives a
*noisy bearing + a rough distance* from loudness, blends multiple sources, and
ignores anything under a per-creature threshold.

**Proposed (testable):**
- `HearingAcuity { threshold, bearing_error_rad, range_bias }` per creature.
- `localize(sounds: &[SoundSource], listener, facing, acuity, rng_seed) -> Option<HeardEstimate>`
  returning an *estimated* position: true bearing perturbed by error that grows
  with distance and shrinks with loudness; distance inferred from perceived
  loudness (so it's wrong, especially through occlusion). Multiple sources →
  weight by perceived loudness, pick/aggregate the dominant, not the true one.
- Tests: louder/closer → smaller error; below threshold → `None`; two sources →
  estimate biases toward the louder; estimate ≠ exact emitter pos.
- Wire: brains navigate toward the *estimate*, so a creature investigates the
  wrong spot, loses the trail, etc.

## 2. Crouch / stance does nothing

**Observed:** crouching doesn't reduce walking noise.

**Proposed (testable):** add a stance multiplier to `footstep_loudness*`:
`Stance { Stand=1.0, Crouch≈0.35, Prone≈0.15 }`. `footstep_loudness_staned(...)`
scales the result. Tests: crouch < stand at the same speed/ground; still = 0
regardless. Wire: feed the player's crouch state in the bin.

## 3. Ambient masking not wired to biomes

**Observed:** ambient sound masking may exist but isn't tied to the biome/world.

**Check + build:** confirm whether a masking floor exists; if so, wire a
`biome_ambient_loudness(Biome) -> u8` (a quiet cave vs a noisy rainforest/river)
and require a sound to exceed the local ambient floor (plus the existing
threshold) to be noticed. Tests: a footstep audible in a silent cave is masked
under a loud waterfall biome; ambient floor varies by biome.

## 4. Vision: no falloff / resolution / motion threshold; one-size cones

**Observed:** flying high overhead is still seen; cones feel too wide; no
atmospheric falloff, no minimum apparent size, no motion gating. Want per-eye
profiles that differ by genome — fauna vs aerial hunters (dragons) vs dinos vs
lauscher (near-blind, hears instead), down to some genomes with weak or no eyes.

**Proposed (testable):**
- `EyeProfile { fov_rad, range, acuity (min apparent size), motion_gain, low_light }`
  with presets: `FAUNA`, `AERIAL_HUNTER` (wide range, narrow keen cone, strong
  motion gain), `DINO`, `LAUSCHER` (tiny range — leans on hearing), `EYELESS`
  (range 0).
- `apparent_size(target_radius, distance) -> angular_size`; a target below the
  eye's `acuity` is *not resolved* (too small/far).
- `sees(eye, eye_pos, facing, target_pos, target_speed) -> Detection {Unseen,
  Motion, Clear}` combining: inside FOV cone, within range w/ atmospheric
  falloff, above min apparent size, and motion gating (a *still* target below
  clear-resolution may still be missed; a moving one triggers `Motion`).
- Genome hook: map archetype/genes → `EyeProfile` (lauscher ≈ eyeless, dragon ≈
  aerial hunter), so a high-flying player is invisible to ground fauna but
  spotted by a dragon.
- Tests: out-of-cone unseen; too-far/too-small unseen; motion seen where a
  freeze isn't; lauscher barely sees but a dragon sees a high flier; eyeless
  never sees.

## Sequencing

Cheapest, highest-impact first: **(2) crouch** and **(3) ambient masking** are
small additions to the existing sound model. **(1) localization** and **(4)
vision profiles** are the bigger, more satisfying bricks. All four are pure
models with a thin bin/world wiring step at the end (feed stance, biome, and
flyer height/motion in).
