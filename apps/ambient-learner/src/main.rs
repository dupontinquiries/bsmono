// src/main.rs
//! # Ambient Learner - Local Temperature Prediction
//!
//! A machine learning application that predicts ambient temperature using
//! device thermal sensors. Learns from user calibrations to adapt to
//! different environments and locations.
//!
//! ## Features
//!
//! - **Adaptive Learning**: Automatically adjusts to different environments
//! - **Location Awareness**: Tracks performance per location via WiFi
//! - **Passive Mode**: Continuously learns from sensor data
//! - **Active Calibration**: User-provided ground truth for accuracy
//! - **Historical Analysis**: View temperature trends over time
//! - **Data Export**: CSV export for external analysis
//!
//! ## Architecture
//!
//! - **Model**: Weighted linear regression with adaptive sensor selection
//! - **Database**: SQLite with WAL mode for concurrent access
//! - **UI**: GTK4/libadwaita for native Linux/mobile experience
//! - **Storage**: Compressed archives for efficient space usage
//!
//! ## Performance
//!
//! - Startup: <3 seconds
//! - Calibration: <5 seconds (20 sensor readings)
//! - UI Updates: 10 FPS (100ms refresh)
//! - Storage: <20 MB/month with compression

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

// The engine (src/engine.rs) carries its own imports now; what is left here
// is what the GTK frontend itself still reaches for.
use rusqlite::Connection;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// Import temperature field types
use temperature_field::TemperatureEstimator;

// Import WorldState architecture
use world_state::SystemState;

// ============================================================================
// Module Declarations
// ============================================================================

mod helpers;
mod forecast;  // Temperature forecasting with neural network
mod sync;      // Distributed database sync via NFC
mod security;  // Cryptographic security (pubkey signing & encryption)
mod dbmaint;   // Database maintenance and bad sector pruning
mod db_writer; // Async batched writer for high-frequency rows
mod temperature_field;  // Physics-aware 3D temperature field
mod sphere;    // Temperature sphere visualization
mod world_state;  // Central state management (WorldState architecture)
mod systems;      // Worker thread systems
mod ui;           // UI abstraction layer (GTK, Iced, Qt)
mod environment;  // IIO sensors: pressure, humidity, light
mod seasons;      // Season + time-of-day "mood" for UI theming
mod screen_temps; // 2D screen-temperature bitfield published to /run
mod screen_temp_hints; // Consumer-driven resolution hints for the bitfield

#[cfg(feature = "dbus")]
mod iio_proxy;    // iio-sensor-proxy D-Bus client (light, accelerometer)

#[cfg(feature = "service")]
mod service;      // Public D-Bus API + XDG inbox watcher

// Utility modules (small, self-contained — currently unused but available)
#[allow(dead_code)]
mod constants;     // Centralized configuration values
#[allow(dead_code)]
mod error;         // Structured error types
#[allow(dead_code)]
mod lock_utils;    // Lock-with-timeout helper
#[allow(dead_code)]
mod metrics;       // Performance counters

#[cfg(feature = "qt")]
mod qt_backend;  // Qt/QML backend bridge

// SIMD module disabled - requires nightly Rust
// Re-enable: `mod simd;` and add `src/simd.rs` (uses std::simd, nightly only)

// ============================================================================
// Phase 1: Viewport Detection & Responsive Layout
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq)]
enum ViewportMode {
    Desktop,   // ≥1200px
    Tablet,    // 768-1199px  
    Mobile,    // <768px
}

impl ViewportMode {
    fn from_width(width: i32) -> Self {
        match width {
            w if w >= 1200 => ViewportMode::Desktop,
            w if w >= 768 => ViewportMode::Tablet,
            _ => ViewportMode::Mobile,
        }
    }
    
    fn as_str(&self) -> &'static str {
        match self {
            ViewportMode::Desktop => "desktop",
            ViewportMode::Tablet => "tablet",
            ViewportMode::Mobile => "mobile",
        }
    }
}

// ============================================================================
// Dynamic Styling Helpers
// ============================================================================

/// Get temperature gradient class based on temperature
fn get_temp_gradient_class(temp: f32) -> &'static str {
    if temp < 10.0 {
        "temp-gradient-cold"
    } else if temp < 18.0 {
        "temp-gradient-cool"
    } else if temp < 24.0 {
        "temp-gradient-moderate"
    } else if temp < 28.0 {
        "temp-gradient-warm"
    } else {
        "temp-gradient-hot"
    }
}

/// Get glow class based on temperature
fn get_temp_glow_class(temp: f32) -> &'static str {
    if temp < 10.0 {
        "glow-cold"
    } else if temp < 18.0 {
        "glow-cool"
    } else if temp < 24.0 {
        "glow-moderate"
    } else if temp < 28.0 {
        "glow-warm"
    } else {
        "glow-hot"
    }
}

/// Get confidence class based on model confidence
fn get_confidence_class(confidence: f32) -> &'static str {
    if confidence > 0.8 {
        "confidence-high"
    } else if confidence > 0.5 {
        "confidence-medium"
    } else {
        "confidence-low"
    }
}

/// Get the seasonal CSS class for the *northern hemisphere* meteorological
/// season corresponding to a given month (1-based).
fn get_season_class(month: u32) -> &'static str {
    match month {
        12 | 1 | 2 => "season-winter",
        3..=5      => "season-spring",
        6..=8      => "season-summer",
        _          => "season-autumn",
    }
}

/// Get the day-phase CSS class for a given hour (0–23).
fn get_phase_class(hour: u32) -> &'static str {
    match hour {
        22..=23 | 0..=4 => "phase-night",
        5..=7           => "phase-dawn",
        8..=11          => "phase-morning",
        12..=16         => "phase-afternoon",
        17..=19         => "phase-dusk",
        20..=21         => "phase-evening",
        _               => "phase-afternoon",
    }
}

/// Get a "weather mood" CSS class for the temperature display.
///
/// Returns an empty string for the comfortable mid-range so the label
/// shows its plain color. At the extremes we layer on either `aurora`
/// (cold) or `heat-haze` (hot) for a small atmospheric effect.
fn get_temp_mood_class(temp: f32) -> &'static str {
    if temp <= 5.0      { "aurora" }
    else if temp >= 28.0 { "heat-haze" }
    else                 { "" }
}

/// Get a CSS class describing the current pressure trend.
/// `trend_hpa` is the 30-minute delta in hectopascals.
fn get_pressure_trend_class(trend_hpa: f32) -> &'static str {
    if trend_hpa >  0.5 { "pressure-rising" }
    else if trend_hpa < -0.5 { "pressure-falling" }
    else                     { "pressure-steady" }
}

/// Get a CSS class describing sensor health.
/// Used on the small "Sensors locked / degraded / unavailable" pill.
fn get_health_class(ok: bool, degraded: bool) -> &'static str {
    if !ok && !degraded { "health-unavailable" }
    else if degraded    { "health-degraded" }
    else                { "health-ok" }
}

/// Apply temperature/confidence styling and the matching atmospheric
/// "mood" class (heat-haze / aurora / nothing) to a widget.
///
/// Seasonal and day-phase classes are applied separately at the page
/// container level via `apply_seasonal_styling` so that the entire
/// page tints together rather than just the temperature display.
fn apply_dynamic_styling(widget: &impl gtk::prelude::WidgetExt, temp: f32, confidence: f32) {
    // Classes this function manages — every category is exhaustively
    // listed so that switching e.g. cold→hot fully removes the old set.
    let classes_to_remove = [
        "temp-gradient-cold", "temp-gradient-cool", "temp-gradient-moderate",
        "temp-gradient-warm", "temp-gradient-hot",
        "glow-cold", "glow-cool", "glow-moderate", "glow-warm", "glow-hot",
        "confidence-high", "confidence-medium", "confidence-low",
        "aurora", "heat-haze",
    ];
    for class in &classes_to_remove {
        widget.remove_css_class(class);
    }

    widget.add_css_class(get_temp_gradient_class(temp));
    widget.add_css_class(get_temp_glow_class(temp));
    widget.add_css_class(get_confidence_class(confidence));
    widget.add_css_class("smooth-transition");

    let mood = get_temp_mood_class(temp);
    if !mood.is_empty() {
        widget.add_css_class(mood);
    }
}

/// Apply a seasonal + day-phase tint to a page container (page1..page5
/// or the window itself). Removes any prior season/phase classes so
/// transitioning across the day is clean.
fn apply_seasonal_styling(
    widget: &impl gtk::prelude::WidgetExt,
    month: u32,
    hour: u32,
) {
    let seasons = ["season-winter", "season-spring", "season-summer", "season-autumn"];
    let phases  = ["phase-night", "phase-dawn", "phase-morning",
                   "phase-afternoon", "phase-dusk", "phase-evening"];
    for c in seasons.iter().chain(phases.iter()) {
        widget.remove_css_class(c);
    }
    widget.add_css_class(get_season_class(month));
    widget.add_css_class(get_phase_class(hour));
}

/// Apply a "calibration just happened" flash to a label.
///
/// The CSS class triggers a 1.2-second one-shot animation. We schedule
/// removal slightly *after* the keyframe finishes so subsequent calls
/// re-trigger cleanly (otherwise re-adding the class while it's already
/// present is a no-op and the user sees nothing on rapid calibrations).
fn pulse_calibration_flash(widget: &impl gtk::prelude::WidgetExt) {
    use gtk::glib;
    let widget_clone = widget.clone();
    widget.remove_css_class("calibration-flash");
    // Add on the next event-loop tick so the remove takes effect first;
    // re-adding within the same tick would not re-trigger the animation.
    glib::idle_add_local_once(move || {
        widget_clone.add_css_class("calibration-flash");
        let widget_for_cleanup = widget_clone.clone();
        glib::timeout_add_local_once(
            std::time::Duration::from_millis(1300),
            move || {
                widget_for_cleanup.remove_css_class("calibration-flash");
            },
        );
    });
}

/// Apply a stagger-fade-in to a sequence of widgets so they appear
/// one-after-the-other when the page first shows.
///
/// Up to five widgets get individual delay classes; anything past
/// position 5 just gets the base `stagger-fade` so they all fade in
/// after the others.
/// Apply a staggered fade-in CSS animation to a row of widgets.
///
/// Up to five widgets get individual delay classes; anything past
/// position 5 just gets the base `stagger-fade` so they all fade in
/// after the others.
///
/// We take `&[&gtk::Widget]` rather than the previous
/// `&[&dyn WidgetExt]` because the WidgetExt trait isn't dyn-compatible
/// in modern gtk-rs (its `PartialEq` / `PartialOrd` supertraits use
/// `Self` as a type parameter). Every gtk widget upcasts cleanly to
/// `gtk::Widget` via the `AsRef<gtk::Widget>` blanket impl that
/// gtk-rs provides for every `IsA<Widget>` type, so callers can pass
/// `temp_label.upcast_ref::<gtk::Widget>()` or — more conveniently —
/// rely on the `as_ref()` coercion in the call site below.
fn apply_stagger_fade(widgets: &[&gtk::Widget]) {
    use gtk::prelude::WidgetExt;
    for (i, w) in widgets.iter().enumerate() {
        w.add_css_class("stagger-fade");
        let position_class = match i {
            0 => "stagger-fade-1",
            1 => "stagger-fade-2",
            2 => "stagger-fade-3",
            3 => "stagger-fade-4",
            4 => "stagger-fade-5",
            _ => continue,
        };
        w.add_css_class(position_class);
    }
}

#[cfg(test)]
mod styling_tests {
    use super::*;

    #[test]
    fn season_class_covers_year() {
        assert_eq!(get_season_class(1),  "season-winter");
        assert_eq!(get_season_class(2),  "season-winter");
        assert_eq!(get_season_class(3),  "season-spring");
        assert_eq!(get_season_class(5),  "season-spring");
        assert_eq!(get_season_class(6),  "season-summer");
        assert_eq!(get_season_class(8),  "season-summer");
        assert_eq!(get_season_class(9),  "season-autumn");
        assert_eq!(get_season_class(11), "season-autumn");
        assert_eq!(get_season_class(12), "season-winter");
    }

    #[test]
    fn phase_class_covers_24h() {
        assert_eq!(get_phase_class(0),  "phase-night");
        assert_eq!(get_phase_class(6),  "phase-dawn");
        assert_eq!(get_phase_class(10), "phase-morning");
        assert_eq!(get_phase_class(14), "phase-afternoon");
        assert_eq!(get_phase_class(18), "phase-dusk");
        assert_eq!(get_phase_class(21), "phase-evening");
        assert_eq!(get_phase_class(23), "phase-night");
    }

    #[test]
    fn temp_mood_only_at_extremes() {
        assert_eq!(get_temp_mood_class(20.0), "");
        assert_eq!(get_temp_mood_class(15.0), "");
        assert_eq!(get_temp_mood_class(0.0),  "aurora");
        assert_eq!(get_temp_mood_class(35.0), "heat-haze");
        // Boundary: 5 is still cold (≤), 28 is still hot (≥)
        assert_eq!(get_temp_mood_class(5.0),  "aurora");
        assert_eq!(get_temp_mood_class(28.0), "heat-haze");
    }

    #[test]
    fn pressure_trend_classifies() {
        assert_eq!(get_pressure_trend_class(2.0),  "pressure-rising");
        assert_eq!(get_pressure_trend_class(0.6),  "pressure-rising");
        assert_eq!(get_pressure_trend_class(0.0),  "pressure-steady");
        assert_eq!(get_pressure_trend_class(-0.3), "pressure-steady");
        assert_eq!(get_pressure_trend_class(-1.5), "pressure-falling");
    }

    #[test]
    fn health_class_priorities() {
        assert_eq!(get_health_class(true,  false), "health-ok");
        assert_eq!(get_health_class(true,  true),  "health-degraded");
        assert_eq!(get_health_class(false, true),  "health-degraded");
        assert_eq!(get_health_class(false, false), "health-unavailable");
    }
}

// ============================================================================
// Location Menu Rebuild Helper
// ============================================================================

