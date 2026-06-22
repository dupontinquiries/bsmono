# Exact Changes for main.rs

## Change 1: Update Imports (Line 30-44)

### FIND THIS (around line 30):
```rust

use libadwaita as adw;
use adw::prelude::*;
use chrono::{Local, Utc};
use gtk4::gdk_pixbuf::PixbufLoader;
use glib;
use gtk4 as gtk;
```

### REPLACE WITH:
```rust

// CLI argument parsing
use clap::Parser;

// GTK imports (conditional)
#[cfg(feature = "gtk")]
use libadwaita as adw;
#[cfg(feature = "gtk")]
use adw::prelude::*;
#[cfg(feature = "gtk")]
use gtk4::gdk_pixbuf::PixbufLoader;
#[cfg(feature = "gtk")]
use glib;
#[cfg(feature = "gtk")]
use gtk4 as gtk;

use chrono::{Local, Utc};
```

---

## Change 2: Add UI Module (Line 78-79)

### FIND THIS (around line 78):
```rust
mod world_state;  // Central state management (WorldState architecture)
mod systems;      // Worker thread systems

#[cfg(feature = "qt")]
```

### REPLACE WITH:
```rust
mod world_state;  // Central state management (WorldState architecture)
mod systems;      // Worker thread systems
mod ui;           // UI abstraction layer (GTK, Iced, Qt)

#[cfg(feature = "qt")]
```

---

## Change 3: Add CLI Args and New main() (BEFORE the old main function)

### FIND THIS (around line 2930):
```rust
fn main() {
    // Use NON_UNIQUE flag to avoid D-Bus registration issues
```

### REPLACE WITH THIS ENTIRE SECTION:

