# vello_cpu, build time, linkers, size, and weak CPUs

Everything below was measured in this container unless it says otherwise. Where I could not
measure something I say so rather than estimate it.

## 1. What vello_cpu is, and why it is here

**Nothing in this monorepo depends on it directly.** `grep vello --include=Cargo.toml`
across the tree returns one hit, and that is a comment. It arrives as a transitive
dependency of **epaint 0.34.3**, which is egui's painting layer:

```
your app → eframe → egui → epaint → vello_cpu 0.0.6 → vello_common → fearless_simd
```

epaint declares it as a hard dependency with `default-features = false, features = ["std",
"u8_pipeline", "f32_pipeline"]`, and there is **no feature flag in epaint that turns it
off** — its `[features]` list has nothing covering the rasteriser.

Its single job is **glyph rasterisation**. `epaint/src/text/font.rs` is the only file that
mentions it: skrifa produces a glyph outline, then

```rust
let mut ctx = vello_cpu::RenderContext::new(width, height);
ctx.set_transform(kurbo::Affine::translate((-bounds.x0, -bounds.y0)));
ctx.set_paint(color::OpaqueColor::<color::Srgb>::WHITE);
ctx.fill_path(&path);
ctx.render_to_pixmap(&mut dest);
```

and epaint copies the alpha channel into its font atlas. That is the whole integration. It
replaced the old `ab_glyph` rasteriser in the egui 0.33/0.34 line. Vello is Linebender's 2D
renderer; `vello_cpu` is its CPU backend, so this is a general-purpose path-filling engine
being used to fill one glyph at a time.

Worth knowing given the "tall blurry font" report: this is the code that decides what a
glyph looks like, and it takes a hinting instance from skrifa. That is a separate lever from
the pixel-grid snapping in `uikit_egui::crisp`, and one I have not investigated.

## 2. What it costs (measured)

Cold build of an empty binary whose only dependency is `vello_cpu` with epaint's exact
feature set, on this container:

| | |
|---|---|
| wall time, cold, 24 units | **19 s** |
| of which `vello_cpu` itself | **9.0 s** — the single largest unit |
| `fearless_simd` | 2.2 s |
| `vello_common` | 1.1 s |
| total CPU across all units | 18.2 s |
| `libvello_cpu.rlib` (dev) | **48 MB** |
| `libfearless_simd.rlib` | 8.0 MB |
| `libvello_common.rlib` | 5.6 MB |
| whole `target/` for that one dep | 120 MB |

So: **~12 s and ~62 MB of rlib, per target directory, per cold build.** Source sizes explain
it — `fearless_simd` is 48k lines of generated SIMD wrappers and `vello_cpu` is 11k lines of
heavily generic pipeline code monomorphised across two pixel formats (`u8_pipeline` +
`f32_pipeline`) and several SIMD levels.

## 3. What actually speeds up your builds

Measured, in order of how much they buy:

### One shared target directory — the big one

This tree has **133 `Cargo.lock` files** and **80 crates that depend on egui or eframe**.
Each app compiled from its own directory gets its own `target/`, and each pays that 12 s and
62 MB again — plus the rest of the egui/wgpu/winit stack, which is far larger.

```toml
# ~/.cargo/config.toml
[build]
target-dir = "/var/tmp/cargo-target"   # one disk, one cache, all apps
```

I have been doing this by hand all session (`CARGO_TARGET_DIR=…`) precisely because a cold
egui build is minutes. The caveat is real: one target dir means one lock, so two apps cannot
build at the same time, and it grows to tens of GB. On this container it repeatedly filled
the disk, which is why several of my builds this session died mid-link.

### sccache — untested here, but this is its exact shape

`vello_cpu` is a leaf dependency with fixed features across every app in the tree, so its
compilation is byte-identical everywhere. That is the ideal sccache case. I could not test
it (not installed, and the network here is restricted to a few registry domains), so this is
reasoning, not a measurement.

### `debug = 0` or `debug = "line-tables-only"`

Measured on the same tree: **19 s → 14 s**, and `target/` 120 MB → 74 MB. Backtraces keep
their line numbers with `line-tables-only`. A cheap win applied repo-wide.

### Things I measured that do NOT help

* **`[profile.dev.package.vello_cpu] opt-level = 1`** — a common "make dependencies fast"
  recipe. Here it made the build **19 s → 64 s** and the rlib 48 → 55 MB. Optimising a
  heavily generic crate costs far more than it saves in a debug build. Don't.
* **Switching the linker to lld for this tree** — 18 s either way. See below for why.

### Not measurable here, but structural

`cargo build -Ztimings`-style parallelism is limited by the dependency chain, and
`vello_cpu` sits deep in it. If a future egui release makes the rasteriser optional, that is
the real fix; until then the only way to not compile it is to not use egui 0.33+.

## 4. Can it be linked as a library instead?

Short answer: not usefully, and not because of vello.

