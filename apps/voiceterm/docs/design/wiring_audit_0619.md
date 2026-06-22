# Wiring audit — v0.1.619 (recent features + voice pipeline)

Question: are the pure cores built this session actually WIRED into the
app, or sitting unrouted? And is the voice pipeline intact end to end?

## Recent pure cores — wired vs. build-gated (all accounted for)

| feature (ver) | called in app? | status |
|---|---|---|
| sysmon (v602) | YES — system_info.rs (5 sites) | wired |
| selection::extract_selection (v611) | YES — main.rs term_selection_text delegates | wired |
| sandbox::shell_quote (v619) | YES — main.rs shell_quote delegates | wired |
| phonetic::candidate_bins (v612) | YES — phonetic.rs:933 in top_matches | wired |
| decision_cache (v613 probe) | YES — lookup 13768 + record 10562/11645 | wired (both sides) |
| parse_fact_request (v603) | NO | build-gated (documented) |
| ui_landmarks (v601) | NO | build-gated (documented) |
| notebook (v608) | NO | build-gated (documented) |
| motion_history (v609) | NO | build-gated (documented) |
| selection::highlight_row_span (v611) | NO | build-gated (documented) |
| phonetic_confusions::phrase_variants (v615) | NO | build-gated (documented) |
| search_suggest::suggest_commands (v600) | NO | build-gated (documented) |

**Conclusion: nothing is accidentally unrouted.** Every uncalled core is
explicitly documented build-gated in its TODO (the wiring point is FFI in
main.rs or QML, unverifiable in-sandbox). The cores that I claimed were
wired ARE wired, confirmed by call-site.

## Voice pipeline trace (intact end to end)

1. **Audio capture** — persistent CPAL stream (voice.rs), 44.1kHz→16k.
2. **Acoustic match** — `phonetic::top_matches_with_hint` (main.rs:15143)
   → `top_matches_inner` (phonetic.rs:777) → `candidate_bins` (933)
   pitch-bin prefilter (v612, WIRED) → DTW over the pruned set. Wake-word
   fingerprints always included.
3. **Wake / transcribe** — whisper Base; wake-word acoustic gate before
   full transcription.
4. **Intent parse** — `voice_parse_candidates` (FFI, main.rs:12893),
   layered:
   - smart-descend / go-home / favorites exact-match (surface-only)
   - workspace + create-dir intents
   - `voice_intent::parse_candidates` generic heuristic (13753)
   - `decision_cache::lookup` (13768, WIRED) — zero-latency repeat
     commands, score scales with confirmation count (CONFIDENT_THRESHOLD)
   - PATH-binary scan (named-tool detection)
5. **Execution** — QML executeIntent runs the chosen Candidate; sentinel
   intents (\x01FAVORITES\x01 etc.) route to UI.
6. **Learn** — `decision_cache::record` (10562) on confirm,
   `record_rejection` (11645) on reject. The cache read in step 4 is fed
   by this.

Also wired: editor_voice (helix match-mode, v599 — 4 sites),
speaker_claim (turn-hijack guard, v598 — 2 sites).

## Where the build-gated voice features WOULD slot in (for when building)
- **phrase_variants retry**: after `parse_candidates` (13753) returns no
  confident candidate, retry parse on phrase_variants(user_text, ~6).
  TODO_phonetic_confusions_wiring.md.
- **parse_fact_request**: as an early layer in voice_parse_candidates
  (context-sensitive, before generic parse). TODO_recent_fact_lookup.md.
- **manpage suggest**: search_suggest::suggest_commands into the
  command-help UI. TODO_manpage_suggestions.md.

These are real, small, well-specified call insertions — each gated only
because the insertion point is unverifiable-in-sandbox FFI/QML.

## QML↔Rust divergence hunt (follow-up to the v0.1.618 label + v0.1.619 shell_quote finds)
Checked the remaining QML JS helpers that could duplicate Rust logic:
- **luminance** (QML hexLuminance + the selection-alpha check at ~5091
  vs Rust system_theme::rgb_luma): SAME Rec.601 coefficients
  (0.299/0.587/0.114). QML returns 0..255 (threshold <128), Rust returns
  0..1 (threshold <0.5) — 128/255≈0.502, so the dark/light decision
  AGREES. Not divergent. Fixed only a mislabeled "Rec.709" comment at
  ~5091 (the code is Rec.601).
- **_entryMatches / _contextMatches** (chord-overlay search): QML-local
  UI filtering of the shortcut cheatsheet; no Rust counterpart, nothing
  to diverge.
Conclusion: the divergence vein produced one real bug (shell_quote,
v0.1.619) + one label fix (v0.1.618) + one comment correction. Now
substantially tapped — the remaining QML helpers are genuinely QML-local.
