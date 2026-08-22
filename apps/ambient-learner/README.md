# Ambient Learner

AI-powered ambient temperature learning system with GTK4 and Qt/QML UIs.

## Features

- 🌡️ **Live Temperature Display** - Real-time ambient temperature
- 🧠 **Neural Network Model** - Self-learning temperature prediction
- 📊 **History Charts** - Track temperature over time
- 📍 **Location Management** - Different profiles per location
- 📡 **Database Sync** - Import/export calibrations
- 🎨 **Modern UI** - GTK4 with Libadwaita or Qt/QML
- 📱 **Mobile-First** - Touch-optimized, responsive design

## Quick Start

### GTK Version (Recommended)
```bash
cargo build --release
./target/release/ambient-learner
```

### Qt Version (Optional)
```bash
cargo build --release --features qt
./target/release/ambient-learner
```

## Qt3D Note

**The qt3d crate is not available on crates.io.** 

Qt3D support requires:
1. System Qt installation with Qt3D module
2. QML runtime detection

The application automatically detects Qt3D availability:
- ✅ **Qt3D available:** Full 3D neural sphere
- ✅ **Qt3D unavailable:** 2D Canvas fallback (still beautiful!)

No special build flag needed - it's runtime detection!

## Installation

See `INSTALL.md` for detailed setup instructions.

### Quick Dependencies

**Ubuntu/Debian (GTK):**
```bash
sudo apt install libgtk-4-dev libadwaita-1-dev
cargo build --release
```

**Ubuntu/Debian (Qt - Optional):**
```bash
sudo apt install qt6-base-dev qt6-declarative-dev
cargo build --release --features qt
```

## Documentation

130+ documentation files in `docs/` covering:
- Bug fixes (14 critical)
- Optimizations (9 systems)
- Testing framework
- Mobile UX guide
- 3D visualization
- And much more!

## Key Docs

- `INSTALL.md` - Installation guide
- `PROJECT_STRUCTURE.md` - Code organization
- `docs/BUG_HUNT_REPORT.md` - Known issues
- `docs/CRITICAL_FIXES_IMPL.md` - Ready fixes
- `docs/QT_MOBILE_COMPLETE.md` - Mobile UI

## Testing

```bash
cargo test           # Run tests
cargo bench          # Run benchmarks
RUST_LOG=debug cargo run  # Debug mode
```

### egui-frontend (2026-08-19)

`egui-frontend/` (a separate crate, deliberately — see its own Cargo.toml
comment on why iced and eframe can't coexist in one manifest here) had no
build-status record anywhere. A session with a real rustc 1.96 (clears egui
0.34's MSRV) ran it for the first time: `cargo test` is 141/141, spanning
the map view (lat/long projection round-trips, nearest-point tap selection,
non-finite-coordinate defense), the pressure/barometer dial, the screen-
temperature heatmap (including version/staleness/corruption defenses on
the published frame format), model sync/calibration display, and the shared
UI model layer. Compiles clean.

## License

MIT License
