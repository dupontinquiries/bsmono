# Fast Rust iteration: cut link time on large debug binaries

A portable recipe for making `cargo build` (debug) iterations fast on projects whose
binaries have grown large — GUI apps, anything pulling in `wgpu`/`eframe`/`bevy`,
game engines, or any crate graph with hundreds of dependencies. Drop the relevant
pieces into any Rust project and adjust per platform.

The advice here targets the **edit → build → run** inner loop only. None of it should
ship in release/distribution builds (see "What never to ship," below).


## The problem, in one sentence

On a big project the compiler usually isn't the bottleneck during iteration —
**dependencies are already cached, so each rebuild recompiles only your changed crate
and then re-links the whole executable.** When that executable is hundreds of MB of
debug info plus a statically-baked standard library, the *link* step dominates and can
take minutes even though compilation took seconds.

So the three levers all attack linking:

1. **A faster linker** (mold / lld / the platform's modern linker) — parallel, cache-friendly.
2. **`-C prefer-dynamic`** — link `libstd` as a shared object instead of baking it into every binary.
3. **Trimming debug info** — keep it out of the executable (split debuginfo) and reduce its volume (`debug = 1`, strip dep debuginfo).

Each helps on its own; together they typically turn a multi-minute re-link into a few seconds.


## Lever 1 — a faster linker

The default GNU `ld` (`bfd`) is single-threaded and slow on large links. Replace it.

### Linux — mold (fastest) or lld

Install one of them with your package manager:

```
# Debian/Ubuntu
apt-get install mold        # or: apt-get install lld
# Fedora
dnf install mold
# Arch
pacman -S mold
```

Point Cargo at it via `.cargo/config.toml` (project root, or `~/.cargo/config.toml` for all projects):

```toml
[target.x86_64-unknown-linux-gnu]
rustflags = ["-C", "link-arg=-fuse-ld=mold"]   # or -fuse-ld=lld
```

`-fuse-ld=mold` requires the C compiler that drives linking to understand the flag:
**gcc ≥ 12 or clang ≥ 12.** If you're on something older, use one of these fallbacks
instead of the flag above:

```toml
# Fallback A: invoke mold's wrapper (works with any toolchain)
#   run builds as:  mold -run cargo build
#
# Fallback B: name the linker driver explicitly
rustflags = ["-C", "linker=clang", "-C", "link-arg=-fuse-ld=mold"]
#
# Fallback C: absolute path to the mold ld shim
rustflags = ["-C", "link-arg=-fuse-ld=/usr/libexec/mold/ld"]
```

### macOS

mold's macOS port is a separate commercial product (`sold`), so don't reach for mold here.
Two good options:

- **Do nothing.** Recent Xcode (15+) ships a new, much faster default linker. On an up-to-date
  toolchain you may already be fine.
- **lld:** `brew install llvm`, then
  `rustflags = ["-C", "link-arg=-fuse-ld=lld"]` under `[target.aarch64-apple-darwin]`
  (or the `x86_64` triple on Intel).

### Windows

Use LLVM's linker. With a recent toolchain:

```toml
[target.x86_64-pc-windows-msvc]
linker = "rust-lld"        # bundled with rustup on recent versions
# older alternative: install LLVM and use lld-link via -C link-arg=-fuse-ld=lld
```

### Verify the linker is actually used

```
cargo build -v 2>&1 | grep -- '-fuse-ld'     # confirm the flag reached rustc/cc
mold --version                                # confirm it's installed
```


## Lever 2 — `-C prefer-dynamic` (dynamic libstd)

By default every Rust binary statically links the standard library, so a large fraction
of each executable is the *same* copy of std baked in again. `-C prefer-dynamic` links
std (and other crates that are available as `dylib`, e.g. proc-macros) **dynamically**,
shrinking the binary and giving the linker far less to write.

```toml
[target.x86_64-unknown-linux-gnu]
rustflags = ["-C", "link-arg=-fuse-ld=mold", "-C", "prefer-dynamic"]
```

In one real GUI project this took the debug binary from ~330 MB to ~107 MB and was the
single biggest contributor to the link speedup.

### The runtime gotcha (read this)

A `prefer-dynamic` binary now depends on `libstd-<hash>.so` at runtime, and the loader
has to find it. The `.so` lives in the toolchain, not on the default library path:

```
<sysroot>/lib/rustlib/<target-triple>/lib/libstd-<hash>.so
# find it with:
ls "$(rustc --print sysroot)"/lib/rustlib/*/lib/libstd-*.so
```

If you run the binary directly you'll get `error while loading shared libraries:
libstd-….so: cannot open shared object file`. Two fixes:

- **Quick (env var):** set the search path when you run it.
  ```
  export LD_LIBRARY_PATH="$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/lib:$LD_LIBRARY_PATH"
  ```
  (macOS: `DYLD_LIBRARY_PATH`.) `cargo run` mostly sets this up for you; the trap is
  running the binary *directly* (scripts, debuggers, CI, screenshot harnesses).

- **Robust (rpath baked into the binary):** add an rpath so it always finds std with no
  env var. Note this hard-codes an absolute path, which is exactly why it's dev-only:
  ```toml
  rustflags = [
    "-C", "link-arg=-fuse-ld=mold",
    "-C", "prefer-dynamic",
    "-C", "link-arg=-Wl,-rpath,/home/you/.rustup/toolchains/<toolchain>/lib/rustlib/x86_64-unknown-linux-gnu/lib",
  ]
  ```

### The toolchain-update gotcha

The `<hash>` in `libstd-<hash>.so` changes when you run `rustup update`. A binary linked
against the old hash won't find the new `.so`. The env-var/rpath approaches point at the
*directory*, so they keep working, but any **already-built** binary is stale — just rebuild.
If you see a missing-`.so` error right after updating Rust: `cargo clean` and rebuild.

### Other `prefer-dynamic` caveats

- **Dev-only.** Never use it for anything you distribute (see below).
- It only dynamicizes crates that exist as `dylib` — primarily `std`. Ordinary crates.io
  dependencies are still linked statically; this is not a way to dynamically link "any lib."
- Don't combine it with `-C lto` or `panic = "abort"` in the same profile; they fight.
  The default `dev` profile has neither, so leave LTO/panic settings to your `release` profile.


## Lever 3 — trim debug info

Debug info is often the majority of a debug binary's bytes. Two independent reductions,
both in the **dev** profile in `Cargo.toml`:

```toml
[profile.dev]
debug = 1                      # line tables only — enough for backtraces, far smaller than full (debug = 2)
split-debuginfo = "unpacked"   # leave debug info in separate object/.dwo files, not in the executable

# Strip debug info from DEPENDENCIES while keeping it for your own crates:
[profile.dev.package."*"]
debug = false
```

Notes:
- `split-debuginfo` values: `"unpacked"` (separate files, Linux/macOS), `"packed"` (a single
  `.dwp`/`.dSYM` bundle), `"off"` (drop it). `"unpacked"` is the fastest to link.
  On macOS the platform default is already unpacked (`.dSYM`).
- `[profile.dev.package."*"] debug = false` is frequently the biggest, lowest-risk win and
  is worth adopting even on its own — you almost never need to step into dependency source.
- `debug = 1` keeps backtraces and line numbers but drops variable/type detail. If you live
  in a debugger and need locals in your own code, that's already preserved by the
  per-package override above; you can also bump your own crate back to `debug = 2` while
  keeping deps at `false`.


## Putting it together

`.cargo/config.toml` (project root):

```toml
# Fast-iteration linking. Dev-only conveniences; see docs/fast-rust-linking.md.
[target.x86_64-unknown-linux-gnu]
rustflags = ["-C", "link-arg=-fuse-ld=mold", "-C", "prefer-dynamic"]
```

`Cargo.toml` (workspace or package root):

```toml
[profile.dev]
debug = 1
split-debuginfo = "unpacked"

[profile.dev.package."*"]
debug = false
```

Then run the inner loop with std on the path (or use the rpath variant and skip this):

```
export LD_LIBRARY_PATH="$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/lib:$LD_LIBRARY_PATH"
cargo run
```


## Verifying the win

Measure a real incremental rebuild — touch a source file so only your crate recompiles
and the binary re-links:

```
touch src/main.rs
time cargo build          # compare before/after the config changes
ls -lh target/debug/<bin> # binary should be much smaller with prefer-dynamic
```

A useful sanity check is that compilation time barely changed but total time dropped —
that confirms you were link-bound and the levers hit the right thing.


## What never to ship

These settings are for the local inner loop. For anything you publish, install, or deploy:

- **Drop `-C prefer-dynamic`.** A dynamically-std binary is not self-contained; it needs the
  exact matching `libstd-<hash>.so` on the target. Release builds should link std statically.
- Keep your real linker choice if you like (mold/lld are fine for release too), but make sure
  CI has it installed or guard the config (below).
- Use a separate `[profile.release]` with your normal `opt-level`, `lto`, `strip`, etc. The
  dev-profile changes here don't affect release.

A clean way to keep release builds honest is to scope the dev conveniences so they can't
leak — e.g. put `prefer-dynamic` only in a profile-gated `RUSTFLAGS` you set for local runs,
or simply remember that `cargo build --release` ignores `[profile.dev]` and you must not put
`prefer-dynamic` anywhere release sees it. Since `.cargo/config.toml` `rustflags` apply to
*all* profiles, the safest pattern if you ship from the same machine is to drive the dev-only
flags through an env wrapper rather than committing them globally:

```
# run-dev.sh
export RUSTFLAGS="-C link-arg=-fuse-ld=mold -C prefer-dynamic"
export LD_LIBRARY_PATH="$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/lib:$LD_LIBRARY_PATH"
exec cargo "$@"
```

…and keep only the always-safe linker choice (and the debug-trim profile) in committed config.


## Gotchas & troubleshooting (general)

- **Don't delete `target/` to keep it out of an archive.** A surprisingly common own-goal: a packaging
  script that runs `find . -name target -prune -exec rm -rf {} +` (or similar) before zipping nukes the
  *entire* build cache, so the very next build is a full from-scratch recompile of every dependency — which
  silently undoes all the link-time wins above and makes each iteration look mysteriously slow again. Exclude
  it at archive time instead (`zip -x '*/target/*'`, `tar --exclude=target`, or a `.gitignore`/`.dockerignore`
  entry). The cache is large but cheap to keep; recompiling it is the expensive part.

- **A killed build can leave a stale lock.** If a build is `Ctrl-C`'d or `kill`ed mid-link, `target/<profile>/.cargo-lock`
  (or partial artifacts) can stall or hang the next invocation — symptom: a "build" with zero CPU and no `rustc`
  process. Remove the lock and rebuild; worst case `cargo clean`.


- **`RUSTFLAGS` env overrides config `rustflags` — they do not merge.** If something in your
  shell/CI already exports `RUSTFLAGS`, the `[target.*] rustflags` in `.cargo/config.toml` are
  silently ignored. Either unset the env var or fold your flags into it.

- **`[build] rustflags` vs `[target.<triple>] rustflags`.** `[build]` applies to *every* target
  including host build-scripts and proc-macros and during cross-compilation; `[target.<triple>]`
  applies only to that triple. Prefer the target-specific form so cross-builds and host tooling
  aren't surprised.

- **CI / containers may lack mold.** A config that hard-requires mold breaks where it isn't
  installed. Either install it in the CI image, or keep mold out of committed config and inject
  it locally via the env wrapper, or gate it:
  ```toml
  # only applies when you opt in with: cargo build --config 'target...'  (or via env)
  ```
  Simplest robust approach: commit only the debug-trim profile; keep linker + prefer-dynamic in
  an uncommitted `run-dev.sh`/local `~/.cargo/config.toml`.

- **rust-analyzer** reads the same config and will use your fast linker — generally a bonus.
  If RA behaves oddly, point it at a separate target dir (`rust-analyzer.cargo.targetDir`) so its
  builds don't thrash the same artifacts your CLI builds use.

- **Stale `.cargo-lock` after a killed build.** If you `Ctrl-C`/kill a build mid-link, a stale
  lock or partial artifacts can force a full recompile next time or hang the next build. If a
  build seems stuck with zero CPU, remove `target/<profile>/.cargo-lock` and rebuild; worst case
  `cargo clean`.

- **`-fuse-ld=mold` "undefined symbol: main" when testing against `/dev/null`.** That's just mold
  refusing to link an empty input; it confirms the flag works. Not an error in real builds.

- **Changing `rustflags` triggers one full recompile.** The first build after editing
  `.cargo/config.toml` rebuilds the whole dependency graph (flags are part of the fingerprint).
  That one-time cost is expected; subsequent incremental rebuilds get the speedup.

- **Don't stack with LTO in dev.** `lto = true` re-introduces a heavy, serial link-time step and
  defeats the purpose. Keep LTO in `release` only.


## Companion levers (when you want more)

- `[profile.dev] codegen-units` defaults to 256 (good for parallel dev compiles) — leave it.
- `incremental = true` is the dev default — keep it.
- On **nightly**, `-Z share-generics` and the parallel front-end (`-Z threads=N`) reduce compile
  (not link) time further.
- For run-heavy loops where startup matters, also consider `opt-level = 1` for *dependencies only*
  via `[profile.dev.package."*"] opt-level = 1`, which speeds the running program with little
  rebuild cost for your own crate.
