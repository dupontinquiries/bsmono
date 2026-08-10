# Compile time: what I measured, and where I was wrong

Measured `apps/voiceterm/egui-frontend` from clean with `cargo build --timings`
rather than reasoning about it.

```
clean build            122 s
voiceterm-egui's own code   4.3 s   ← 3.5%
everything else           118 s     ← dependencies
```

**The app's own code is under 4% of a clean build.** That reframes the question:
splitting a crate for parallelism does nothing for a clean build dominated by
dependencies, and dependencies are cached — so the number that matters day to
day is the *rebuild*, which is exactly the 4%.

Two levers follow, and they are for different builds.

## Clean builds: dependency count, not app structure

The slowest units were `x11rb-protocol` (10.5s), `vello_cpu` (6.6s), `egui`
(5.8s), `read-fonts` (5.0s), `wayland-protocols` (4.9s).

**Twenty-four crates in this repo build both the x11 and wayland backends.** A
machine runs one. So `app` now decomposes into `app-x11` and `app-wayland`, with
the default unchanged so nothing breaks.

**The measured saving was 5 s, not the 15 s I projected**, because `winit` still
pulls part of `x11rb` transitively — six artifacts survive in a wayland-only
build against ten. The split is still right (it is a real choice a phone target
should have) but it is a 4% win, and calling it more would be inventing a
result.

## Rebuilds: the profile was optimising the wrong thing

`apps/voiceterm` had `[profile.dev] opt-level = 1`, which applies to **the crate
and every dependency**. That is backwards for both:

* Own code — 133k lines in **one crate**, so every touch pays a full
  single-threaded frontend pass. Optimising it costs time on every edit.
* Dependencies — compiled once and cached, so optimising them costs nothing
  after the first build.

Now `opt-level = 0` for own code, `opt-level = 2` and `debug = false` for
`package."*"`. logscope had already found `split-debuginfo = "unpacked"`; that is
now applied here too.

## The real structural blocker, unfixed

`apps/voiceterm` is **133,691 lines in a single crate**, with `main.rs` alone at
23,387. Rust's frontend — parse, type-check, borrow-check — is single-threaded
*per crate*, so no amount of `codegen-units` parallelises it. That is the
multi-threading ceiling for this app, and it is a real refactor rather than a
setting: the fix is extracting leaf modules into crates that build in parallel
and rebuild independently.

`nproc` in this container is **1**, so the timing run measured a serial build and
cannot say how much parallelism a split would actually buy. That number needs a
real machine before anyone spends a week on the refactor.
