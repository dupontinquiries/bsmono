# Sandbox build environment (v0.1.645+) — cargo check NOW WORKS

## What changed
Previously assumed the sandbox could only run pure-module tests. WRONG —
the user pointed out apt + llvm + Qt are reachable. Confirmed:
- apt-get reaches archive.ubuntu.com (network OK for Ubuntu pkgs + crates.io).
- Installed: clang-18 + libclang-dev + llvm-18-dev, qt6-base-dev +
  qt6-declarative-dev (Qt 6.4.2 + qmake6), libasound2-dev, cmake, pkg-config.
- The REAL blocker was the Rust version: a transitive dep (idna_adapter)
  needs edition2024 → Rust >=1.85; sandbox shipped 1.75. The user's
  uploaded offline bundle (rust-1_96_0-x86_64) installed via its INSTALL.sh
  to /opt/rust196 fixed it.

## RESULT: `cargo check` compiles main.rs (Qt bridge + all)
- `cargo check` (default features): PASSES (qmetaobject C++ bridge builds,
  main.rs type-checks, ~1m25s).
- `cargo check --features voice`: PASSES (whisper-rs bindgen via clang,
  cpal/alsa). ~1m23s.
- NOT yet tested: --features llamacpp (needs the Vulkan SDK/headers; the
  loader libvulkan.so.1 is present but no ICD/lavapipe — llamacpp also only
  matters for in-process inference, not the core/Qt compile).

## How to invoke (every session — these are NOT persisted across resets)
```
# one-time per sandbox (network required):
apt-get update && apt-get install -y clang libclang-dev llvm-dev \
  qt6-base-dev qt6-declarative-dev libasound2-dev cmake pkg-config
# install rust 1.96 from the uploaded bundle:
cd /tmp && unzip -q <rust-1_96_0-x86_64...zip> && \
  PREFIX=/opt/rust196 sh rust-install/INSTALL.sh
# then for every cargo invocation:
export PATH=/opt/rust196/bin:$PATH
export LIBCLANG_PATH=/usr/lib/llvm-18/lib
export PKG_CONFIG_PATH=/usr/lib/x86_64-linux-gnu/pkgconfig
cargo check                  # core + Qt bridge
cargo check --features voice # + whisper/cpal
```

## What this upgrades
The whole class of "needs your build to verify" FFI/qt_method/wiring work
can now be COMPILE-checked here before shipping. Still can't RUN it (no
display/GPU/audio), so gesture/render/voice-runtime behavior still needs
the user's device — but type/borrow/missing-initializer/FFI-signature
errors are now caught in-sandbox. This is why e.g. the v625 E0063
missing-initializer bug, or the v637 detent that referenced an undefined
property, would now be caught BEFORE the user builds (well — cargo check
catches Rust-side; the QML undefined-property is still runtime-only).

## Caveat
Env is per-session (filesystem resets). Re-run the install block each new
sandbox. ~3-4 min total setup. Worth it for any turn that touches FFI/
qt_method/wiring; pure-module turns can still use the fast
scripts/cargo_test_pure.sh harness without the full toolchain.
