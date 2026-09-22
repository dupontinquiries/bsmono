# TESTING — fixtures, gates, and what counts as tested

Read the repo's `CLAUDE.md` §2 (build one crate at a time) and §5b (a test
that has never failed has not been tested) before changing Studio's code.

## Fixture tiers: no footage in the repo

The film's proxies are 355 MB. Even micro-proxies (96×54, 6 fps, 12 kbps
Opus) measured about 23 MB for the whole shoot. Neither goes in git.

| tier | what | size in repo | where it runs |
|---|---|---|---|
| 1. Generated | `studio_video::testkit::shoot()`: camera takes and recorder files built from seeds, cached in the temp dir | 0 bytes (it is code) | every `cargo test` |
| 2. Features of real footage | onset curves of the real shoot (`examples/onset_pack`), 100 bytes/second, not playable as speech | 670 KB xz for the whole film. Keep it OUT of git (a release asset or LFS) unless the team decides otherwise | opt-in, see below |
| 3. Real micro-proxies | downsized real clips for picture tests | ~23 MB | outside the repo, fetched by hash |

**Tier 2 check.** The packed film re-pairs to the identical 47 Sure pairs and
offsets as the full-precision run, so it is a valid regression fixture for
pairing:
```
cargo run --release -p studio-video --example onset_pack -- match film.onsets
# expect: 47 sure of 75
```

**Tier 3 manifest.** When built: one line per file with its sha256 and size.
A test reads files only after verifying the hash, and fails with "fixture
missing" rather than passing.

## Gates: a skipped test must not look like a pass

- `STUDIO_REQUIRE_FFMPEG=1` turns "no ffmpeg" from a skip into a failure for
  tests that use `testkit::ffmpeg_or_skip`. Set it in CI and whenever you
  claim coverage. Checked: with ffmpeg moved off PATH, 3 tests fail with it
  set and all "pass" without it, which is exactly the trap.
- Use the same pattern for tier 2 and tier 3 fixtures when they get tests:
  `STUDIO_REQUIRE_FIXTURES=1`.
- Older studio-video tests use their own `has_ffmpeg()` and skip silently.
  Moving them onto `ffmpeg_or_skip` is open work.

## What to test, by kind

- **Pass/fail.** Assert on the mechanism: which file paired, the offset
  within 20 ms, the pixel colour of a rendered cut. Not "it returned Ok".
- **Regression.** Pin real-footage results (tier 2) and known edge cases
  with a comment naming the bug. Example:
  `a_short_file_that_lines_up_with_one_burst_does_not_steal_the_take`.
- **Behaviour pinned on purpose.** When a limit is real, pin it so a future
  change has to decide it on real footage. Example:
  `a_short_take_pairs_right_but_is_not_sure` (12 s takes pair right at
  z ≈ 11, below the Sure bar of 13).
- **Performance.** Measure and record: the whole film pairs in 21 s
  (release build, one machine). Budgets go in `#[ignore]`d tests that print
  numbers, run on purpose. Wall-clock asserts in the default suite flake on
  a loaded sandbox.
- **Memory.** Peak RSS of the operation on a stated input (e.g. pairing 77
  × 114 takes, or one 4K frame through the interruption filter), recorded
  the same way.
- **Edits that change lengths.** See TOUCHUP_VFX.md §5: trim, ripple,
  move and undo, each as a fold of ops, each asserting where looks and
  tickets end up.

## The break check

Before trusting a new test, break the code it covers and confirm it fails.
Record that you did it in the commit message.

On this work, the check caught two fixtures that could not fail:
- a decoy too weak to steal the match;
- pure-tone "syllables" that let the matcher hear the noise floor, made
  worse by raw xorshift seeds producing correlated "independent" noise.

Both were replaced.

## Known failures on main

`slate_ocr::tests::it_reads_a_board_off_a_frame` and
`it_does_not_care_which_side_is_ink` fail. The OCR reader is unfinished
(`docs/PICK-UP-HERE.md`), not broken by this work. Expect
`cargo test -p studio-video` to report exactly these two.
