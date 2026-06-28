# App distribution — the "gitmind package manager" direction

This captures the redesign of "push + launch" into a pull-based app distributor: a
client browses a server's *recognised apps*, asks for one built for its target, and
the server cross-builds it and ships the binary. The current push-the-whole-monorepo
flow is the older shape; this is where it's going.

## The flow

```
client (wants an app)                         server (has the apps)
─────────────────────                         ─────────────────────
log into GUI                                  log into GUI
  │                                              │
  ├─ browse recognised apps  ───────────────▶  scan_dir / remote_listing_cmd
  │   (repos only, with badges)                 → AppEntry{name,is_cargo,is_git}
  │                                              │
  ├─ tap an app → see info  ────────────────▶  remote_details_cmd / app_info_local
  │   (name, crate count, size, README)         → AppDetails{crates,size,readme}
  │                                              │
  ├─ "download + install" ──┐                    │
  │   pick a build target   │                    │
  │                         └──────────────────▶ cross_build_argv(app, triple)
  │                                              → cross build --release --target …
  │                                              │
  └─ binary lands in ~/.local/bin  ◀───────────  ship to install_dir(home)
      (or /tmp then the device cp's it)           (scp over the existing ssh seam)
```

## What's built now (verifiable)

`appcatalog.rs` — recognition + metadata (name, crate count, size, README), local
and over SSH; the Deploy tab badges recognised apps (📦) and shows their info.

`buildflags.rs` — the customizable build knobs, all unit-tested:
- `scan_cargo_features` — an app's `[features]` + its `default` set.
- `scan_cmake_options` — `option(NAME "help" ON/OFF)` from CMakeLists.txt.
- `arch_default_features` — the repo's **per-architecture** default set (see the
  metadata convention below), falling back to `[features] default`.
- `cargo_build_argv` / `cmake_configure_argv` — the cross-build command from a chosen
  flag set; `built_binary_path` / `install_dir` for delivery.

`install_app` (main.rs, async job) — wires the **cargo** path end to end: cross-build
on the server → pull the binary → install. Delivery follows the chosen route:
- **default — stage via /tmp**: `scp` the binary to `/tmp/<app>`, then `cp` into
  `~/.local/bin`. The `/tmp` copy stays, so a bad build/binary is easy to inspect.
- **direct**: `scp` straight into `~/.local/bin`.
Build output (incl. errors) is captured and returned to the client, so a failed
cross-build shows up in the GUI instead of vanishing.

The Deploy tab's installer panel exposes all of the above: pick a target (musl/gnu),
the repo's per-arch default features come pre-ticked, toggle from there, choose the
delivery route, install.

### Per-architecture default-features convention

In an app's Cargo.toml:

```toml
[package.metadata.gitmind]
default-features-musl = ["small", "no-x11"]
default-features-gnu  = ["full"]
```

Whichever matches the target's libc is pre-enabled for the client; absent that, the
plain `[features] default` set is used.

## What's next (on-device, not yet wired)

1. **CMake builds**: options are scanned + exposed today, and `cmake_configure_argv`
   builds the `-D…` command, but cross-compiling CMake needs a per-target toolchain
   file — that's an on-device prerequisite, so the CMake *build* (vs flag exposure)
   is the remaining piece for C++ apps.
2. **Dependencies / "package manager" niceties**: an app declares what it needs
   (toolkits, runtime libs, sibling apps); the server resolves + reports them before
   install. With gitmind's content-addressed history underneath this becomes a
   personal mirror/package manager. `sharekit` + `dht://` (see SHARING.md) plug in
   here for discovery without typing `host:port`.

## Security note

The installed-key restrictions from the defence-in-depth work (`restrict` +
`from="<lan/24>"`, no agent/port forwarding outbound) apply to every ssh hop the
distributor makes, so a compromised device can't ride the install path to the rest
of the fleet.
