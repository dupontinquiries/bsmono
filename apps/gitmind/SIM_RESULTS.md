# Simulation results (real runs, not estimates)

Toolchain: Rust 1.96.0. All numbers below come from `gitmind-sim`
building **real git repositories** and comparing against gitmind's chunk
store. Reproduce with:

```
cargo run -p gitmind-sim --release   # writes sim_out/metrics.json
python3 sim/plot.py                  # writes sim_out/*.png
```

## What passed

- `cargo test -p gitmind-core` — 6 unit tests (oid, chunk-store roundtrip
  + dedup, media detection).
- `cargo test -p gitmind-core --test git_integration` — 6 integration
  tests on real git repos, including the safety-critical ones:
  *snapshot leaves the working index and HEAD byte-for-byte unchanged*,
  *snapshot skips when nothing changed*, *timeline orders correctly*,
  *materialize creates a side worktree*, *dedup holds across versions*.

## What the storage numbers actually show

Two scenarios, 10 versions each, measured cumulative storage.

**Small 16 KiB edit to a 16 MiB asset:** git-loose jumps to ~34 MB and
stays (a fresh full blob every commit until you `gc`); git-packed holds
~17 MB; gitmind logical bytes track ~17→18.5 MB. **Append-only growth:**
git-loose climbs to ~34 MB; git-packed and gitmind both rise to ~18 MB,
nearly on top of each other.

Honest reading — git's pack delta-compression is genuinely excellent, so
on a single file gitmind does **not** beat fully-packed git; it *matches*
it in logical bytes. The real wins it demonstrates:

1. **Packed-level efficiency at write time, no `gc` step.** gitmind dedups
   on ingest, so it tracks the git-*packed* line continuously while
   git-*loose* (the real on-disk state between gc runs) balloons to 2×
   (edit) or linearly (append).
2. **Where it pulls ahead** (not in these single-file charts): global
   dedup across different files/repos and incompressible media with
   shared regions — git only deltas within a pack window, gitmind dedups
   every identical chunk everywhere.

## Findings / tuning items the sim surfaced

- **redb file preallocation**: the on-disk `.redb` grows in big steps
  (the dashed line sits well above logical bytes — ~34 MB file for ~18 MB
  of data). The data is competitive; the *file* over-allocates. Tune
  redb's growth / add periodic compaction, and report logical bytes in
  the UI, not raw file size.
- **Ingest throughput: ~61 MB/s single-threaded** (zstd level 9 + BLAKE3
  + redb). zstd-9 is the bottleneck. Levers: drop to zstd-3 for hot
  ingest, or rayon across chunks — both straightforward, deferred until
  the path is exercised for real.

## Screenshots

`sim_out/chart_small_edit.png`, `chart_append.png`, `chart_incremental.png`
are the measurement charts. `ui_timeline_preview.png` is a design preview
of the touch UI (status rows with swipe affordances + the checkpoint
timeline) rendered from sample data — a live Qt screenshot comes once the
`gui` crate is built on a machine with Qt6.
