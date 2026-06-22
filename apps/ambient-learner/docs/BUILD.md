# Ambient Learner - Build Instructions

This is a Rust GTK4 application that monitors thermal sensors and uses adaptive learning to predict ambient temperature.

## Prerequisites

### 1. Install Rust
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
```

### 2. Install GTK4 Development Libraries

**Ubuntu/Debian:**
```bash
sudo apt-get update
sudo apt-get install libgtk-4-dev
```

**Fedora:**
```bash
sudo dnf install gtk4-devel
```

**Arch Linux:**
```bash
sudo pacman -S gtk4
```

**Alpine Linux:**
```bash
sudo apk add rust cargo gtk4.0-dev
```

**macOS (using Homebrew):**
```bash
brew install gtk4
```

## Building the Application

### Method 1: Using Cargo (Recommended)

1. Navigate to the project directory:
```bash
cd /path/to/ambient-learner
```

2. Build the project:
```bash
cargo build --release
```

3. Run the application:
```bash
cargo run --release
```

### Method 2: Direct Compilation

If you prefer to compile without Cargo:

```bash
rustc src/main.rs \
  --edition 2021 \
  --extern gtk4=$(pkg-config --libs gtk4) \
  --extern glib=$(pkg-config --libs glib-2.0) \
  -o ambient-learner
```

## Troubleshooting

### Common Issues

**Issue 1: "gtk4 not found"**
- Make sure you've installed the GTK4 development libraries
- Verify with: `pkg-config --modversion gtk4`

**Issue 2: "cannot find crate for gtk4"**
- Run `cargo clean` and then `cargo build` again
- Check your Cargo.toml dependencies

**Issue 3: No thermal sensors found**
- This app reads from `/sys/class/thermal/thermal_zone*/temp`
- Only works on Linux systems with accessible thermal sensors
- You may need to run with appropriate permissions

**Issue 4: SVG rendering issues**
- Make sure gdk-pixbuf is properly installed
- The SVG rendering uses librsvg under the hood

## Running the Application

The compiled binary will be located at:
```
target/release/ambient-learner
```

You can run it with:
```bash
./target/release/ambient-learner
```

## Features

- **Temperature Monitoring**: Reads from Linux thermal sensors
- **Adaptive Learning**: Uses weighted statistics to predict ambient temperature
- **Calibration**: Allows manual calibration to improve accuracy
- **Visual Feedback**: Shows sensor weights with animated SVG visualization
- **Confidence Metric**: Displays prediction confidence based on sensor variance

## System Requirements

- Linux operating system (for thermal sensor access)
- GTK4 runtime libraries
- Read access to `/sys/class/thermal/`

## Notes

- The application monitors thermal sensors in `/sys/class/thermal/thermal_zone*/temp`
- If no thermal sensors are found, the model will initialize with 0 sensors
- The calibration feature allows you to manually input known temperature values to improve accuracy
