# cartoon-allmylife

A synthetic, shapes-only stand-in for the "All My Life" music video, reproducing what the
analysis of the real edit found. 24 s, 320x180, 24 fps, ~250 KB.

- A white-outfit dancer in three places (field, car roof, parking lot).
- Two looks: neutral daylight and the pink split tone.
- Two kaleidoscope shots, a clone shot (three dancers), a glitch transition (13.6 s),
  a fade to the credits (21.8 s).
- A 92 BPM synthetic track: kicks, hats, a pad, a sung line. Some cuts on kicks, some off;
  shots marked on_move end in a surge of the dancer's arms.

`truth.json` is ground truth (generated, not estimated): every cut's frame, kind and
whether it is on a kick or on the move; every shot's place, look and effect; the kicks.

Regenerate: `cargo run -p studio-app --example cartoon_allmylife -- fixtures/cartoon-allmylife`
