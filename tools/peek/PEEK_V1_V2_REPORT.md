# peek v1 vs v2 — measured, and whether it beats my native tools

## What v2 adds (daemonless, per peek.md's "stop here" advice)
- **mmap newline index** built once per file per call, reused across that call's
  ranges (the only cache a daemonless tool can have). v1 scans line-by-line per range.
- **`@symbol` jump**: `peek2 file.rs:@gameplay_tick` finds the definition
  (`fn`/`struct`/`enum`/…) and shows it + 20 lines — no grep-then-view round trip.
- **`-C N` context** and bucket-extend (`5xx+20`).

## Benchmark (this sandbox; date(ns) timing, representative)
| case | v1 | v2 | sed |
|---|---|---|---|
| source 4970 lines, 5 ranges/call | 6.7 ms | 3.0 ms | 2.9 ms |
| 2M-line file, 5 spread ranges/call | 162 ms | 75 ms | 94 ms |
| 2M-line file, single deep range | 93 ms | 72 ms | 78 ms |

Reading it honestly:
- On **source files** (my real case, <5k lines) everything is **sub-10 ms** — speed
  is irrelevant; v1/v2/sed are a wash. v1's only cost is re-scanning per range.
- On **large files with multiple ranges**, v2's reused index is ~2× v1 and beats
  sed — the index amortises. Matters for logs, not for code.

## Does peek save *my* tooling cost vs `view`/`sed`/`bash`?
- For a **single known line-range**, native `view` (range + line numbers, no bash
  hop) is equal-or-better. peek adds nothing there.
- peek wins on two patterns: **surveying** — `peek -h 'src/**/*.rs:1'` reads every
  module's header line in ONE call (vs N `view` calls; a real call+token saver, and
  it's how I found the duplicate `main_menu`); and **symbol navigation** — `:@name`
  replaces grep-then-view with one call.
- Net: keep **v2** (strictly better than v1 — faster + symbol jump), use it for
  multi-file surveys and symbol jumps, keep using native `view` for ordinary
  single-file ranges. A daemon would only pay off for repeated big-log work, which
  isn't my pattern — so **daemonless is correct**, matching your own call in peek.md.

## Piping to build patches?
`peek` writes plain stdout, so `peek … | rg … | …` works for *finding/filtering*
regions. But peek is a viewer, not a differ — `peek > x.patch` is not a valid diff.
Useful as the front of a patch workflow (locate the region with `:@symbol`/`-H -n`,
then edit), not as the patch producer itself.
