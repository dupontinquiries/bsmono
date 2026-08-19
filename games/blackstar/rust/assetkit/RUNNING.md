# Running assetkit

## The GUI compiles clean now (this repo's monorepo layout, rustc 1.96)

`src/main.rs` was written in a container with rustc 1.75, where egui couldn't
build at all, so it went untested for a while — that history is kept below
since the fix path it describes (point the manifest at your checkout, mind
the lockfile version) is still accurate for anyone building outside this
monorepo's own layout. Inside this monorepo, with a real rustc >= 1.76, it
Just Builds — `Cargo.toml` already points at `../../../../shared/uikit/...`,
which resolves correctly from `games/blackstar/rust/assetkit`.

Bumping the pinned `egui`/`eframe` from 0.29 to 0.34 (to match `uikit-egui`'s
own pin) surfaced three real API changes — `eframe::App`'s required method
renamed `update` → `ui`, `run_native`'s creation closure now returns a
`Result`, and `eframe`'s `glow` feature alone no longer pulls in a window
backend (needs `wayland`/`x11` added explicitly) — all fixed; see the
README's "What is verified" section for the specifics. `cargo check --bin
assetkit` and `--bin assetkit-cli` are both clean.

**Not yet confirmed**: this was built and checked in a headless sandbox with
no display — it has never actually been run and clicked through. If you hit
something that doesn't match the walkthrough below, that's the next thing to
find, not assume-away.

The CLI is still the fully cross-checked-against-python half and needs none
of this.

## 1. The CLI works right now, on any rustc

    cd rust/assetkit
    rustc --edition 2021 -O src/cli.rs -o assetkit-cli

    ./assetkit-cli                      # subcommand list
    ./assetkit-cli info  path/scene.gltf
    ./assetkit-cli score iqm/ --top 40 --csv scores.csv
    ./assetkit-cli view  engine_src/data/imported_jazz_part0.iqm --views front,side

No cargo, no dependencies, no network.

## 2. The GUI

**Requires rustc >= 1.76.** Check with `rustc --version`.

### a. Point the manifest at your monorepo

`Cargo.toml` assumes the checkout sits beside the game project:

    uikit-egui      = { path = "../../../logscope-monorepo/shared/uikit/crates/uikit-egui" }
    snapclone-theme = { path = "../../../logscope-monorepo/shared/uikit/crates/snapclone-theme" }

Edit both to wherever yours actually is. An absolute path is fine.

### b. Your uikit workspace has a lockfile this cargo may reject

`shared/uikit/Cargo.lock` is lockfile v4. If cargo complains, delete it and let it
regenerate — it is a lockfile, not a source file.

### c. Build and run FROM THE PROJECT ROOT

    cd /path/to/cybertron-cmake-project
    cargo run --release --manifest-path rust/assetkit/Cargo.toml --bin assetkit

The working directory matters: the app's default paths are `content/maps.json`,
`iqm`, `engine_src`, all relative to the project root.

## 3. Put the tools on PATH first

The Export, Raids and GAN pages shell out. Without these on PATH the buttons run
and report "command not found" into the log panel:

    assetkit-cli        built in step 1
    library_score       rustc -O tools/library_score.rs -o library_score
    python3 + tools/    part_harvest.py, limb_split.py, install_models.py,
                        shape_pairs.py, shape_baseline.py

Simplest:

    export PATH="$PWD:$PWD/tools:$PWD/rust/assetkit:$PATH"

## 4. What each page needs

    Library   a directory of .iqm / .obj / .gltf — try `engine_src/data` or `iqm`
    Viewer    click a row on Library first
    Raids     content/maps.json (ships with the project). Generate, then Write
    Export    the python tools on PATH
    GAN       stage 1 needs a donor folder; stage 2 needs pairs built first

## If the GUI will not build

Use the CLI — it does everything the GUI drives:

    assetkit-cli score  <dir>      Library page
    assetkit-cli view   <file>     Viewer page
    assetkit-cli raids             Raids page
    assetkit-cli place  <donors>   GAN stage 1
    assetkit-cli pairs  <donors> --degrade box    GAN stage 2
    assetkit-cli train  pairs/pairs.f32           the CPU falsifier

and send me the compiler errors — with the real API in front of me they should be
quick to fix.
