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

## Findings from the lost session (recovered from its record)

The three sections below were measured in a session whose commits were lost
in a sandbox reset. The TEXT is recovered from that session's record; the
renders and comparison tables it cites (goat.oneshot-rolls-vs-render.txt,
the Opus render) were not, and were not re-made. Code status is noted per
item. Band levels here are ffmpeg bandpass numbers (see
tools/analysis/README.md: up to ~5 dB off FFT bands in absolute terms).

### 1. After one-shot samplers and rolls at any key (m56)

Measured WITH the importer setting one-shot on its 14 samplers and taking
rolls at any key (4 triplet rolls kept as ratchets, 28 notes still off the
grid -- mostly partial rolls the exact-set rule does not take). Section
levels barely moved: truncation was not the main loss. The 808 is on the
artist's exact hits at 3:04-3:14 (184.75 / 187.5 / 190.25 / 193.0 s) with
the right envelope, ~25-28 dB too quiet. The tape stops are dblue_TapeStop
automation at 1:07.5, 1:08.9, 1:13.1 and 1:14.4 (no counterpart yet).

Code now: the ENGINE's one-shot channel and wire v27 are rebuilt; the
IMPORTER does not set one-shot yet and still takes rolls at key 60 only.

### 2. The "master chain loss" was a render-start effect (m57)

Bypassing the compressor, the multibands OR the limiter each seemed to
restore the 3:04 low end -- because each bypass was a WINDOW render starting
at 3:04, and a window render with the FULL chain matched master-off too.
Only the whole-song render, sliced at 3:04, was ~10 dB short at 63 Hz.
studio-fx recovery tests show the limiter, compressor and multiband all let
go after a loud passage (rebuilt, and strengthened: the recorded version
could not fail for the limiter or the multiband).

Also found since: the importer maps Maximus / Soundgoodizer / Ozone 8 /
Fruity Multiband Compressor to a multiband whose bands are all BYPASSED --
on imported projects those stages do no dynamics at all.

Code now: samplers at FL's default knob play at unity (+5.2 dB, as clips
do) -- rebuilt, keyed on the channel kind.

### 3. The render-start difference: the AUTOTUNE (m58)

Not automation: the importer writes no automation lanes, and none of
Goat's 14 automation clips are applied, so they act the same on a window
and a whole-song render. Bisecting the render START (same 3:04-3:14 slice,
63 Hz): from 0:00 -46.0 dB; from 0:30 -38.7; from 1:40 -37.1; from 2:56
-36.6. Bypassing the master chain or the limiters in a whole-song render
changed nothing. A per-track probe (solo, [184,194) fresh vs from 0 with a
persistent effect cache) isolated ONE track: "voxmel", the Fruity Slicer
vocal through the V insert's Pitcher -> autotune: -32.6 dB fresh, -40.7
from 0:00; with the autotune bypassed, -32.6 both ways.

The autotune LOSES LEVEL WHILE CORRECTING. Fresh at 3:04 it has not locked
and passes the voice dry; from 0:00 it is correcting. Its shifter reads two
taps a FIXED half-window (11 ms) apart under Hann crossfades -- unity for
the window, not for a periodic signal. Steady tones 40 cents off a note
lose 0 to -2.9 dB by pitch (studio-fx realtune level_tests, re-measured
after the rebuild: same numbers); Goat's vocal lost 8 dB. Fix: period-
aligned tap spacing. Likely part of the "barren vocals".

A second, smaller start-dependence remains OUTSIDE the effects (all fx
bypassed: whole-song -32.6 vs window -38.3 dB at 63 Hz), not yet found.
Window renders are not safe stand-ins for the song until it is.
