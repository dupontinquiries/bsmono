# ecosystem monorepo

Unified tree for the Rust / Qt / C++ portfolio. This is the **skeleton stage**:
the source is organized and de-duplicated, but nothing here has been compiled or
wired into a single build yet. The next stages add (a) static verification via
the suspender tools, then (b) the cross-target build + Flatpak orchestration.

## Layout

```
apps/        user-facing applications
system/      system-tier services / daemons (snapclone, photosync)
tools/       dev-loop + verification suspenders (peek, drift, rustcheck)
vendor/      vendored third-party (opendht — current upstream)
manifest/    machine-readable project + target metadata (drives build/CI)
build/       cross-target build orchestrator (stub)
verify.sh    runs the suspenders over the whole tree (stub — next stage)
.gitmind/    gitmind graph store (to be initialised by `gin` on hardware)
```

See `LAYOUT.md` for the taxonomy rationale, the branching model, and the
deferred build-wiring tasks.

## Why an orchestrator, not one cargo workspace

The portfolio mixes Cargo, CMake/C++ (snapclone, photosync, opendht) and Qt.
A single `cargo build` cannot span CMake or nested Cargo workspaces, so the
unified build is an **orchestrator** that drives each sub-project with its own
build system, per target triple. `manifest/projects.toml` declares how.

## Verification model

Heavy crates (unendlich → Vulkan/glfw, snapclone → C++/gst) are not compiled in
the authoring sandbox. Instead the **suspenders** — `tools/peek` (wiring-audit),
`tools/rustcheck`, `tools/drift` (boundary-drift) — provide lightweight static
checks over the tree. Real compilation happens on developer hardware.
