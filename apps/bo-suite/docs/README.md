# bo-suite research docs

Living folder for the noise-cancellation research that informs the
codebase. Update when significant findings change.

## Files

- **v1-plan.md** — Architecture decision: what ships in v1, what
  stays in research mode, why. Read this first if you're new to
  the codebase.

- **anc-findings.html** — Visual recap of all the empirical
  experiments. Charts, color-coded "FLIP" badges where prior
  conclusions reversed, glossary of all the jargon. Open in a
  browser.

- **recording_analysis.png** — Spectrogram + harmonic comparison of
  a real street recording against the synthetic test signals. The
  bottom panel is the v1 wake-up call: real engines have 8+
  substantial harmonics, the canceller tracks 3.

- **recording_analysis.py** — Python script that produced the
  above. Reusable for future recordings — drop a WAV in and rerun
  to get spectrogram + harmonic profile + fundamental track.

- **real_engine_street.wav** — The 40-second street recording
  used as the reality-check baseline. Keep this committed so
  regression tests can compare against a known signal forever.

## Notation

When new findings reverse prior conclusions, add a section to
anc-findings.html with a `<span class="flip-tag">FLIP</span>` or
`<span class="critical-tag">CRITICAL</span>` badge. Don't delete
the old finding — keep both visible so the audit trail is
preserved.

## The recurring pattern

Every theoretical bet I made on this canceller failed when tested
against harder simulated conditions, and almost all of them failed
again when cross-referenced against the real recording. The right
research process is: synth-test → reverb-augment → real-recording
check → only then claim a win.
