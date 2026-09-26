# Recording an instrument for Studio (piano, guitar, …)

Studio's instruments are GENERATED where math is enough (`SynthKind::Poly`,
patches of ~60 bytes) and SAMPLED only where it is not -- piano and guitar.
The goal is a whole instrument in single-digit megabytes (a phone, a 500 MB
upload limit), not a 50-100 GB library. This is what to record so that works.

## What to record

* **Notes:** one every 3 semitones (a minor third): C, D#, F#, A in every
  octave you want. Three octaves covers most parts; the notes in between are
  pitched from the nearest recording (at most 1.5 semitones away, where
  resampling still sounds natural).
* **Two velocities:** soft and hard. Studio crossfades between them by note
  velocity. (A third, medium, helps a piano; optional.)
* **Length:** hold each note ~2-3 s, then let it ring out to silence. The
  tail can be shortened or generated later; a cut tail cannot be restored.
* **Guitar extras (optional):** a few muted / palm-muted notes, and one
  slide or bend, if the parts use them.

About 3 octaves x 4 notes x 2 velocities = ~24-50 files per instrument.

## How

* One note per file, starting at the note's attack (trim silence before it;
  a few ms of pre-roll is fine).
* Mono is fine for guitar; stereo for piano if you like its image.
* Same mic position, gain and room for every note of an instrument -- the
  instrument is one recording session, not a collection.
* No effects (no reverb, no compression): Studio adds those at the bus.

## Format and names

* **Opus 192 kbit/s** (or WAV/FLAC -- Studio converts). 192k Opus keeps an
  instrument around 5 MB.
* Name each file `<instrument>_<note><octave>_<velocity>.opus`, e.g.
  `piano_C4_soft.opus`, `piano_D#4_hard.opus`, `guitar_F#2_soft.opus`
  (C4 = middle C = MIDI 60).
* Put them in one folder per instrument and send the folder (zip is fine).

## What Studio will do with them (planned, not built yet)

A sampled instrument kind that picks the nearest recorded note and velocity,
pitch-shifts it the rest of the way, crossfades velocities, and generates the
release tail where a recording is shorter than a held note. The sample set is
stored ONCE in the library's content-addressed media store (flp-batch
--store), so every project using the piano costs nothing extra.
