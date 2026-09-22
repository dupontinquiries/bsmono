# tools

Standalone helper binaries used across the monorepo: `astfind`, `drift`,
`health`, `kbm3`, `kitfinder`, `peek`, `rustcheck`, `sanity` -- and the Python
helpers `gitsweep`, `chunkup`, `chatgrep` and `coursekit` (C20).

## `cargo-ailite` is NOT here

It lives at `maintenance/tools/cargo-ailite`, which is the path CLAUDE.md §2
cites and the one every session's build commands use. `ailite-cache` sits
beside it and the schedule-first experiment is at
`patches/cargo-ailite-schedule-first.patch`.

Three byte-identical copies arrived here with the c5blackstar-008 merge, which
vendored them at `<repo>/tools/` so that a STANDALONE blackstar clone could run
its own `fastcheck` loop. That is the right call for that repo and a duplicate
here, so the copies were dropped and `games/blackstar-rs/tools/fastcheck` now
looks in `maintenance/tools/` first and `tools/` second — working in both
shapes.

The reason the tool has to be findable at all, kept from the vendored README:
somebody cloning a bundle hit `no such subcommand: ailite` and reconstructed it
from the comments in `fastcheck`. The reconstruction was missing the levers,
because they live in the tool rather than in the script that calls it.

| the real tool | the reconstruction |
|---|---|
| `debug = 0`, `strip = "debuginfo"` — the actual size/time levers | not present |
| `CARGO_BUILD_JOBS=1` — caps peak RAM, matters on 1 vCPU | not present |
| `--test-threads=1` in test mode | not present |
| deliberately does NOT touch opt-level (dev is already 0) | added an opt-level knob |
| `gate` mode — checks only crates git says changed | not implemented |
| `AILITE_BUILD_DIR` — shunt artifacts off `target/` | not present |