* **`prefer-dynamic`** (which `apps/logscope/.cargo/config.toml` already sets, alongside
  `-fuse-ld=gold`) makes rustc link the Rust std and any dylib-capable crates dynamically.
  It does not turn a normal rlib dependency into a shared object; you would have to build
  `vello_cpu` as a `cdylib`/`dylib` crate type, which needs a patched copy of its manifest
  and gives you a Rust ABI that is not stable across compiler versions. For a dependency
  that changes only when you bump egui, that is a lot of fragility to save 12 s.
* **The rebuild you are actually paying for is not vello.** Cargo already compiles it once
  per target dir and caches it; a normal edit-compile loop never rebuilds it. What makes
  your loop slow is *your own* crates plus the final link. The 12 s only shows up on a cold
  tree — which, with 133 lockfiles, is more often than it should be, and that is a target-dir
  problem rather than a vello problem.

## 5. Linkers

The measurement above (18 s with the system linker, 18 s with lld) is honest but
unrepresentative: that test binary links almost nothing. Link time scales with the number
and size of objects, and an egui app pulls in wgpu, winit and a windowing stack. That is
where a linker swap pays, and it is worth re-measuring on a real app rather than on my
synthetic one.

What is in the tree now:

* `apps/logscope/.cargo/config.toml` — `-fuse-ld=gold` plus `-C prefer-dynamic`. **gold is
  deprecated** and has been removed from recent binutils; mold or lld is strictly better
  today.
* `apps/fao-codec/.cargo/config.toml` — `-C target-cpu=native`. Fine for a local build, but
  it produces binaries that will `SIGILL` on an older CPU, so it must never be used for
  anything you push to a device. Worth a comment in that file.
* Everything else uses the default `cc` → system linker.

Available in this container: `rust-lld` (ships with the toolchain), `gcc-ld`. No mold.

The modern setting, per target rather than globally so a cross-build is unaffected:

```toml
[target.x86_64-unknown-linux-gnu]
rustflags = ["-C", "link-arg=-fuse-ld=mold"]   # or lld
```

Rust 1.90 already defaults to lld for `x86_64-unknown-linux-gnu`, so on a current toolchain
the explicit flag mostly matters for *other* targets — including the aarch64 phone targets,
where the cross linker is whatever `gitmind`'s `buildflags::cross_env_on_path` finds.

## 6. Binary size

I did not measure a stripped release build of an egui app this turn — a release build of one
takes long enough that it would have been the whole turn. The levers, in the order they
usually pay:

```toml
[profile.release]
strip = "symbols"       # usually the largest single win on a Rust binary
lto = "thin"            # "fat" is slower to build, a little smaller
codegen-units = 1       # slower build, smaller and faster output
panic = "abort"         # drops the unwinder; check nothing in the tree catches panics
opt-level = "z"         # smaller, and slower — measure before adopting
```

Two notes specific to this tree:

* `panic = "abort"` interacts with anything that relies on `catch_unwind`. Several apps here
  run work on background threads and report failures rather than dying; that pattern is
  usually `Result`-based, but it is worth grepping before flipping it globally.
* `opt-level = "z"` on `vello_cpu` specifically would be interesting to measure, since a
  monomorphised SIMD pipeline is exactly the kind of code that bloats. It is also exactly
  the code where being slow hurts, since it runs per glyph on a cache miss.

## 7. Weak CPUs, phones, and RISC-V

This is the part with a concrete finding.

`fearless_simd 0.3.0` — the dispatch layer under `vello_cpu` — has SIMD implementations for
exactly three architecture families:

```rust
#[cfg(target_arch = "aarch64")]                                    // NEON
#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]    // wasm SIMD
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]           // SSE/AVX
```

**There is no RISC-V vector path.** On a RISC-V target the dispatch falls back to
`Level::Fallback`, i.e. scalar code, and glyph rasterisation runs at whatever a scalar loop
manages. That does not break anything — it is a correct fallback, and text will render — but
if you are targeting a RISC-V SBC, expect font atlas population to be materially slower than
on an ARM phone with NEON, and expect the cost to appear as a stutter the first time a new
glyph size or font is used rather than as a steady frame-rate drop.

For the ARM phone targets this project actually ships to (postmarketOS, Fairphone,
PinePhone, Asahi), aarch64 gets the NEON path and this is a non-issue.

Two other things that matter more than SIMD on a weak device:

* **`u8_pipeline` vs `f32_pipeline`.** epaint enables both. The u8 pipeline is the cheap one;
  the f32 one exists for higher-precision compositing. I have not traced which one epaint's
  glyph fill actually selects — worth knowing, because if it is f32 on a device with no FPU
  headroom that is a lot of avoidable work per glyph.
* **The atlas is the real cost.** Rasterisation happens once per (glyph, size, hinting)
  triple, then it is a texture blit. Anything that makes sizes churn — and a font size
  computed as a fraction of a box does exactly that, which is why `uikit_egui::crisp` snaps
  them — forces re-rasterisation of the whole alphabet at a new size. On a phone that is the
  difference you would actually feel, and it is under your control in a way that vello's
  SIMD level is not.

## 8. If you want one thing

Set a shared `target-dir` and `debug = "line-tables-only"`. Measured here, that is the
difference between paying 19 s and 120 MB per app per cold build and paying it once. The
rest of this document is refinement on top of that.
