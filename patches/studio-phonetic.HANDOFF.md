# Handoff — studio-app `phonetic.rs` (orphaned port, caller lost)

`studio-phonetic.rs` is `apps/studio/crates/studio-app/src/phonetic.rs` as it
sat on disk at the end of the studio-live session: the voiceterm-qt
`phonetic_confusions` port — a retry pass for voice commands a recognizer
mis-heard. 349 lines, std-only (`HashMap`, `OnceLock`), 10 unit tests.

**Why it is here and not in the crate.** The session's own README is explicit:
the `voice.rs` integration into `Dialog::hear` that gave this file a caller was
lost (a sandbox reset between tool-call batches; confirmed gone by grepping for
its markers, not just by `git diff`), together with the keymap-wiring and
`export.rs` overflow fixes from the same session. What survived is this file,
with no `mod phonetic;` and no call site. Committing it into `src/` as-is
would trip `orphan_check.py` and compile to nothing.

**To land it:**
1. `cp patches/studio-phonetic.rs apps/studio/crates/studio-app/src/phonetic.rs`
2. `#[allow(dead_code)] mod phonetic;` in studio-app's `lib.rs`, run its 10
   tests: `CARGO_BUILD_JOBS=1 cargo test -p studio-app phonetic`.
3. Re-do the `Dialog::hear` retry wiring in `voice.rs` — the README names it as
   "better done as an explicit next task", and there is no copy to recover.

The lost keymap work (`ToggleRecord`, `SetGain`, `ToggleLoop`, `Yank`/`Paste`,
the exhaustive-match fix, the mixer peer-presence indicator, the `in_rack`
spacebar gating) and the three `export.rs` `saturating_*` fixes are NOT here
and have no source anywhere. Redo, don't recover.
