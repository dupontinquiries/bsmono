# Terminal re-parse perf — diagnosis via simulation (v0.1.671)

## Symptom (user, v670)
Worse terminal performance. WEB2QML_PROFILE logs showed `republish gen=N
html=NNNB` firing every frame while idle, with html OSCILLATING (2540↔2536↔
2632↔2684B). Each republish whose html differs fires term_output_changed →
a full-screen RichText re-parse.

## Method: simulate before touching the render path (scripts/perf-sim.sh)
Modelled the publish→re-parse decision as a pure function and fuzzed
realistic frame sequences. Findings:
- **Pure cursor blink** (text identical, caret moves): the v628 html-equality
  dedup ALREADY handles it (~1 re-parse / 600 frames). NOT the cause.
- **The existing row_cache::classify_change** (cursor-only → cheap signal)
  was built v630 but NEVER WIRED into main.rs — and wouldn't fix this anyway,
  because the html genuinely differs each frame.
- **Content/statusline oscillation** (html differs every frame): the real
  cause. Equality dedup AND the cursor classifier both fail because the bytes
  truly change. Re-parses = every frame (600/600).

## Fix shipped (safe, measured)
A NORMALIZED dedup (row_cache::normalize_for_dedup): trim per-line trailing
whitespace + collapse internal whitespace runs, compare the normalized form
before firing the re-parse. perf-sim Case A (cosmetic-only oscillation:
trailing ws) → 600 re-parses collapse to 1. Case B (a REAL animating field,
e.g. a clock/position) stays 600 — i.e. normalization does NOT hide genuine
content changes. So it's a strict win: kills cosmetic churn, never suppresses
real updates.

## STILL TO DIAGNOSE (needs the user's profile capture)
If the oscillation is a REAL animating field (not cosmetic), normalization
won't fix it (by design). To tell which: run WEB2QML_PROFILE=1, grab two
consecutive idle `republish` frames, diff their html.
- cosmetic delta → normalize_for_dedup already handles it. ✓
- real field animating → next fix is idle-republish coalescing (cap re-parse
  rate when gen churns but no user input) OR stop re-rendering rows whose
  logical content is unchanged (wire the unused row_cache::classify_change +
  per-row diffing). Both are follow-ups; measure the capture first.

## Also noted
row_cache::classify_change (v630, cursor-only cheap signal) is built + tested
but UNWIRED. Wiring it would also let pure cursor moves skip the re-parse
(belt-and-suspenders with the equality dedup). Separate follow-up.

## v0.1.688: gate republish when backgrounded
Runtime log (v685) showed hundreds of identical republishes while the app was
backgrounded: `[profile] republish gen=247..850 html=4379B` — cursor-blink /
status-redraw dirty flags kept firing needs_republish → pty.republish()
(grid→HTML render) every frame, even though the preview wasn't visible and the
v628/v671 downstream dedup suppressed the re-parse. The render itself is still
wasted work.
Fix: a process-wide APP_BACKGROUNDED atomic (set by set_app_backgrounded) now
gates the needs_republish→republish() call. When backgrounded, the idle
grid→HTML re-render is skipped. Real output bursts still flow through the
reader thread (which bumps the snapshot generation independently), and
is_dirty() is a pure read that doesn't clear the flag, so the first
foreground poll republishes any accumulated change — no state loss.
NOTE: row_cache::classify_change (v630, cursor-only vs content) remains
unwired and could further suppress the FOREGROUND cursor-blink re-render; left
as a follow-up since it needs the grid to expose cursor pos + content hash.
