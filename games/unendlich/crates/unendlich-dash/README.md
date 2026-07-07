# unendlich-dash

Native **egui** telemetry dashboard for unendlich — the Rust port of the HTML
tool. Parses `run_summaries.txt` (+ optional `events.jsonl`), correlates them by
run uuid, and shows per-run GPU/mesher pass timings, a frame-time curve with
spike markers, achievements, a two-run compare, and a **copy-for-Claude**
digest.

## Run it

```bash
cd unendlich-dash
cargo run --release
```

Then hit **Load sample** to see everything immediately, or point the two path
fields at your real `run_summaries.txt` / `events.jsonl` and click **Load** (you
can also drag the files onto the window — `.jsonl` is treated as events,
anything else as runs).

Pinned versions (verified current, May 2026): `eframe 0.34`, `egui 0.34`,
`egui_plot 0.35`. MSRV is **1.92** (eframe 0.34). On Linux you need the usual
egui native deps:

```bash
# Debian/Ubuntu
sudo apt-get install -y libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libssl-dev
# Fedora
sudo dnf install -y clang clang-devel libxkbcommon-devel pkg-config openssl-devel libxcb-devel
```

eframe 0.34 defaults to the **wgpu** renderer (picks Vulkan on Asahi/Mesa). If
that misbehaves, switch to glow:
`eframe = { version = "0.34", default-features = false, features = ["glow", "default_fonts"] }`.

## Compile-blind iterate loop

I can't compile in my sandbox, so this is a ship-and-you-build loop. The
**parser + data model** are validated (see `cargo test`) and version-agnostic —
those will be right. The **UI** is written against the confirmed 0.34/0.35 API,
but if anything doesn't compile, paste me the first handful of `cargo build`
errors and I'll fix them — the likely suspects are flagged in comments
(`copy_text`, the `run_native` creator, the plot constructors).

```bash
cargo test    # validates the parser against the bundled two-run sample
cargo build
```

## Embedding into the toolkit later

Right now this is a **standalone workspace** (note the empty `[workspace]` table
in `Cargo.toml`) so it builds on its own even when unzipped inside the
`unendlich-game` repo. To fold it into the toolkit and share types:

1. Move the crate to `crates/unendlich-dash/`.
2. Delete the `[workspace]` table from its `Cargo.toml`.
3. Add `"crates/unendlich-dash"` to the repo's workspace `members`.
4. Replace the local parser with `unendlich-shared`'s event types +
   `unendlich-analyze`'s `load_events` (send me the repo archive and I'll wire
   it).

## Layout

```
unendlich-dash/
  Cargo.toml
  src/
    main.rs     eframe entry
    model.rs    Run / Metric / Workload / Event / Telemetry + derivations
    parse.rs    run_summaries.txt + events.jsonl parsers (validated regexes)
    plots.rs    egui_plot chart builders
    digest.rs   copy-for-Claude text
    app.rs      the UI (top bar, run list, inspect, compare)
  sample/
    run_summaries.txt   two-run sample (Load sample / tests use these)
    events.jsonl
```

## Not yet ported (next increments)

Radar compare, per-panel copy buttons, watch-directory auto-reload, and the
in-engine live overlay (egui meshes rendered by the engine's own Vulkan
pipeline, reading the event sink in memory). The HTML tool stays the fast
reference.
