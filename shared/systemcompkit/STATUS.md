# systemcompkit — status and design notes

Compositor-integration state-tracking for a shell built with `eguikit` over
`cosmic-comp`. This file exists so the reasoning behind each protocol choice
lives in one place a future session can read directly, rather than requiring
someone to reconstruct it from three separate commit messages.

## The split, and why

Mirrors `bt-kit` exactly, for the same reason: `idle.rs`, `layer.rs`, and
`toplevel.rs` are pure — no `wayland-client`, no `smithay`, testable with
plain Rust values. `wayland_io.rs` (feature `wayland_io`, off by default) is
the thin, real layer that actually opens a socket. Most of the monorepo has
no reason to link a Wayland client library just to get the tracking logic;
only whatever becomes the actual shell needs the real thing.

## Protocol choices, each verified against source rather than assumed

| Module | Protocol | Verified how |
|---|---|---|
| `idle` | `ext-idle-notify-v1` | `cosmic-comp`'s own source (`cosmic-idle`'s handling), paired with `wlr-output-power-management` for the screen-off step (that pairing is a caller's job, not this crate's) |
| `layer` | `wlr-layer-shell-unstable-v1` | real `layer_shell.rs` handler found in `cosmic-comp`'s source |
| `toplevel` | `ext-foreign-toplevel-list-v1` + COSMIC's `zcosmic_toplevel_info_v1` extension | protocol XML's own text ("it thus extends ext_foreign_toplevel_v1") — **not** `cosmic-protocols`' own shipped example, which uses a path the XML marks `deprecated-since="2"` |
| `capture` | `ext-image-copy-capture-v1` + `ext-image-capture-source-v1` | confirmed correct at the time `layer`/`toplevel` were built (the earlier "not yet implemented" note was for this exact protocol pair) |
| (not `toplevel`) | `wlr-foreign-toplevel-management` | deliberately **not** used — confirmed non-interoperable with `cosmic-comp`, which speaks the `ext-`/`zcosmic` family instead. This is the same protocol-family lesson the phosh-vs-KWin research surfaced earlier; recorded here so it isn't re-litigated by someone who only reads this crate in isolation |
| (not `capture`) | `wlr-screencopy-unstable-v1` | deliberately **not** used — `cosmic-comp` has moved to the newer standardized protocol above |
| `wayland_io::LayerSurface::paint_argb8888` | `wl_shm` (core protocol) | this crate's own `examples/paint_probe.rs`-then-`ldd` check (see below) — not a doc claim, an actual link-time verification |

## The Vulkan/EGL vs. wl_shm finding, recorded here since it shapes `wayland_io`'s own dependency shape

`wayland-client`'s **default** backend (what every type in this file uses)
is pure-Rust with no C ABI underneath — no `wl_display*`/`wl_proxy*` pointer
exists to hand to `raw-window-handle`, which any GPU rendering path
(Vulkan via `topbar-render`, or EGL/GL) needs. That's not a Rust-bindings
limitation to route around: confirmed by checking ARM's own open-source
Vulkan WSI layer's build dependencies, which list `libwayland-client` as a
hard requirement — the C library dependency is inherent to how Wayland's
GPU-presentation extensions work, not a choice this crate or `topbar-render`
made. `wl_shm` (what `paint_argb8888` uses) is different: it's plain wire
protocol — an fd passed over the same socket everything else in this file
already speaks — so it needs none of that. Verified, not assumed: built a
throwaway example binary linking `LayerSurface::connect` with the
`wayland_io` feature on (and `system` feature deliberately left OFF), then
ran `ldd` against it and grepped for `wayland` in the output. Empty. No
`libwayland-*.so` anywhere in the link.

## What's real vs. what's the honest ceiling

Every `wayland_io.rs` type-checks against the real crate APIs (`wayland-client`
0.31, `wayland-protocols` 0.32, `wayland-protocols-wlr` 0.3, `cosmic-protocols`
0.2) — confirmed by actually compiling against them, catching and fixing
real mismatches along the way (`ExtIdleNotifierV1`'s `Dispatch` event type,
`Connection::flush`'s real error type, a wrong `wayland-protocols` feature
flag caught by reading the crate's own macro invocation).

What is **not** verified anywhere in this crate: the actual round trip
against a running compositor. There is no Wayland compositor in a build
sandbox to connect to. Every claim in this file and in the module-level docs
is scoped to "compiles against the real API" — nothing stronger. The first
real test of any of this is running it against `cosmic-comp` on actual
hardware.

## Known simplifications, not oversights

- **`toplevel`'s property updates aren't batched on the protocol's own
  `done` event.** The protocol double-buffers property changes and signals
  "done changing for now" separately; this dispatches each `Title`/`AppId`/
  `StateChanged` as it arrives. `ToplevelTracker::apply` already tolerates
  that correctly (no wrong data results), but a caller could in principle
  observe a toplevel mid-update across two dispatch cycles rather than
  seeing every field change atomically together. Fixing this means giving
  `ToplevelTracker` a staged-then-committed model, a real design change, not
  a quick patch — not done here.
- **`layer`'s surface creation doesn't handle a compositor that rejects the
  requested layer/anchor combination.** The protocol allows a compositor to
  simply not configure a surface it doesn't like; this crate has no timeout
  or fallback for that case yet.
- **No reconnect/global-removal handling anywhere.** All three `wayland_io`
  session types assume a short-lived connection for one purpose; `Dispatch`
  impls for `wl_registry` are empty no-ops rather than reacting to a global
  disappearing mid-session. A long-lived shell process would need this
  filled in.

## What isn't built yet

- `topbar-core`/`topbar-render` now consume this crate (`layer`'s
  `top_panel`/`side_panel`, `LayerSurface::request_resize`/`paint_argb8888`,
  `LayerSurface::connection()` for raw-window-handle) — the "nothing
  consumes this crate" gap noted above is closed. See
  `shared/topbar-STATUS.md` for that work's own status and TODOs; this
  file stays scoped to `systemcompkit` itself.
- `capture`'s `CaptureSource::Toplevel` path is now wired (`ToplevelSession::
  capture_toplevel(id, paint_cursors)`), retaining the real
  `ExtForeignToplevelHandleV1` proxies specifically for this — the gap
  named right after this crate's toplevel-wiring commit is closed.
- `capture`'s actual buffer allocation and frame request/read-back —
  deliberately out of scope everywhere in this crate; see `capture.rs`'s own
  header for why that's a different kind of work than protocol-state
  tracking.
- **Buffer reuse for `paint_argb8888`.** A fresh `tempfile`+`wl_shm_pool`+
  `wl_buffer` per call is correct (the `Release` event handler destroys each
  buffer once the compositor's done with it — nothing leaks) but wasteful
  for a real per-frame paint loop. Double-buffering is real, undone work.
- **Live-resize (`request_resize`) is unexercised against a real compositor**,
  same ceiling as everything else in this file per the section above.

## The actual next decision, not an engineering task

Whether `hatch-egui` becomes that shell — using `layer::LayerSurfaceIntent`
instead of its current plain toplevel window, using `toplevel`/`idle` for
switcher/lock-timing behavior — is a product decision, not something to
infer from this crate existing. `hatch-egui`'s own design deliberately
avoided layer-shell entirely (a poke-file-toggled plain window, sidestepping
the whole surface-role question) and that was the right call for what it
was at the time. Revisiting it is a real choice about what `hatch` is *for*
now, not a natural continuation this crate should assume for anyone.
