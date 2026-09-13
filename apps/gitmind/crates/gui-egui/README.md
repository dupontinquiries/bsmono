# gitmind-egui

The main GUI for gitmind (Qt/QML in `../gui` is the fallback). All nine tabs —
Fleet, Push, Network, Apps, Import, Branches, Files, Timeline, Settings — call
`cockpit::` directly; see `../../GITMIND_EGUI_PORT.md` for the port's history
and architecture notes.

## Build

```sh
# Headless: the lib, every page, and all tests — no eframe, no display.
# This is what CI / a sandbox without a GPU should run.
cargo build -p gitmind-egui --lib --no-default-features
cargo test  -p gitmind-egui --no-default-features

# The runnable app (default-on: a plain `cargo build -p gitmind-egui`
# already includes this — `--features app` is only needed alongside
# --no-default-features).
cargo build -p gitmind-egui --features app
```

If the mold linker pin in the workspace bites on your machine, clear
`RUSTFLAGS` for the build:

```sh
RUSTFLAGS='' cargo build -p gitmind-egui --features app
```

## Launch

```sh
cargo run -p gitmind-egui --features app
# or, after building:
./target/debug/gitmind-egui
```

Running the `app`-featured binary without a display (or on a build where
`eframe` didn't get linked) prints a one-line message telling you to add
`--features app` rather than failing silently — see `main.rs`.

For scripted screenshots or UI tests, `GITMIND_EGUI_TAB=<name>` starts on a
given tab instead of the default (matches `Tab::label()`, case-insensitive —
e.g. `GITMIND_EGUI_TAB=settings cargo run -p gitmind-egui --features app`).
This is a testing knob, not a user-facing setting.

## Renderer

The app pins `eframe::Renderer::Glow` explicitly in `main.rs` rather than
letting eframe pick — leaving it to eframe let wgpu in via feature
unification on at least one host (Asahi) and it exceeded that adapter's
`max_color_attachments` before ever drawing a frame. If you add a dependency
that pulls in `egui-wgpu`, check that this pin is still what runs.

## Troubleshooting a window that won't open

`uikit_egui::backend_check::preflight()` runs first and prints a diagnosis
before `eframe::run_native` gets a chance to panic inside a dependency. The
usual cause on Linux is a static-musl build that can't `dlopen`
`libwayland-client`; the panic that `wayland-sys` itself would print points
at a package that's already installed, which is why the preflight check
exists — read its message rather than the panic underneath it.
