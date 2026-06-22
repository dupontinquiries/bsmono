# TODO: wire phonetic_confusions into voice intent-retry

## Background
phonetic_confusions was the ONE genuinely unrouted module (flagged unwired
since v0.1.194). It maps words whisper commonly mis-hears to alternates
(curated in data/phonetic_confusions.toml), e.g. ls↔less↔list,
grep↔great↔grip, cd↔seedy↔city. The idea: when a voice command fails to
match an intent because whisper mis-heard a word, retry with phonetic
alternates.

## Shipped (pure, tested — v0.1.615)
- `alternates_for(word) -> &[String]` (pre-existing) — alternates for one
  word.
- `phrase_variants(phrase, max) -> Vec<String>` (NEW) — single-substitution
  phrase expansion: swaps ONE confused word at a time, bounded, original
  excluded, no-op substitutions skipped. 5 tests. This is the exact
  transform the retry path calls.

## Wiring (build-gated — voice_parse_intent is in main.rs FFI)
In `voice_parse_intent` (main.rs ~2296) — the qt_method that turns a
transcribed phrase into an intent:
1. Try the normal intent match on the raw phrase (as today).
2. If it returns NO confident intent, call
   `phonetic_confusions::phrase_variants(phrase, N)` (N ~5-8) and try the
   normal intent match on each variant in order.
3. First variant that yields a confident intent wins; surface it (and
   optionally note "heard X, matched Y" for the user).
4. If none match, fall back to current no-match behavior.

Keep it CHEAP + bounded: only runs on a no-match (not the hot path), N
small, single-substitution (already bounded in phrase_variants). Don't
let it override a confident first-pass match.

### Why not auto-wired here
voice_parse_intent is FFI in the unbuildable main.rs and the voice
pipeline is a hot, hard-to-test path. The pure transform is done +
tested; the wiring is a small call but needs the running build to verify
it doesn't regress normal matching or add latency. Test on-device: say a
command whisper tends to mis-hear (e.g. it transcribes "great" for
"grep") and confirm the retry now resolves it.

## Optional follow-ups
- Telemetry: count how often a variant (vs the raw phrase) produced the
  winning match — tells you whether the confusion data is earning its
  keep (the v0.1.194 note's "6 months without use → reconsider").
- Let the magic-profile fingerprint DB (TODO_fingerprint_db) feed
  user-specific confusions back into the TOML overlay.

## v0.1.620 — WIRED (needs on-device confirmation)
Inserted the retry at the documented point (voice_parse_candidates, after
voice_intent::parse_candidates, main.rs ~13753). Behavior:
- only fires when the best candidate scores < 0.55 (weak/web-search-only)
- tries phrase_variants(user_text, 6), re-parses each
- merges any candidate scoring > best+0.05; re-sorts by score
- additive only (never removes/reorders originals' relative standing)
The merge/sort logic was verified in isolation (simulated "get status" →
recovers "git status" strong match). phrase_variants itself is tested.
REMAINING: on-device confirmation that (a) it actually recovers real
mis-hears and (b) adds no perceptible latency on the common (confident)
path — it shouldn't, since it's gated on a weak parse. If latency shows,
lower the variant cap from 6.
