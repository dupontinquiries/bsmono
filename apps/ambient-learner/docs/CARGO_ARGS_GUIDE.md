# Cargo Argument Passing Guide

## 🐛 **The `--` Issue**

### Problem:
```bash
cargo r -- --ui gtk      # ❌ Unknown argument
cargo r --release        # ✅ Works (uses default gtk)
```

### Why This Happens:

Cargo uses `--` to separate cargo arguments from program arguments:

```bash
cargo run [CARGO_ARGS] -- [PROGRAM_ARGS]
         ↑               ↑
    cargo options    your app options
```

**BUT** in debug mode, cargo needs feature selection BEFORE `--`:

### The Fix:

```bash
# Debug mode (correct way):
cargo run --features gtk -- --ui gtk
cargo run --features iced -- --ui iced

# Release mode (correct way):
cargo run --release --features gtk -- --ui gtk
cargo run --release --features iced -- --ui iced

# Using default feature:
cargo run --release    # Uses default gtk, no --ui needed
```

---

## ✅ **Correct Commands**

### GTK UI (Debug):
```bash
# Method 1: Use default feature
cargo run

# Method 2: Specify feature + UI
cargo run --features gtk -- --ui gtk

# Method 3: Just build and run directly
cargo build && ./target/debug/ambient-learner --ui gtk
```

### GTK UI (Release):
```bash
cargo run --release
# or
cargo run --release --features gtk -- --ui gtk
```

### Iced UI (Debug):
```bash
cargo run --features iced -- --ui iced
```

### Iced UI (Release):
```bash
cargo run --release --features iced -- --ui iced
```

### Headless:
```bash
cargo run --release -- --headless
```

---

## 🎯 **Recommended Workflow**

### For Development:
```bash
# Build once
cargo build --all-features

# Then run directly (faster):
./target/debug/ambient-learner --ui gtk
./target/debug/ambient-learner --ui iced
./target/debug/ambient-learner --headless
```

### For Testing:
```bash
# Release build with all features
cargo build --release --all-features

# Test each UI:
./target/release/ambient-learner --ui gtk
./target/release/ambient-learner --ui iced
./target/release/ambient-learner --headless
```

---

## 🔧 **Make It Easier with Aliases**

Add to `~/.bashrc` or `~/.zshrc`:

```bash
# Ambient Learner shortcuts
alias al-gtk='cargo run --release --features gtk'
alias al-iced='cargo run --release --features iced -- --ui iced'
alias al-headless='cargo run --release -- --headless'
alias al-build='cargo build --release --all-features'
```

Then just:
```bash
al-gtk      # Run GTK UI
al-iced     # Run Iced UI
al-headless # Run headless mode
```

---

## 📝 **Alternative: Use `cargo-make`**

Install:
```bash
cargo install cargo-make
```

Create `Makefile.toml`:
```toml
[tasks.gtk]
command = "cargo"
args = ["run", "--release", "--features", "gtk"]

[tasks.iced]
command = "cargo"
args = ["run", "--release", "--features", "iced", "--", "--ui", "iced"]

[tasks.headless]
command = "cargo"
args = ["run", "--release", "--", "--headless"]

[tasks.build-all]
command = "cargo"
args = ["build", "--release", "--all-features"]
```

Then:
```bash
cargo make gtk       # Run GTK
cargo make iced      # Run Iced
cargo make headless  # Run headless
cargo make build-all # Build everything
```

---

## 🎯 **Quick Reference**

| What You Want | Command |
|---------------|---------|
| GTK (debug) | `cargo run` |
| GTK (release) | `cargo run --release` |
| Iced (debug) | `cargo run --features iced -- --ui iced` |
| Iced (release) | `cargo run --release --features iced -- --ui iced` |
| Headless | `cargo run --release -- --headless` |
| Build all | `cargo build --all-features` |
| Run pre-built | `./target/release/ambient-learner --ui [gtk\|iced]` |

---

## 💡 **Why Features Are Needed**

The `--ui` flag tells the *already-compiled* program which UI to use.
The `--features` flag tells *cargo* which code to compile.

If you compile with `--features gtk` only, then `--ui iced` won't work because Iced code isn't compiled in!

```bash
# This compiles only GTK:
cargo build --features gtk

# So this fails (Iced not compiled):
./target/debug/ambient-learner --ui iced
# Error: "Iced UI not available"

# Solution: Compile with both:
cargo build --all-features
./target/debug/ambient-learner --ui iced  # ✅ Works
```

---

## 🚀 **Best Practice**

**For development:**
```bash
# One-time: build with all features
cargo build --all-features

# Then run directly (instant startup):
./target/debug/ambient-learner --ui gtk
./target/debug/ambient-learner --ui iced
```

**For release:**
```bash
# Build once
cargo build --release --all-features

# Run many times:
./target/release/ambient-learner --ui gtk
./target/release/ambient-learner --ui iced
```

This avoids the cargo overhead and makes switching UIs instant!
