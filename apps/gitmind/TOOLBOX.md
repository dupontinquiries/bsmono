# gitmind toolbox

The workspace builds a set of independently-distributable binaries. Pure-
Rust tools cross-compile cleanly; the Qt GUI builds natively where Qt6 lives.

## Binaries

| binary        | crate         | needs Qt | what it is                                   |
|---------------|---------------|----------|----------------------------------------------|
| `gitmind`     | gitmind-cli   | no       | CLI + frontend dispatcher (launches GUI on touch) |
| `gitmind-gui` | gitmind-gui   | **yes**  | qmetaobject + QML touch UI                   |
| `peek`        | peek          | no       | fast line-range file viewer                  |
| `peek2`       | peek          | no       | indexed viewer (buckets, `@symbol`, context) |
| `gitmind-sim` | gitmind-sim   | no       | storage benchmark harness                    |

`cargo build --release` builds everything **except** `gitmind-gui` (it's
excluded from `default-members` so a plain build never needs Qt). Build the
GUI explicitly on a Qt6 host: `cargo build --release -p gitmind-gui`.

## The GUI ↔ core bridge (FFI)

`gitmind-gui` exposes one `Backend` QObject (qmetaobject) as the QML
`backend` context property. Every method calls **gitmind-core** — the exact
same code the CLI runs — and returns JSON the QML parses. So the CLI and GUI
can't drift: a core fix fixes both. The GUI's file viewer calls the **peek**
library directly (`peek::view::read_to_string`). QML is embedded with `qrc!`
so the shipped GUI is a single self-contained binary.

## Cross builds

Pure-Rust bins via [`cross`](https://github.com/cross-rs/cross) (handles
target linkers in a container):

```bash
cross build --release --target aarch64-unknown-linux-gnu \
    -p gitmind-cli -p peek -p gitmind-sim
```

`dist/build-toolbox.sh [targets…]` wraps this and collects the binaries
under `dist/<target>/`. Common targets: `x86_64-unknown-linux-gnu`,
`aarch64-unknown-linux-gnu` (phones / postmarketOS), `x86_64-pc-windows-gnu`.

**GUI cross-compilation is the hard exception:** a Qt app needs Qt6 built
for the target, which `cross` doesn't provide. Build `gitmind-gui` natively
on each platform — on postmarketOS (aarch64) that means building on-device
or in an aarch64 container that has Qt6. The pure-Rust tools have no such
constraint.
