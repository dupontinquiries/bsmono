# Tooling — recommended dev/CI toolset

Notes for the agent environment and for hardware/CI. **Caveat:** the authoring
sandbox has no Rust toolchain and resets between sessions, and it can't fetch
GitHub release binaries (blocked host). So everything below is documented to be
installed **on hardware/CI**, where it persists. In-sandbox I rely on
toolchain-free checks (rustcheck `--fast`, peek's grep audit, direct `wc`/`grep`).

## Toolchains

- **Rust 1.96** — supplied as an offline component dist (`rustc` + `cargo` +
  `std`) in the user's `rustup.zip`. Installs without network via each
  component's `install.sh --prefix=<dir>`.
- **Rust 1.75** — needed alongside 1.96 (e.g. `kanal` 0.1.0-pre8 requires
  rustc < 1.81; channel-bench pins it). Keep both selectable (rustup toolchains
  or two prefixes on PATH).

## Requested tools

| Tool | Purpose | Install (hardware/CI) | In-sandbox |
|---|---|---|---|
| **mold** | fast linker; big dev-build speedup | `apt-get install mold` (Ubuntu 24.04) or prebuilt | only matters when linking; document only |
| **sccache** | caches compiled crate artifacts | `cargo install sccache` or prebuilt binary | needs toolchain |
| **tokei** | LOC/code stats | `cargo install tokei` or prebuilt | release binary blocked here; use `wc`/`grep` instead |

Wire mold + sccache into builds via `~/.cargo/config.toml`:
```toml
[build]
rustc-wrapper = "sccache"
[target.x86_64-unknown-linux-gnu]
linker = "clang"
rustflags = ["-C", "link-arg=-fuse-ld=mold"]
```

## Recommended additions (rationale)

- **cargo-nextest** — faster, clearer test runner; good for the per-project CI.
- **cargo-deny** — license/advisory/dup-dependency gate across the workspace.
- **cargo-hakari** — workspace-hack to speed multi-crate builds in a monorepo.
- **cross** — clean cross-compiles to `aarch64`/`musl` (the phone/Alpine
  targets) without hand-built sysroots; complements snapclone's existing
  sysroot-over-rsync script.
- **bacon** — background `cargo check` while editing (fast feedback loop).
- **just** — task runner; a natural home for the build-orchestrator targets.
- **cargo-bloat** / **cargo-llvm-lines** — find size/compile-time hot spots
  (useful for unendlich's large tree).
- **typos** — fast source typo check; cheap CI suspender.

## System dependencies (apt) — for the build host

Direct answer to "anything else permanent via apt?": **nothing apt-installs
persists in the authoring sandbox** (it resets each session), and system
packages can't live in the repo. So the durable form is this documented set,
installed on your build host / CI image:

```sh
# toolchain + linker + native build
sudo apt-get install -y build-essential clang mold cmake ninja-build pkg-config
# Qt apps (voiceterm, bo-suite, and other Qt-cdylib frontends)
sudo apt-get install -y qt6-base-dev qt6-declarative-dev
# unendlich (Vulkan)
sudo apt-get install -y libvulkan-dev vulkan-validationlayers glslang-tools
# snapclone (GStreamer + the vendored opendht's deps)
sudo apt-get install -y libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
                        nettle-dev libgnutls28-dev libmsgpack-dev libargon2-dev
# bot-arena (GTK4)
sudo apt-get install -y libgtk-4-dev
```

(Verify exact package names against each project's build docs during the
build-wiring stage; this is the starting set, not a guarantee.) The Rust cargo
tools above (sccache/tokei/nextest/…) install per-user via `cargo install` or
prebuilt binaries — not apt.

## Dual-toolchain matrix (which projects want what)

- `kanal`-using benches → 1.75 (rustc < 1.81).
- Everything else → 1.96 (current).
- Document per-project pins in each project's `rust-toolchain.toml` during the
  build-wiring stage so CI picks the right one automatically.
