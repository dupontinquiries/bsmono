# Learning example: string-edit fit-to-vocab

This is the algorithm I wrote in school in `kMelodizer.py`. It's preserved here
as a learning artifact, not as the production path. The production code in
`src/dict.rs` does the same job in phoneme space, where it actually belongs.

The runnable port lives in `examples/learning_string_matching.rs`:

```bash
cargo run --example learning_string_matching
```

## The idea

Given a small target vocabulary (e.g. every distinct word from a Juice WRLD
song) and an arbitrary input word, find the closest in-vocab word by trying
three single-edit moves in order:

1. **DROP** — delete one character. Closest dict word obtainable this way?
2. **SWAP** — swap two adjacent characters. Closest dict word?
3. **REPLACE** — replace one character with any a–z letter. Closest dict word?

First hit wins. Ties broken by alphabetical order (the original tie-breaker).

```text
> horse
    horse → hearse        :SWAP

> persn
    persn → person        :DROP

> trough
    trough → through      :REPLACE
```

## Why it works, sort of

For very small vocabularies of phonetically-clustered words it can produce
satisfying nudges. *Horse* and *hearse* share most letters; swapping the `o`
and the `e` in *horse* gets you to *hearse*, which happens to also be
phonetically close.

It works *when English spelling cooperates*. That's the trap.

## Why it's a workaround for sound matching

The whole framing — "fit a word to a melody" — implies the algorithm is doing
something with sound. It isn't. At no point does the algorithm look at, hear,
or reason about phonemes. It edits character strings. The connection to sound
is whatever incidental correlation English orthography has with English
pronunciation, which is famously unreliable:

| Input | What string-edit returns | What sound-matching should return |
|---|---|---|
| `night` | (no edit produces *knight*) | `knight` — phonetically identical, edit distance 0 |
| `through` | likely no match | `thru` — same pronunciation |
| `fone` | no in-vocab match | `phone` — `F OW1 N` either way |
| `kat` | maybe *cat* via REPLACE | `cat` — but only because k → c happened to be in the search space |

Two specific failure modes are worth naming because they show up constantly:

**Length-sensitive misses.** `through → thru` and `phone → fone` are
phonetically identical but character-distant. Drop/swap/replace can't bridge
them because they assume the edit budget is one *character*, when the relevant
edit budget is one *phoneme*.

**Spelling-conspiracy hits.** `cat → cot` is a one-REPLACE win that *also*
happens to be a near-rhyme. `cat → kat` is a one-REPLACE win that's the same
word phonetically. The algorithm can't tell those situations apart — to it,
they're both just "one letter different."

The root cause is that **the algorithm collapsed audio → text → characters
before doing any work**. In the original Python, Sphinx converted speech to a
text string up front, and from then on everything operated on letters. The
"melodizer" name describes an intent the code never executed.

## What the production path does instead

`src/dict.rs` loads CMUdict (a list of ~134,000 English words paired with their
ARPABET phoneme sequences) and operates in phoneme space:

```text
HEARSE  HH ER1 S
CURSE   K ER1 S
NIGHT   N AY1 T
KNIGHT  N AY1 T
THROUGH TH R UW1
THRU    TH R UW1
```

- **`Dict::rhymes(a, b)`** — equal phoneme sequences from the last
  stressed vowel onward. *Hearse / curse* matches (`[ER, S]`). *Night /
  knight* matches.
- **`Dict::phonetic_distance(a, b)`** — Levenshtein, but over phoneme
  sequences. *Through / thru* is distance 0. *Night / knight* is distance 0.
- **`Dict::syllables(word)`** — count vowel phonemes. *Fire* is 2 (`AY1`,
  `ER0`), not the 1 that vowel-cluster counting reports.
- **`Dict::nearest_phonetic(word, vocab)`** — the proper version of fit-to-vocab.
  Phonetic Levenshtein nearest neighbour. Same shape as the kMelodizer chain,
  right substrate.

The production vocab module (`src/vocab.rs`) is dict-first: it tries phoneme
operations and falls back to the character-edit chain *only* for words the
dict doesn't know. The fallback exists because the embedded starter dict is
small (~250 entries); drop in the full `cmudict-0.7b.txt` and you'll see
"fallback path used" approach zero in the debug panel.

## Spectrum of "doing it properly"

The phoneme layer is one step up from string editing. The full ladder:

1. **String rules.** Where kMelodizer (and this example) lives. Edits over
   characters. Workaround for not having phonemes.
2. **Phoneme rules.** Where the production `dict.rs` lives. Same operations
   (edit distance, suffix match, syllable count) over the right substrate.
   Cheap, dependency-light, no models. ~80% of the perceived quality jump for
   ~10% of the work.
3. **Acoustic features.** Real pitch tracking (YIN, autocorrelation, cepstrum)
   on the hum side; formant analysis if you want vowel quality. Replaces RMS
   onset counting with something that knows what a syllable actually is.
4. **Statistical / ML.** CRNN for syllable segmentation; MFCC + DTW for
   matching hummed audio to known phrases. Now the system understands the
   audio rather than measuring proxies for it.
5. **End-to-end neural.** Whisper-class audio encoder plus a language model.
   Different project entirely.

kRapper currently runs at level 2 on the word side and level 1 on the audio
side. Promoting the audio side to level 3 (proper pitch tracking) is the
clear next move — the YIN algorithm fits in maybe 80 lines of Rust and would
turn the hum analysis from "count loud bursts" into "track the pitched parts
of voiced speech."
