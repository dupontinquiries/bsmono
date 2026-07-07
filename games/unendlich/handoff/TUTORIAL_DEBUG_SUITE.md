# Training ladder as a bending debug suite (r645)

`sim::tutorial_ladder` is the practice-world tutorial **and** a runnable harness
for the bending harmonisation mechanic — the part that's been hard to poke at by
hand because it's a new system.

## Run a ladder right now (no game build needed)
```
rustc --edition 2021 tools/tutorial_ladder_run.rs -o /tmp/ladder && /tmp/ladder
```
It scripts a player through all seven elements at different pitch offsets /
sustains and prints whether each harmonisation was accepted and how coupling
accrued. Edit the `script` array to probe the mechanic:
- change the cents offset to find the exact ±15-cent edge,
- shorten `sustain` below 3.0 to confirm "couldn't hold the note" rejects,
- repeat an element to watch coupling climb 5% → cap 15%.

Sample output (from the committed script):
```
Fire   target 440Hz  sung 446.4Hz (+25c, 3.0s)  harmonised 0/2  coupling  0%   ← too sharp, correctly rejected
Air    target 528Hz  sung 528.0Hz ( +0c, 1.5s)  harmonised 0/2  coupling  0%   ← on pitch, sustain too short
Storm  target 174Hz  sung 173.2Hz ( -8c, 4.0s)  harmonised 2/2  coupling 10%   ← flat but inside the window
```

## What it validates (the debug value)
- **Pitch match** — `cents_off` / `is_harmonised`: the ±15-cent window and the
  3-second sustain gate. Anchored in tests (octave = 1200c, 4 Hz @ 440 ≈ 15.6c).
- **Coupling accrual** — +5% per harmonisation, hard cap 15% (tutorial). This is
  the number that carries into the main world, so it's the one to trust.
- **No-perfection-gating** — the ladder always completes even if the player never
  harmonises; failure yields 0 coupling, not a block.
- **Independence / unlock** — any element order; Choir guide unlocks after one.

## Wiring into the practice world (engine/bin)
The core is UI-agnostic. In the practice-world instance:
1. Hold a `LadderState` in the tutorial session.
2. Drive `advance(element)` from the beat-completion trigger (arrival, sense,
   voice shape, technique, choir note, harmonisation, what-comes-next).
3. On beats 5–6 (`Beat::uses_choir()`), feed the mic's detected F0 + how long
   it's been held into `attempt_harmonise(element, f0_hz, sustain_s)`. Show the
   green match ring when `cents_off().abs() <= CENTS_TOLERANCE`; fire the
   HARMONISATION MOMENT void shader when `triggered`.
4. On element complete, mark the tile ✓; when `choir_unlocked()`, reveal the
   Chorus Guide tile.
5. Persist `carry_forward_coupling(element)` (0..=0.15) to the profile; it's
   active in the main world immediately.

Element data (`fundamental_hz`, `voice_shape`, `first_technique`, `mentor`) is on
the `Element` enum. Fundamentals are the §7.2 values (Earth 220, Water 293,
Fire 440, Air 528, Storm 174, Brine 396, Information 741). Water/Air/Brine pitches
were the three §7.2 rows not explicitly tied to a named mentor in the doc — adjust
if the canonical assignment differs; they're one line each.

## Not yet modelled (deliberately out of the pure core)
- The **practice-dragon** partial coupling (bands 13–16, 3%) and the Lauscher
  curiosity accumulator — separate systems; the ladder is the harmonisation spine.
- Autocorrelation F0 detection itself (the mic pipeline) — the core takes an
  already-detected frequency so it stays pure and testable.