/// Rebuild location menu from current location list
///
/// This function is called whenever locations change to update the menu immediately.
/// Without this, the menu requires an app restart to show new locations.
fn rebuild_location_menu(
    menu_box: &gtk::Box,
    popover: &gtk::Popover,
    locations: &Arc<Mutex<Vec<Location>>>,
    current_location_idx: &Arc<Mutex<usize>>,
    menu_btn: &gtk::MenuButton,
) {
    println!("🔄 Rebuilding location menu...");
    
    // Remove all existing children
    while let Some(child) = menu_box.first_child() {
        menu_box.remove(&child);
    }
    
    // Rebuild menu from current locations
    let locs = locations.lock().unwrap();
    
    for (idx, location) in locs.iter().enumerate() {
        let btn = gtk::Button::with_label(&location.name);
        btn.add_css_class("flat");
        btn.set_hexpand(true);
        
        // Clone for closure
        let current_idx = current_location_idx.clone();
        let menu_btn = menu_btn.clone();
        let popover = popover.clone();
        let location_name = location.name.clone();
        
        btn.connect_clicked(move |_| {
            *current_idx.lock().unwrap() = idx;
            menu_btn.set_label(&location_name);
            popover.popdown();
            println!("Switched to location: {}", location_name);
        });
        
        menu_box.append(&btn);
    }
    
    println!("✓ Location menu rebuilt with {} items", locs.len());
}

// ============================================================================
// The engine: configuration, Model, ThermalPoller, Database, device state
// ============================================================================
//
// Extracted to src/engine.rs so the egui frontend can share it. Everything
// that was here is still reachable by the same names through the glob.
mod engine;
use engine::*;

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
    #[arg(help = "UI framework: gtk, iced, qt")]
    ui: String,
    
    /// Skip UI and run in headless mode
    #[arg(long)]
    headless: bool,
    
    /// Enable debug output
    #[arg(short, long)]
    debug: bool,

    /// Import a database from this path, copying it into the
    /// app's data directory under a fresh `sensors_<ts>.db` name
    /// and using it for this session.
    ///
    /// The original file is not modified. If a database with that
    /// destination filename already exists, the import is refused.
    ///
    /// Combine with --headless to do an import-only run.
    #[arg(long, value_name = "PATH")]
    import_db: Option<PathBuf>,

    /// Snapshot the current database to this path using SQLite's
    /// online backup API (safe even if writers are active) and
    /// then exit.
    #[arg(long, value_name = "PATH")]
    export_db: Option<PathBuf>,

    /// Override the discovered "latest database" with an explicit
    /// path. Unlike --import-db, this doesn't copy — the app
    /// reads and writes the file in-place.
    #[arg(long, value_name = "PATH")]
    db: Option<PathBuf>,
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
        if let Some(p) = &args.db {
            println!("   DB path override: {}", p.display());
        }
    }

    // Apply --db override before anything that calls
    // get_latest_database(). Subsequent CLI ops (--export-db,
    // --import-db) and all UI launches consult this path.
    if let Some(path) = args.db.as_ref() {
        set_db_override(path.clone());
    }

    // --export-db: snapshot the current "latest" database and exit.
    // Done first so it works even on a partially-broken install.
    if let Some(dest) = args.export_db.as_ref() {
        match export_database_cli(dest) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("❌ Export failed: {e}");
                std::process::exit(1);
            }
        }
    }

    // --import-db: copy an external database into our data dir
    // under a fresh timestamped name. Subsequent runs will pick it
    // up via get_latest_database().
    if let Some(src) = args.import_db.as_ref() {
        match import_database_cli(src) {
            Ok(path) => {
                println!("✅ Imported database into {}", path.display());
                // If the user passed --headless along with --import-db,
                // exit after the import. Otherwise fall through to
                // the normal UI/headless dispatch — they want to
                // start the app on the just-imported DB.
                if !args.headless {
                    // No-op; continue to UI launch.
                } else {
                    std::process::exit(0);
                }
            }
            Err(e) => {
                eprintln!("❌ Import failed: {e}");
                std::process::exit(1);
            }
        }
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
        "qt" | "qml" => {
            #[cfg(feature = "qt")]
            { "qt" }
            #[cfg(not(feature = "qt"))]
            {
                eprintln!("❌ Qt UI not available (compile with --features qt)");
                std::process::exit(1);
            }
        }
        other => {
            eprintln!("❌ Unknown UI framework: {}", other);
            eprintln!("   Available: gtk, iced, qt");
            std::process::exit(1);
        }
    };
    
    println!("🎨 UI Framework: {}", ui_framework);
    
    // Run selected UI
    match ui_framework {
        "gtk" => run_gtk_ui(),
        "iced" => run_iced_ui(),
        #[cfg(feature = "qt")]
        "qt" => {
            if let Err(e) = qt_backend::run() {
                eprintln!("❌ Qt UI error: {}", e);
                std::process::exit(1);
            }
        }
        _ => unreachable!(),
    }
}

// ============================================================================
// Headless Mode
// ============================================================================

