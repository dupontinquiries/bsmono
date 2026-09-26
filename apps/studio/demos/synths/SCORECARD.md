# Synth scorecard -- the Poly stand-ins against the artist's renders

For every synth channel that plays ALONE somewhere in a song (no other rack
channel, no audio clip), `examples/synth_score` measures, at those notes:
the TIMBRE error (dB RMS over harmonics 1-8, each floored at -40 dB) and the
LEVEL error, Studio's render against the artist's. "guessed" = the patch
chosen from the channel's name; "fitted" = `examples/fit_synths`, the
harmonic profile measured from the artist's render at the same solo notes.
(Octave bands measure tonal balance, not timbre: Blue Moon's keys were ~2 dB
RMS in bands both ways, which is why this metric exists.)

| song | channel | solo notes | timbre guessed -> fitted | level guessed -> fitted |
|---|---|---|---|---|
| Goat v5 | Omnisphere #3 | 24 | 18.8 -> **1.9 dB** | +4.0 -> -0.9 dB |
| wavydrizzy part 6 | Omnisphere #24 | 24 | 15.6 -> **6.4 dB** | -34.3 -> -30.3 dB (!) |
| Blue Moon | FL Keys | 11 | 3.5 -> 3.7 dB | +6.9 -> +7.9 dB |
| Red Roses v2 | -- | none alone | -- | -- |
| pianovibes v5 | -- | none alone | -- | -- |
| Balance | -- | none alone | -- | -- |
| A Top A Tower | -- | (not finished) | -- | -- |

Reading it:
* Where a synth plays alone, FITTING WORKS: Goat's 18.8 -> 1.9 dB is near a
  match (level within a dB); wavydrizzy's 15.6 -> 6.4.
* Blue Moon's keys gain nothing: the guessed "keys" patch was already close
  -- the artist also heard it as the closest.
* wavydrizzy's LEVEL at those notes is -30..-34 dB: Studio's render is nearly
  silent there -- a routing / duck / mute problem on that channel, not timbre.
  Investigate first.
* Four of seven songs have NO synth playing alone: coverage needs separating
  layered parts (measure one channel's harmonics away from the others'
  sounding notes' harmonics).
