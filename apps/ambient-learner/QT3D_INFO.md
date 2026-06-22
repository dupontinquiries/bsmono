# Qt3D Support Information

## Important: qt3d Crate Not Available

The `qt3d` Rust crate is **not available on crates.io**. This is why you get the "crate not found" error.

## How Qt3D Works in This Project

Qt3D support is handled **at runtime via QML**, not via Rust bindings:

### Architecture:

```
Rust Backend (qt_backend.rs)
     ↓
QML Runtime
     ↓
Qt3D Module (if installed on system)
     ↓
3D Rendering
```

### Building:

```bash
# Just use the qt feature (no qt3d needed!)
cargo build --release --features qt
```

### Runtime Detection:

The QML file automatically detects Qt3D:

```qml
// In NeuralSphere.qml
Loader {
    source: Qt3D.available ? 
        "NeuralSphere.qml" :    // 3D version
        "NeuralSphere2D.qml"    // 2D fallback
}
```

## Installing System Qt3D (Optional)

### Ubuntu/Debian:
```bash
sudo apt install qt6-3d-dev qml-module-qt3d
```

### Fedora:
```bash
sudo dnf install qt6-qt3d-devel
```

### Arch:
```bash
sudo pacman -S qt6-3d
```

### macOS:
```bash
brew install qt@6
# Qt3D included
```

## Verification

Check if Qt3D is available:

```bash
# List QML modules
qmlplugindump -notrelocatable QtQuick.Scene3D 2.15

# Should show Qt3D components if installed
```

## Fallback Behavior

| Qt3D Status | Result |
|-------------|--------|
| ✅ Installed | Beautiful 3D sphere with lighting, particles, rotation |
| ❌ Not installed | Clean 2D Canvas visualization (still looks great!) |

## Recommendation

**For most users:** Build with just `--features qt`

**For advanced users:** Install system Qt3D for full 3D experience

## No Rust Binding Needed!

Unlike other Qt features, Qt3D doesn't need Rust bindings because:
1. QML handles all 3D rendering
2. Rust backend just provides data
3. Runtime detection works automatically

This is actually **better** because:
- ✅ Smaller binary
- ✅ No compile-time dependencies
- ✅ Automatic graceful fallback
- ✅ Works on more systems

## Summary

**Don't add `qt3d` to Cargo.toml** - it doesn't exist on crates.io!

**Instead:**
1. Build: `cargo build --features qt`
2. (Optional) Install system Qt3D
3. Run and enjoy automatic detection

**Both 2D and 3D versions look beautiful!** 🎨
