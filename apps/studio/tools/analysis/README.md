# Scratch analysis (investigation only)

Quick numpy scripts used while chasing import and synth problems against the
artist's renders. NOT part of Studio: the measurements Studio relies on
(timbre / image / body fits, reference comparison) are Rust, tested, and run
without Python (studio_app::synth_fit, examples/compare_reference).

* `bands.py FILE [start dur] [--raw]` -- octave-band levels (63 Hz..8 kHz),
  one decode + one FFT, sharp band edges.
  NOTE: earlier numbers in commit messages / demo READMEs were made with
  ffmpeg `bandpass` filters (gentle 12 dB/oct skirts, which LEAK neighbouring
  energy -- e.g. an 808's ~42 Hz fundamental into the 63 Hz band). The two
  methods disagree by up to ~5 dB absolute; comparisons made WITHIN one
  method remain valid. Measured on Goat's render at 3:04: 63 Hz -23.7
  (bandpass) vs -28.6 dB (FFT). numpy is ~4x faster here.
