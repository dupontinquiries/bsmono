# Build & test tooling — faster linking + smaller binaries (sandbox bootstrap)

Companion to `TOOLING_sccache_nextest.md`. That doc's speedups need *persistent*
machines; the ones here are different — **they work inside the build sandbox**,
per session, and are worth running at bootstrap.

## TL;DR — what to run at session start
```bash
# 1. Linkers: ld.gold is preinstalled; mold + lld install from the allowlisted
#    ubuntu mirror (unlike sccache, whose installer is NOT allowlisted).
apt-get install -y mold lld >/dev/null 2>&1 || true

# 2. Make mold + split-debuginfo the default for THIS sandbox only (sandbox cargo
#    home — NOT the repo, so it never touches the operator's M2 build).
CARGO_CFG="${CARGO_HOME:-$HOME/.cargo}/config.toml"
mkdir -p "$(dirname "$CARGO_CFG")"
cat >> "$CARGO_CFG" <<'EOF'
[build]
rustflags = ["-C", "link-arg=-fuse-ld=mold", "-C", "split-debuginfo=unpacked"]
EOF
```
That alone speeds every link (`cargo test`, `cargo build`) with **no runtime
caveat**. For the absolute fastest test/run loop add `prefer-dynamic` (see below).

`scripts/sandbox-fastbuild.sh` does exactly this; run it once per session.

## The crucial caveat first: `cargo check` does NOT link
The standard verify gate here is `cargo check` (type-check → `.rmeta`, no codegen,
no link). **None of these levers speed up `cargo check`.** They speed up the things
that actually *link a binary*:
- `cargo test` / `cargo test --lib --no-run` (a test binary) — newly relevant since
  r328 made the lib tests linkable without glfw;
- `cargo build` (the game binary);
- the flattened `rustc --test` verification harnesses (e.g. the net-module check).

So: keep using `cargo check` as the fast inner gate. Reach for these when you're
running tests or producing a binary.

## The three levers

### 1. `prefer-dynamic` — biggest win, zero install
Dynamically link `std` instead of baking the whole static `std` archive into every
binary. `std` + embedded debuginfo is the bulk of a Rust binary. The dynamic
`libstd-*.so` already exists in this sandbox's sysroot, so it Just Works.

Measured here on a trivial `std` program (HashMap + a thread):

| build                                   | size      |
|-----------------------------------------|-----------|
| static + debuginfo (default)            | 4,770,640 |
| `-C prefer-dynamic`                     |   427,136 |  ~11× smaller
| `-C prefer-dynamic -C split-debuginfo=unpacked` | 181,208 |  ~26× smaller

For scale: the project's `unendlich-engine` lib **test binary is ~240 MB** statically.
Most of that is `std` + debuginfo the linker writes on every relink. `prefer-dynamic`
stops linking `std` into it at all → far less for the linker to do and to write.

Caveat: a `prefer-dynamic` binary needs `libstd-*.so` findable at runtime. When you
run such a binary directly:
```bash
export LD_LIBRARY_PATH="$(rustc --print sysroot)/lib:$LD_LIBRARY_PATH"
```
(`cargo test` usually sets the rpath for you, but export it if a binary can't find
`libstd`.) **Never ship a `prefer-dynamic` binary** — it isn't self-contained.

### 2. mold / lld — a faster linker
Linking a 240 MB binary is dominated by symbol resolution + writing the output.
mold is the fastest widely-available linker; lld is next; gold beats GNU bfd.
Published benchmarks put mold roughly **2–5× faster than bfd** on large links (not
re-measured here — budget; see the gotcha below). Select it with
`-C link-arg=-fuse-ld=mold` (or `=lld`, `=gold`). Available here:
`mold`, `ld.lld`/`lld`, `ld.gold` (after the apt step above; gold is preinstalled).

### 3. `split-debuginfo=unpacked` — keep debuginfo out of the link
Debug info is written to sidecar files instead of stamped into (and processed by)
the final link. Smaller binary, less linker work. Safe to leave on always. (See the
26× row above — most of the gap from 0.43 MB → 0.18 MB is this.)

## The gotcha that cost a turn: don't toggle `RUSTFLAGS` to compare
cargo's fingerprint includes `RUSTFLAGS`. Changing it — e.g. looping over linkers to
benchmark them — **invalidates the entire build and forces a full recompile** of the
whole dependency graph, not just a relink. Three linkers in a loop = three full
rebuilds, which exceeds a single turn's budget. Set the linker **once** (in the
sandbox `config.toml` above) and leave it; the first build after setting it pays a
one-time full rebuild, then every subsequent link is fast.

To measure *just* the link without a recompile: delete only the final binary
(`rm target/debug/deps/<crate>-<hash>`) and re-run `cargo test --no-run` with
`RUSTFLAGS` unchanged — cargo relinks from cached objects.

## Do NOT put any of this in the repo's `.cargo/config.toml`
The operator builds on a real Apple M2 (Asahi/Honeykrisp). A committed
`-fuse-ld=mold` breaks any machine without mold; a committed `prefer-dynamic`
produces non-portable release binaries. These belong in the **sandbox** cargo home
(`$CARGO_HOME/config.toml`, created by the bootstrap script) or in per-command
`RUSTFLAGS`, never in the tree. The FS resets between sessions, so re-run the
bootstrap each time.

## What this does and doesn't change
- Faster: `cargo test`, `cargo build`, `rustc --test` harnesses — the link step and
  the bytes written.
- Unchanged: `cargo check` (no link), and correctness/behavior (these are link/output
  options, not codegen semantics).
- Orthogonal and stackable with `sccache` + `cargo-nextest` (that doc) on persistent
  machines.
