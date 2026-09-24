# Blue Moon (feat. A.M.) — demo projects

The artist's own song (rights held by the artist; used here with permission),
from two FL Studio 20.8 zipped loop packages: the full song and the
instrumental. 176 BPM, ~160 bars, 64 channels, 44 playlist tracks, 33 media
files (drum one-shots, the vocal takes, risers, crashes).

What it exercises: layered drums on samplers, 10 synth parts (Omnisphere, FL
Keys) as piano-roll patterns, vocal chains on busses (EQ, limiter, multiband,
reverb, delay, autotune), 21 busses, overlapping clips, long patterns.

## What is here, and what is not

* `*.flp.json` — the inventories the native reader (`flp_read`) takes from
  the `.flp`s: channels, patterns and notes, the playlist, and the MIXER with
  its routing (which insert feeds which, at what send level) and every
  effect slot.
* `*.import.txt` — exactly what the import approximated or left out. READ IT:
  the synths are STAND-INS (Omnisphere cannot be hosted and its sound is not
  in the file -- the notes are all there, for an in-house synth to play),
  effects are at Studio's default settings, automation and the master chain
  are not imported, insert pans are not kept (Studio busses have none).
* The mix: 27 busses, 12 feeding other busses, 6 sends at FL's levels.
* NOT in git: the media -- the artist's recordings, as 48 kbit/s Opus
  (92 MB of WAV -> 1.4 MB) -- and the `.studio` files that point at it. They
  ship as a zip. To rebuild from the FL package:

      unzip "Blue_Moon__feat__A_M__.zip" -d media
      for f in media/*.wav; do ffmpeg -i "$f" -c:a libopus -b:a 48k "${f%.wav}.opus"; done
      cargo run -p studio-app --example flp_import -- media/untitled.flp media "Blue Moon (feat. A.M.)" media/blue-moon.studio

No Python needed: the `.flp` is read natively. `tools/flp_dump.py` (PyFLP)
is kept as a cross-check -- it agreed with the native reader on every
channel, note, playlist item and fader, and disagreed on the effect slots,
where PyFLP is wrong (see `flp_read`'s docs).

The instrumental differs from the full song by six playlist tracks switched
off in FL (the vocals); they import muted, not removed.
