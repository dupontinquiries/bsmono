# Tuning presets

Hand-tuned settings from play sessions, kept as save files. To use one:

    cp presets/rl-accurate-v5.save.bkp skydribble.save

(back up your own `skydribble.save` first — the game rewrites it on exit).

- `rl-accurate*.save.bkp` — successive passes at a Rocket-League-like feel;
  v5 is the latest.
- `rl-tuned.save.bkp` — the working save those passes ended at.
- `myconfig.save.bkp` — a personal setup.

The physics changes that came out of this tuning are in the engine itself
(see the v1.51 entry in DESIGN.md); these files carry the *settings* half:
camera, mutators, and handling values.