```rust
// ============================================================================
// CLI Arguments
// ============================================================================

/// Ambient temperature learning app with multiple UI options
#[derive(Parser, Debug)]
#[command(name = "ambient-learner")]
#[command(author, version, about, long_about = None)]
struct Args {
    /// UI framework to use
    #[arg(short, long, value_name = "UI", default_value = "gtk")]
    #[arg(help = "UI framework: gtk, iced")]
    ui: String,
    
    /// Skip UI and run in headless mode
    #[arg(long)]
    headless: bool,
    
    /// Enable debug output
    #[arg(short, long)]
    debug: bool,
}

// ============================================================================
// Main Entry Point
// ============================================================================

fn main() {
    // Parse CLI arguments
    let args = Args::parse();
    
    if args.debug {
        println!("🐛 Debug mode enabled");
        println!("   UI: {}", args.ui);
        println!("   Headless: {}", args.headless);
    }
    
    // Run in headless mode if requested
    if args.headless {
        println!("🤖 Running in headless mode (no UI)");
        run_headless();
        return;
    }
    
    // Determine UI framework
    let ui_framework = match args.ui.to_lowercase().as_str() {
        "gtk" | "gtk4" => {
            #[cfg(feature = "gtk")]
            { "gtk" }
            #[cfg(not(feature = "gtk"))]
            {
                eprintln!("❌ GTK UI not available (compile with --features gtk)");
                std::process::exit(1);
            }
        }
        "iced" => {
            #[cfg(feature = "iced")]
            { "iced" }
            #[cfg(not(feature = "iced"))]
            {
                eprintln!("❌ Iced UI not available (compile with --features iced)");
                std::process::exit(1);
            }
        }
        other => {
            eprintln!("❌ Unknown UI framework: {}", other);
            eprintln!("   Available: gtk, iced");
            std::process::exit(1);
        }
    };
    
    println!("🎨 UI Framework: {}", ui_framework);
    
    // Run selected UI
    match ui_framework {
        "gtk" => run_gtk_ui(),
        "iced" => run_iced_ui(),
        _ => unreachable!(),
    }
}

// ============================================================================
// Headless Mode
// ============================================================================

fn run_headless() {
    println!("[1/5] Initializing sensors...");
    let poller = helpers::ThermalPoller::new();
    println!("✓ Found {} thermal sensors", poller.sensor_names().len());
    
    println!("[2/5] Creating models...");
    let model = Arc::new(Mutex::new(helpers::Model::new(poller.sensor_names())));
    let neural_field = Arc::new(Mutex::new(
        temperature_field::TemperatureField::new(poller.sensor_names().len())
    ));
    
    println!("[3/5] Creating WorldState...");
    let world = Arc::new(RwLock::new(SystemState::new()));
    {
        let mut state = world.write().unwrap();
        state.sensor_names = poller.sensor_names().clone();
        state.sensor_count = poller.sensor_names().len();
    }
    
    println!("[4/5] Spawning worker systems...");
    let _handles = systems::spawn_all(world.clone(), model, neural_field);
    
    println!("[5/5] Running headless loop...");
    println!("✓ Ready! Press Ctrl+C to exit");
    
    loop {
        std::thread::sleep(Duration::from_secs(10));
        
        if let Ok(state) = world.read() {
            println!("📊 Temp: {:.1}°C | Conf: {:.0}% | Model: {} | Cals: {}", 
                state.ambient_temp,
                state.confidence * 100.0,
                state.model_type.as_str(),
                state.calibration_count
            );
        }
    }
}

// ============================================================================
// Iced UI
// ============================================================================

#[cfg(feature = "iced")]
fn run_iced_ui() {
    println!("🎨 Launching Iced UI...");
    
    // Initialize core systems
    println!("[1/5] Initializing sensors...");
    let poller = helpers::ThermalPoller::new();
    println!("✓ Found {} thermal sensors", poller.sensor_names().len());
    
    println!("[2/5] Creating models...");
    let model = Arc::new(Mutex::new(helpers::Model::new(poller.sensor_names())));
    let neural_field = Arc::new(Mutex::new(
        temperature_field::TemperatureField::new(poller.sensor_names().len())
    ));
    
    println!("[3/5] Creating WorldState...");
    let world = Arc::new(RwLock::new(SystemState::new()));
    {
        let mut state = world.write().unwrap();
        state.sensor_names = poller.sensor_names().clone();
        state.sensor_count = poller.sensor_names().len();
    }
    
    println!("[4/5] Spawning worker systems...");
    let _handles = systems::spawn_all(world.clone(), model, neural_field);
    
    println!("[5/5] Starting Iced UI...");
    if let Err(e) = ui::iced::run(world) {
        eprintln!("❌ Iced UI error: {:?}", e);
        std::process::exit(1);
    }
}

#[cfg(not(feature = "iced"))]
fn run_iced_ui() {
    eprintln!("❌ Iced UI not compiled in");
    std::process::exit(1);
}

// ============================================================================
// GTK UI
// ============================================================================

#[cfg(feature = "gtk")]
fn run_gtk_ui() {
    println!("🎨 Launching GTK4 UI...");
    
    // Use NON_UNIQUE flag to avoid D-Bus registration issues
```

**NOTE:** The `run_gtk_ui()` function wraps your existing main() code. Everything after "Use NON_UNIQUE flag" stays exactly the same!

---

## Change 4: Close run_gtk_ui() at END of file

### FIND THIS (very last lines of file):
```rust
    println!("✓ UI initialization complete!");
}
```

### REPLACE WITH:
```rust
    println!("✓ UI initialization complete!");
}

#[cfg(not(feature = "gtk"))]
fn run_gtk_ui() {
    eprintln!("❌ GTK UI not compiled in");
    std::process::exit(1);
}
```

---

## Summary

**4 changes to main.rs:**

1. ✅ Line ~30: Make GTK imports conditional, add clap import
2. ✅ Line ~78: Add `mod ui;` declaration
3. ✅ Line ~2930: Replace old `main()` with new CLI-aware version + helper functions
4. ✅ Last line: Close `run_gtk_ui()` with fallback

**The key insight:** Your old `main()` function becomes `run_gtk_ui()`. Everything inside it stays the same!

---

## Quick Verification

After pasting, search for these to verify:

```bash
# Should find these:
grep "use clap::Parser" src/main.rs
grep "mod ui;" src/main.rs  
grep "fn run_gtk_ui()" src/main.rs
grep "fn run_iced_ui()" src/main.rs
grep "fn run_headless()" src/main.rs
```

All 5 should return results!
