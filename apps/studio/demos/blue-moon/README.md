# Blue Moon (feat. A.M.) — demo projects

The artist's own song (rights held by the artist; used here with permission),
from two FL Studio 20.8 zipped loop packages: the full song and the
instrumental. 176 BPM, ~160 bars, 64 channels, 44 playlist tracks, 33 media
files (drum one-shots, the vocal takes, risers, crashes).

What it exercises: layered drums on samplers, 10 synth parts (Omnisphere, FL
Keys) as piano-roll patterns, vocal chains on busses (EQ, limiter, multiband,
reverb, delay, autotune), 21 busses, overlapping clips, long patterns.

## What is here, and what is not

* `*.flp.json` — the inventories `tools/flp_dump.py` read from the `.flp`s.
* `*.import.txt` — exactly what the import approximated or left out. READ IT:
  the synths are STAND-INS (Omnisphere cannot be hosted and its sound is not
  in the file), effects are at Studio's default settings, automation and the
  master chain are not imported, hi-hat rolls faster than 16ths are merged.
* NOT in git: the media (150 MB of the artist's recordings) and the
  `.studio` files that point at it. They are delivered as a zip; to rebuild:

      unzip "Blue_Moon__feat__A_M__.zip" -d media           # the FL package
      flpenv/bin/python tools/flp_dump.py media/untitled.flp > blue-moon.flp.json
      cargo run -p studio-app --example flp_import -- blue-moon.flp.json media "Blue Moon (feat. A.M.)" media/blue-moon.studio

  (`flpenv`: Python 3.10 + pyflp -- see the script's header for why 3.10.)

The instrumental differs from the full song by six playlist tracks switched
off in FL (the vocals); they import muted, not removed.
