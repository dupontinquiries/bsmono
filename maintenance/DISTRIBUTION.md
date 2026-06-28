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

`appcatalog.rs` — the recognition + metadata foundation, all unit-tested:
- `scan_dir` / `parse_app_listing` — recognise real repos (Cargo.toml and/or .git),
  not arbitrary folders. The Deploy tab now badges recognised apps (📦) and the
  listing is limited to what's a real app when you ask for info.
- `app_info_local` / `parse_app_details` — the catalog metadata: **name, crate
  count, on-disk size, README excerpt** (exactly the fields the client shows).
- `remote_listing_cmd` / `remote_details_cmd` — the SSH-side shell that produces the
  same data from the server; `deploy::remote_capture` runs them; the parsers are
  tested, the SSH execution is on-device.
- **Build-and-ship seam** (`cross_build_argv`, `install_dir`, `built_binary_path`,
  `BUILD_TARGETS`) — tested command/path construction for the next step.

## What's next (on-device, not yet wired)

1. **Build-type picker + build**: client picks a `BUILD_TARGETS` triple, server runs
   `cross_build_argv` (the Asahi box has `cross` + the aarch64 musl/gnu std; the
   sandbox has neither, so this only runs on the real machine).
2. **Ship the binary**: scp `built_binary_path` → the client's `install_dir(home)`
   over the ssh seam that already exists (`SSH_OPTS`, host-key trust, key auth). Or
   drop in `/tmp` and let the client `cp` it (avoids writing another device's bin dir
   directly).
3. **Dependencies / "package manager" niceties**: an app can declare what it needs
   (toolkits, runtime libs, sibling apps); the server resolves + reports them before
   install, so this becomes a personal mirror/package manager with gitmind's
   content-addressed history underneath. This is where `sharekit` (see SHARING.md)
   and the `dht://` provider plug in for discovery without typing `host:port`.

## Security note

The installed-key restrictions from the defence-in-depth work (`restrict` +
`from="<lan/24>"`, no agent/port forwarding outbound) apply to every ssh hop the
distributor makes, so a compromised device can't ride the install path to the rest
of the fleet.
