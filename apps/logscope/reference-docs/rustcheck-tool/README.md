# rustcheck — an AI-centric Rust + GPU-shader checker

Answers one question fast and cheaply for an agent editing code: **did my change
break this?** Cheapest tier first; the compiler runs only when it's the thing
that'll actually tell you something.

Ships in two compatible implementations with identical CLI, output, and exit
codes:

| file | what it is |
|------|-----------|
| `rustcheck.py` | the Python source (stdlib-only, no deps, no build step) |
| `rustcheck` | native Rust binary — x86-64 ELF, ~2 MB, fastest static tier |
| `rustcheck-py` | the Python version frozen into a standalone x86-64 ELF (no Python needed) |
| `rustcheck.rs` | the Rust source (single file; build with the `Cargo.toml` shown below) |

```
● static  OK
● semantic  cargo check -p unendlich-shared   (0.24s)
  error[E0308] crates/unendlich-shared/src/lib.rs:13:54  mismatched types

FAIL  0 static  ·  1 errors  ·  0 warnings
```

## Tiers

**Tier 0 — static (Rust source), no toolchain, ~1 s for 169 files.** Merge-conflict
markers, unbalanced `() [] {}`, unterminated string/char/block comment, and
`mod foo;` with a missing backing file. Rust-aware: it does **not** trip on
lifetimes (`'a`), labels (`'outer:`), raw strings (`r#"..."#`), byte literals, or
nested block comments. A hard structural break here short-circuits the compiler.

**Tier 1 — semantic, real `cargo check` (or clippy), scoped + cached.** The real
front-end catches the bulk of breakage: type mismatches, moved/borrowed values,
missing imports, unknown names, wrong arities, unsatisfied trait bounds. In a
workspace it maps a changed file to its owning member and runs `cargo check -p
that_crate`, not the whole tree. Cargo's incremental cache means deps compile
once (measured: 41 s cold to 0.3 s warm). JSON diagnostics are compacted to one
line each and deduped.

It separates **your** errors from a **dependency/environment** build failure
(e.g. a missing system lib like `gdk-pixbuf`/`pango`). Those report **BLOCKED**,
not a false PASS and not a code FAIL — so an agent loop doesn't misread an env
problem as a regression.

**Single-file fallback:** a `.rs` file with no `Cargo.toml` ancestor is checked
with `rustc --emit=metadata`.

## Shader checker (GLSL / SPIR-V)

Routes by extension (`.vert .frag .comp .geom .tesc .tese .glsl ... .spv`), basic
Vulkan-GLSL rules, no GPU and no glslang required (but it uses `glslangValidator`
automatically if installed):

- structural: balanced delimiters, unterminated block comment, `#if/#endif` balance
- `#version` present and `void main()` present — **stage shaders only**
  (`.glsl` include/library fragments are exempt)
- **descriptor binding collisions** — two resources sharing `(set, binding)`
- **location collisions** — duplicate `layout(location=N)` among inputs / outputs
- **missing binding** — a `uniform`/`buffer` declared without `binding=`
- compute missing `local_size_x`; vertex never writing `gl_Position` (warn);
  fragment with no output (warn); `>1` push_constant block
- legacy builtins: `gl_FragColor`, `texture2D()`, `varying`, `attribute` (warn)
- `.spv`: validates the SPIR-V magic and reports version/endianness/size

Validated against the project's real shaders (43 GLSL + 37 SPIR-V): no false
positives.

## Usage

```
rustcheck .                        # check the crate/workspace in CWD
rustcheck --git                    # check only what git says changed
rustcheck --changed src/voice.rs   # check the crate owning this file
rustcheck --fast src/*.rs          # Tier 0 only, never invoke the compiler
rustcheck --clippy .               # use clippy if installed (else cargo check)
rustcheck --deny-warnings .        # warnings fail the check
rustcheck --full /tmp/x.rs         # include rustc's full rendered diagnostic
rustcheck assets/shaders/*.comp    # shader rules
rustcheck --json .                 # structured output for an agent to parse
```

Other flags: `--no-default-features`, `--features "a,b"`, `--all-targets`,
`--tests`, `--offline`, `--edition 2021|2024`, `--timeout SECONDS` (default 600),
`--jobs N` (parallel static/shader scan), `--shaders` (force shader mode).

## Exit codes

| code | meaning |
|------|---------|
| `0`  | clean |
| `1`  | problems in *your* code — hard static break, compiler error, or shader error (or, with `--deny-warnings`, any warning) |
| `2`  | inconclusive / **BLOCKED** — a dependency or the environment failed to build, or the check timed out; your code may be fine |

An agent loop should treat `2` as "couldn't verify here," not "I broke it."

## What it does and doesn't catch

It catches essentially everything `cargo check` does — the large majority of what
stops a Rust build (syntax, types, borrows, lifetimes-against-declarations,
name/import/trait resolution, arity) — plus the instant Tier-0 structural catches
and the basic shader rules. It does **not** run codegen, so it won't catch link
errors, monomorphization-only failures, or runtime/logic bugs; `--tests` /
`cargo test` remain the next step.

## Requirements & building

Needs `cargo`/`rustc` for Tier 1; both versions auto-discover a toolchain in
`~/.local/bin`, `~/.cargo/bin`, `/usr/local/bin`. Tier 0 and the shader checker
work with no toolchain at all.

- **Python:** 3.8+ (`tomllib` on 3.11+, regex fallback below). Run `rustcheck.py`
  directly, or use the frozen `rustcheck-py` binary.
- **Rust:** put `rustcheck.rs` at `src/main.rs` next to this `Cargo.toml`:

  ```toml
  [package]
  name = "rustcheck"
  version = "0.2.0"
  edition = "2021"
  [[bin]]
  name = "rustcheck"
  path = "src/main.rs"
  [dependencies]
  serde_json = "1"
  regex = "1"
  [profile.release]
  opt-level = 3
  lto = "thin"
  codegen-units = 1
  strip = true
  ```

  then `cargo build --release` -> `target/release/rustcheck`.

The included `rustcheck` binary is a dynamically-linked glibc x86-64 ELF (built
against the offline Rust 1.96 bundle); on a musl host, rebuild for a fully static
binary with `--target x86_64-unknown-linux-musl`.
