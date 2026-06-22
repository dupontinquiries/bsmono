# Voice pipeline internals

Deep dive into the phonetic-side machinery that complements whisper.
For the high-level voice flow, see [pipeline.md](pipeline.md).

## Phonetic fingerprint DB

`src/phonetic.rs` and `~/.config/web2qml/voice_fingerprints.bin`.

Each fingerprint is:

```
struct Fingerprint {
    label: String,        // what command this represents
    mfcc: Vec<f32>,       // n_frames × N_MFCC floats (typically 13 per frame)
    n_frames: usize,
    profile: TonalProfile,
    created_at: u64,
    last_matched_at: u64,
    match_count: u32,
    style_id: u8,         // 0..5 (see Glossary)
}
```

Storage: binary file, magic `FP04`, version-tagged so older formats
(FP01, FP02, FP03) load forward-compatibly with defaults filled in.

Capacity:
- `MAX_PER_LABEL = 6` — six fingerprints per command label
- `MAX_PER_STYLE = 2` — two per style within each label
- `TOTAL_CAP = 1000` — global ceiling

Eviction: when at per-style cap for a new fingerprint, drop worst of
same label+style (lowest match_count, oldest created_at). When at
per-label cap but per-style room exists, drop worst of same label
regardless of style.

## Matcher

`top_matches_with_hint(audio_16k, n, pitch_hint) -> Vec<(label, score)>`

Steps:

1. Compute MFCC for query audio
2. Compute query TonalProfile
3. Apply pitch hint if provided (overrides query-derived pitch
   estimate; useful for filtering by speaker's typical pitch range)
4. Filter candidate fingerprints by pitch bin (~150Hz range around
   query pitch)
5. For each candidate, compute DTW distance between query MFCC and
   candidate MFCC
6. Normalize and rank
7. Apply tonal-profile similarity penalty (lower for different
   speakers)
8. Apply same-style bonus (5% better score when query and candidate
   both have non-default style and match)
9. Sort, truncate to n, mark winner's `match_count`

## Confusion table

`src/confusion_table.rs` and `~/.config/web2qml/confusion_table.bin`.

Learns when whisper says X but the user actually meant Y. Format:
`whisper_text → correction (count, last_used)`.

Triggered when user picks a non-whisper alternative from candidates.
The correction is applied to future whisper outputs before intent
parsing.

Capped at 500 entries with LRU eviction.

## Static phonetic confusions

`src/phonetic_confusions.rs` + `data/phonetic_confusions.toml`.

Hardcoded groups of words that whisper commonly confuses. E.g.:

```toml
[[group]]
words = ["ls", "less", "list"]

[[group]]
words = ["cd", "seedy", "see d", "city"]
```

When phonetic matching finds a winner, if the winner is in a group,
all members are considered "near misses" and surface as candidates.

## Token stats

`src/token_stats.rs` and `~/.config/web2qml/tokens.bin`.

Tracks per-token statistics across confirmed/rejected utterances:

- **SEED_FILLERS** — built-in list (um, uh, please, basically, ...).
  Universal; works on day one.
- **Learned fillers** — tokens that, when present vs absent, don't
  change the parsed intent. Empirically derived from utterance
  history.

Both lists are stripped before intent parsing.

The empirical filler test: for each candidate token, look at all
recent utterances containing it. Compute the intent distribution
with the token present vs with it removed. If the distributions
are essentially identical, the token is a filler.

## Decision cache

`src/decision_cache.rs` and `~/.config/web2qml/decision_cache.bin`.

Maps confirmed phrase → executed command. After "show files" was
voiced and confirmed to mean "ls -lh", the next "show files" hits
the cache and resolves instantly without re-classification.

Score is based on count + recency. Threshold 0.55 to apply the
cached decision; below threshold falls through to normal parsing.

## Audio profile

`src/audio_profile.rs` and `~/.config/web2qml/audio_profile_v2.bin`.

Single-speaker profile (the "primary user"). Fields:
- noise_floor (RMS during silence)
- speech_rms (median during speech frames)
- pitch_hint (typical fundamental)
- wake-word fingerprints

Calibrated continuously from background ring buffer samples.
Periodically saved (~every 30s when changes accumulate).

This is distinct from the speaker REGISTRY (`speakers.toml`) which
holds multiple registered users for multi-user mode. The
audio_profile is the foreground speaker's running characterization,
used for VAD calibration and pitch-hint matching.

## Background fingerprint enrichment

`background_listener` thread in `voice.rs`. Pulls from BgRingBuf
during idle moments (no foreground recording). If audio passes basic
quality check (sufficient RMS, voicing), runs phonetic match
against the DB. High-confidence matches feed into the audio_profile
update cycle.

This is how the system "learns" the user's voice during normal use
without explicit training mode.

## Cross-checks at finalization

When whisper finalizes an utterance:

1. Phonetic match runs on the same audio
2. If phonetic best-match score is high AND its label differs from
   whisper's text, surface BOTH as candidates
3. User picks one; choice gets recorded in confusion_table

This is the user-driven correction loop. Each correction teaches the
system one new mistranscription pattern.

## Persistence policy

Default: session-only learning (nothing written to disk).
Opt-in: `WEB2QML_PERSIST_LEARNING=1` enables disk writes.

Files written when persistence enabled:
- `voice_fingerprints.bin`
- `audio_profile_v2.bin`
- `tokens.bin`
- `confusion_table.bin`
- `decision_cache.bin`
- `llm_cache.bin`

Read on startup regardless of the env var (you can always load
existing data; the flag controls whether new data is saved).

## Where to tune

| What | Where | Notes |
|------|-------|-------|
| Phonetic match threshold | `phonetic.rs` constants + per-callsite | 0.55 in preview (permissive), 0.70+ in cross-check |
| MAX_PER_LABEL | `phonetic.rs:56` | Currently 6 |
| MAX_PER_STYLE | `phonetic.rs:201` | Currently 2 |
| VAD silence duration | `voice.rs` | Currently 825ms |
| Speaker similarity threshold | `speakers.rs:identify` | Currently 0.65 |
| Style classification thresholds | `phonetic.rs:classify_style` | Hand-tuned, conservative |
