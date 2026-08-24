# tools

## `cargo-ailite`

A fail-fast, single-core, low-disk `cargo` wrapper. `games/blackstar-rs/tools/fastcheck`
uses it for the fast edit-test loop, and **it is vendored here because it used
to be assumed on `PATH`** — which is fine on the machine it was written on and
a dead end for anyone cloning a bundle.

Somebody did clone one, hit `no such subcommand: ailite`, and reconstructed
the tool from the comments in `fastcheck`. That was the right instinct and the
reconstruction was missing the levers that matter, because they live in the
tool rather than in the script that calls it:

| the real tool | the reconstruction |
|---|---|
| `debug = 0`, `strip = "debuginfo"` — the actual size/time levers | not present |
| `CARGO_BUILD_JOBS=1` — caps peak RAM, matters on 1 vCPU | not present |
| `--test-threads=1` in test mode | not present |
| deliberately does NOT touch opt-level (dev is already 0) | added an opt-level knob |
| `gate` mode — checks only crates git says changed | not implemented |
| `AILITE_BUILD_DIR` — shunt artifacts off `target/` | not present |

`fastcheck` prefers whatever `cargo-ailite` is on `PATH`, falls back to this
copy, and falls back again to plain `cargo` with a warning. A fresh clone
works with no setup.

## `ailite-cache`

Makes cargo's warm state survive a sandbox reset. Worth reaching for when
`target/` is lost; unnecessary otherwise.

## `ailite-schedule-first.patch`

Needs a forked cargo. Not adopted — recorded so the option is findable.
