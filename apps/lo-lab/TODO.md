# lo-lab — is `just-js/lo` a good runtime for a small-screen tablet app?

Status: **parked, nothing built yet.** Upstream snapshot is in `vendor/lo` (see its `UPSTREAM.txt`; unmodified).
This folder only holds the questions. Written 2026-10-03.

## What lo actually is (checked in the source, not the README's pitch)

- A **JavaScript runtime on V8**, written in **C++** (`lo.cc`, `main.cc`, ~20 C++/header files) with a JS layer
  (`lib/`, `main.js`). **There is no Rust in it.** The "Rust-like" idea does not apply to the runtime itself.
- Its pitch is a very small, fast-starting runtime that reaches C libraries through **V8 fast-call FFI bindings**
  (`bindings/`) instead of a large built-in API surface. It ships a musl/glibc compat shim (`musl-glibc-compat.c`)
  and an arm64 build target (`make ARCH=arm64 lo`).
- It links system libssl, zlib and a custom libcurl by default.

So the real question is not "Rust successor" but: **does a tiny V8 runtime beat our Rust + eink-kit path on the
reMarkable Paper Pro, for apps like jotter/scribe?**

## Questions to answer, in order

1. **Does it build for the tablet at all?** The tablet needs a *static aarch64 musl* binary (see the jotter
   container build, `gitmind eink build --engine container`). lo's default build is dynamic glibc + libcurl.
   Try `make ARCH=arm64` in the `tideshell-aarch64` alpine image first; note every dependency that fights it.
2. **Size and memory.** Binary size, hello-world RSS and startup time on the laptop (aarch64) and then on the
   tablet. V8 alone is large; compare against jotter's static Rust binary (and its RSS) for the same screen.
3. **Can it draw?** lo has no DOM and no canvas. Painting would go through an FFI binding to `eink-kit`
   (framebuffer via qtfb/AppLoad). Prototype: a binding that blits a buffer and reads touch events. If that needs
   a Rust shim anyway, the runtime is adding a layer, not removing one.
4. **Battery.** The tablet is e-ink and idle most of the time, so the number that matters is idle wake cost and
   time-to-first-frame, not throughput. Measure idle CPU with a timer wakeup once a second.
5. **What would JS buy us?** Honest answer to write down before building: faster UI iteration without a
   container build per change (jotter's build loop is long), at the cost of a second language and V8's footprint.
   If hot-reload on the tablet is the only win, a scripting layer in Rust (an embedded interpreter) may be cheaper.

## Decision rule

Only continue past step 3 if steps 1–2 give a static binary within a small multiple of jotter's size and RSS.
Otherwise write the numbers here and close it — a clean "no, because" is a fine result (CLAUDE.md §7b: archive,
do not leave it half-wired).

## Not goals

No wiring into any app's manifest, no changes under `vendor/lo`, no claim about performance until measured here.
