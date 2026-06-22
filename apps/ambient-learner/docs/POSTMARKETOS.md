# Building on postmarketOS (aarch64)

This guide is specifically for building the Ambient Learner app natively on aarch64 postmarketOS devices (like PinePhone, etc.).

## Quick Installation

postmarketOS is based on Alpine Linux, so we use `apk`:

```bash
# Install Rust, Cargo, GTK4, and libadwaita development packages
sudo apk add rust cargo gtk4.0-dev libadwaita-dev

# Additional required packages
sudo apk add pkgconfig gcc musl-dev
```

## Complete Build Environment Setup

For native compilation on your postmarketOS device:

```bash
sudo apk add \
    rust \
    cargo \
    gtk4.0-dev \
    libadwaita-dev \
    glib-dev \
    cairo-dev \
    pango-dev \
    gdk-pixbuf-dev \
    librsvg-dev \
    pkgconfig \
    gcc \
    g++ \
    make \
    musl-dev
```

## Building the Application

```bash
cd /path/to/ambient-learner
cargo build --release
```

The build may take a while on mobile hardware (especially on devices like PinePhone). Expect 5-15 minutes depending on your device.

## postmarketOS-Specific Considerations

### 1. **Thermal Sensors**
postmarketOS devices expose thermal sensors at `/sys/class/thermal/thermal_zone*/temp`. This application will automatically detect and monitor all available thermal zones on your device.

To check what sensors are available:
```bash
ls -l /sys/class/thermal/
cat /sys/class/thermal/thermal_zone0/temp
```

### 2. **Display Scaling**
Mobile devices often use HiDPI displays. You may want to set GTK scaling:
```bash
# In your ~/.profile or session startup
export GDK_SCALE=2
export GDK_DPI_SCALE=0.5
```

Or for Phosh (default postmarketOS UI):
```bash
gsettings set org.gnome.desktop.interface scaling-factor 2
```

### 3. **Performance Optimization**
For faster builds on resource-constrained devices:

```bash
# Reduce parallel jobs to avoid running out of memory
cargo build --release -j 2

# Or set in ~/.cargo/config.toml:
```

Create or edit `~/.cargo/config.toml`:
```toml
[build]
jobs = 2

[profile.release]
opt-level = 2  # Slightly faster compilation than opt-level 3
lto = "thin"   # Thin LTO instead of full LTO
```

### 4. **Swap Space**
If you encounter out-of-memory errors during compilation:

```bash
# Check current swap
free -h

# Add swap file if needed (1GB example)
sudo dd if=/dev/zero of=/swapfile bs=1M count=1024
sudo chmod 600 /swapfile
sudo mkswap /swapfile
sudo swapon /swapfile

# Make permanent by adding to /etc/fstab:
echo '/swapfile none swap sw 0 0' | sudo tee -a /etc/fstab
```

### 5. **Battery Considerations**
Native compilation can drain battery quickly. It's recommended to:
- Keep device plugged in during build
- Build in release mode only when ready to deploy
- Use `cargo check` for quick validation during development

## Running the Application

```bash
./target/release/ambient-learner
```

### Running on Phosh
If using Phosh (GNOME-based mobile shell), the app should appear in your app drawer after building. You can create a desktop entry:

```bash
sudo nano /usr/share/applications/ambient-learner.desktop
```

Add:
```desktop
[Desktop Entry]
Name=Ambient Learner
Comment=Temperature monitoring and prediction
Exec=/path/to/ambient-learner/target/release/ambient-learner
Icon=utilities-system-monitor
Terminal=false
Type=Application
Categories=System;Monitor;
```

### Running on Plasma Mobile
Similar to Phosh, but you may want to adjust the window size in the source code for better mobile fit.

## Device-Specific Notes

### PinePhone / PinePhone Pro
- Usually has 2-4 thermal zones (CPU, GPU, battery, PMIC)
- Builds may take 10-20 minutes on PinePhone, 5-10 on PinePhone Pro
- Recommended to use at least 2GB swap

### Librem 5
- Similar thermal zones to PinePhone
- Generally faster compilation times
- May have better thermal management

## Optimizing for Mobile UI

The current window size (420x640) is already mobile-friendly, but you may want to adjust for your specific device:

In `src/main.rs`, modify:
```rust
let window = gtk::ApplicationWindow::builder()
    .application(app)
    .title("Ambient Learner")
    .default_width(360)   // Adjust for your screen
    .default_height(720)  // Adjust for your screen
    .build();
```

## Debugging on Device

To see debug output when running:
```bash
RUST_LOG=debug ./target/release/ambient-learner
```

To check which thermal sensors are being monitored:
```bash
RUST_BACKTRACE=1 ./target/release/ambient-learner
```

## Cross-Compilation Alternative

If native compilation is too slow, you can cross-compile from a faster machine:

```bash
# On your development machine (x86_64 Linux):
rustup target add aarch64-unknown-linux-musl

# Install cross-compilation toolchain
sudo apt-get install gcc-aarch64-linux-gnu

# Build
cargo build --release --target aarch64-unknown-linux-musl

# Copy to device
scp target/aarch64-unknown-linux-musl/release/ambient-learner user@device:/home/user/
```

However, native compilation ensures maximum compatibility with postmarketOS libraries.

## Troubleshooting

### "cannot find -lgcc_s"
This is normal with musl on Alpine-based systems. The binary will still work.

### Out of memory during link phase
Reduce optimization or add swap:
```bash
# Use less aggressive optimization
cargo build --profile release-small
```

Add to `Cargo.toml`:
```toml
[profile.release-small]
inherits = "release"
opt-level = "z"
lto = false
```

### GTK warnings about display
Make sure you're running in a graphical session (Phosh/Plasma Mobile), not SSH:
```bash
echo $DISPLAY
echo $WAYLAND_DISPLAY
```

## Performance Tips

1. Use `cargo build` (debug) during development - much faster
2. Only use `--release` for final builds
3. Consider `cargo-watch` for auto-rebuild during development:
   ```bash
   cargo install cargo-watch
   cargo watch -x check
   ```

4. Strip the binary to reduce size:
   ```bash
   strip target/release/ambient-learner
   ```

## Expected Results

On a typical PinePhone, you should see:
- 4-6 thermal zones detected
- Temperature readings updating every second
- Adaptive learning improving predictions over time
- Low battery impact when running (passive monitoring mode)
