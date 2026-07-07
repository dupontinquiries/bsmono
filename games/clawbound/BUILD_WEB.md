# Building the CLAWBOUND web test client (WASM + WebGPU)

The Rust core (`src/lib.rs`) is platform-agnostic, so the web build is a
**recompile of the same game**, not a port. The only platform-specific piece is
`src/app.rs` — a winit + wgpu **surface** render loop that reuses the exact scene
builders (`build_scene`) and shaders (`SCENE_WGSL` / `POST_WGSL`) from the
headless renderer. It is compiled in behind the `app` (native) / `web` (wasm)
Cargo features and is already wired:

- native window: `cargo run --features app -- --play`   <- verified to compile
- web (wasm):    the `web_start()` fn in `app.rs` is the `#[wasm_bindgen(start)]`
                 entry; it binds to `<canvas id="clawbound">` in `web/index.html`.

> Why it isn't prebuilt in the authoring sandbox: that toolchain only ships the
> `x86_64` standard library -- no `wasm32` std and no `rustup` to add it, and
> `static.rust-lang.org` isn't reachable there. On a normal `rustup` install the
> steps below just work. The native windowed build DOES compile in the sandbox,
> which validates everything except the small wasm-bindgen glue.

## 1. One-time setup
    rustup target add wasm32-unknown-unknown
    cargo install --locked trunk           # or wasm-bindgen-cli (see 3b)

## 2. Enable the WebGPU backend for wgpu on wasm
The `web` feature already pulls the wasm deps (wasm-bindgen, web-sys, ...). wgpu
also needs its web backend on the wasm target -- add to Cargo.toml:

    [target.'cfg(target_arch = "wasm32")'.dependencies.wgpu]
    version = "22"
    features = ["webgpu", "webgl"]   # webgl = fallback without WebGPU

(The renderer already requests downlevel_webgl2_defaults limits on wasm so the
WebGL fallback path stays within spec.)

## 3a. Build + serve with Trunk (simplest)
    cd web
    trunk serve --release --features web       # http://localhost:8080

`web/index.html` loads `./pkg/clawbound_rs.js`; Trunk emits it. For a static
bundle: `trunk build --release --features web`.

## 3b. Or build with wasm-bindgen-cli directly
    cargo build --release --target wasm32-unknown-unknown --features web
    wasm-bindgen target/wasm32-unknown-unknown/release/clawbound-rs.wasm \
      --out-dir web/pkg --target web --no-typescript
    python3 -m http.server -d web 8080

## 4. Open it
Chrome/Edge 113+ (WebGPU by default) or Safari Technology Preview. Browsers
without WebGPU use the WebGL fallback. This is the low-fi phone test client --
same simulation and look as native, lower fidelity for a weak GPU.

## Notes
- Keep wasm-bindgen-cli the SAME version as the wasm-bindgen dependency.
- Blank canvas -> check the console; a WebGPU-unsupported error means use
  `["webgl"]` or a WebGPU-capable browser.
