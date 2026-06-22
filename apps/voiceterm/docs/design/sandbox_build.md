# Sandbox full-build capability (discovered v0.1.645→646)

## TL;DR
The sandbox CAN compile the full project (main.rs + qmetaobject + scraper
+ portable-pty + vte, and the `voice` feature's whisper-rs/cpal/alsa FFI).
The only thing that ever blocked it was the stock toolchain being Rust
1.75, too old for a transitive dep (idna_adapter) that needs edition2024
(cargo >= 1.85). Installing Rust 1.96 from the user's offline bundle fixed
it. clang 18, Qt6 (libs + headers), libvulkan, and crates.io access are
all present.

## Setup (per fresh sandbox — the install lives in $HOME/.local)
    cd /tmp && unzip -q /mnt/user-data/uploads/rust-1_96_0-x86_64-unknown-linux-gnu.zip
    PREFIX=$HOME/.local sh rust-install/INSTALL.sh
    export PATH="$HOME/.local/bin:$PATH"   # also appended to ~/.bashrc

## What builds (verified)
- `cargo check --no-default-features`  → main.rs + qmetaobject + scraper +
  portable-pty + vte. ~7s.
- `cargo check --features voice`        → adds whisper-rs-sys (bindgen),
  alsa, cpal FFI. ~10s cold.
- NOT yet tried: `--features llamacpp` (llama-cpp-2 + GGML_VULKAN). libvulkan
  is present; may work, may need more. Try when relevant.

## What this changes
Compile-checking main.rs is now possible HERE — catches qt_struct
missing-initializer (E0063), FFI signature mismatches, and feature-gated
breakage BEFORE the user builds. The old "pure modules only" limit was an
untested assumption; it's lifted for compile-checking. (Running the GUI
still needs the user's machine — no display/Vulkan device here — and the
user's real binary is aarch64 Asahi, so this is a HOST x86_64 check, not
their artifact.)

## Caveat learned
A warning under one --features config (e.g. an "unused import" that's only
used in a feature-gated line) is NOT necessarily dead in another config.
Check the configs that USE the symbol before "cleaning up."

## UPDATE: llamacpp is the one wall (CMake/Vulkan SDK)
`cargo check --features "voice claude llamacpp"` fails ONLY in the ggml
Vulkan backend's CMake step: ggml-vulkan/CMakeLists.txt find_package(Vulkan)
fails — libvulkan.so is present but the full Vulkan SDK + glslc shader
compiler are not. Everything ELSE compiles. So the working compile gate is:
    cargo check --features "voice claude"
This covers main.rs, qmetaobject (qt_struct/qt_method correctness), all the
voice FFI, and feature-gated code — i.e. it catches the E0063 / FFI / type
errors that previously could only surface on the user's build. The
llama-vulkan inference path still needs the user's machine to compile.
