# Installation & Build Instructions

## Prerequisites

### For GTK UI (Default)
**Ubuntu/Debian:**
```bash
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev
```

**Fedora:**
```bash
sudo dnf install gcc pkg-config gtk4-devel libadwaita-devel
```

**Arch:**
```bash
sudo pacman -S base-devel gtk4 libadwaita
```

**macOS:**
```bash
brew install gtk4 libadwaita
```

### For Qt UI (Optional)
**Ubuntu/Debian:**
```bash
sudo apt install qt6-base-dev qt6-declarative-dev qt6-charts-dev qt6-3d-dev
```

**Fedora:**
```bash
sudo dnf install qt6-qtbase-devel qt6-qtdeclarative-devel
```

**macOS:**
```bash
brew install qt@6
```

### Rust
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

## Building

### GTK Version (Recommended)
```bash
cd ambient-learner-project
cargo build --release
```

Binary location: `target/release/ambient-learner`

### Qt Version
```bash
cargo build --release --features qt
```

**Note:** Qt3D support is detected at runtime via QML, not via Rust crate.
See `QT3D_INFO.md` for details on optional Qt3D installation.

## Running

### First Run
```bash
# GTK
./target/release/ambient-learner

# Qt
cargo run --release --features qt
```

### With Debug Logging
```bash
RUST_LOG=debug ./target/release/ambient-learner
```

### With Metrics
```bash
RUST_LOG=info ./target/release/ambient-learner
```

## Testing

### Run All Tests
```bash
cargo test
```

### Run Specific Test
```bash
cargo test test_temperature_bounds
```

### Run Benchmarks
```bash
cargo bench
```

## Installing System-Wide

### Linux
```bash
sudo cp target/release/ambient-learner /usr/local/bin/
sudo chmod +x /usr/local/bin/ambient-learner
```

### Desktop Entry (Linux)
```bash
cat > ~/.local/share/applications/ambient-learner.desktop << 'DESKTOP'
[Desktop Entry]
Name=Ambient Learner
Comment=AI-powered temperature learning
Exec=/usr/local/bin/ambient-learner
Icon=temperature-symbolic
Terminal=false
Type=Application
Categories=Utility;Science;
DESKTOP
```

## Troubleshooting

### Build Fails
```bash
# Clean and rebuild
cargo clean
cargo build --release
```

### GTK Not Found
```bash
# Check GTK installation
pkg-config --modversion gtk4

# Should output: 4.x.x
```

### Qt Not Found
```bash
# Check Qt installation
qmake --version

# Should show Qt 5.15+ or Qt 6.x
```

### Runtime Errors
```bash
# Enable verbose logging
RUST_LOG=trace ./target/release/ambient-learner 2>&1 | tee debug.log
```

### Permission Issues
```bash
# Ensure config directory is writable
mkdir -p ~/.config/ambient-learner
chmod 755 ~/.config/ambient-learner
```

## Data Directory

Application data stored at:
- **Linux**: `~/.config/ambient-learner/`
- **macOS**: `~/Library/Application Support/ambient-learner/`
- **Windows**: `%APPDATA%\ambient-learner\`

## Uninstalling

```bash
# Remove binary
sudo rm /usr/local/bin/ambient-learner

# Remove data (optional)
rm -rf ~/.config/ambient-learner

# Remove desktop entry
rm ~/.local/share/applications/ambient-learner.desktop
```

## Next Steps

After successful installation:
1. Run the application
2. Calibrate with a known temperature
3. Let it learn from your environment
4. Check the documentation in `docs/` folder

For detailed documentation, see:
- `README.md` - Project overview
- `PROJECT_STRUCTURE.md` - Code organization
- `docs/QUICK_START.md` - Getting started guide
- `docs/BUG_HUNT_REPORT.md` - Known issues & fixes