fn run_headless() {
    println!("[1/5] Initializing sensors...");
    let poller = ThermalPoller::new();
    println!("✓ Found {} thermal sensors", poller.sensor_names().len());
    
    println!("[2/5] Creating models...");
    let model = Arc::new(Mutex::new(Model::new(poller.sensor_names())));
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
    // Headless mode doesn't persist — no DB writer.
    let (_handles, _calibration_queue) =
        systems::spawn_all(world.clone(), model, neural_field, None);
    
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
    let poller = ThermalPoller::new();
    println!("✓ Found {} thermal sensors", poller.sensor_names().len());
    
    println!("[2/5] Creating models...");
    let model = Arc::new(Mutex::new(Model::new(poller.sensor_names())));
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

    // Set up the async DB writer. Same shape as Qt's path: try the
    // latest DB, fall back to "no persistence this run" if none
    // exists.
    let db_writer_opt = match get_latest_database() {
        Some(path) => match db_writer::DbWriter::spawn(path.clone()) {
            Ok(w) => {
                println!("💾 DB writer spawned ({})", path.display());
                Some(w)
            }
            Err(e) => {
                eprintln!(
                    "⚠️  DB writer init failed ({}): readings won't persist",
                    e
                );
                None
            }
        },
        None => {
            println!("📁 No existing database; readings won't persist this run");
            None
        }
    };

    let (_handles, calibration_queue) = systems::spawn_all(
        world.clone(),
        model,
        neural_field,
        db_writer_opt,
    );
    
    println!("[5/5] Starting Iced UI...");
    if let Err(e) = ui::iced::run(world, calibration_queue) {
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

/// "5m ago", "2h ago", "3d ago" — short age formatter for the GTK
/// audit log row subtitles. Matches the Iced backend's
/// `format_age_short` and Qt's `format_age` so the same data is
/// rendered consistently across backends.
#[cfg(feature = "gtk")]
fn format_age_gtk(seconds: u64) -> String {
    if seconds < 60 {
        format!("{}s ago", seconds)
    } else if seconds < 3600 {
        format!("{}m ago", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h ago", seconds / 3600)
    } else {
        format!("{}d ago", seconds / 86_400)
    }
}

/// Wrap a page widget with a floating title and dynamic color
#[cfg(feature = "gtk")]
fn wrap_with_floating_title(
    content: gtk::Widget,
    section_name: &str,
    section_class: &str,
) -> gtk::Box {
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    vbox.add_css_class(section_class);
    vbox.set_vexpand(true);
    vbox.set_hexpand(true);
    
    // Floating title
    let title = gtk::Label::new(Some(section_name));
    title.add_css_class("section-title-float");
    title.set_halign(gtk::Align::Start);
    
    vbox.append(&title);
    vbox.append(&content);
    
    vbox
}

#[cfg(feature = "gtk")]
fn run_gtk_ui() {
    println!("🎨 Launching GTK4 UI...");
    
    // Use NON_UNIQUE flag to avoid D-Bus registration issues
    // This allows multiple instances and avoids "failed to register: timeout" errors
    let app = adw::Application::builder()
        .application_id("org.localweather.App")
        .flags(gtk4::gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(build_ui);
    
    println!("Starting application...");
    let result = app.run();
    println!("Application exited with code: {:?}", result);
}

fn build_ui(app: &adw::Application) {
    println!("=== Ambient Learner v0.2.0 ===");
    println!("Data directory: {}", DATA_DIR);
    // Setup graceful shutdown and initialize UI components
    
    // TEMPORARILY DISABLED - Testing if this causes the hang
    /*
    // Setup graceful shutdown handlers for Ctrl+C and SIGTERM
    println!("[DEBUG] Step 2: Setting up shutdown handlers");
    let (shutdown_tx, shutdown_rx) = glib::MainContext::channel(glib::Priority::DEFAULT);
    
    std::thread::spawn(move || {
        use signal_hook::consts::signal::*;
        use signal_hook::iterator::Signals;
        
        // Try to register signal handlers, but don't fail if we can't
        match Signals::new(&[SIGINT, SIGTERM]) {
            Ok(mut signals) => {
                println!("✓ Registered graceful shutdown handlers (Ctrl+C, SIGTERM)");
                
                for sig in signals.forever() {
                    match sig {
                        SIGINT => {
                            println!("\n🛑 Received Ctrl+C (SIGINT), shutting down gracefully...");
                            let _ = shutdown_tx.send(());
                            break;
                        }
                        SIGTERM => {
                            println!("\n🛑 Received SIGTERM, shutting down gracefully...");
                            let _ = shutdown_tx.send(());
                            break;
                        }
                        _ => unreachable!(),
                    }
                }
            }
            Err(e) => {
                eprintln!("⚠️  Failed to register signal handlers: {}", e);
                eprintln!("⚠️  Ctrl+C will still work but may not save data cleanly");
            }
        }
    });
    
    println!("[DEBUG] Step 3: Attaching shutdown receiver");
    // Handle shutdown signal on main thread
    let app_clone = app.clone();
    shutdown_rx.attach(None, move |_| {
        app_clone.quit();
        glib::ControlFlow::Break
    });
    */
    println!("[DEBUG] Step 2: Signal handlers DISABLED for testing");
    
    println!("[DEBUG] Step 4: Loading CSS");
    // Don't print success here - it's done in the thread above if successful
    
    // Load custom CSS for liquid glass effects
    let css_provider = gtk::CssProvider::new();
    let css = include_str!("style.css");
    css_provider.load_from_data(css);
    
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("Could not connect to display"),
        &css_provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    println!("✓ Loaded liquid glass CSS styles");
    
    println!("[1/10] Loading locations (async)...");
    
    // Empty list while loading (filled when ready)
    let locations: Arc<Mutex<Vec<Location>>> = Arc::new(Mutex::new(Vec::new()));
    let current_location_idx = Arc::new(Mutex::new(0));
    
    // Load locations in background
    let locations_for_load = locations.clone();
    std::thread::spawn(move || {
        let locs = load_locations();
        println!("Background: Loaded {} locations", locs.len());
        *locations_for_load.lock().unwrap() = locs;
    });
    
    println!("Locations loading in background...");
    
    println!("[2/10] Starting background cleanup...");
    // Start background cleanup
    cleanup_old_data();
    
    println!("[3/10] Starting model load (async)...");
    
    // Create empty model initially, load in background
    let poller = ThermalPoller::new();
    let model = Arc::new(Mutex::new(Model::new(poller.sensor_names())));
    
    // Create neural temperature field (experimental physics-aware model)
    let num_sensors = poller.sensor_names().len();
    let neural_field = Arc::new(Mutex::new(
        temperature_field::TemperatureField::new(num_sensors)
    ));
    println!("✓ Neural temperature field initialized ({} sensors)", num_sensors);
    
    // Initialize forecast model
    let forecast_model = Arc::new(Mutex::new(forecast::ForecastModel::new()));
    
    // Load models in background
    let model_for_load = model.clone();
    let neural_field_for_load = neural_field.clone();
    std::thread::spawn(move || {
        // Small delay to let UI thread start first
        std::thread::sleep(std::time::Duration::from_millis(100));
        
        // Load legacy model
        if let Some(model_path) = get_latest_model() {
            println!("Background: Found model: {}", model_path.display());
            match Model::load(&model_path) {
                Ok(loaded_model) => {
                    println!("✓ Model loaded! Calibrations: {}, Active sensors: {}", 
                        loaded_model.calibration_count,
                        loaded_model.stats.iter().filter(|s| s.dropout_score > 0.1).count()
                    );
                    *model_for_load.lock().unwrap() = loaded_model;
                }
                Err(e) => {
                    eprintln!("Failed to load model: {}", e);
                }
            }
        } else {
            println!("No existing model found, using new model");
        }
        
        // Load neural field
        if let Some(neural_path) = get_latest_neural_field() {
            println!("Background: Found neural field: {}", neural_path.display());
            match temperature_field::TemperatureField::load(&neural_path) {
                Ok(loaded_field) => {
                    println!("✓ Neural field loaded! Calibrations: {}, Sensors: {}", 
                        loaded_field.get_calibration_count(),
                        loaded_field.get_num_sensors()
                    );
                    *neural_field_for_load.lock().unwrap() = loaded_field;
                }
                Err(e) => {
                    eprintln!("Failed to load neural field: {}", e);
                }
            }
        } else {
            println!("No existing neural field found, using new neural field");
        }
    });

    // ========================================================================
    // WorldState Architecture - Central State + Worker Systems
    // ========================================================================
    println!("⚙️  Initializing WorldState architecture...");
    
    // Create central WorldState
    let world = Arc::new(RwLock::new(SystemState::new()));
    
    // Initialize with sensor info
    {
        let mut state = world.write().unwrap();
        state.sensor_names = poller.sensor_names().clone();
        state.sensor_count = num_sensors;
    }
    println!("✓ WorldState initialized");
    
    // Spawn worker systems (sensor polling + model inference).
    //
    // GTK passes `None` for the DB writer because the GTK UI tick
    // already does its own hourly DB persistence with change
    // detection (see `last_reading_hash` further down). Routing
    // through both paths would double-persist; the GTK path is
    // older and we'd rather not touch it for this change.
    let (_system_handles, _calibration_queue) = systems::spawn_all(
        world.clone(),
        model.clone(),
        neural_field.clone(),
        None,
    );
    println!("✓ Worker systems spawned (sensor + model)");

    println!("[4/10] Loading database...");
    
    // Try to load existing database first, create new one only if none exists
    let db = if let Some(db_path) = get_latest_database() {
        println!("📂 Found existing database: {}", db_path.display());
        match Database::open(&db_path) {
            Ok(loaded_db) => {
                // Show detailed database stats
                match loaded_db.get_database_stats() {
                    Ok((cal_count, read_count, oldest, newest)) => {
                        println!("  📊 Calibrations: {}", cal_count);
                        println!("  📊 Readings: {}", read_count);
                        
                        if let (Some(oldest_ts), Some(newest_ts)) = (oldest, newest) {
                            let oldest_dt = chrono::DateTime::from_timestamp(oldest_ts, 0)
                                .map(|dt| dt.format("%Y-%m-%d").to_string())
                                .unwrap_or_else(|| "unknown".to_string());
                            let newest_dt = chrono::DateTime::from_timestamp(newest_ts, 0)
                                .map(|dt| dt.format("%Y-%m-%d").to_string())
                                .unwrap_or_else(|| "unknown".to_string());
                            println!("  📅 Data range: {} to {}", oldest_dt, newest_dt);
                            
                            let days = (newest_ts - oldest_ts) / 86400;
                            if days > 0 {
                                println!("  ⏱️  Data span: {} days", days);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("  ⚠️  Could not get database stats: {}", e);
                    }
                }
                
                // Perform health check
                if let Err(e) = loaded_db.check_health() {
                    eprintln!("⚠️  Database health issues detected: {}", e);
                    eprintln!("⚠️  Running automatic maintenance...");
                    
                    // Auto-fix corrupted data
                    if let Err(e) = loaded_db.perform_maintenance(365) {
                        eprintln!("❌ Maintenance failed: {}", e);
                    }
                }
                
                println!("✅ Database loaded successfully from {}", db_path.display());
                Arc::new(Mutex::new(loaded_db))
            }
            Err(e) => {
                eprintln!("❌ Failed to open existing database: {}", e);
                eprintln!("⚠️  This is unusual - the database may be corrupted");
                eprintln!("⚠️  Creating new database as fallback...");
                eprintln!("⚠️  Old database preserved with .backup extension");
                Arc::new(Mutex::new(Database::new(&get_timestamp()).unwrap()))
            }
        }
    } else {
        println!("📭 No existing database found, creating new one");
        Arc::new(Mutex::new(Database::new(&get_timestamp()).unwrap()))
    };
    
    // Clean up old database files (keep only 5 most recent)
    cleanup_old_databases(5);

    println!("[5/10] Initializing state...");
    let pulse_state = Rc::new(RefCell::new(0.0f32));
    let history_buffer: Arc<Mutex<VecDeque<(Instant, Vec<f32>)>>> = Arc::new(Mutex::new(VecDeque::new()));
    let sensor_lock_ok = Arc::new(Mutex::new(true));
    let week_offset = Rc::new(RefCell::new(0usize));
    
    // Cached SSID to avoid blocking WiFi calls
    let cached_ssid: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    
    // TEMPORARILY DISABLED - Testing if WiFi detection causes timeout
    /*
    // Detect WiFi ONCE at startup, then cache it
    // Continuous polling was causing UI freezes
    {
        let cached_ssid_clone = cached_ssid.clone();
        std::thread::spawn(move || {
            println!("Detecting WiFi network (one-time)...");
            
            // Catch all errors to prevent any crashes
            match std::panic::catch_unwind(|| {
                get_current_network_ssid_with_fallback()
            }) {
                Ok(Some(ssid)) => {
                    if let Ok(mut cache) = cached_ssid_clone.lock() {
                        *cache = Some(ssid.clone());
                        println!("WiFi cached: {}", ssid);
                    }
                }
                Ok(None) => {
                    println!("No WiFi detected (no active connection)");
                }
                Err(e) => {
                    eprintln!("WiFi detection panicked: {:?}", e);
                    println!("Continuing without WiFi detection");
                }
            }
        });
    }
    */
    println!("[DEBUG] WiFi detection DISABLED for testing");

    println!("[6/10] Starting sensor polling thread...");
    // Polling thread
    {
        let model = model.clone();
        let neural_field_clone = neural_field.clone();
        let db = db.clone();
        let history_buffer = history_buffer.clone();
        let sensor_lock_ok = sensor_lock_ok.clone();
        let locations = locations.clone();
        let current_location_idx = current_location_idx.clone();

        thread::spawn(move || {
            let mut poller = ThermalPoller::new();
            let mut last_save = Instant::now();
            let mut consecutive_failures = 0;
            let mut last_reading_hash: u64 = 0;

            loop {
                if let Some(readings) = poller.poll() {
                    *sensor_lock_ok.lock().unwrap() = true;
                    consecutive_failures = 0;
                    let now = Instant::now();

                    {
                        let mut m = model.lock().unwrap();
                        m.update_passive(&readings);
                        m.predict(&readings);
                    }
                    
                    // Update neural field history (for temporal gradients)
                    {
                        if let Ok(mut nf) = neural_field_clone.try_lock() {
                            let device_state = get_device_state();
                            nf.add_snapshot(&readings, &device_state);
                        }
                        // If lock fails, skip this update (non-critical)
                    }

                    {
                        let mut buffer = history_buffer.lock().unwrap();
                        buffer.push_back((now, readings.clone()));

                        while let Some((old_time, _)) = buffer.front() {
                            if old_time.elapsed().as_secs() > HISTORY_SECONDS {
                                buffer.pop_front();
                            } else {
                                break;
                            }
                        }
                    }

                    if last_save.elapsed().as_secs() >= 3600 {
                        // Compute hash of readings to avoid duplicate saves
                        use std::collections::hash_map::DefaultHasher;
                        use std::hash::{Hash, Hasher};
                        
                        let mut hasher = DefaultHasher::new();
                        for &val in &readings {
                            ((val * 100.0) as i32).hash(&mut hasher);
                        }
                        let reading_hash = hasher.finish();
                        
                        // Only save if readings have meaningfully changed
                        if reading_hash != last_reading_hash {
                            let reading = SensorReading {
                                time: SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap()
                                    .as_secs()
                                    .to_string(),
                                sensors: readings,
                                cpu_load: 0.0,
                                als_lux: 0,
                                charging: false,
                                screen: 100,
                            };

                            if let Ok(db) = db.lock() {
                                // Get current location ID
                                let location_id = {
                                    let locs = locations.lock().unwrap();
                                    let idx = *current_location_idx.lock().unwrap();
                                    locs.get(idx).map(|l| l.id.clone())
                                };
                                
                                match db.store_reading(&reading, location_id.as_deref()) {
                                    Ok(_) => {
                                        println!("Hourly reading saved to database");
                                        last_reading_hash = reading_hash;
                                    }
                                    Err(e) => eprintln!("Failed to save reading: {}", e),
                                }
                            }
                        } else {
                            println!("Hourly reading skipped (no significant change)");
                        }

                        last_save = now;
                    }
                } else {
                    consecutive_failures += 1;
                    *sensor_lock_ok.lock().unwrap() = false;
                    
                    // Try to reinitialize poller after persistent failures
                    if consecutive_failures > 10 {
                        poller = ThermalPoller::new();
                        consecutive_failures = 0;
                    }
                }

                thread::sleep(Duration::from_millis(PASSIVE_INTERVAL_MS));
            }
        });
    }

    println!("[7/10] Building UI window...");
    // Build UI with libadwaita
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Local Weather")
        .default_width(400)
        .default_height(700)
        .build();

    // Phase 1: Viewport detection
    let current_mode = Rc::new(RefCell::new(ViewportMode::Desktop));
    let mode_for_resize = current_mode.clone();

    window.connect_default_width_notify(move |window| {
        let width = window.default_width();
        let new_mode = ViewportMode::from_width(width);
        let old_mode = *mode_for_resize.borrow();
        
        if old_mode != new_mode {
            println!("📱 Viewport mode: {} → {} ({}px)", 
                old_mode.as_str(), 
                new_mode.as_str(),
                width
            );
            *mode_for_resize.borrow_mut() = new_mode;
        }
    });

    // Debug: Press 1/2/3 to test layouts
    #[cfg(debug_assertions)]
    {
        let window_clone = window.clone();
        let event_controller = gtk::EventControllerKey::new();
        
        event_controller.connect_key_pressed(move |_, keyval, _, _| {
            use gtk::gdk::Key;
            
            match keyval {
                Key::_1 => {
                    window_clone.set_default_width(375);
                    window_clone.set_default_height(812);
                    println!("🎮 Testing: Mobile (375x812)");
                    gtk::glib::Propagation::Stop
                }
                Key::_2 => {
                    window_clone.set_default_width(768);
                    window_clone.set_default_height(1024);
                    println!("🎮 Testing: Tablet (768x1024)");
                    gtk::glib::Propagation::Stop
                }
                Key::_3 => {
                    window_clone.set_default_width(1920);
                    window_clone.set_default_height(1080);
                    println!("🎮 Testing: Desktop (1920x1080)");
                    gtk::glib::Propagation::Stop
                }
                _ => gtk::glib::Propagation::Proceed,
            }
        });
        
        window.add_controller(event_controller);
        println!("🎮 Debug mode: Press 1/2/3 to test viewport sizes");
    }

    // Toast overlay for notifications
    let toast_overlay = adw::ToastOverlay::new();
    
    let main_box = gtk::Box::new(gtk::Orientation::Vertical, 0);

    // Header bar
    let header = adw::HeaderBar::new();
    main_box.append(&header);

    // ViewStack wrapped in SwipeableStack for swipe gestures
    let stack = adw::ViewStack::new();
    stack.set_vexpand(true);
    
    // Note: ViewStack doesn't support transition_type/duration directly
    // Transitions are handled automatically by libadwaita
    
    // Wrap stack in a swipeable container for touch gestures
    let swipeable_stack = gtk::Box::new(gtk::Orientation::Vertical, 0);
    swipeable_stack.append(&stack);
    swipeable_stack.set_vexpand(true);
    
    // Add swipe gesture with timestamp-based debouncing
    let swipe_gesture = gtk::GestureSwipe::new();
    swipe_gesture.set_touch_only(false);
    swipe_gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
    
    let stack_for_swipe = stack.clone();
    
    // Track last swipe time to prevent rapid double-swipes
    let last_swipe_time = std::rc::Rc::new(std::cell::Cell::new(std::time::Instant::now()));
    
    swipe_gesture.connect_swipe(move |gesture, velocity_x, _velocity_y| {
        println!("🔄 Swipe detected: velocity_x = {}", velocity_x);
        
        // Check if enough time has passed since last swipe (500ms debounce)
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(last_swipe_time.get());
        
        if elapsed < std::time::Duration::from_millis(500) {
            println!("⏸️  Swipe too soon after previous ({:?} ago), BLOCKING", elapsed);
            gesture.set_state(gtk::EventSequenceState::Denied);
            return;
        }
        
        // Get CURRENT page before any state changes
        let current = stack_for_swipe.visible_child_name();
        let current_str = current.as_ref().map(|s| s.as_str()).unwrap_or("");
        
        println!("📍 Starting from page: '{}'", current_str);
        
        // Calculate target page
        let target_page = if velocity_x < -300.0 {
            match current_str {
                "live" => Some("model"),
                "model" => Some("history"),
                "history" => Some("locations"),
                "locations" => Some("sync"),
                _ => None,
            }
        } else if velocity_x > 300.0 {
            match current_str {
                "sync" => Some("locations"),
                "locations" => Some("history"),
                "history" => Some("model"),
                "model" => Some("live"),
                _ => None,
            }
        } else {
            println!("🐌 Swipe too slow (velocity: {}, threshold: 300)", velocity_x);
            None
        };
        
        if let Some(target) = target_page {
            println!("🎯 Target: '{}' from '{}'", target, current_str);
            
            // Update last swipe time IMMEDIATELY
            last_swipe_time.set(now);
            
            // Change page
            stack_for_swipe.set_visible_child_name(target);
            
            println!("✅ Page changed, 500ms lockout active");
            gesture.set_state(gtk::EventSequenceState::Claimed);
        } else {
            gesture.set_state(gtk::EventSequenceState::Denied);
        }
    });
    
    swipeable_stack.add_controller(swipe_gesture);

    // View switcher
    let switcher_bar = adw::ViewSwitcherBar::new();
    switcher_bar.set_stack(Some(&stack));
    switcher_bar.set_reveal(true);

    println!("[8/10] Creating pages...");

    // PAGE 1: Live Temperature
    let page1 = gtk::Box::new(gtk::Orientation::Vertical, 16);
    page1.set_margin_top(16);
    page1.set_margin_bottom(16);
    page1.set_margin_start(16);
    page1.set_margin_end(16);
    page1.set_valign(gtk::Align::Fill);
    page1.set_vexpand(true);
    page1.add_css_class("glass-panel");  // Base glass effect
    page1.add_css_class("liquid-wave");  // Animated wave effect

    let temp_label = gtk::Label::new(None);
    temp_label.add_css_class("display");
    // Subtle "always alive" oscillation so the card never looks frozen.
    temp_label.add_css_class("idle-glow");
    temp_label.set_markup("<span size='80000' weight='bold'>--°C</span>");
    temp_label.set_margin_top(24);  // Extra top padding for prominence
    temp_label.set_margin_bottom(8);

    let conf_label = gtk::Label::new(None);
    conf_label.add_css_class("title-2");
    conf_label.set_margin_top(12);

    let status_label = gtk::Label::new(Some("Initializing..."));
    status_label.add_css_class("title-4");
    status_label.set_margin_top(8);

    let sensor_lock_label = gtk::Label::new(Some(""));
    sensor_lock_label.add_css_class("title-4");
    sensor_lock_label.set_margin_top(8);

    // Forecast section — `forecast-card` adds the hover-lift transition
    // defined in style.css.
    let forecast_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    forecast_box.set_margin_top(20);
    forecast_box.add_css_class("glass-panel-subtle");
    forecast_box.add_css_class("forecast-card");
    forecast_box.set_margin_start(12);
    forecast_box.set_margin_end(12);

    let forecast_title = gtk::Label::new(Some("🔮 Forecast"));
    forecast_title.add_css_class("title-3");
    forecast_title.set_halign(gtk::Align::Start);
    forecast_title.set_margin_top(8);
    forecast_title.set_margin_start(12);

    let forecast_1h_label = gtk::Label::new(Some("1 hour: --°C"));
    forecast_1h_label.add_css_class("title-4");
    forecast_1h_label.set_halign(gtk::Align::Start);
    forecast_1h_label.set_margin_start(12);

    let forecast_24h_label = gtk::Label::new(Some("24 hours: --°C"));
    forecast_24h_label.add_css_class("title-4");
    forecast_24h_label.set_halign(gtk::Align::Start);
    forecast_24h_label.set_margin_start(12);
    forecast_24h_label.set_margin_bottom(8);

    forecast_box.append(&forecast_title);
    forecast_box.append(&forecast_1h_label);
    forecast_box.append(&forecast_24h_label);

    // Stagger the initial fade-in across the page's main rows so the
    // first reveal feels intentional rather than a hard pop.
    // gtk::Widget is the common base every widget upcasts to.
    use gtk::prelude::Cast;
    apply_stagger_fade(&[
        temp_label.upcast_ref::<gtk::Widget>(),
        conf_label.upcast_ref::<gtk::Widget>(),
        status_label.upcast_ref::<gtk::Widget>(),
        sensor_lock_label.upcast_ref::<gtk::Widget>(),
        forecast_box.upcast_ref::<gtk::Widget>(),
    ]);

    page1.append(&temp_label);
    page1.append(&conf_label);
    page1.append(&status_label);
    page1.append(&sensor_lock_label);
    page1.append(&forecast_box);
    
    // Neural Field section (new physics-aware model)
    let neural_field_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    neural_field_box.set_margin_top(20);
    neural_field_box.add_css_class("glass-panel-subtle");
    neural_field_box.set_margin_start(12);
    neural_field_box.set_margin_end(12);
    
    let neural_title = gtk::Label::new(Some("🧠 Neural Field (Experimental)"));
    neural_title.add_css_class("title-3");
    neural_title.set_halign(gtk::Align::Start);
    neural_title.set_margin_top(8);
    neural_title.set_margin_start(12);
    
    let neural_ambient_label = gtk::Label::new(Some("Ambient: --°C"));
    neural_ambient_label.add_css_class("title-4");
    neural_ambient_label.set_halign(gtk::Align::Start);
    neural_ambient_label.set_margin_start(12);
    
    let neural_cardinal_label = gtk::Label::new(Some("Spatial: N/A"));
    neural_cardinal_label.add_css_class("caption");
    neural_cardinal_label.set_halign(gtk::Align::Start);
    neural_cardinal_label.set_margin_start(12);
    
    let neural_status_label = gtk::Label::new(Some("Status: Not initialized"));
    neural_status_label.add_css_class("caption");
    neural_status_label.set_halign(gtk::Align::Start);
    neural_status_label.set_margin_start(12);
    neural_status_label.set_margin_bottom(8);
    
    neural_field_box.append(&neural_title);
    neural_field_box.append(&neural_ambient_label);
    neural_field_box.append(&neural_cardinal_label);
    neural_field_box.append(&neural_status_label);
    
    page1.append(&neural_field_box);
    
    // Button container for calibrate and log weather
    let button_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    button_box.set_halign(gtk::Align::Center);
    button_box.set_margin_top(20);
    
    // Calibrate button (for model training)
    let calibrate_btn = gtk::Button::with_label("🎯 Calibrate");
    calibrate_btn.add_css_class("pill");
    
    // Log Weather button (for actual observations)
    let log_weather_btn = gtk::Button::with_label("📝 Add Data Point");
    log_weather_btn.add_css_class("suggested-action");
    log_weather_btn.add_css_class("pill");
    
    button_box.append(&calibrate_btn);
    button_box.append(&log_weather_btn);
    
    page1.append(&button_box);

    // Phase 1: Wrap pages with floating titles and dynamic colors
    let page1_wrapped = wrap_with_floating_title(page1.clone().into(), "LIVE", "section-live");
    stack.add_titled(&page1_wrapped, Some("live"), "1. Live");
    println!("[DEBUG] Page 1 added");

    // PAGE 2: Model (Network Visualization)
    let page2 = gtk::Box::new(gtk::Orientation::Vertical, 4);
    page2.set_margin_top(4);
    page2.set_margin_bottom(4);
    page2.set_margin_start(4);
    page2.set_margin_end(4);
    page2.set_vexpand(true);
    page2.set_hexpand(true);

    // Temperature sphere visualization
    let sphere_picture = gtk::Picture::new();
    sphere_picture.set_vexpand(true);
    sphere_picture.set_hexpand(true);
    sphere_picture.set_can_shrink(true);
    sphere_picture.set_content_fit(gtk::ContentFit::Contain);
    sphere_picture.set_height_request(300);

    let sphere_label = gtk::Label::new(Some("🌡️ Temperature Distribution"));
    sphere_label.add_css_class("heading");
    sphere_label.set_margin_top(8);
    sphere_label.set_margin_bottom(4);

    let model_picture = gtk::Picture::new();
    model_picture.set_vexpand(true);
    model_picture.set_hexpand(true);
    model_picture.set_can_shrink(true);  // Allow shrinking to fit
    model_picture.set_content_fit(gtk::ContentFit::Contain);  // Scale to fit

    let calibrate_btn = gtk::Button::with_label("🎯 Calibrate Model");
    calibrate_btn.add_css_class("suggested-action");
    calibrate_btn.add_css_class("pill");
    calibrate_btn.set_margin_top(6);
    calibrate_btn.set_margin_bottom(6);
    calibrate_btn.set_halign(gtk::Align::Center);

    page2.append(&sphere_label);
    page2.append(&sphere_picture);
    page2.append(&model_picture);
    page2.append(&calibrate_btn);

    let page2_wrapped = wrap_with_floating_title(page2.into(), "MODEL", "section-model");
    stack.add_titled(&page2_wrapped, Some("model"), "2. Network");
    println!("[DEBUG] Page 2 added");

    // PAGE 3: History
    let page3 = gtk::Box::new(gtk::Orientation::Vertical, 0);
    page3.set_vexpand(true);

    // Overlay for picture with spinner
    let history_overlay = gtk::Overlay::new();
    
    let history_picture = gtk::Picture::new();
    history_picture.set_vexpand(true);
    history_picture.set_hexpand(true);
    
    let history_spinner = gtk::Spinner::new();
    history_spinner.set_halign(gtk::Align::Center);
    history_spinner.set_valign(gtk::Align::Center);
    history_spinner.set_width_request(48);
    history_spinner.set_height_request(48);
    
    history_overlay.set_child(Some(&history_picture));
    history_overlay.add_overlay(&history_spinner);

    // Improved week navigation with better UI
    let history_controls = gtk::Box::new(gtk::Orientation::Vertical, 0);
    
    // Week label at top
    let week_info_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    week_info_box.set_margin_top(12);
    week_info_box.set_margin_bottom(8);
    
    let week_label = gtk::Label::new(Some("This Week"));
    week_label.add_css_class("title-3");
    
    let date_range_label = gtk::Label::new(Some(""));
    date_range_label.add_css_class("caption");
    date_range_label.add_css_class("dim-label");
    
    week_info_box.append(&week_label);
    week_info_box.append(&date_range_label);
    
    // Navigation buttons with better styling
    let nav_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    nav_box.set_halign(gtk::Align::Center);
    nav_box.set_margin_top(8);
    nav_box.set_margin_bottom(12);
    nav_box.set_margin_start(24);
    nav_box.set_margin_end(24);

    let prev_week_btn = gtk::Button::with_label("◀");
    prev_week_btn.add_css_class("circular");
    prev_week_btn.set_tooltip_text(Some("Previous Week"));
    
    let today_btn = gtk::Button::with_label("Today");
    today_btn.add_css_class("pill");
    today_btn.set_hexpand(true);
    
    let next_week_btn = gtk::Button::with_label("▶");
    next_week_btn.add_css_class("circular");
    next_week_btn.set_tooltip_text(Some("Next Week"));

    nav_box.append(&prev_week_btn);
    nav_box.append(&today_btn);
    nav_box.append(&next_week_btn);

    // Export button
    let export_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    export_box.set_halign(gtk::Align::Center);
    export_box.set_margin_bottom(12);
    
    let export_btn = gtk::Button::with_label("Export as CSV");
    export_btn.add_css_class("pill");
    export_btn.set_tooltip_text(Some("Export daily history to CSV file"));
    
    export_box.append(&export_btn);

    history_controls.append(&week_info_box);
    history_controls.append(&nav_box);
    history_controls.append(&export_box);

    page3.append(&history_overlay);
    page3.append(&history_controls);

    // ─── Calibration audit log (collapsible) ─────────────────
    //
    // Mirror of the Qt and Iced audit logs. Wrapped in an
    // adw::ExpanderRow so it's collapsed by default — most users
    // don't need to look at this, but the data is there if you
    // want to investigate "did something else calibrate my model?"
    //
    // The expander uses adw::PreferencesGroup as a container so it
    // gets the standard libadwaita group styling (rounded card,
    // separator lines between rows). That's the same look the
    // Locations page uses for its location list, so the whole UI
    // feels consistent.
    let audit_group = adw::PreferencesGroup::new();
    audit_group.set_margin_start(12);
    audit_group.set_margin_end(12);
    audit_group.set_margin_top(8);
    audit_group.set_margin_bottom(12);

    let audit_expander = adw::ExpanderRow::new();
    audit_expander.set_title("Calibration audit log");
    audit_expander.set_subtitle(
        "Who has calibrated this model. Tap to expand."
    );

    // Hold the rows separately so we can refresh them without
    // rebuilding the expander shell.
    let audit_row_keys: Rc<RefCell<Vec<adw::ActionRow>>> =
        Rc::new(RefCell::new(Vec::new()));

    audit_group.add(&audit_expander);
    page3.append(&audit_group);

    // Refresh the audit log every 2 seconds. The data only changes
    // on user action (or external IPC), so polling at 60 Hz would
    // be wasteful. 2 seconds keeps it responsive enough that a
    // user who calibrates and immediately scrolls down sees their
    // entry appear.
    {
        let world_for_audit = world.clone();
        let audit_expander_for_tick = audit_expander.clone();
        let audit_row_keys_for_tick = audit_row_keys.clone();
        glib::timeout_add_seconds_local(2, move || {
            let entries: Vec<(f32, f32, u64, Option<String>)> = {
                let state = match world_for_audit.read() {
                    Ok(s) => s,
                    Err(_) => return glib::ControlFlow::Continue,
                };
                let now = std::time::Instant::now();
                state
                    .calibration_samples
                    .iter()
                    .rev()
                    .take(20)
                    .map(|s| {
                        let mean = if s.sensors.is_empty() {
                            0.0
                        } else {
                            let valid: Vec<f32> = s.sensors.iter()
                                .copied().filter(|v| v.is_finite())
                                .collect();
                            if valid.is_empty() {
                                0.0
                            } else {
                                valid.iter().sum::<f32>()
                                    / valid.len() as f32
                            }
                        };
                        (
                            s.target,
                            mean,
                            now.duration_since(s.when).as_secs(),
                            s.source.clone(),
                        )
                    })
                    .collect()
            };

            // Rebuild rows. We could diff for efficiency, but at
            // ≤20 rows and 0.5 Hz refresh, the rebuild cost is
            // negligible compared to maintaining incremental
            // state. Simplicity wins.
            for old_row in audit_row_keys_for_tick.borrow_mut().drain(..) {
                audit_expander_for_tick.remove(&old_row);
            }

            if entries.is_empty() {
                let empty_row = adw::ActionRow::new();
                empty_row.set_title("No calibrations recorded yet");
                empty_row.set_subtitle(
                    "They'll appear here after you tap Calibrate."
                );
                empty_row.set_sensitive(false);
                audit_expander_for_tick.add_row(&empty_row);
                audit_row_keys_for_tick.borrow_mut().push(empty_row);
                return glib::ControlFlow::Continue;
            }

            for (target, mean, age, source) in entries {
                let row = adw::ActionRow::new();

                // Source attribution. Same colored-pill logic as
                // Qt: known UIs vs external apps get different
                // styling. GTK doesn't have arbitrary background
                // colors on labels without custom CSS, so we use
                // Pango markup to color the text instead.
                let source_label = source.clone()
                    .unwrap_or_else(|| "(unknown)".to_string());
                let is_known_ui = matches!(
                    source.as_deref(),
                    Some("qt-ui") | Some("gtk-ui") | Some("iced-ui"),
                );
                // Pango markup colors. Light-gray-blue for known UIs;
                // amber-ish for external sources; dim gray for
                // (unknown). Markup is HTML-like and escaped.
                let color = if is_known_ui {
                    "#7faecf"  // muted blue
                } else if source.as_deref().map(|s| !s.is_empty()).unwrap_or(false) {
                    "#c89a5c"  // amber — external app, draw attention
                } else {
                    "#888888"  // dim
                };
                // Note: glib::markup_escape_text is the safe way to
                // include user-provided strings in Pango markup.
                // A source string with `<` or `&` would otherwise
                // break the markup parser.
                let escaped = glib::markup_escape_text(&source_label);
                row.set_title(&format!(
                    "<span color=\"{}\">{}</span>", color, escaped
                ));
                // Setting use_markup on the title is essential —
                // without it, the <span> renders as literal text.
                // (Available since libadwaita 1.2.)
                row.set_use_markup(true);
                row.set_title_lines(1);

                let delta = target - mean;
                let sign = if delta >= 0.0 { "+" } else { "" };
                row.set_subtitle(&format!(
                    "Target {:.1}°C  ·  device showed {:.1}°C ({}{:.1})  ·  {}",
                    target, mean, sign, delta, format_age_gtk(age)
                ));

                audit_expander_for_tick.add_row(&row);
                audit_row_keys_for_tick.borrow_mut().push(row);
            }

            glib::ControlFlow::Continue
        });
    }
    // ────────────────────────────────────────────────────────

    let page3_wrapped = wrap_with_floating_title(page3.into(), "HISTORY", "section-history");
    stack.add_titled(&page3_wrapped, Some("history"), "3. History");
    println!("[DEBUG] Page 3 added");

    // PAGE 4: Locations
    let page4 = gtk::Box::new(gtk::Orientation::Vertical, 12);
    page4.set_margin_top(12);
    page4.set_margin_bottom(12);
    page4.set_margin_start(12);
    page4.set_margin_end(12);

    let locations_scroll = gtk::ScrolledWindow::new();
    locations_scroll.set_vexpand(true);
    
    let locations_list = gtk::Box::new(gtk::Orientation::Vertical, 8);
    locations_scroll.set_child(Some(&locations_list));

    // Add location button
    let add_location_btn = gtk::Button::with_label("➕ Add New Location");
    add_location_btn.add_css_class("suggested-action");
    add_location_btn.add_css_class("pill");
    add_location_btn.set_margin_top(12);

    page4.append(&locations_scroll);
    page4.append(&add_location_btn);

    let page4_wrapped = wrap_with_floating_title(page4.into(), "LOCATIONS", "section-locations");
    stack.add_titled(&page4_wrapped, Some("locations"), "4. 📍");
    println!("[DEBUG] Page 4 added");
    
    // PAGE 5: Database Sync (Import/Export)
    let page5 = gtk::Box::new(gtk::Orientation::Vertical, 12);
    page5.set_margin_top(12);
    page5.set_margin_bottom(12);
    page5.set_margin_start(12);
    page5.set_margin_end(12);
    page5.add_css_class("glass-panel");
    
    let sync_title = gtk::Label::new(Some("📡 Database Management"));
    sync_title.add_css_class("title-1");
    sync_title.set_halign(gtk::Align::Start);
    sync_title.set_margin_bottom(8);
    
    let sync_subtitle = gtk::Label::new(Some("Manage calibration databases"));
    sync_subtitle.add_css_class("title-4");
    sync_subtitle.set_halign(gtk::Align::Start);
    sync_subtitle.set_margin_bottom(16);
    
    // Database list scroll
    let db_scroll = gtk::ScrolledWindow::new();
    db_scroll.set_vexpand(true);
    db_scroll.set_hexpand(true);
    db_scroll.set_min_content_height(200);
    
    let db_list = gtk::ListBox::new();
    db_list.add_css_class("boxed-list");
    db_scroll.set_child(Some(&db_list));
    
    // Current database info
    let (cal_count, model_type) = model.try_lock()
        .map(|m| {
            let count = m.calibration_count;
            let mtype = if count >= 20 { "Neural" } else { "Linear" };
            (count, mtype)
        })
        .unwrap_or((0, "Loading..."));
    
    let current_db_row = adw::ActionRow::builder()
        .title("📍 Current Database")
        .subtitle(&format!("{} calibrations ({} model)", cal_count, model_type))
        .build();
    
    let export_btn = gtk::Button::with_label("📤 Export");
    export_btn.add_css_class("flat");
    export_btn.set_valign(gtk::Align::Center);
    current_db_row.add_suffix(&export_btn);
    
    db_list.append(&current_db_row);
    
    // Available databases section
    let available_title_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    available_title_box.set_margin_top(16);
    available_title_box.set_margin_bottom(8);
    
    let available_title = gtk::Label::new(Some("Available Databases"));
    available_title.add_css_class("title-3");
    available_title.set_halign(gtk::Align::Start);
    available_title.set_hexpand(true);
    
    let refresh_btn = gtk::Button::with_label("🔄");
    refresh_btn.add_css_class("flat");
    refresh_btn.add_css_class("circular");
    
    available_title_box.append(&available_title);
    available_title_box.append(&refresh_btn);
    
    // List available database files
    let db_files_list = gtk::ListBox::new();
    db_files_list.add_css_class("boxed-list");
    
    // Function to populate database list
    fn populate_db_list(
        db_list: &gtk::ListBox, 
        current_db_path: &str,
        neural_field: &Arc<Mutex<temperature_field::TemperatureField>>,
        toast_overlay: &adw::ToastOverlay,
    ) {
        // Clear existing
        while let Some(child) = db_list.first_child() {
            db_list.remove(&child);
        }
        
        // Find all .db files in weather_data
        if let Ok(entries) = std::fs::read_dir(DATA_DIR) {
            let mut db_files: Vec<_> = entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path().extension()
                        .and_then(|s| s.to_str())
                        .map(|s| s == "db")
                        .unwrap_or(false)
                })
                .collect();
            
            // Sort by modification time (newest first)
            db_files.sort_by_key(|e| {
                std::cmp::Reverse(
                    e.metadata()
                        .and_then(|m| m.modified())
                        .ok()
                )
            });
            
            for entry in db_files {
                let path = entry.path();
                let filename = path.file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown");
                
                // Check if this is the current database
                let is_current = current_db_path.contains(filename);
                
                // Get file size and calibration count
                let size = entry.metadata()
                    .map(|m| m.len())
                    .unwrap_or(0);
                let size_kb = size / 1024;
                
                // Try to open and get calibration count
                let cal_count = rusqlite::Connection::open(&path)
                    .ok()
                    .and_then(|conn| {
                        conn.query_row(
                            "SELECT COUNT(*) FROM calibrations",
                            [],
                            |row| row.get::<_, i64>(0)
                        ).ok()
                    })
                    .unwrap_or(0);
                
                let row = adw::ActionRow::builder()
                    .title(filename)
                    .subtitle(&format!("{} calibrations, {} KB", cal_count, size_kb))
                    .build();
                
                if is_current {
                    row.add_prefix(&gtk::Image::from_icon_name("emblem-default-symbolic"));
                }
                
                // Add bootstrap button
                let bootstrap_btn = gtk::Button::with_label("🚀 Bootstrap");
                bootstrap_btn.add_css_class("flat");
                bootstrap_btn.set_valign(gtk::Align::Center);
                bootstrap_btn.set_tooltip_text(Some("Train neural field with this database's calibrations"));
                
                let path_clone = path.clone();
                let neural_clone = neural_field.clone();
                let toast_clone = toast_overlay.clone();
                bootstrap_btn.connect_clicked(move |_btn| {
                    // Open database and get calibrations
                    if let Ok(db_conn) = rusqlite::Connection::open(&path_clone) {
                        let result: Result<Vec<(Vec<f32>, f32)>, rusqlite::Error> = (|| {
                            let mut stmt = db_conn.prepare(
                                "SELECT sensors, target FROM calibrations ORDER BY timestamp"
                            )?;
                            
                            let calibrations = stmt.query_map([], |row| {
                                let sensors_json: String = row.get(0)?;
                                let target: f32 = row.get(1)?;
                                let sensors: Vec<f32> = serde_json::from_str(&sensors_json)
                                    .unwrap_or_default();
                                Ok((sensors, target))
                            })?
                            .filter_map(Result::ok)
                            .filter(|(s, _)| !s.is_empty())
                            .collect();
                            
                            Ok(calibrations)
                        })();
                        
                        match result {
                            Ok(calibrations) if !calibrations.is_empty() => {
                                let count = calibrations.len();
                                let neural_for_thread = neural_clone.clone();
                                
                                // Bootstrap in background thread
                                std::thread::spawn(move || {
                                    if let Ok(mut nf) = neural_for_thread.lock() {
                                        println!("🚀 Starting bootstrap from {} calibrations...", count);
                                        match nf.bootstrap_from_calibrations(&calibrations, 0.001) {
                                            Ok(_) => {
                                                println!("✅ Bootstrap complete! Trained from {} calibrations", count);
                                            }
                                            Err(e) => {
                                                eprintln!("❌ Bootstrap failed: {}", e);
                                            }
                                        }
                                    }
                                });
                                
                                let toast = adw::Toast::new("🚀 Bootstrap training started...");
                                toast.set_timeout(2);
                                toast_clone.add_toast(toast);
                            }
                            Ok(_) => {
                                let toast = adw::Toast::new("⚠ No calibrations found in database");
                                toast.set_timeout(2);
                                toast_clone.add_toast(toast);
                            }
                            Err(e) => {
                                let toast = adw::Toast::new(&format!("✗ Failed to read database: {}", e));
                                toast.set_timeout(3);
                                toast_clone.add_toast(toast);
                            }
                        }
                    }
                });
                
                row.add_suffix(&bootstrap_btn);
                
                // Add switch button
                let switch_btn = gtk::Button::with_label("Switch");
                switch_btn.add_css_class("flat");
                switch_btn.set_valign(gtk::Align::Center);
                row.add_suffix(&switch_btn);
                
                db_list.append(&row);
            }
        }
    }
    
    // Get current database path for highlighting
    let current_db_path = format!("{}/{}", DATA_DIR, 
        std::fs::read_dir(DATA_DIR)
            .ok()
            .and_then(|entries| {
                entries.filter_map(Result::ok)
                    .find(|e| e.path().extension().map(|s| s == "db").unwrap_or(false))
                    .map(|e| e.file_name().to_string_lossy().to_string())
            })
            .unwrap_or_else(|| "sensors.db".to_string()));
    
    populate_db_list(&db_files_list, &current_db_path, &neural_field, &toast_overlay);
    
    // Refresh button handler (now that db_files_list and current_db_path exist)
    {
        let db_files_list_clone = db_files_list.clone();
        let current_db_path_clone = current_db_path.clone();
        let neural_field_clone = neural_field.clone();
        let toast_overlay_clone = toast_overlay.clone();
        
        refresh_btn.connect_clicked(move |_| {
            populate_db_list(&db_files_list_clone, &current_db_path_clone, &neural_field_clone, &toast_overlay_clone);
        });
    }
    
    // Import section
    let import_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
    import_box.set_margin_top(16);
    
    let import_title = gtk::Label::new(Some("Import from File"));
    import_title.add_css_class("title-3");
    import_title.set_halign(gtk::Align::Start);
    
    let import_subtitle = gtk::Label::new(Some("Merge calibrations from external database"));
    import_subtitle.add_css_class("dim-label");
    import_subtitle.set_halign(gtk::Align::Start);
    
    let import_btn = gtk::Button::with_label("📥 Import & Merge Database");
    import_btn.add_css_class("suggested-action");
    import_btn.add_css_class("pill");
    import_btn.set_margin_top(8);
    
    import_box.append(&import_title);
    import_box.append(&import_subtitle);
    import_box.append(&import_btn);
    
    page5.append(&sync_title);
    page5.append(&sync_subtitle);
    page5.append(&db_scroll);
    page5.append(&available_title_box);
    page5.append(&db_files_list);
    page5.append(&import_box);
    
    let page5_wrapped = wrap_with_floating_title(page5.into(), "SYNC", "section-sync");
    stack.add_titled(&page5_wrapped, Some("sync"), "5. 📡");
    println!("[DEBUG] Page 5 added");
    println!("[DEBUG] Starting swipe gesture setup");

    // Add swipe gestures for page navigation (5 pages now)
    {
        let stack_clone = stack.clone();
        let gesture = gtk::GestureSwipe::new();
        gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
        
        gesture.connect_swipe(move |_, velocity_x: f64, _velocity_y: f64| {
            if velocity_x.abs() < 300.0 {
                return; // Require stronger swipe for page change
            }
            
            let current_page = stack_clone.visible_child_name();
            let next_page = if velocity_x < 0.0 {
                // Swipe left - next page
                match current_page.as_ref().map(|s| s.as_str()) {
                    Some("live") => Some("model"),
                    Some("model") => Some("history"),
                    Some("history") => Some("locations"),
                    Some("locations") => Some("sync"),
                    _ => None,
                }
            } else {
                // Swipe right - previous page
                match current_page.as_ref().map(|s| s.as_str()) {
                    Some("sync") => Some("locations"),
                    Some("locations") => Some("history"),
                    Some("history") => Some("model"),
                    Some("model") => Some("live"),
                    _ => None,
                }
            };
            
            if let Some(page) = next_page {
                stack_clone.set_visible_child_name(page);
            }
        });
        
        stack.add_controller(gesture);
    }

    main_box.append(&swipeable_stack);
    main_box.append(&switcher_bar);
    
    // Location selector bar (opens UPWARD for mobile)
    let location_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    location_bar.set_halign(gtk::Align::Center);
    location_bar.set_margin_top(4);
    location_bar.set_margin_bottom(4);
    
    let location_icon = gtk::Label::new(Some("📍"));
    
    // Use MenuButton instead of DropDown for upward opening
    let location_menu_btn = gtk::MenuButton::new();
    location_menu_btn.set_label("Loading...");
    location_menu_btn.set_direction(gtk::ArrowType::Up);  // Opens upward!
    
    let popover = gtk::Popover::new();
    popover.set_position(gtk::PositionType::Top);  // Force top positioning
    
    let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    menu_box.add_css_class("menu");
    
    location_bar.append(&location_icon);
    location_bar.append(&location_menu_btn);
    
    main_box.append(&location_bar);
    
    // Populate location menu
    {
        let locations = locations.clone();
        let current_location_idx = current_location_idx.clone();
        let menu_btn = location_menu_btn.clone();
        
        // Set initial label
        {
            let locs = locations.lock().unwrap();
            let idx = *current_location_idx.lock().unwrap();
            if let Some(loc) = locs.get(idx) {
                menu_btn.set_label(&loc.name);
            }
        }
        
        // Build menu items
        let locs = locations.lock().unwrap();
        for (idx, location) in locs.iter().enumerate() {
            let btn = gtk::Button::with_label(&location.name);
            btn.add_css_class("flat");
            btn.set_hexpand(true);
            
            let _locations = locations.clone();  // Reserved for future use
            let current_location_idx = current_location_idx.clone();
            let menu_btn = location_menu_btn.clone();
            let popover = popover.clone();
            let location_name = location.name.clone();
            
            btn.connect_clicked(move |_| {
                *current_location_idx.lock().unwrap() = idx;
                menu_btn.set_label(&location_name);
                popover.popdown();
                println!("Switched to location: {}", location_name);
            });
            
            menu_box.append(&btn);
        }
        
        popover.set_child(Some(&menu_box));
        location_menu_btn.set_popover(Some(&popover));
    }
    
    toast_overlay.set_child(Some(&main_box));
    window.set_content(Some(&toast_overlay));

    // ========================================================================
    // Button Handlers (MUST be before UI update loop to avoid moved values)
    // ========================================================================
    
    // Calibrate button
    {
        let model = model.clone();
        let neural_field = neural_field.clone();
        let db = db.clone();
        let history_buffer = history_buffer.clone();
        let pulse_state = pulse_state.clone();
        let window = window.clone();
        let sensor_lock_ok = sensor_lock_ok.clone();
        let locations = locations.clone();
        let current_location_idx = current_location_idx.clone();
        let toast_overlay_clone = toast_overlay.clone();

        calibrate_btn.connect_clicked(move |_| {
            let lock_ok = *sensor_lock_ok.lock().unwrap();
            
            if !lock_ok {
                let dialog = adw::MessageDialog::builder()
                    .transient_for(&window)
                    .heading("Sensors Unavailable")
                    .body("Cannot calibrate: thermal sensors are locked by another process. Please close other instances of this app.")
                    .build();
                dialog.add_response("ok", "OK");
                dialog.present();
                
                let toast = adw::Toast::new("⚠ Sensors unavailable");
                toast.set_timeout(3);
                toast_overlay_clone.add_toast(toast);
                return;
            }

            let dialog = adw::MessageDialog::builder()
                .transient_for(&window)
                .heading("Calibrate Model")
                .body("Enter the known ambient temperature:")
                .build();

            dialog.add_response("cancel", "Cancel");
            dialog.add_response("ok", "Calibrate");
            dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);

            let entry = adw::EntryRow::new();
            entry.set_title("Temperature (°C)");
            entry.set_input_purpose(gtk::InputPurpose::Number);
            entry.set_input_hints(gtk::InputHints::NO_EMOJI | gtk::InputHints::NO_SPELLCHECK);
            
            // Set default value
            if let Some(text_widget) = entry.first_child() {
                if let Ok(text_entry) = text_widget.downcast::<gtk::Text>() {
                    text_entry.set_text("20");
                    text_entry.select_region(0, -1);
                }
            }

            let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
            content.append(&entry);
            dialog.set_extra_child(Some(&content));

            let model = model.clone();
            let neural_field = neural_field.clone();
            let db = db.clone();
            let history_buffer = history_buffer.clone();
            let locations = locations.clone();
            let current_location_idx = current_location_idx.clone();
            let pulse_state_local = pulse_state.clone();

            dialog.connect_response(None, move |dialog: &adw::MessageDialog, response| {
                if response == "ok" {
                    if let Ok(target) = entry.text().parse::<f32>() {
                        if target >= TEMP_MIN && target <= TEMP_MAX {
                            let model = model.clone();
                            let neural_field_calib = neural_field.clone();
                            let db = db.clone();
                            let history_buffer = history_buffer.clone();
                            let locations = locations.clone();
                            let current_location_idx = current_location_idx.clone();
                            
                            // Reset pulse state
                            *pulse_state_local.borrow_mut() = 0.0;
                            
                            // Do calibration in background thread
                            thread::spawn(move || {
                                println!("Calibration: Starting burst readings...");
                                let mut poller = ThermalPoller::new();
                                let mut burst_readings = Vec::new();

                                for _ in 0..ACTIVE_BURST_COUNT {
                                    if let Some(reading) = poller.poll() {
                                        burst_readings.push(reading);
                                    }
                                    thread::sleep(Duration::from_millis(ACTIVE_INTERVAL_MS));
                                }

                                println!("Calibration: Got {} burst readings", burst_readings.len());

                                let history: Vec<Vec<f32>> = {
                                    let buffer = history_buffer.lock().unwrap();
                                    buffer.iter().map(|(_, r)| r.clone()).collect()
                                };

                                let mut all_readings = history;
                                all_readings.extend(burst_readings);

                                if !all_readings.is_empty() {
                                    let num_sensors = all_readings[0].len();
                                    let mut avg_reading = vec![0.0; num_sensors];

                                    for reading in &all_readings {
                                        for (i, &val) in reading.iter().enumerate() {
                                            avg_reading[i] += val;
                                        }
                                    }

                                    for val in &mut avg_reading {
                                        *val /= all_readings.len() as f32;
                                    }

                                    println!("Calibration: Applying to model...");

                                    // Update linear model
                                    let model_path = {
                                        let mut m = model.lock().unwrap();
                                        m.calibrate(&avg_reading, target);
                                        m.prune_weak_sensors();
                                        PathBuf::from(DATA_DIR).join(format!("model_{}.json", get_timestamp()))
                                    };
                                    
                                    // Calibrate neural field
                                    if let Ok(mut nf) = neural_field_calib.try_lock() {
                                        let device_state = get_device_state();
                                        nf.calibrate(&avg_reading, &device_state, target, 0.001);
                                        println!("✓ Neural field calibrated");
                                    }
                                    
                                    // Save linear model
                                    let model_clone = model.clone();
                                    if let Ok(mut m) = model_clone.lock() {
                                        match m.save(&model_path) {
                                            Ok(_) => println!("✓ Model saved: {}", model_path.display()),
                                            Err(e) => eprintln!("✗ Failed to save model: {}", e),
                                        }
                                    }
                                    
                                    // Save neural field
                                    let neural_path = PathBuf::from(DATA_DIR).join(format!("neural_field_{}.json", get_timestamp()));
                                    let neural_clone = neural_field_calib.clone();
                                    if let Ok(nf) = neural_clone.lock() {
                                        match nf.save(&neural_path) {
                                            Ok(_) => println!("✓ Neural field saved: {}", neural_path.display()),
                                            Err(e) => eprintln!("✗ Failed to save neural field: {}", e),
                                        }
                                    }

                                    // Store calibration in database
                                    if let Ok(db) = db.lock() {
                                        let location_id = {
                                            let locs = locations.lock().unwrap();
                                            let idx = *current_location_idx.lock().unwrap();
                                            locs.get(idx).map(|l| l.id.clone())
                                        };
                                        match db.store_calibration(&avg_reading, target, location_id.as_deref()) {
                                            Ok(_) => println!("✓ Calibration saved to database"),
                                            Err(e) => eprintln!("✗ Failed to save calibration: {}", e),
                                        }
                                    }
                                    
                                    println!("✓ Calibration complete!");
                                } else {
                                    eprintln!("✗ Calibration failed: No readings available");
                                }
                            });
                        }
                    }
                }
                dialog.close();
            });

            dialog.present();
        });
    }
    
    // Log Weather button handler
    {
        let window = window.clone();
        let current_location_idx = current_location_idx.clone();
        let locations = locations.clone();
        let toast_overlay_clone = toast_overlay.clone();
        
        log_weather_btn.connect_clicked(move |_| {
            let dialog = adw::MessageDialog::builder()
                .transient_for(&window)
                .heading("Add Weather Data Point")
                .body("Log actual outdoor temperature observation")
                .build();
            
            dialog.add_response("cancel", "Cancel");
            dialog.add_response("ok", "Log");
            dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
            
            let temp_entry = adw::EntryRow::new();
            temp_entry.set_title("Temperature (°C)");
            temp_entry.set_input_purpose(gtk::InputPurpose::Number);
            
            let notes_entry = adw::EntryRow::new();
            notes_entry.set_title("Notes (optional)");
            
            let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
            content.append(&temp_entry);
            content.append(&notes_entry);
            dialog.set_extra_child(Some(&content));
            
            let toast_overlay_for_dialog = toast_overlay_clone.clone();
            let locations_for_dialog = locations.clone();
            let current_location_idx_for_dialog = current_location_idx.clone();
            
            dialog.connect_response(None, move |dialog: &adw::MessageDialog, response| {
                if response == "ok" {
                    if let Ok(temp) = temp_entry.text().parse::<f32>() {
                        let location = {
                            let locs = locations_for_dialog.lock().unwrap();
                            let idx = *current_location_idx_for_dialog.lock().unwrap();
                            locs.get(idx)
                                .map(|l| l.name.clone())
                                .unwrap_or_else(|| "Unknown".to_string())
                        };
                        
                        let notes = notes_entry.text();
                        
                        println!("📊 Weather data: {}°C at {} - {}", temp, location, notes);
                        // TODO: Store to weather_history database table
                        
                        let toast = adw::Toast::new(&format!("✓ Logged {}°C", temp));
                        toast.set_timeout(2);
                        toast_overlay_for_dialog.add_toast(toast);
                    }
                }
                dialog.close();
            });
            
            dialog.present();
        });
    }

    // UI update loop with adaptive rate and battery optimization
    {
        let model = model.clone();
        let neural_field = neural_field.clone();
        let history_buffer = history_buffer.clone();
        let forecast_model = forecast_model.clone();
        let _pulse_state = pulse_state.clone();
        let sensor_lock_ok = sensor_lock_ok.clone();
        let window_for_active = window.clone();
        let page1_for_animations = page1.clone();
        let forecast_1h = forecast_1h_label.clone();
        let forecast_24h = forecast_24h_label.clone();
        let sphere_picture_clone = sphere_picture.clone();
        let db_for_logging = db.clone();
        let locations_for_logging = locations.clone();
        let current_location_idx_for_logging = current_location_idx.clone();
        let world_for_ui = world.clone();  // WorldState for UI updates
        
        // Neural field UI labels
        let neural_ambient_clone = neural_ambient_label.clone();
        let neural_cardinal_clone = neural_cardinal_label.clone();
        let neural_status_clone = neural_status_label.clone();
        
        // Cache to reduce lock contention
        let mut last_temp = 20.0f32;
        let mut last_conf = 0.0f32;
        let mut last_active = 0usize;
        let mut last_calibrations = 0usize;
        let mut last_forecast_update = std::time::Instant::now();
        let mut last_sensor_log = std::time::Instant::now();
        let mut sphere_cache = sphere::SphereCache::new();

        // Track (month, hour) so we only re-apply season/phase classes when
        // they actually change — saves churn on every animation tick.
        // `(0, 99)` is an invalid pair that guarantees the first iteration
        // runs the apply.
        let mut last_season_phase: (u32, u32) = (0, 99);
        
        // Battery optimization: track temperature variance for adaptive polling
        let mut temp_history: std::collections::VecDeque<f32> = std::collections::VecDeque::with_capacity(20);
        let mut update_count = 0u32;
        let last_interaction = std::time::Instant::now();
        let mut animations_paused = false;
        
        // Adaptive update interval (starts at 100ms)
        let update_interval = std::cell::Cell::new(100u64);

        glib::timeout_add_local(Duration::from_millis(100), move || {
            update_count += 1;
            
            // Battery optimization: detect if window is active
            let window_active = window_for_active.is_active();
            
            // Battery optimization: detect user inactivity
            let idle_time = last_interaction.elapsed();
            
            // Battery optimization: pause animations when inactive or idle
            if !window_active || idle_time > Duration::from_secs(30) {
                if !animations_paused {
                    page1_for_animations.add_css_class("animations-paused");
                    animations_paused = true;
                    println!("🔋 Battery save: animations paused");
                }
            } else {
                if animations_paused {
                    page1_for_animations.remove_css_class("animations-paused");
                    animations_paused = false;
                    println!("⚡ Animations resumed");
                }
            }
            
            // Battery optimization: ultra-low power when in background
            if !window_active {
                // Skip most updates when not visible
                if update_count % 50 != 0 { // Only update every 5 seconds
                    return glib::ControlFlow::Continue;
                }
            }
            
            // ================================================================
            // NEW: Read from WorldState (populated by worker threads)
            // ================================================================
            let (temp, conf, model_source_str, calibrations, active) = {
                if let Ok(state) = world_for_ui.read() {
                    (
                        state.ambient_temp,
                        state.confidence,
                        state.model_type.as_str(),
                        state.calibration_count,
                        state.sensor_count
                    )
                } else {
                    // Fallback if lock fails
                    (last_temp, last_conf, "Unknown", last_calibrations, last_active)
                }
            };
            
            let model_source = model_source_str;
            
            // Keep m_clone for compatibility (can remove later)
            let m_clone = model.lock().ok().map(|m| m.clone()).unwrap_or_else(|| {
                Model::new(vec![])
            });
            
            // Battery optimization: track temperature variance for adaptive polling
            temp_history.push_back(temp);
            if temp_history.len() > 20 {
                temp_history.pop_front();
            }
            
            // Calculate variance every 2 seconds (20 updates at 100ms)
            if update_count % 20 == 0 && temp_history.len() >= 10 {
                let mean = temp_history.iter().sum::<f32>() / temp_history.len() as f32;
                let variance = temp_history.iter()
                    .map(|t| (t - mean).powi(2))
                    .sum::<f32>() / temp_history.len() as f32;
                
                // Adaptive interval based on temperature stability and user activity
                let new_interval = if !window_active {
                    5000 // Background: very slow updates (5s)
                } else if idle_time > Duration::from_secs(300) {
                    2000 // 5min idle: slow updates (2s)
                } else if idle_time > Duration::from_secs(30) {
                    1000 // 30s idle: reduced updates (1s)
                } else if variance > 0.5 {
                    100  // Temperature changing rapidly: fast updates
                } else if variance > 0.1 {
                    500  // Temperature changing slowly: medium updates
                } else {
                    1000 // Temperature stable: slow updates
                };
                
                if new_interval != update_interval.get() {
                    update_interval.set(new_interval);
                    println!("🔋 Adaptive polling: {}ms (variance: {:.3}, idle: {}s)", 
                        new_interval, variance, idle_time.as_secs());
                }
            }
            
            // Only update UI if values changed
            if (temp - last_temp).abs() > 0.05 {
                let color = temp_to_color(temp);
                temp_label.set_markup(&format!(
                    "<span size='80000' weight='bold' foreground='{}'>{:.1}°C</span>",
                    color, temp
                ));
                
                // Apply dynamic gradient and glow based on temperature and confidence.
                // The temp label additionally picks up `aurora` / `heat-haze` mood
                // animations from get_temp_mood_class via apply_dynamic_styling.
                apply_dynamic_styling(&temp_label, temp, conf);
                apply_dynamic_styling(&page1, temp, conf);
                
                last_temp = temp;
            }

            if (conf - last_conf).abs() > 0.01 {
                conf_label.set_markup(&format!("<span size='x-large' weight='medium'>Confidence: {:.0}%</span>", conf * 100.0));
                last_conf = conf;
            }
            
            if active != last_active || calibrations != last_calibrations {
                status_label.set_text(&format!("Active sensors: {} | Calibrations: {} | Model: {}", 
                    active, calibrations, model_source));

                // Calibration count just changed: pulse a celebratory flash on the
                // status label. Only do this for *increases* — sensor count drops
                // shouldn't celebrate.
                if calibrations > last_calibrations {
                    pulse_calibration_flash(&status_label);
                }

                last_active = active;
                last_calibrations = calibrations;
            }

            // Sensor health pill: swap CSS classes so the keyframe animation
            // ("breathe" when ok, faster "blink" when degraded, steady when
            // unavailable) takes over from the static markup color.
            let lock_ok = *sensor_lock_ok.lock().unwrap();
            for c in ["health-ok", "health-degraded", "health-unavailable"] {
                sensor_lock_label.remove_css_class(c);
            }
            if lock_ok {
                sensor_lock_label.add_css_class("health-ok");
                sensor_lock_label.set_markup("<span size='large' weight='medium'>● Sensors locked</span>");
            } else {
                sensor_lock_label.add_css_class("health-unavailable");
                sensor_lock_label.set_markup("<span size='large' weight='medium'>⚠ Sensors unavailable</span>");
            }

            // Re-evaluate seasonal/day-phase styling once a minute. Applying
            // the same class twice is a no-op so this is cheap; we only do
            // a remove/add when the hour or month actually changes.
            {
                use chrono::{Datelike, Timelike};
                let now = chrono::Local::now();
                let new_season_phase = (now.month(), now.hour());
                if new_season_phase != last_season_phase {
                    apply_seasonal_styling(&page1, now.month(), now.hour());
                    last_season_phase = new_season_phase;
                }
            }
            
            // Update forecasts every 60 seconds
            if last_forecast_update.elapsed() > Duration::from_secs(60) {
                if let Ok(mut fm) = forecast_model.try_lock() {
                    // Add current temperature to forecast history
                    fm.add_observation(forecast::TimeSeriesPoint {
                        timestamp: chrono::Utc::now().timestamp(),
                        temperature: temp,
                        location_id: "current".to_string(),
                    });
                    
                    // Generate forecasts
                    if let Some(forecast_1h_data) = fm.forecast(1.0) {
                        forecast_1h.set_markup(&format!(
                            "<span size='large'>1 hour: <b>{:.1}°C</b> ({:.1}-{:.1}°C)</span>",
                            forecast_1h_data.temperature,
                            forecast_1h_data.min_temp,
                            forecast_1h_data.max_temp
                        ));
                    }
                    
                    if let Some(forecast_24h_data) = fm.forecast(24.0) {
                        forecast_24h.set_markup(&format!(
                            "<span size='large'>24 hours: <b>{:.1}°C</b> ({:.1}-{:.1}°C)</span>",
                            forecast_24h_data.temperature,
                            forecast_24h_data.min_temp,
                            forecast_24h_data.max_temp
                        ));
                    }
                    
                    last_forecast_update = std::time::Instant::now();
                }
            }
            
            // Log sensor data to database every 60 seconds
            if last_sensor_log.elapsed() > Duration::from_secs(60) {
                if let Ok(history) = history_buffer.try_lock() {
                    if let Some((_timestamp, readings)) = history.back() {
                        // Get prediction (neural if available, otherwise linear)
                        let ambient_prediction = if let Ok(nf) = neural_field.try_lock() {
                            if nf.get_calibration_count() >= 5 {
                                drop(nf);
                                if let Ok(mut nf_mut) = neural_field.try_lock() {
                                    let device_state = get_device_state();
                                    Some(nf_mut.estimate_ambient(readings, &device_state))
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                        
                        let prediction = ambient_prediction.unwrap_or(temp);
                        
                        // Get Unix timestamp (seconds since epoch)
                        let unix_timestamp = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs() as i64;
                        
                        // Compress reading (357 bytes → 63 bytes, 82% reduction)
                        let hour_start = (unix_timestamp / 3600) * 3600;
                        let compressed = helpers::CompressedReading::compress(
                            unix_timestamp,
                            readings,
                            prediction,
                            hour_start
                        );
                        
                        // Store compressed (would need to update Database impl)
                        // For now, still store as JSON but log compression stats
                        if update_count % 600 == 0 {  // Log every 10 minutes (600 updates at 100ms)
                            let json_size = serde_json::to_string(&readings).unwrap().len();
                            let compressed_size = compressed.to_bytes().len();
                            let ratio = (1.0 - (compressed_size as f32 / json_size as f32)) * 100.0;
                            println!("📊 Compression: {} bytes → {} bytes ({:.0}% reduction)", 
                                json_size, compressed_size, ratio);
                        }
                        
                        // Store reading (using existing JSON format for compatibility)
                        // Get device state for full reading
                        let device_state = get_device_state();
                        let reading = SensorReading {
                            time: unix_timestamp.to_string(),
                            sensors: readings.clone(),
                            cpu_load: device_state.cpu_load,
                            als_lux: 0,  // TODO: read actual ambient light sensor
                            charging: device_state.charging,
                            screen: (device_state.screen_brightness * 255.0) as u8,
                        };
                        
                        if let Ok(db) = db_for_logging.try_lock() {
                            let location_id = {
                                let locs = locations_for_logging.lock().unwrap();
                                let idx = *current_location_idx_for_logging.lock().unwrap();
                                locs.get(idx).map(|l| l.id.clone())
                            };
                            
                            if let Err(e) = db.store_reading(&reading, location_id.as_deref()) {
                                eprintln!("Failed to log sensor data: {}", e);
                            }
                        }
                    }
                }
                last_sensor_log = std::time::Instant::now();
            }
            
            // Update temperature sphere every 1 second (10 updates at 100ms)
            if update_count % 10 == 0 {
                if let Ok(history) = history_buffer.try_lock() {
                    if let Some((_, readings)) = history.back() {
                        let svg = sphere_cache.get_sphere(readings, 400, 400);
                        
                        // Convert SVG string to paintable
                        let bytes = glib::Bytes::from(svg.as_bytes());
                        
                        if let Ok(paintable) = gtk::gdk::Texture::from_bytes(&bytes) {
                            sphere_picture_clone.set_paintable(Some(&paintable));
                        }
                    }
                }
            }
            
            // Update Neural Field predictions (physics-aware model)
            {
                if let Ok(nf) = neural_field.try_lock() {
                    let history_size = nf.get_history_size();
                    
                    // Need at least 60 seconds of history for gradients
                    if history_size >= 60 {
                        // Get current sensor readings
                        if let Ok(_m) = model.try_lock() {
                            // Get latest sensor reading from history
                            if let Ok(history) = history_buffer.try_lock() {
                                if let Some((_, readings)) = history.back() {
                                    // Check sensor count match
                                    let expected_sensors = nf.get_num_sensors();
                                    if readings.len() != expected_sensors {
                                        // Sensor mismatch - show error
                                        neural_status_clone.set_markup(&format!(
                                            "<span foreground='#E24A4A'>⚠ Sensor mismatch: {} sensors, expected {}</span>",
                                            readings.len(), expected_sensors
                                        ));
                                        neural_ambient_clone.set_text("Ambient: --°C");
                                        neural_cardinal_clone.set_text("Incompatible sensor count");
                                    } else {
                                        let device_state = get_device_state();
                                        
                                        // Make neural_field mutable for prediction
                                        drop(nf);  // Release immutable lock
                                        if let Ok(mut nf_mut) = neural_field.try_lock() {
                                            // Get ambient prediction
                                            let neural_ambient = nf_mut.estimate_ambient(readings, &device_state);
                                            
                                            neural_ambient_clone.set_markup(&format!(
                                                "<span size='large'>Ambient: <b>{:.1}°C</b></span>",
                                                neural_ambient
                                            ));
                                            
                                            // Get spatial predictions (1m in each direction)
                                            let north = nf_mut.estimate_temperature(0.0, 1.0, 0.0, readings, &device_state);
                                            let south = nf_mut.estimate_temperature(0.0, -1.0, 0.0, readings, &device_state);
                                            let east = nf_mut.estimate_temperature(1.0, 0.0, 0.0, readings, &device_state);
                                            let west = nf_mut.estimate_temperature(-1.0, 0.0, 0.0, readings, &device_state);
                                            
                                            neural_cardinal_clone.set_markup(&format!(
                                                "<span>N: {:.1}°C | S: {:.1}°C | E: {:.1}°C | W: {:.1}°C</span>",
                                                north, south, east, west
                                            ));
                                            
                                            // Update status based on calibration count
                                            let cal_count = nf_mut.get_calibration_count();
                                            let status_text = if cal_count == 0 {
                                                String::from("<span foreground='#888888'>● Untrained (0 calibrations)</span>")
                                            } else if cal_count < 5 {
                                                format!("<span foreground='#FFA500'>● Learning ({} calibrations)</span>", cal_count)
                                            } else {
                                                format!("<span foreground='#7ED321'>● Trained ({} calibrations)</span>", cal_count)
                                            };
                                            neural_status_clone.set_markup(&status_text);
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                        // Building history
                        neural_status_clone.set_markup(&format!(
                            "<span foreground='#888888'>Building history... ({}/60s)</span>",
                            history_size
                        ));
                    }
                }
                // If lock fails, skip update (non-critical)
            }

            // Smart SVG caching - only regenerate when model actually changed
            // Uses hash to detect changes, smooth animation via pulse
            // A thread_local RefCell rather than `static mut`.
            //
            // `&CACHED_SVG` below — and the autoref in `.is_none()` — are
            // exactly what the `static_mut_refs` lint rejects, and that lint is
            // a HARD ERROR in edition 2024 rather than the warning it is today.
            // This is a GTK callback and therefore always on the main thread,
            // so a thread_local is the closest safe equivalent: same lifetime,
            // same single-threaded assumption, no unsafe and no lint to
            // suppress.
            thread_local! {
                static SVG_CACHE: std::cell::RefCell<(u64, Option<String>)> =
                    const { std::cell::RefCell::new((0, None)) };
            }
            
            // Compute lightweight hash of model state
            let model_hash = {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut hasher = DefaultHasher::new();
                calibrations.hash(&mut hasher);
                active.hash(&mut hasher);
                ((temp * 10.0) as i32).hash(&mut hasher);
                hasher.finish()
            };
            
            SVG_CACHE.with(|cache| {
                let mut cache = cache.borrow_mut();
                // Only regenerate SVG if model changed
                if model_hash != cache.0 || cache.1.is_none() {
                    let svg = generate_network_svg(&m_clone, 0.0);
                    cache.0 = model_hash;
                    cache.1 = Some(svg);

                    if let Some(svg) = &cache.1 {
                        let loader = PixbufLoader::new();
                        let _ = loader.write(svg.as_bytes());
                        let _ = loader.close();
                        if let Some(pixbuf) = loader.pixbuf() {
                            model_picture.set_pixbuf(Some(&pixbuf));
                        }
                    }
                }
            });

            glib::ControlFlow::Continue
        });
    }

    // Save model periodically (only if changed) - in background thread
    {
        let model = model.clone();
        
        thread::spawn(move || {
            loop {
                thread::sleep(Duration::from_secs(60));
                
                // Check if changed and save in background
                let should_save = {
                    let m = model.lock().unwrap();
                    m.has_changed()
                };
                
                if should_save {
                    if let Ok(mut m) = model.lock() {
                        let model_path = PathBuf::from(DATA_DIR).join(format!("model_{}.json", get_timestamp()));
                        match m.save(&model_path) {
                            Ok(_) => println!("Model saved: {} (calibrations: {})", model_path.display(), m.calibration_count),
                            Err(e) => eprintln!("Failed to save model: {}", e),
                        }
                    }
                }
            }
        });
    }

    
    // Export button handler
    {
        let window = window.clone();
        let db = db.clone();
        let toast_overlay_clone = toast_overlay.clone();
        
        export_btn.connect_clicked(move |_| {
            let file_dialog = gtk::FileDialog::builder()
                .title("Export Database")
                .initial_name(&format!("ambient_learner_export_{}.db", 
                    chrono::Local::now().format("%Y%m%d_%H%M%S")))
                .build();
            
            let _window_clone = window.clone();
            let db_clone = db.clone();
            let toast_clone = toast_overlay_clone.clone();
            
            file_dialog.save(Some(&window), gtk::gio::Cancellable::NONE, move |result| {
                if let Ok(file) = result {
                    if let Some(path) = file.path() {
                        // Get current database path
                        let db_guard = db_clone.lock().unwrap();
                        let current_db_path = format!("{}/{}", DATA_DIR, 
                            std::fs::read_dir(DATA_DIR)
                                .ok()
                                .and_then(|entries| {
                                    entries.filter_map(Result::ok)
                                        .find(|e| e.path().extension().map(|s| s == "db").unwrap_or(false))
                                        .map(|e| e.file_name().to_string_lossy().to_string())
                                })
                                .unwrap_or_else(|| "sensors.db".to_string()));
                        drop(db_guard);
                        
                        // Copy database file
                        if let Err(e) = std::fs::copy(&current_db_path, &path) {
                            eprintln!("Failed to export database: {}", e);
                            let toast = adw::Toast::new(&format!("❌ Export failed: {}", e));
                            toast.set_timeout(5);
                            toast_clone.add_toast(toast);
                        } else {
                            println!("✅ Database exported to: {:?}", path);
                            let toast = adw::Toast::new("✅ Database exported successfully");
                            toast.set_timeout(3);
                            toast_clone.add_toast(toast);
                        }
                    }
                }
            });
        });
    }
    
    // Import button handler  
    {
        let window = window.clone();
        let db = db.clone();
        let toast_overlay_clone = toast_overlay.clone();
        
        import_btn.connect_clicked(move |_| {
            let file_dialog = gtk::FileDialog::builder()
                .title("Import Database")
                .build();
            
            // Add file filter for .db files
            let filter = gtk::FileFilter::new();
            filter.add_pattern("*.db");
            filter.set_name(Some("Database files (*.db)"));
            
            let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            file_dialog.set_filters(Some(&filters));
            
            let _window_clone = window.clone();
            let _db_clone = db.clone();
            let toast_clone = toast_overlay_clone.clone();
            
            file_dialog.open(Some(&window), gtk::gio::Cancellable::NONE, move |result| {
                if let Ok(file) = result {
                    if let Some(path) = file.path() {
                        println!("📥 Importing database from: {:?}", path);
                        
                        // Verify it's a valid SQLite database
                        match rusqlite::Connection::open(&path) {
                            Ok(conn) => {
                                // Check if it has calibration_data table
                                let has_table: Result<bool, _> = conn.query_row(
                                    "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='calibration_data'",
                                    [],
                                    |row| row.get(0)
                                );
                                
                                match has_table {
                                    Ok(true) => {
                                        // Count calibrations
                                        let count: Result<i64, _> = conn.query_row(
                                            "SELECT COUNT(*) FROM calibration_data",
                                            [],
                                            |row| row.get(0)
                                        );
                                        
                                        if let Ok(cal_count) = count {
                                            println!("✅ Valid database with {} calibrations", cal_count);
                                            
                                            let toast = adw::Toast::new(&format!("📊 Database has {} calibrations", cal_count));
                                            toast.set_timeout(5);
                                            toast_clone.add_toast(toast);
                                        } else {
                                            // Try to get more info
                                            let toast = adw::Toast::new("⚠️ Database found but couldn't count calibrations");
                                            toast.set_timeout(5);
                                            toast_clone.add_toast(toast);
                                        }
                                    }
                                    Ok(false) => {
                                        // Check for other possible table names (legacy compatibility)
                                        let alt_tables = vec!["calibrations", "data", "sensor_data"];
                                        let mut found = false;
                                        
                                        for table_name in alt_tables {
                                            let has_alt: Result<bool, _> = conn.query_row(
                                                &format!("SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='{}'", table_name),
                                                [],
                                                |row| row.get(0)
                                            );
                                            
                                            if let Ok(true) = has_alt {
                                                found = true;
                                                let toast = adw::Toast::new(&format!("⚠️ Legacy database format (table: {})", table_name));
                                                toast.set_timeout(5);
                                                toast_clone.add_toast(toast);
                                                break;
                                            }
                                        }
                                        
                                        if !found {
                                            let toast = adw::Toast::new("❌ Not a valid ambient learner database");
                                            toast.set_timeout(5);
                                            toast_clone.add_toast(toast);
                                        }
                                    }
                                    Err(e) => {
                                        eprintln!("Database query error: {}", e);
                                        let toast = adw::Toast::new(&format!("❌ Database error: {}", e));
                                        toast.set_timeout(5);
                                        toast_clone.add_toast(toast);
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("Failed to open database: {}", e);
                                let toast = adw::Toast::new(&format!("❌ Invalid database file"));
                                toast.set_timeout(5);
                                toast_clone.add_toast(toast);
                            }
                        }
                    }
                }
            });
        });
    }

    // Populate locations list
    fn refresh_locations_list(
        locations_list: &gtk::Box,
        locations: &Arc<Mutex<Vec<Location>>>,
        current_idx: &Arc<Mutex<usize>>,
    ) {
        // Clear existing
        while let Some(child) = locations_list.first_child() {
            locations_list.remove(&child);
        }
        
        let locs = locations.lock().unwrap();
        for (idx, location) in locs.iter().enumerate() {
            let row = adw::ActionRow::new();
            row.set_title(&location.name);
            
            let subtitle = match &location.location_type {
                LocationType::City(city) => format!("📍 {}", city),
                LocationType::NetworkId(ssid) => format!("📶 {}", ssid),
                LocationType::Unknown => "❓ Unknown".to_string(),
            };
            row.set_subtitle(&subtitle);
            
            // Current location indicator
            if idx == *current_idx.lock().unwrap() {
                row.add_prefix(&gtk::Image::from_icon_name("emblem-ok-symbolic"));
            }
            
            locations_list.append(&row);
        }
    }
    
    // Initial population
    refresh_locations_list(&locations_list, &locations, &current_location_idx);
    
    // Add location dialog
    {
        let window = window.clone();
        let locations = locations.clone();
        let locations_list = locations_list.clone();
        let current_location_idx = current_location_idx.clone();
        
        // Clone menu widgets BEFORE closure (to avoid borrow-after-move)
        let menu_box_for_dialog = menu_box.clone();
        let popover_for_dialog = popover.clone();
        let menu_btn_for_dialog = location_menu_btn.clone();
        
        // Clone cached SSID for non-blocking access
        let cached_ssid_for_dialog = cached_ssid.clone();
        
        add_location_btn.connect_clicked(move |_| {
            let dialog = adw::MessageDialog::builder()
                .transient_for(&window)
                .heading("Add New Location")
                .build();

            dialog.add_response("cancel", "Cancel");
            dialog.add_response("city", "Add City");
            dialog.add_response("network", "Pin to Network");
            dialog.set_response_appearance("city", adw::ResponseAppearance::Suggested);

            let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
            
            let name_entry = adw::EntryRow::new();
            name_entry.set_title("Location Name");
            name_entry.set_text("Home");
            
            let city_entry = adw::ComboRow::new();
            city_entry.set_title("City (for City location)");
            city_entry.set_subtitle("Search or scroll to find your city");
            
            // Get comprehensive city list
            let cities = get_timezone_cities();
            println!("Loaded {} cities for selection", cities.len());
            
            let city_model = gtk::StringList::new(&cities.iter().map(|s| s.as_str()).collect::<Vec<_>>());
            city_entry.set_model(Some(&city_model));
            // city_entry.set_enable_search(true);  // API removed in newer libadwaita
            
            let network_label = gtk::Label::new(None);
            // Use cached SSID instead of blocking call (safe - no unwrap)
            let ssid = cached_ssid_for_dialog.lock().ok()
                .and_then(|guard| guard.clone());
            
            if let Some(ssid) = ssid {
                network_label.set_markup(&format!("Current network: <b>{}</b>", ssid));
            } else {
                network_label.set_markup("<i>No network detected</i>");
            }
            network_label.set_margin_top(8);
            
            content.append(&name_entry);
            content.append(&city_entry);
            content.append(&network_label);
            dialog.set_extra_child(Some(&content));

            let locations = locations.clone();
            let locations_list = locations_list.clone();
            let current_location_idx = current_location_idx.clone();
            
            // Use the pre-cloned menu widgets from outer scope
            let menu_box = menu_box_for_dialog.clone();
            let popover = popover_for_dialog.clone();
            let menu_btn = menu_btn_for_dialog.clone();
            
            // Clone cached SSID for network response
            let cached_ssid = cached_ssid_for_dialog.clone();
            
            dialog.connect_response(None, move |dialog: &adw::MessageDialog, response| {
                match response {
                    "city" => {
                        let name = name_entry.text().to_string();
                        let city_idx = city_entry.selected();
                        let city = cities.get(city_idx as usize).cloned().unwrap_or_else(|| "Montreal".to_string());
                        
                        let location = Location::new(name.clone(), LocationType::City(city));
                        
                        println!("📍 Adding city location: {}", name);
                        
                        locations.lock().unwrap().push(location.clone());
                        let _ = save_locations(&locations.lock().unwrap());
                        refresh_locations_list(&locations_list, &locations, &current_location_idx);
                        
                        // CRITICAL FIX: Rebuild menu immediately!
                        println!("🔄 Rebuilding menu after add...");
                        rebuild_location_menu(
                            &menu_box,
                            &popover,
                            &locations,
                            &current_location_idx,
                            &menu_btn
                        );
                        
                        // Switch to new location
                        let new_idx = locations.lock().unwrap().len() - 1;
                        *current_location_idx.lock().unwrap() = new_idx;
                        menu_btn.set_label(&name);
                        
                        println!("✓ Location added and menu updated!");
                    }
                    "network" => {
                        // Use cached SSID instead of blocking call (safe - no unwrap)
                        let ssid = cached_ssid.lock().ok()
                            .and_then(|guard| guard.clone());
                        
                        if let Some(ssid) = ssid {
                            let name = name_entry.text().to_string();
                            let location = Location::new(name.clone(), LocationType::NetworkId(ssid));
                            
                            println!("📶 Adding network location: {}", name);
                            
                            if let Ok(mut locs) = locations.lock() {
                                locs.push(location.clone());
                                let _ = save_locations(&locs);
                            }
                            
                            refresh_locations_list(&locations_list, &locations, &current_location_idx);
                            
                            // CRITICAL FIX: Rebuild menu immediately!
                            println!("🔄 Rebuilding menu after add...");
                            rebuild_location_menu(
                                &menu_box,
                                &popover,
                                &locations,
                                &current_location_idx,
                                &menu_btn
                            );
                            
                            // Switch to new location
                            let new_idx = locations.lock().unwrap().len() - 1;
                            *current_location_idx.lock().unwrap() = new_idx;
                            menu_btn.set_label(&name);
                            
                            println!("✓ Location added and menu updated!");
                        }
                    }
                    _ => {}
                }
                dialog.close();
            });

            dialog.present();
        });
    }

    // History navigation with touch gestures
    {
        let week_offset_ref = week_offset.clone();
        let history_picture_clone = history_picture.clone();
        let week_label_clone = week_label.clone();
        let date_range_label_clone = date_range_label.clone();
        let next_week_btn_clone = next_week_btn.clone();

        // Create update function that can be called multiple times
        let update_history = {
            let week_offset = week_offset_ref.clone();
            let history_picture = history_picture_clone.clone();
            let week_label = week_label_clone.clone();
            let date_range_label = date_range_label_clone.clone();
            let next_week_btn = next_week_btn_clone.clone();
            let history_spinner = history_spinner.clone();
            
            Rc::new(move || {
                let offset = *week_offset.borrow();
                
                // Start spinner
                history_spinner.start();
                history_spinner.set_visible(true);
                
                // Clone for the idle callback
                let history_picture = history_picture.clone();
                let week_label = week_label.clone();
                let date_range_label = date_range_label.clone();
                let next_week_btn = next_week_btn.clone();
                let history_spinner = history_spinner.clone();
                
                // Spawn background task to avoid blocking UI
                glib::idle_add_local_once(move || {
                    let daily_data = {
                        let all_dbs = list_all_databases();
                        let mut combined_data = Vec::new();
                        
                        for db_path in all_dbs {
                            if let Ok(conn) = Connection::open(&db_path) {
                                let temp_db = Database { conn, path: db_path };
                                let data = temp_db.get_daily_averages(offset, 7, None);
                                combined_data.extend(data);
                            }
                        }
                        
                        combined_data.sort_by(|a, b| b.0.cmp(&a.0));
                        combined_data.truncate(7);
                        combined_data
                    };

                    let svg = generate_history_svg(&daily_data);

                    let loader = PixbufLoader::new();
                    let _ = loader.write(svg.as_bytes());
                    let _ = loader.close();
                    if let Some(pixbuf) = loader.pixbuf() {
                        history_picture.set_pixbuf(Some(&pixbuf));
                    }

                    // Update labels
                    if offset == 0 {
                        week_label.set_text("This Week");
                        next_week_btn.set_sensitive(false);
                    } else {
                        let weeks_ago = offset / 7 + 1;
                        week_label.set_text(&format!("{} Week{} Ago", weeks_ago, if weeks_ago > 1 { "s" } else { "" }));
                        next_week_btn.set_sensitive(true);
                    }
                    
                    // Show date range if we have data
                    if !daily_data.is_empty() {
                        let first = &daily_data.last().unwrap().0;
                        let last = &daily_data.first().unwrap().0;
                        date_range_label.set_text(&format!("{} to {}", first, last));
                    } else {
                        date_range_label.set_text("No data available");
                    }
                    
                    // Stop spinner
                    history_spinner.stop();
                    history_spinner.set_visible(false);
                });
            })
        };

        // Touch gesture support for swipe navigation
        let gesture = gtk::GestureSwipe::new();
        gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
        
        {
            let week_offset = week_offset_ref.clone();
            let update_history = update_history.clone();
            
            gesture.connect_swipe(move |_, velocity_x: f64, _velocity_y: f64| {
                // Swipe right = go back in time (previous week)
                // Swipe left = go forward in time (next week)
                if velocity_x.abs() > 200.0 {
                    if velocity_x < 0.0 {
                        // Swipe left - next week (more recent)
                        let mut offset = week_offset.borrow_mut();
                        if *offset >= 7 {
                            *offset -= 7;
                            drop(offset);
                            (update_history)();
                        }
                    } else {
                        // Swipe right - previous week (older)
                        *week_offset.borrow_mut() += 7;
                        (update_history)();
                    }
                }
            });
        }
        
        history_picture.add_controller(gesture);

        prev_week_btn.connect_clicked({
            let week_offset = week_offset_ref.clone();
            let update_history = update_history.clone();
            move |_| {
                *week_offset.borrow_mut() += 7;
                (update_history)();
            }
        });

        next_week_btn.connect_clicked({
            let week_offset = week_offset_ref.clone();
            let update_history = update_history.clone();
            move |_| {
                let mut offset = week_offset.borrow_mut();
                if *offset >= 7 {
                    *offset -= 7;
                    drop(offset);
                    (update_history)();
                }
            }
        });

        today_btn.connect_clicked({
            let week_offset = week_offset_ref.clone();
            let update_history = update_history.clone();
            move |_| {
                *week_offset.borrow_mut() = 0;
                (update_history)();
            }
        });

        // Refresh history when switching to History tab
        {
            let update_history = update_history.clone();
            stack.connect_visible_child_name_notify(move |stack| {
                if let Some(name) = stack.visible_child_name() {
                    if name.as_str() == "history" {
                        (update_history)();
                    }
                }
            });
        }

        // Initial load when app starts
        {
            let update_history = update_history.clone();
            glib::timeout_add_local_once(Duration::from_millis(500), move || {
                (update_history)();
            });
        }
    }

    // Export CSV handler
    {
        let window = window.clone();

        export_btn.connect_clicked(move |_| {
            let export_path = PathBuf::from(DATA_DIR).join(format!("history_export_{}.csv", get_timestamp()));
            
            // Combine all databases for export
            let all_dbs = list_all_databases();
            
            if all_dbs.is_empty() {
                let dialog = adw::MessageDialog::builder()
                    .transient_for(&window)
                    .heading("No Data Available")
                    .body("There is no historical data to export.")
                    .build();
                dialog.add_response("ok", "OK");
                dialog.present();
                return;
            }

            // Export from all databases
            let mut success = false;
            for db_path in all_dbs {
                if let Ok(conn) = Connection::open(&db_path) {
                    let temp_db = Database { conn, path: db_path.clone() };
                    if temp_db.export_to_csv(&export_path).is_ok() {
                        success = true;
                        break; // Export from the first available DB
                    }
                }
            }

            if success {
                let dialog = adw::MessageDialog::builder()
                    .transient_for(&window)
                    .heading("Export Successful")
                    .body(&format!("History exported to:\n{}", export_path.display()))
                    .build();
                dialog.add_response("ok", "OK");
                dialog.present();
            } else {
                let dialog = adw::MessageDialog::builder()
                    .transient_for(&window)
                    .heading("Export Failed")
                    .body("Could not export data. Please check permissions.")
                    .build();
                dialog.add_response("ok", "OK");
                dialog.present();
            }
        });
    }

    println!("[9/10] All handlers configured");
    println!("[10/10] Presenting window...");
    window.present();
    
    // Wait for locations to load, then rebuild menu ONCE on main thread
    {
        let locations_clone = locations.clone();
        let current_idx_clone = current_location_idx.clone();
        let menu_box_clone = menu_box.clone();
        let popover_clone = popover.clone();
        let menu_btn_clone = location_menu_btn.clone();
        let cached_ssid_clone = cached_ssid.clone();
        
        // Use a simple timeout that exits after menu is built
        let mut menu_built = false;
        
        glib::timeout_add_local(Duration::from_millis(100), move || {
            if menu_built {
                return glib::ControlFlow::Break;
            }
            
            // Try to get locations without blocking
            if let Ok(locs) = locations_clone.lock() {
                if !locs.is_empty() {
                    println!("📥 Locations loaded, rebuilding menu...");
                    drop(locs);
                    
                    // Rebuild menu (on main thread, safe)
                    rebuild_location_menu(
                        &menu_box_clone,
                        &popover_clone,
                        &locations_clone,
                        &current_idx_clone,
                        &menu_btn_clone
                    );
                    
                    // Try to detect location using cached WiFi
                    if let Some(ssid) = cached_ssid_clone.lock().ok().and_then(|c| c.clone()) {
                        if let Ok(locs) = locations_clone.lock() {
                            for (idx, loc) in locs.iter().enumerate() {
                                if loc.matches_network(&ssid) {
                                    if let Ok(mut current_idx) = current_idx_clone.lock() {
                                        *current_idx = idx;
                                        menu_btn_clone.set_label(&loc.name);
                                        println!("✓ Auto-detected location: {}", loc.name);
                                    }
                                    break;
                                }
                            }
                        }
                    }
                    
                    menu_built = true;
                    return glib::ControlFlow::Break;
                }
            }
            
            glib::ControlFlow::Continue
        });
    }
    
    println!("✓ UI initialization complete!");
}

#[cfg(not(feature = "gtk"))]
fn run_gtk_ui() {
    eprintln!("❌ GTK UI not compiled in");
    std::process::exit(1);
}
