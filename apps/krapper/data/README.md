# Pronunciation dictionary

kRapper uses ARPABET phoneme data to do real rhyme matching, syllable counting,
and phonetic word-to-word similarity (see `src/dict.rs`).

## Files

| File | What it is |
|---|---|
| `cmudict_starter.txt` | A curated ~250-word subset, embedded at compile time via `include_str!`. Covers both artist vocabs and the most common English words. |
| `cmudict-0.7b.txt` *(not included)* | The full Carnegie Mellon Pronouncing Dictionary, ~134k entries. Drop it here and kRapper will prefer it at startup. |

## Upgrading to the full dictionary

```bash
cd data/
curl -LO https://raw.githubusercontent.com/cmusphinx/cmudict/master/cmudict-0.7b
mv cmudict-0.7b cmudict-0.7b.txt
```

kRapper checks for `data/cmudict-0.7b.txt` at startup. If found, it's used
*instead of* the embedded starter set. If not, the starter set is used and
words outside it fall back to the vowel-cluster heuristic for syllable
counting and to suffix-matching for rhyme.

## Format

CMUdict format, one entry per line:

```
WORD<two spaces>PHONE PHONE PHONE ...
```

Vowel phonemes carry a stress digit appended: `0` = unstressed, `1` = primary
stress, `2` = secondary. Lines starting with `;;;` are comments.

Example:

```
HEARSE  HH ER1 S
CURSE   K ER1 S
FIRE    F AY1 ER0
```

`HEARSE` and `CURSE` share the rhyme key `[ER, S]` (last stressed vowel onward,
stress digits stripped) → they rhyme. `FIRE` has two vowel phonemes
(`AY1`, `ER0`) → 2 syllables, not the 1 that the old vowel-cluster heuristic
would give.
