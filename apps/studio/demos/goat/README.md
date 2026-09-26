# Goat v5 (2026) -- battle-testing the FL import

FL 24.2, 174 BPM, 6-minute song, 133 MB of WAVs, 12 cut groups, four
Omnisphere stand-ins, Pitcher (D major), two Fruity Slicers, dblue_TapeStop.

Baseline render (current importer, before the fixes below) against the
artist's render (goat.baseline-vs-render.txt): timing 0 ms, onset correlation
0.4-0.7 in many sections -- the arrangement, clips and patterns land. LEVEL:
bars 17-32 +14 dB, bars 81-112 +5..+15 dB, bars 129-224 -5 / -12 dB in
blocks. At 3:06 the artist's mix has -24 dB at 63 Hz and -34 dB at 4-8 kHz;
Studio's is 17 dB short in the lows and 11-13 dB short up top.

Diagnosed:
1. SAMPLER NOTES CUT AT THEIR WRITTEN LENGTH. "808 - Tesla": 48-tick notes
   (0.17 s) on a 1.38 s sample; the snare: length-0 notes -> one step
   (0.086 s). Studio: "the sampler's window IS its gate". FL, with the
   volume envelope off (every Goat sampler), plays the WHOLE sample -- a
   one-shot, cut only by its cut group -- which is what the artist's low end
   shows. Fix (next): a per-channel `one_shot` in the rack, set by the FL
   importer for samplers; Studio's native gate kept for everything else.
2. HAT ROLLS AT KEY 59. The closed hat's 16th-note triplets (8 / 16 ticks
   into 24-tick steps) are written at key 59: pitched notes, not steps, so
   the step-roll detector (key 60) missed them. Fix (next): a roll at any
   key -> one step hit with a ratchet AND a tune offset (key - 60).
Open: the intro's +14 dB (something plays loud that FL has quiet or absent);
two Fruity Slicers play whole recordings (slice map not decoded);
dblue_TapeStop has no counterpart; 14 automation clips not applied.
