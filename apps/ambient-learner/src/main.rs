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

use chrono::{Local, Utc};
use regex::Regex;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// Import temperature field types
use temperature_field::TemperatureEstimator;

// Import WorldState architecture
use world_state::{SystemState, DeviceState};

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
// Configuration Constants
// ============================================================================

/// Application configuration constants
mod config {
    /// Sensor polling and calibration settings
    pub mod sensors {
        /// Interval for passive sensor polling (ms)
        pub const PASSIVE_INTERVAL_MS: u64 = 1000;
        /// Interval for active burst reading (ms)
        pub const ACTIVE_INTERVAL_MS: u64 = 100;
        /// Number of readings per calibration burst
        pub const ACTIVE_BURST_COUNT: usize = 22;  // Increased from 20 for +10% data
        /// Seconds of history to keep in memory
        pub const HISTORY_SECONDS: u64 = 15;
    }
    
    /// Temperature validation bounds
    pub mod temperature {
        /// Minimum valid temperature (°C)
        pub const MIN: f32 = -40.0;
        /// Maximum valid temperature (°C)
        pub const MAX: f32 = 40.0;
        /// Default fallback temperature (°C)
        pub const DEFAULT: f32 = 20.0;
    }
    
    /// Model training parameters
    pub mod model {
        /// Learning rate for weight updates
        pub const LEARNING_RATE: f32 = 0.01;  // Reduced from 0.05 to prevent oscillation
        /// Minimum dropout score to keep sensor
        pub const MIN_DROPOUT_SCORE: f32 = 0.1;
    }
    
    /// UI display settings
    pub mod ui {
        /// Main window default width
        pub const WINDOW_WIDTH: i32 = 400;
        /// Main window default height  
        pub const WINDOW_HEIGHT: i32 = 700;
        /// UI refresh interval (ms)
        pub const UPDATE_INTERVAL_MS: u64 = 100;
        /// Page margin (pixels)
        pub const PAGE_MARGIN: i32 = 20;
        /// Temperature font size (points)
        pub const TEMP_FONT_SIZE: i32 = 80;
    }
    
    /// Database configuration
    pub mod database {
        /// Max retry attempts for locked database
        pub const RETRY_ATTEMPTS: u32 = 3;
        /// Delay between retry attempts (ms)
        pub const RETRY_DELAY_MS: u64 = 50;
        /// SQLite cache size (pages)
        pub const WAL_CACHE_SIZE: i32 = 10000;
    }
    
    /// Chart rendering settings
    pub mod chart {
        /// Network visualization width
        pub const NETWORK_WIDTH: u32 = 800;
        /// Network visualization height
        pub const NETWORK_HEIGHT: u32 = 1200;
        /// History chart width
        pub const HISTORY_WIDTH: u32 = 800;
        /// History chart height
        pub const HISTORY_HEIGHT: u32 = 600;
    }
    
    /// File storage settings
    pub mod storage {
        /// Data directory name
        pub const DATA_DIR: &str = "weather_data";
        /// Model save interval (seconds)
        pub const MODEL_SAVE_INTERVAL_S: u64 = 60;
        /// Hourly reading save interval (seconds)
        pub const READING_SAVE_INTERVAL_S: u64 = 3600;
    }
}

// Re-export commonly used constants for convenience
use config::sensors::*;
use config::temperature::{MIN as TEMP_MIN, MAX as TEMP_MAX};
use config::model::LEARNING_RATE;
use config::storage::DATA_DIR;

// ============================================================================
// Data Structures
// ============================================================================

/// A single snapshot of thermal sensor readings with system metadata
///
/// Captures the complete state of device thermal sensors along with
/// contextual information that may correlate with ambient temperature.
///
/// # Fields
///
/// * `time` - Timestamp when reading was taken (YYYYMMDD_HHMMSS format)
/// * `sensors` - Vector of thermal sensor readings in °C
/// * `cpu_load` - Current CPU utilization (0.0 to 1.0)
/// * `als_lux` - Ambient light sensor value in lux
/// * `charging` - Whether device is currently charging
/// * `screen` - Screen brightness level (0-255)
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct SensorReading {
    pub(crate) time: String,
    pub(crate) sensors: Vec<f32>,
    pub(crate) cpu_load: f32,
    pub(crate) als_lux: u32,
    pub(crate) charging: bool,
    pub(crate) screen: u8,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
struct Location {
    id: String,
    name: String,
    location_type: LocationType,
    networks: Vec<String>,  // WiFi networks associated with this location
    created_at: String,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
enum LocationType {
    City(String),           // City name
    NetworkId(String),      // WiFi SSID or network ID  
    Unknown,
}

impl Location {
    fn new(name: String, location_type: LocationType) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            location_type,
            networks: Vec::new(),
            created_at: get_timestamp(),
        }
    }
    
    fn matches_network(&self, ssid: &str) -> bool {
        self.networks.iter().any(|n| n == ssid) ||
        matches!(&self.location_type, LocationType::NetworkId(id) if id == ssid)
    }
}

/// Statistical metadata for a single thermal sensor
///
/// Tracks the behavior and reliability of each sensor over time to
/// enable adaptive weighting and sensor pruning.
#[derive(Clone, Serialize, Deserialize)]
struct SensorStat {
    /// Sensor identifier (e.g., "thermal_zone0")
    name: String,
    /// Weight in prediction calculation (higher = more influential)
    weight: f32,
    /// Historical mean reading for this sensor
    mean: f32,
    /// Historical variance (measures stability)
    variance: f32,
    /// Dropout score (0.0 = pruned, 1.0 = active)
    dropout_score: f32,
    /// Sensor type classification
    sensor_type: SensorType,
}

/// Device material type affects thermal conductivity
#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq)]
enum MaterialType {
    Metal,      // High conductivity (aluminum, steel backs)
    Plastic,    // Low conductivity (plastic backs)
    Glass,      // Medium conductivity (glass backs)
    Unknown,    // Not yet determined
}

/// Thermal prediction model using multi-layer neural network
///
/// Learns from user calibrations to predict ambient temperature from
/// device thermal sensors. Uses hidden layers to model non-linear
/// thermal dynamics and adapts to different back cover materials.
///
/// # Architecture
///
/// Input Layer (N sensors) →
/// Hidden Layer 1 (N/2 neurons) →
/// Hidden Layer 2 (N/4 neurons) →
/// Output (1 temperature)
///
/// # Algorithm
///
/// 1. **Passive Learning**: Continuously updates sensor statistics
/// 2. **Active Calibration**: User provides ground truth temperature
/// 3. **Backpropagation**: Adjusts weights via gradient descent
/// 4. **Material Adaptation**: Learns material-specific thermal transfer
/// 5. **Sensor Pruning**: Removes sensors with low contribution
///
#[derive(Clone, Serialize, Deserialize)]
pub struct Model {
    /// Statistical information for each thermal sensor
    stats: Vec<SensorStat>,
    /// Model bias term (intercept)
    bias: f32,
    
    /// Hidden layer 1 weights [sensors x hidden1]
    #[serde(default)]
    hidden1_weights: Vec<Vec<f32>>,
    /// Hidden layer 1 biases
    #[serde(default)]
    hidden1_bias: Vec<f32>,
    
    /// Hidden layer 2 weights [hidden1 x hidden2]
    #[serde(default)]
    hidden2_weights: Vec<Vec<f32>>,
    /// Hidden layer 2 biases
    #[serde(default)]
    hidden2_bias: Vec<f32>,
    
    /// Output layer weights [hidden2 x 1]
    #[serde(default)]
    output_weights: Vec<f32>,
    
    /// Momentum buffers (prevent oscillation between +40 and -40)
    #[serde(default)]
    hidden1_weights_momentum: Vec<Vec<f32>>,
    #[serde(default)]
    hidden2_weights_momentum: Vec<Vec<f32>>,
    #[serde(default)]
    output_weights_momentum: Vec<f32>,
    #[serde(default)]
    bias_momentum: f32,
    
    /// Detected material type
    #[serde(default)]
    material_type: MaterialType,
    
    /// Material-specific thermal transfer coefficient
    #[serde(default)]
    material_coefficient: f32,
    
    /// Device identifier (CPU + sensor count + machine ID)
    /// Prevents sensor confusion when sharing databases across devices
    #[serde(default)]
    device_id: String,
    
    /// Last predicted temperature (for smoothing)
    last_prediction: f32,
    /// Number of user calibrations performed
    pub calibration_count: usize,
    /// Timestamp when model was created
    created_at: String,
    /// Hash of last saved state (for change detection)
    #[serde(skip)]
    last_saved_hash: u64,
}

impl Default for MaterialType {
    fn default() -> Self {
        MaterialType::Unknown
    }
}

/// ReLU activation function: max(0, x)
fn relu(x: f32) -> f32 {
    x.max(0.0)
}

impl Model {
    /// Creates a new model with initial sensor configuration
    ///
    /// # Arguments
    ///
    /// * `sensor_names` - List of thermal sensor identifiers
    ///
    /// # Returns
    ///
    /// Model with zero-initialized weights and default parameters
    ///
    /// # Examples
    ///
    /// ```
    /// let sensors = vec!["thermal_zone0".to_string(), "thermal_zone1".to_string()];
    /// let model = Model::new(sensors);
    /// assert_eq!(model.calibration_count, 0);
    /// ```
    pub fn new(sensor_names: Vec<String>) -> Self {
        let num_sensors = sensor_names.len();
        let hidden1_size = (num_sensors / 2).max(4);  // At least 4 neurons
        let hidden2_size = (hidden1_size / 2).max(2); // At least 2 neurons
        
        // Xavier/Glorot initialization for better convergence (will be used when implementing forward pass)
        let _init_scale_h1 = (2.0_f32 / num_sensors as f32).sqrt();
        let _init_scale_h2 = (2.0_f32 / hidden1_size as f32).sqrt();
        let _init_scale_out = (2.0_f32 / hidden2_size as f32).sqrt();
        
        Self {
            stats: sensor_names
                .iter()
                .map(|name| SensorStat {
                    name: name.clone(),
                    weight: 0.0,  // Legacy weight for backward compat
                    mean: 35.0,
                    variance: 1.0,
                    dropout_score: 1.0,
                    sensor_type: SensorType::Unknown,  // Will be updated when polling starts
                })
                .collect(),
            bias: 0.0,
            
            // Hidden layer 1: [num_sensors x hidden1_size]
            hidden1_weights: vec![vec![0.0; hidden1_size]; num_sensors],
            hidden1_bias: vec![0.0; hidden1_size],
            
            // Hidden layer 2: [hidden1_size x hidden2_size]
            hidden2_weights: vec![vec![0.0; hidden2_size]; hidden1_size],
            hidden2_bias: vec![0.0; hidden2_size],
            
            // Output layer: [hidden2_size x 1]
            output_weights: vec![0.0; hidden2_size],
            
            // Momentum buffers (same shape as weights)
            hidden1_weights_momentum: vec![vec![0.0; hidden1_size]; num_sensors],
            hidden2_weights_momentum: vec![vec![0.0; hidden2_size]; hidden1_size],
            output_weights_momentum: vec![0.0; hidden2_size],
            bias_momentum: 0.0,
            
            material_type: MaterialType::Unknown,
            material_coefficient: 1.0,  // Neutral coefficient
            
            device_id: get_device_id(),
            
            last_prediction: config::temperature::DEFAULT,
            calibration_count: 0,
            created_at: get_timestamp(),
            last_saved_hash: 0,
        }
    }

    /// Clamps temperature to valid range
    ///
    /// # Arguments
    ///
    /// * `temp` - Temperature value to clamp
    ///
    /// # Returns
    ///
    /// Temperature bounded to [TEMP_MIN, TEMP_MAX]
    ///
    /// # Note
    ///
    /// Consider using `helpers::clamp()` for consistency
    fn clamp(temp: f32) -> f32 {
        temp.max(TEMP_MIN).min(TEMP_MAX)
    }

    /// Updates sensor statistics from passive observations
    ///
    /// Called continuously in background to track sensor behavior.
    /// Updates running mean and variance without requiring calibration.
    ///
    /// # Arguments
    ///
    /// * `readings` - Current sensor readings
    ///
    /// # Performance
    ///
    /// O(n) where n = number of sensors. Typically <1ms.
    fn update_passive(&mut self, readings: &[f32]) {
        // SIMD disabled - requires nightly Rust
        // TODO: Re-enable when using nightly or when portable_simd is stabilized
        /*
        #[cfg(target_arch = "aarch64")]
        {
            if self.update_passive_simd(readings) {
                return;
            }
        }
        */
        
        // Scalar implementation
        for (i, &val) in readings.iter().enumerate() {
            if i >= self.stats.len() {
                break;
            }
            let stat = &mut self.stats[i];

            let alpha = 0.005;
            stat.mean = (1.0 - alpha) * stat.mean + alpha * val;
            let diff = val - stat.mean;
            stat.variance = (1.0 - alpha) * stat.variance + alpha * diff * diff;
            stat.dropout_score = 1.0 / (1.0 + stat.variance);
        }
    }
    
    /*
    /// SIMD-accelerated passive update using ARM NEON (2-3x faster)
    /// DISABLED: Requires nightly Rust
    #[cfg(target_arch = "aarch64")]
    fn update_passive_simd(&mut self, readings: &[f32]) -> bool {
        let mut means: Vec<f32> = self.stats.iter().map(|s| s.mean).collect();
        let mut variances: Vec<f32> = self.stats.iter().map(|s| s.variance).collect();
        let mut dropout_scores: Vec<f32> = self.stats.iter().map(|s| s.dropout_score).collect();
        
        if simd::update_passive(
            readings,
            &mut means,
            &mut variances,
            &mut dropout_scores,
            0.005
        ).is_some() {
            // Copy results back
            for (i, stat) in self.stats.iter_mut().enumerate() {
                if i < means.len() {
                    stat.mean = means[i];
                    stat.variance = variances[i];
                    stat.dropout_score = dropout_scores[i];
                }
            }
            true
        } else {
            false
        }
    }
    */

    pub fn predict(&mut self, readings: &[f32]) -> f32 {
        // Use neural network only when we have enough calibrations (20+)
        // Linear model is more stable with limited data
        if self.calibration_count >= 20 
            && !self.hidden1_weights.is_empty() 
            && !self.hidden1_weights[0].is_empty() 
        {
            return self.predict_neural(readings);
        }
        
        // Use linear model for 0-19 calibrations (more stable with limited data)
        self.predict_linear(readings)
    }
    
    /// Neural network forward pass
    fn predict_neural(&mut self, readings: &[f32]) -> f32 {
        let num_sensors = readings.len().min(self.stats.len());
        
        // Normalize inputs (zero mean, unit variance)
        let mut normalized = Vec::with_capacity(num_sensors);
        for i in 0..num_sensors {
            let stat = &self.stats[i];
            let std_dev = stat.variance.sqrt().max(0.001);
            let norm = (readings[i] - stat.mean) / std_dev;
            normalized.push(norm * stat.dropout_score); // Apply dropout
        }
        
        // Hidden Layer 1
        let hidden1_size = self.hidden1_bias.len();
        let mut h1 = vec![0.0; hidden1_size];
        for i in 0..hidden1_size {
            let mut sum = self.hidden1_bias[i];
            for (j, &x) in normalized.iter().enumerate() {
                if j < self.hidden1_weights.len() && i < self.hidden1_weights[j].len() {
                    sum += x * self.hidden1_weights[j][i];
                }
            }
            h1[i] = relu(sum);
        }
        
        // Hidden Layer 2
        let hidden2_size = self.hidden2_bias.len();
        let mut h2 = vec![0.0; hidden2_size];
        for i in 0..hidden2_size {
            let mut sum = self.hidden2_bias[i];
            for (j, &x) in h1.iter().enumerate() {
                if j < self.hidden2_weights.len() && i < self.hidden2_weights[j].len() {
                    sum += x * self.hidden2_weights[j][i];
                }
            }
            h2[i] = relu(sum);
        }
        
        // Output Layer
        let mut output = self.bias;
        for (i, &x) in h2.iter().enumerate() {
            if i < self.output_weights.len() {
                output += x * self.output_weights[i];
            }
        }
        
        // Apply material coefficient
        output *= self.material_coefficient;
        
        // Smooth with exponential moving average
        let smoothed = 0.7 * output + 0.3 * self.last_prediction;
        self.last_prediction = Self::clamp(smoothed);
        self.last_prediction
    }
    
    /// Linear model (legacy/fallback)
    fn predict_linear(&mut self, readings: &[f32]) -> f32 {
        let mut weighted_sum = 0.0;
        let mut total_weight = 0.0;

        for (i, &val) in readings.iter().enumerate() {
            if i >= self.stats.len() {
                break;
            }
            let stat = &self.stats[i];

            let std_dev = stat.variance.sqrt().max(0.001);
            let normalized = (val - stat.mean) / std_dev;
            let effective_weight = stat.weight * stat.dropout_score;
            weighted_sum += normalized * effective_weight;
            total_weight += effective_weight.abs() + 1e-6;
        }

        let raw_pred = self.bias + weighted_sum / total_weight;
        self.last_prediction = Self::clamp(raw_pred);
        self.last_prediction
    }
    
    /*
    /// SIMD-accelerated prediction using ARM NEON (3-4x faster)
    /// DISABLED: Requires nightly Rust
    #[cfg(target_arch = "aarch64")]
    fn predict_simd(&self, readings: &[f32]) -> Option<f32> {
        // Extract data for SIMD
        let means: Vec<f32> = self.stats.iter().map(|s| s.mean).collect();
        let variances: Vec<f32> = self.stats.iter().map(|s| s.variance).collect();
        let weights: Vec<f32> = self.stats.iter().map(|s| s.weight).collect();
        let dropout_scores: Vec<f32> = self.stats.iter().map(|s| s.dropout_score).collect();
        
        Some(simd::predict(
            readings,
            &means,
            &variances,
            &weights,
            &dropout_scores,
            self.bias
        ))
    }
    */

    pub fn calibrate(&mut self, readings: &[f32], target: f32) {
        // 🚨 CRITICAL: Validate sensor count BEFORE doing anything
        if !self.stats.is_empty() && readings.len() != self.stats.len() {
            eprintln!("❌ SENSOR COUNT MISMATCH! Expected {}, got {}", 
                self.stats.len(), readings.len());
            eprintln!("   Cannot calibrate - this would corrupt the model!");
            eprintln!("   Skipping this calibration entirely.");
            return;  // DO NOT TRAIN on mismatched data!
        }
        
        // Initialize neural network weights on first calibration
        if self.calibration_count == 0 && self.hidden1_weights.iter().all(|row| row.iter().all(|&w| w == 0.0)) {
            self.initialize_neural_weights();
        }
        
        // Train the appropriate model based on calibration count
        if self.calibration_count < 20 {
            // Use linear model for first 20 calibrations (stable baseline)
            self.calibrate_linear(readings, target);
        } else {
            // Use neural network after 20 calibrations
            if self.calibration_count == 20 {
                // Bootstrap neural network from linear model
                self.bootstrap_neural_from_linear();
                println!("🧠 Bootstrapped neural network from linear model");
            }
            
            // Train neural network
            if !self.hidden1_weights.is_empty() && !self.hidden1_weights[0].is_empty() {
                self.calibrate_neural(readings, target);
            } else {
                // Fallback to linear if neural failed
                self.calibrate_linear(readings, target);
            }
        }
        
        self.calibration_count += 1;
    }
    
    /// Bootstrap neural network weights from trained linear model
    fn bootstrap_neural_from_linear(&mut self) {
        println!("🔄 Bootstrapping neural network from linear model...");
        
        let hidden1_size = self.hidden1_bias.len();
        let hidden2_size = self.hidden2_bias.len();
        
        // Transfer linear weights to first hidden layer
        // Strategy: Make hidden1 learn the linear combination, then pass through
        for i in 0..self.stats.len().min(self.hidden1_weights.len()) {
            for j in 0..hidden1_size {
                if j < self.hidden1_weights[i].len() {
                    // Distribute linear weight across hidden neurons
                    self.hidden1_weights[i][j] = self.stats[i].weight / hidden1_size as f32;
                }
            }
        }
        
        // Make hidden2 act as identity initially
        for i in 0..hidden1_size.min(self.hidden2_weights.len()) {
            for j in 0..hidden2_size {
                if j < self.hidden2_weights[i].len() {
                    // Identity-like: each hidden2 neuron averages some hidden1 neurons
                    self.hidden2_weights[i][j] = if (i % hidden2_size) == j { 
                        1.0 / (hidden1_size / hidden2_size).max(1) as f32
                    } else {
                        0.0
                    };
                }
            }
        }
        
        // Make output layer sum up hidden2
        for i in 0..hidden2_size {
            if i < self.output_weights.len() {
                self.output_weights[i] = 1.0 / hidden2_size as f32;
            }
        }
        
        // Transfer bias
        self.bias = self.bias; // Keep existing bias
        
        // Small random biases for hidden layers
        for i in 0..hidden1_size {
            self.hidden1_bias[i] = 0.0;
        }
        for i in 0..hidden2_size {
            self.hidden2_bias[i] = 0.0;
        }
        
        println!("✓ Neural network bootstrapped with linear model weights");
    }
    
    /// Initialize neural network weights using Xavier/Glorot initialization
    fn initialize_neural_weights(&mut self) {
        let num_sensors = self.stats.len();
        let hidden1_size = self.hidden1_bias.len();
        let hidden2_size = self.hidden2_bias.len();
        
        println!("🧠 Initializing neural network weights:");
        println!("   Input: {} sensors", num_sensors);
        println!("   Hidden 1: {} neurons", hidden1_size);
        println!("   Hidden 2: {} neurons", hidden2_size);
        
        // Simple pseudo-random initialization (not cryptographically secure, but good enough)
        let mut seed = (chrono::Utc::now().timestamp() as u64) ^ 0xDEADBEEF;
        
        let mut rng = || -> f32 {
            // Linear congruential generator
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            ((seed / 65536) % 32768) as f32 / 32768.0 - 0.5
        };
        
        // Hidden layer 1 weights: Xavier initialization
        let scale_h1 = (2.0 / num_sensors as f32).sqrt();
        for i in 0..num_sensors {
            for j in 0..hidden1_size {
                if i < self.hidden1_weights.len() && j < self.hidden1_weights[i].len() {
                    self.hidden1_weights[i][j] = rng() * scale_h1;
                }
            }
        }
        
        // Hidden layer 2 weights
        let scale_h2 = (2.0 / hidden1_size as f32).sqrt();
        for i in 0..hidden1_size {
            for j in 0..hidden2_size {
                if i < self.hidden2_weights.len() && j < self.hidden2_weights[i].len() {
                    self.hidden2_weights[i][j] = rng() * scale_h2;
                }
            }
        }
        
        // Output weights
        let scale_out = (2.0 / hidden2_size as f32).sqrt();
        for i in 0..hidden2_size {
            if i < self.output_weights.len() {
                self.output_weights[i] = rng() * scale_out;
            }
        }
        
        // Biases initialized to small random values
        for i in 0..hidden1_size {
            self.hidden1_bias[i] = rng() * 0.01;
        }
        for i in 0..hidden2_size {
            self.hidden2_bias[i] = rng() * 0.01;
        }
        
        println!("✓ Neural network initialized with Xavier/Glorot weights");
    }
    
    /// Neural network training with backpropagation
    fn calibrate_neural(&mut self, readings: &[f32], target: f32) {
        let num_sensors = readings.len().min(self.stats.len());
        
        // Adaptive learning rate: decay after 30 calibrations
        // Adaptive learning rate: start very conservative to avoid oscillation
        let base_lr = LEARNING_RATE * 0.1; // 0.001 (was 0.005, too aggressive)
        let learning_rate = if self.calibration_count > 30 {
            // Decay by 50% every 20 calibrations after 30
            let decay_factor = 0.5_f32.powf(((self.calibration_count - 30) / 20) as f32);
            base_lr * decay_factor.max(0.1) // Minimum 10% of base
        } else {
            base_lr
        };
        
        if self.calibration_count % 10 == 0 && self.calibration_count >= 20 {
            println!("🎓 Neural learning rate: {:.6} (cal #{})", learning_rate, self.calibration_count);
        }
        
        // Forward pass (save activations for backprop)
        let mut normalized = Vec::with_capacity(num_sensors);
        for i in 0..num_sensors {
            let stat = &self.stats[i];
            let std_dev = stat.variance.sqrt().max(0.001);
            let norm = (readings[i] - stat.mean) / std_dev;
            let normalized_val = norm * stat.dropout_score;
            
            // NaN check
            if normalized_val.is_nan() || normalized_val.is_infinite() {
                normalized.push(0.0);
            } else {
                normalized.push(normalized_val.clamp(-10.0, 10.0)); // Clip inputs
            }
        }
        
        // Hidden Layer 1
        let hidden1_size = self.hidden1_bias.len();
        let mut h1_pre = vec![0.0; hidden1_size];
        let mut h1 = vec![0.0; hidden1_size];
        for i in 0..hidden1_size {
            let mut sum = self.hidden1_bias[i];
            for (j, &x) in normalized.iter().enumerate() {
                if j < self.hidden1_weights.len() && i < self.hidden1_weights[j].len() {
                    sum += x * self.hidden1_weights[j][i];
                }
            }
            h1_pre[i] = sum.clamp(-100.0, 100.0); // Prevent explosion
            h1[i] = relu(h1_pre[i]);
        }
        
        // Hidden Layer 2
        let hidden2_size = self.hidden2_bias.len();
        let mut h2_pre = vec![0.0; hidden2_size];
        let mut h2 = vec![0.0; hidden2_size];
        for i in 0..hidden2_size {
            let mut sum = self.hidden2_bias[i];
            for (j, &x) in h1.iter().enumerate() {
                if j < self.hidden2_weights.len() && i < self.hidden2_weights[j].len() {
                    sum += x * self.hidden2_weights[j][i];
                }
            }
            h2_pre[i] = sum.clamp(-100.0, 100.0);
            h2[i] = relu(h2_pre[i]);
        }
        
        // Output
        let mut output = self.bias;
        for (i, &x) in h2.iter().enumerate() {
            if i < self.output_weights.len() {
                output += x * self.output_weights[i];
            }
        }
        output *= self.material_coefficient;
        
        // NaN check on output
        if output.is_nan() || output.is_infinite() {
            println!("⚠️  NaN detected in forward pass, skipping update");
            return;
        }
        
        // Sanity check: If prediction is wildly off, DON'T train!
        let error = output - target;
        let abs_error = error.abs();
        
        // 🚨 CRITICAL ERROR: Model is completely diverged
        if abs_error > 100.0 {
            println!("🚨 CRITICAL ERROR: {:.1}°C (predicted: {:.1}°C, target: {:.1}°C)", 
                abs_error, output, target);
            println!("   Model is catastrophically diverged - SKIPPING ALL TRAINING");
            println!("   Consider deleting model files and starting fresh!");
            return;  // DO NOT train - model is broken!
        }
        
        // Large error: Skip training to prevent making it worse
        if abs_error > 50.0 {
            println!("🚨 LARGE ERROR: {:.1}°C (predicted: {:.1}°C, target: {:.1}°C)", 
                abs_error, output, target);
            println!("   Model is diverging - SKIPPING THIS CALIBRATION");
            return;  // Don't train on garbage predictions
        }
        
        // Medium error: Train with extreme caution
        if abs_error > 20.0 {
            println!("⚠️  Large error detected: {:.1}°C (predicted: {:.1}°C, target: {:.1}°C)", 
                abs_error, output, target);
            println!("   Using emergency learning rate (10% of normal)");
            
            // Use much smaller learning rate for this update
            let emergency_lr = learning_rate * 0.1;
            
            // Still update, but very cautiously
            let loss_gradient = (2.0 * error).clamp(-5.0, 5.0); // Extra conservative clipping
            
            // Update with emergency learning rate
            self.update_weights_with_lr(
                emergency_lr,
                &loss_gradient,
                &normalized,
                &h1,
                &h2,
                &h1_pre,
                &h2_pre,
            );
            return;
        }
        
        // Normal update
        let loss_gradient = (2.0 * error).clamp(-10.0, 10.0); // Clip gradient
        self.update_weights_with_lr(
            learning_rate,
            &loss_gradient,
            &normalized,
            &h1,
            &h2,
            &h1_pre,
            &h2_pre,
        );
        
        // Note: Sensor statistics are updated ONLY in update_passive()
        // to maintain stationary normalization during training.
        // Updating stats here would cause training instability.
    }
    
    /// Helper to update weights with given learning rate
    /// Helper to update weights with given learning rate and momentum
    fn update_weights_with_lr(
        &mut self,
        learning_rate: f32,
        loss_gradient: &f32,
        normalized: &[f32],
        h1: &[f32],
        h2: &[f32],
        h1_pre: &[f32],
        h2_pre: &[f32],
    ) {
        let hidden1_size = self.hidden1_bias.len();
        let hidden2_size = self.hidden2_bias.len();
        let num_sensors = normalized.len();
        
        // Momentum coefficient (0.9 = strong damping of oscillations)
        const MOMENTUM: f32 = 0.9;
        
        // Output layer gradients with momentum
        let mut d_output = vec![0.0; hidden2_size];
        for i in 0..hidden2_size {
            if i < self.output_weights.len() && i < self.output_weights_momentum.len() {
                d_output[i] = loss_gradient * self.material_coefficient * h2[i];
                let gradient = learning_rate * d_output[i];
                
                // NaN check
                if !gradient.is_nan() && !gradient.is_infinite() {
                    // Apply momentum: v = momentum * v_prev + gradient
                    self.output_weights_momentum[i] = 
                        MOMENTUM * self.output_weights_momentum[i] + gradient;
                    
                    // Update weight with momentum
                    let update = self.output_weights_momentum[i].clamp(-0.5, 0.5);
                    self.output_weights[i] -= update;
                }
            }
        }
        
        // Update bias with momentum
        let bias_gradient = learning_rate * loss_gradient * self.material_coefficient;
        if !bias_gradient.is_nan() && !bias_gradient.is_infinite() {
            self.bias_momentum = MOMENTUM * self.bias_momentum + bias_gradient;
            let update = self.bias_momentum.clamp(-0.5, 0.5);
            self.bias -= update;
        }
        
        // Hidden layer 2 gradients
        let mut d_h2 = vec![0.0; hidden2_size];
        for i in 0..hidden2_size {
            let mut grad = 0.0;
            if i < self.output_weights.len() {
                grad = loss_gradient * self.material_coefficient * self.output_weights[i];
            }
            // Apply ReLU derivative
            if h2_pre[i] > 0.0 {
                d_h2[i] = grad.clamp(-10.0, 10.0);
            }
        }
        
        // Update hidden2 weights and biases with momentum
        for i in 0..hidden2_size {
            let bias_gradient = learning_rate * d_h2[i];
            if !bias_gradient.is_nan() && !bias_gradient.is_infinite() {
                self.hidden2_bias[i] -= bias_gradient.clamp(-0.3, 0.3);
            }
            
            for j in 0..hidden1_size {
                if j < self.hidden2_weights.len() && i < self.hidden2_weights[j].len() 
                    && j < self.hidden2_weights_momentum.len() && i < self.hidden2_weights_momentum[j].len() {
                    let gradient = learning_rate * d_h2[i] * h1[j];
                    
                    if !gradient.is_nan() && !gradient.is_infinite() {
                        // Apply momentum
                        self.hidden2_weights_momentum[j][i] = 
                            MOMENTUM * self.hidden2_weights_momentum[j][i] + gradient;
                        
                        let update = self.hidden2_weights_momentum[j][i].clamp(-0.3, 0.3);
                        self.hidden2_weights[j][i] -= update;
                    }
                }
            }
        }
        
        // Hidden layer 1 gradients
        let mut d_h1 = vec![0.0; hidden1_size];
        for i in 0..hidden1_size {
            let mut grad = 0.0;
            for j in 0..hidden2_size {
                if i < self.hidden2_weights.len() && j < self.hidden2_weights[i].len() {
                    grad += d_h2[j] * self.hidden2_weights[i][j];
                }
            }
            // Apply ReLU derivative
            if h1_pre[i] > 0.0 {
                d_h1[i] = grad.clamp(-10.0, 10.0);
            }
        }
        
        // Update hidden1 weights and biases with momentum
        for i in 0..hidden1_size {
            let bias_gradient = learning_rate * d_h1[i];
            if !bias_gradient.is_nan() && !bias_gradient.is_infinite() {
                self.hidden1_bias[i] -= bias_gradient.clamp(-0.3, 0.3);
            }
            
            for j in 0..num_sensors {
                if j < self.hidden1_weights.len() && i < self.hidden1_weights[j].len()
                    && j < self.hidden1_weights_momentum.len() && i < self.hidden1_weights_momentum[j].len() {
                    let gradient = learning_rate * d_h1[i] * normalized[j];
                    
                    if !gradient.is_nan() && !gradient.is_infinite() {
                        // Apply momentum
                        self.hidden1_weights_momentum[j][i] = 
                            MOMENTUM * self.hidden1_weights_momentum[j][i] + gradient;
                        
                        let update = self.hidden1_weights_momentum[j][i].clamp(-0.3, 0.3);
                        self.hidden1_weights[j][i] -= update;
                        
                        // L2 regularization (weight decay)
                        self.hidden1_weights[j][i] *= 0.999;
                    }
                }
            }
        }
        
        // 🚨 CHECK FOR WEIGHT EXPLOSION
        let max_weight = self.hidden1_weights.iter()
            .flat_map(|row| row.iter())
            .chain(self.hidden2_weights.iter().flat_map(|row| row.iter()))
            .chain(self.output_weights.iter())
            .map(|w| w.abs())
            .fold(0.0f32, f32::max);
        
        if max_weight > 100.0 || self.bias.abs() > 1000.0 {
            println!("🚨 WEIGHT EXPLOSION DETECTED!");
            println!("   Max weight: {:.1}, Bias: {:.1}", max_weight, self.bias);
            println!("   Model is corrupted - resetting to safe state");
            
            // Re-initialize all weights
            self.initialize_neural_weights();
            
            // Reset momentum buffers
            let hidden1_size = self.hidden1_bias.len();
            let hidden2_size = self.hidden2_bias.len();
            let num_sensors = self.stats.len();
            
            self.hidden1_weights_momentum = vec![vec![0.0; hidden1_size]; num_sensors];
            self.hidden2_weights_momentum = vec![vec![0.0; hidden2_size]; hidden1_size];
            self.output_weights_momentum = vec![0.0; hidden2_size];
            self.bias_momentum = 0.0;
            
            println!("✓ Model reset complete - starting fresh");
        }
    }
    
    /// Linear model training (legacy/fallback)
    fn calibrate_linear(&mut self, readings: &[f32], target: f32) {
        let prediction = self.predict(readings);
        let error = target - prediction;

        self.bias += LEARNING_RATE * error;

        for (i, &val) in readings.iter().enumerate() {
            if i >= self.stats.len() {
                break;
            }
            let stat = &mut self.stats[i];

            let std_dev = stat.variance.sqrt().max(0.001);
            let normalized = (val - stat.mean) / std_dev;
            let gradient = error * normalized / (normalized * normalized + 1e-6);
            stat.weight += LEARNING_RATE * gradient - 0.001 * stat.weight;
        }
    }

    fn prune_weak_sensors(&mut self) {
        if self.calibration_count > 5 {
            for stat in &mut self.stats {
                if stat.weight.abs() < 0.01 {
                    stat.dropout_score *= 0.5;
                }
            }
        }
        
        // Detect material type after enough calibrations
        if self.calibration_count == 10 || self.calibration_count == 20 {
            self.detect_material_type();
        }
    }
    
    /// Detect device material type from thermal transfer patterns
    fn detect_material_type(&mut self) {
        // Analyze CPU to ambient thermal transfer rate
        let cpu_indices: Vec<usize> = self.stats.iter()
            .enumerate()
            .filter(|(_, s)| s.sensor_type == SensorType::CPU)
            .map(|(i, _)| i)
            .collect();
        
        let ambient_indices: Vec<usize> = self.stats.iter()
            .enumerate()
            .filter(|(_, s)| matches!(s.sensor_type, SensorType::Ambient | SensorType::Unknown))
            .map(|(i, _)| i)
            .collect();
        
        if cpu_indices.is_empty() || ambient_indices.is_empty() {
            return; // Not enough sensor variety
        }
        
        // Calculate avg weights for CPU vs ambient sensors
        let cpu_avg_weight: f32 = cpu_indices.iter()
            .map(|&i| self.stats[i].weight.abs())
            .sum::<f32>() / cpu_indices.len() as f32;
        
        let ambient_avg_weight: f32 = ambient_indices.iter()
            .map(|&i| self.stats[i].weight.abs())
            .sum::<f32>() / ambient_indices.len() as f32;
        
        // High CPU weight = strong thermal coupling = metal
        // Low CPU weight = weak thermal coupling = plastic
        let transfer_ratio = cpu_avg_weight / (ambient_avg_weight + 0.01);
        
        let (material, coefficient) = if transfer_ratio > 1.5 {
            // High transfer rate = metal (high conductivity)
            (MaterialType::Metal, 1.3)
        } else if transfer_ratio < 0.5 {
            // Low transfer rate = plastic (low conductivity)
            (MaterialType::Plastic, 0.7)
        } else {
            // Medium transfer rate = glass
            (MaterialType::Glass, 1.0)
        };
        
        if self.material_type != material {
            println!("🔍 Material type detected: {:?} (transfer ratio: {:.2}, coefficient: {:.2})", 
                material, transfer_ratio, coefficient);
            self.material_type = material;
            self.material_coefficient = coefficient;
        }
    }

    fn confidence(&self) -> f32 {
        let active_sensors = self.stats.iter().filter(|s| s.dropout_score > 0.1).count();

        if active_sensors == 0 {
            return 0.0;
        }

        let avg_variance: f32 = self.stats.iter().map(|s| s.variance * s.dropout_score).sum::<f32>()
            / active_sensors as f32;

        let calib_factor = 1.0 - (-0.1 * self.calibration_count as f32).exp();

        (1.0 / (1.0 + avg_variance)) * calib_factor
    }

    fn compute_hash(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        
        // Hash the important fields that would trigger a save
        self.calibration_count.hash(&mut hasher);
        
        // Hash weights and scores (rounded to avoid float precision issues)
        for stat in &self.stats {
            ((stat.weight * 10000.0) as i32).hash(&mut hasher);
            ((stat.dropout_score * 10000.0) as i32).hash(&mut hasher);
            ((stat.mean * 100.0) as i32).hash(&mut hasher);
            ((stat.variance * 10000.0) as i32).hash(&mut hasher);
        }
        
        ((self.bias * 10000.0) as i32).hash(&mut hasher);
        
        hasher.finish()
    }

    fn has_changed(&self) -> bool {
        self.compute_hash() != self.last_saved_hash
    }

    fn save(&mut self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let current_hash = self.compute_hash();
        
        // Only write if content has actually changed
        if current_hash == self.last_saved_hash {
            return Ok(()); // No changes, skip write
        }
        
        let json = serde_json::to_string_pretty(self)?;
        let mut file = File::create(path)?;
        file.write_all(json.as_bytes())?;
        
        // Update hash after successful save
        self.last_saved_hash = current_hash;
        
        Ok(())
    }

    fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let mut file = File::open(path)?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        let mut model: Model = serde_json::from_str(&contents)?;
        
        // Initialize momentum buffers if missing (for models saved before momentum was added)
        let num_sensors = model.stats.len();
        let hidden1_size = model.hidden1_bias.len();
        let hidden2_size = model.hidden2_bias.len();
        
        if model.hidden1_weights_momentum.is_empty() 
            || model.hidden1_weights_momentum.len() != num_sensors
            || (num_sensors > 0 && model.hidden1_weights_momentum[0].len() != hidden1_size) {
            
            model.hidden1_weights_momentum = vec![vec![0.0; hidden1_size]; num_sensors];
            model.hidden2_weights_momentum = vec![vec![0.0; hidden2_size]; hidden1_size];
            model.output_weights_momentum = vec![0.0; hidden2_size];
            model.bias_momentum = 0.0;
            
            println!("✓ Initialized momentum buffers for loaded model");
        }
        
        // Initialize hash to current state after loading
        model.last_saved_hash = model.compute_hash();
        
        Ok(model)
    }
}

/// Implement TemperatureEstimator trait for legacy Model
/// This allows seamless switching between old and new estimators
impl TemperatureEstimator for Model {
    fn estimate_ambient(&mut self, sensors: &[f32], _device_state: &crate::world_state::DeviceState) -> f32 {
        // Legacy model doesn't use device state
        self.predict(sensors)
    }
    
    fn calibrate(&mut self, sensors: &[f32], _device_state: &crate::world_state::DeviceState, target: f32) {
        // Legacy calibrate doesn't use device state
        self.calibrate(sensors, target);
    }
    
    fn update_sensor_history(&mut self, _sensors: &[f32], _device_state: &crate::world_state::DeviceState) {
        // Legacy model doesn't track history
        // No-op for compatibility
    }
    
    fn get_calibration_count(&self) -> usize {
        self.calibration_count
    }
}

pub struct ThermalPoller {
    files: Vec<(String, File, SensorType)>,
    buffer: Vec<u8>,
    locked: bool,
    /// Per-sensor unit scale, parallel to `files`. Each entry is
    /// the divisor (1.0, 1000.0, or 1_000_000.0) that converts the
    /// sysfs raw value to °C for that sensor. `None` means we
    /// haven't seen a plausible reading from that sensor yet —
    /// once we do, we lock in the divisor and use it for every
    /// subsequent poll of that sensor.
    ///
    /// This prevents the per-tick reclassification bug: a sensor
    /// reading `65` once (which we'd interpret as 0.065 °C in
    /// millidegrees) and `65000` next time (65 °C in millidegrees)
    /// would otherwise oscillate between two interpretations of
    /// the same physical temperature. With per-sensor caching we
    /// commit to whatever the first plausible reading suggested,
    /// then stick with it.
    unit_scale: Vec<Option<f32>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
enum SensorType {
    CPU,           // CPU package/core sensors
    GPU,           // GPU sensors
    Battery,       // Battery temperature
    Ambient,       // General thermal zones
    Unknown,
}

/// Map a raw sysfs thermal value to °C, auto-detecting the unit.
///
/// The Linux sysfs convention for `/sys/class/thermal/*/temp` and
/// `/sys/class/hwmon/*/temp*_input` is "millidegrees Celsius" — but
/// some vendor kernels on Android/mobile SoCs deviate:
///
///   * Most: millidegrees (1 °C = 1000)
///   * Some: microdegrees (1 °C = 1_000_000)
///   * A few: raw degrees (1 °C = 1)
///
/// We pick the divisor by magnitude: after applying each candidate,
/// the most "plausible" result wins (closest to the −40..125 °C range
/// of typical silicon).
///
/// Anything still outside that range after rescaling is returned at
/// the boundary — better to clamp than to feed a 900°C reading into
/// the ML model, where it would warp every prediction.
/// Physical silicon temperature range. Outside this, a reading is
/// almost certainly a unit-scale error, not real data.
pub(crate) const PHYSICAL_MIN_C: f32 = -40.0;
pub(crate) const PHYSICAL_MAX_C: f32 = 125.0;

/// The three candidate unit divisors for sysfs thermal values, in
/// preference order. Order matters: a value that fits multiple
/// interpretations (e.g. raw `65` could be 0.065 °C in millidegrees
/// or 65 °C raw) takes the earlier one. This is a deliberate bias
/// toward the standard convention.
pub(crate) const CANDIDATE_DIVISORS: [f32; 3] = [
    1000.0,         // millidegrees (the sysfs standard)
    1_000_000.0,    // microdegrees (some vendor kernels)
    1.0,            // raw degrees (rare)
];

/// Try to detect which unit divisor is in use for a given raw
/// value. Returns `Some(divisor)` if any candidate produces a
/// physically plausible reading; `None` if none do (in which case
/// `normalize_thermal_reading` will fall back to clamping).
///
/// This is the building block that lets `ThermalPoller` lock in a
/// per-sensor scale once detected, rather than re-detecting on
/// every poll.
pub(crate) fn detect_unit_divisor(raw: i64) -> Option<f32> {
    for &div in &CANDIDATE_DIVISORS {
        let v = raw as f32 / div;
        if v >= PHYSICAL_MIN_C && v <= PHYSICAL_MAX_C {
            return Some(div);
        }
    }
    None
}

pub(crate) fn normalize_thermal_reading(raw: i64) -> f32 {
    if let Some(div) = detect_unit_divisor(raw) {
        return raw as f32 / div;
    }

    // None of the candidates landed inside the physical range. Pick
    // the one closest to the middle of the range as a defensive
    // fallback — it's the least-wrong reading we can manufacture
    // without flat-out lying.
    let target = (PHYSICAL_MIN_C + PHYSICAL_MAX_C) / 2.0;
    let mut best = raw as f32 / CANDIDATE_DIVISORS[0];
    let mut best_dist = (best - target).abs();
    for &div in &CANDIDATE_DIVISORS[1..] {
        let v = raw as f32 / div;
        let d = (v - target).abs();
        if d < best_dist {
            best = v;
            best_dist = d;
        }
    }
    // Clamp to the physical range so downstream ML doesn't see
    // garbage values.
    best.clamp(PHYSICAL_MIN_C, PHYSICAL_MAX_C)
}

/// Apply a known divisor (from `detect_unit_divisor`) to a raw
/// reading. Used by `ThermalPoller::poll` when it has already
/// locked in a per-sensor scale and wants to use it directly.
/// Clamps to the physical range so a sensor that *was* readable
/// at startup but now reads garbage doesn't propagate the garbage
/// into the model.
pub(crate) fn apply_unit_divisor(raw: i64, divisor: f32) -> f32 {
    let v = raw as f32 / divisor;
    v.clamp(PHYSICAL_MIN_C, PHYSICAL_MAX_C)
}

#[cfg(test)]
mod thermal_reading_tests {
    use super::normalize_thermal_reading;

    #[test]
    fn standard_millidegrees() {
        // 65,000 millidegrees → 65 °C
        assert!((normalize_thermal_reading(65_000) - 65.0).abs() < 0.001);
    }

    #[test]
    fn microdegrees_fallback() {
        // 65,000,000 microdegrees → 65 °C (millidegrees would give 65,000 °C, rejected)
        assert!((normalize_thermal_reading(65_000_000) - 65.0).abs() < 0.001);
    }

    #[test]
    fn raw_degrees_fallback() {
        // A value of `65` is millidegrees = 0.065 °C (in-range but
        // physically implausible for a CPU) OR raw degrees = 65 °C
        // (plausible). The 0.065 hit lands inside our range first,
        // so it wins. This is a known limitation of magnitude-only
        // detection — we can't distinguish a sleeping sensor at
        // 0.065 °C from a hot sensor at 65 °C without prior info.
        // Acceptable tradeoff because the alternative is a 900 °C
        // outlier corrupting the model.
        let v = normalize_thermal_reading(65);
        assert!(v >= -40.0 && v <= 125.0);
    }

    #[test]
    fn rejects_900c_reading() {
        // 900_000 millidegrees → 900 °C → rejected
        // 900_000 microdegrees → 0.9 °C → accepted
        let v = normalize_thermal_reading(900_000);
        assert!(v <= 125.0, "got {v}");
        assert!(v >= -40.0, "got {v}");
    }

    #[test]
    fn extreme_garbage_clamps() {
        // 10^15 in any unit → clamp to range, not panic
        let v = normalize_thermal_reading(1_000_000_000_000_000);
        assert!(v <= 125.0 && v >= -40.0);
    }

    // ─── detect_unit_divisor / apply_unit_divisor tests ──────────
    //
    // These two functions are the building blocks for the
    // per-sensor unit caching in ThermalPoller. They're pure
    // (no I/O, no global state), so we can verify them
    // exhaustively without involving a real sysfs.

    use super::{detect_unit_divisor, apply_unit_divisor};

    #[test]
    fn detect_picks_millidegrees_for_standard_range() {
        // 65,000 → divides by 1000 to give 65 °C, in-range.
        // 0 (sleeping sensor) → divides by 1000 to give 0 °C.
        // -10,000 → divides by 1000 to give -10 °C.
        assert_eq!(detect_unit_divisor(65_000), Some(1000.0));
        assert_eq!(detect_unit_divisor(0), Some(1000.0));
        assert_eq!(detect_unit_divisor(-10_000), Some(1000.0));
    }

    #[test]
    fn detect_picks_microdegrees_when_millidegrees_overranges() {
        // 65 million → millidegrees gives 65,000 °C (rejected),
        // microdegrees gives 65 °C (accepted).
        assert_eq!(detect_unit_divisor(65_000_000), Some(1_000_000.0));
    }

    #[test]
    fn detect_returns_none_for_unfittable_values() {
        // 10^15 doesn't fit any divisor — would be way out of
        // range for all three. Should return None so callers can
        // distinguish "this is garbage" from "this is fine".
        assert_eq!(detect_unit_divisor(1_000_000_000_000_000), None);
        // Same for the negative extreme.
        assert_eq!(detect_unit_divisor(-1_000_000_000_000_000), None);
    }

    #[test]
    fn detect_handles_boundary_temperatures() {
        // Exactly +125 °C and exactly -40 °C should land in-range
        // for millidegrees. Off-by-one in our boundary check would
        // reject these.
        assert_eq!(detect_unit_divisor(125_000), Some(1000.0));
        assert_eq!(detect_unit_divisor(-40_000), Some(1000.0));
        // Just outside should fail to find millidegrees but still
        // succeed via microdegrees (where they become tiny).
        let just_above = detect_unit_divisor(125_001);
        assert_eq!(just_above, Some(1_000_000.0));
    }

    #[test]
    fn apply_divisor_round_trip() {
        // The point of the cached path: once we know a sensor's
        // divisor, we apply it directly. Verify three values
        // round-trip cleanly with each candidate divisor.
        for &div in &[1000.0_f32, 1_000_000.0_f32, 1.0_f32] {
            for &expected_c in &[25.0_f32, -10.0_f32, 75.0_f32] {
                let raw = (expected_c * div) as i64;
                let actual = apply_unit_divisor(raw, div);
                assert!(
                    (actual - expected_c).abs() < 0.01,
                    "round-trip failed at div={} expected={} got={}",
                    div, expected_c, actual,
                );
            }
        }
    }

    #[test]
    fn apply_divisor_clamps_garbage() {
        // If a sensor that *was* readable starts reporting
        // garbage (a real hardware-fault scenario), the cached
        // path should still clamp instead of feeding nonsense to
        // the model.
        let crazy = apply_unit_divisor(900_000_000, 1000.0);
        assert!(crazy <= 125.0 && crazy >= -40.0);
    }

    #[test]
    fn caching_prevents_per_tick_reinterpretation() {
        // This is the central bug per-sensor caching was added to
        // fix. Simulate two reads from the "same" sensor at
        // different magnitudes (the actual hardware would never
        // do this — but a near-boundary sensor could oscillate
        // and we want to verify the cache locks behavior).
        //
        // Without caching: 65 → 0.065 °C; 65,000 → 65 °C.
        //                  Same sensor reads two ~1000× values.
        // With caching: first reading locks divisor. Second
        //               reading is interpreted with the same
        //               divisor.
        //
        // We can't drive a real ThermalPoller here (it'd need
        // sysfs files), so this is a pure-logic re-enactment:
        // "if my cached divisor is 1000, then 65 stays 0.065, and
        // 65000 stays 65" — and the user-visible *consistency* is
        // what matters, not the absolute values.
        let cached_div = detect_unit_divisor(65).unwrap(); // 1000
        let read_a = apply_unit_divisor(65, cached_div);
        let read_b = apply_unit_divisor(65_000, cached_div);
        // Both interpreted as millidegrees: 0.065 and 65.
        assert!((read_a - 0.065).abs() < 0.001);
        assert!((read_b - 65.0).abs() < 0.001);
        // The key property: the same DIVISOR was used. Without
        // caching, the 65 case would have used div=1.0 and the
        // 65,000 case would have used div=1000.0 — same physical
        // input would have produced two different sense-of-scale.
    }
}

impl ThermalPoller {
    pub fn new() -> Self {
        let mut files = Vec::new();
        let base = Path::new("/sys/class/thermal");

        if let Ok(entries) = std::fs::read_dir(base) {
            for entry in entries.flatten() {
                let name = entry.file_name().into_string().unwrap_or_default();
                if name.starts_with("thermal_zone") {
                    let path = entry.path();
                    let temp_path = path.join("temp");
                    let type_path = path.join("type");
                    
                    // Read sensor type
                    let sensor_type = if let Ok(type_str) = std::fs::read_to_string(&type_path) {
                        let type_lower = type_str.to_lowercase();
                        if type_lower.contains("cpu") || type_lower.contains("x86_pkg_temp") 
                            || type_lower.contains("coretemp") || type_lower.contains("k10temp") {
                            SensorType::CPU
                        } else if type_lower.contains("gpu") || type_lower.contains("amdgpu") 
                            || type_lower.contains("nvidia") {
                            SensorType::GPU
                        } else if type_lower.contains("battery") || type_lower.contains("bat") {
                            SensorType::Battery
                        } else if type_lower.contains("acpi") {
                            SensorType::Ambient
                        } else {
                            SensorType::Unknown
                        }
                    } else {
                        SensorType::Unknown
                    };
                    
                    if let Ok(file) = File::open(&temp_path) {
                        let sensor_name = name.clone();
                        files.push((name, file, sensor_type));
                        println!("📡 Detected sensor: {} (type: {:?})", sensor_name, sensor_type);
                    }
                }
            }
        }
        
        // Also check /sys/class/hwmon for CPU sensors
        let hwmon_base = Path::new("/sys/class/hwmon");
        if let Ok(entries) = std::fs::read_dir(hwmon_base) {
            for entry in entries.flatten() {
                let hwmon_path = entry.path();
                let name_file = hwmon_path.join("name");
                
                if let Ok(name_str) = std::fs::read_to_string(&name_file) {
                    let name_lower = name_str.to_lowercase();
                    let sensor_type = if name_lower.contains("coretemp") || name_lower.contains("k10temp") 
                        || name_lower.contains("cpu") {
                        SensorType::CPU
                    } else {
                        continue; // Skip non-CPU hwmon sensors
                    };
                    
                    // Look for temp*_input files
                    if let Ok(hwmon_entries) = std::fs::read_dir(&hwmon_path) {
                        for hwmon_entry in hwmon_entries.flatten() {
                            let filename = hwmon_entry.file_name().into_string().unwrap_or_default();
                            if filename.starts_with("temp") && filename.ends_with("_input") {
                                if let Ok(file) = File::open(hwmon_entry.path()) {
                                    let sensor_name = format!("{}_{}", name_str.trim(), filename);
                                    files.push((sensor_name.clone(), file, sensor_type));
                                    println!("📡 Detected hwmon sensor: {} (type: {:?})", sensor_name, sensor_type);
                                }
                            }
                        }
                    }
                }
            }
        }
        
        println!("📊 Total sensors: {} (CPU: {}, GPU: {}, Battery: {}, Ambient: {}, Unknown: {})",
            files.len(),
            files.iter().filter(|(_, _, t)| *t == SensorType::CPU).count(),
            files.iter().filter(|(_, _, t)| *t == SensorType::GPU).count(),
            files.iter().filter(|(_, _, t)| *t == SensorType::Battery).count(),
            files.iter().filter(|(_, _, t)| *t == SensorType::Ambient).count(),
            files.iter().filter(|(_, _, t)| *t == SensorType::Unknown).count(),
        );

        let n_files = files.len();
        Self {
            files,
            buffer: vec![0; 32],
            locked: false,
            unit_scale: vec![None; n_files],
        }
    }

    fn try_lock(&mut self) -> bool {
        // Try to read from all sensors to verify access
        for (_, file, _) in &mut self.files {
            let _ = file.seek(SeekFrom::Start(0));
            if file.read(&mut self.buffer).is_err() {
                self.locked = false;
                return false;
            }
        }
        self.locked = true;
        true
    }

    pub fn poll(&mut self) -> Option<Vec<f32>> {
        if !self.locked && !self.try_lock() {
            return None;
        }

        let mut out = Vec::new();
        let mut success = true;

        for (idx, (_, file, _)) in self.files.iter_mut().enumerate() {
            let _ = file.seek(SeekFrom::Start(0));
            match file.read(&mut self.buffer) {
                Ok(n) => {
                    if let Ok(s) = std::str::from_utf8(&self.buffer[..n]) {
                        if let Ok(val) = s.trim().parse::<i64>() {
                            // Per-sensor unit caching:
                            //   * If we've already locked in a divisor for
                            //     this sensor, use it. Avoids per-tick
                            //     reclassification when a sensor reads a
                            //     value near a boundary (e.g. 65 ↔ 65000
                            //     which would otherwise be interpreted
                            //     differently and produce ~1000× swings).
                            //   * Otherwise, run detection on this read.
                            //     If detection succeeds, cache the
                            //     divisor for next time.
                            //   * If detection fails (no candidate in the
                            //     physical range), don't cache — but
                            //     still fall back to the defensive-clamp
                            //     path so the model gets a usable value
                            //     instead of a NaN or unbounded outlier.
                            let scaled = match self.unit_scale.get(idx).and_then(|s| *s) {
                                Some(div) => apply_unit_divisor(val, div),
                                None => {
                                    let v = normalize_thermal_reading(val);
                                    // Cache only when detection actually
                                    // landed in-range; if we had to
                                    // clamp, don't lock in a bad guess.
                                    if let Some(div) = detect_unit_divisor(val) {
                                        if let Some(slot) = self.unit_scale.get_mut(idx) {
                                            *slot = Some(div);
                                        }
                                    }
                                    v
                                }
                            };
                            out.push(scaled);
                        } else {
                            success = false;
                            break;
                        }
                    } else {
                        success = false;
                        break;
                    }
                }
                Err(_) => {
                    success = false;
                    break;
                }
            }
        }

        if success && out.len() == self.files.len() {
            Some(out)
        } else {
            self.locked = false;
            None
        }
    }

    pub fn sensor_names(&self) -> Vec<String> {
        self.files.iter().map(|(name, _, _)| name.clone()).collect()
    }
    
    pub fn sensor_types(&self) -> Vec<(String, SensorType)> {
        self.files.iter().map(|(name, _, stype)| (name.clone(), *stype)).collect()
    }
}

pub struct Database {
    conn: Connection,
    path: PathBuf,
}

impl Database {
    fn new(timestamp: &str) -> Result<Self, rusqlite::Error> {
        fs::create_dir_all(DATA_DIR).ok();
        let path = PathBuf::from(DATA_DIR).join(format!("sensors_{}.db", timestamp));
        let conn = Connection::open(&path)?;

        // Enable WAL mode for better concurrent access and performance
        // conn.execute("PRAGMA journal_mode=WAL", [])?;
        conn.pragma_update(None, "journal_mode", &"WAL")?;
        conn.execute("PRAGMA synchronous=NORMAL", [])?;  // Faster writes
        conn.execute("PRAGMA cache_size=10000", [])?;  // 10MB cache
        // Built-in busy handler: when another connection holds the
        // write lock, SQLite waits up to this long internally before
        // returning SQLITE_BUSY. Avoids the application-level retry
        // loop for sub-2-second contention.
        conn.busy_timeout(std::time::Duration::from_millis(2000))?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS readings (
                timestamp INTEGER PRIMARY KEY,
                data TEXT NOT NULL,
                location_id TEXT
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS calibrations (
                timestamp INTEGER PRIMARY KEY,
                sensors TEXT NOT NULL,
                target REAL NOT NULL,
                location_id TEXT
            )",
            [],
        )?;

        conn.execute("CREATE INDEX IF NOT EXISTS idx_readings_time ON readings(timestamp)", [])?;
        conn.execute("CREATE INDEX IF NOT EXISTS idx_readings_location ON readings(location_id)", [])?;

        println!("✓ Database initialized with WAL mode");
        Ok(Self { conn, path })
    }

    /// Take a consistent snapshot of the database at `src` into `dst`.
    ///
    /// Uses SQLite's online backup API: opens both DBs, runs `step(-1)`
    /// to copy all pages in one shot, and closes cleanly. This is the
    /// only safe way to copy a SQLite file that may have an active
    /// `-wal` or `-journal` sidecar.
    ///
    /// The destination file is created fresh; an existing one is
    /// truncated. Backup files are tiny relative to typical disk
    /// sizes so we don't bother with rotation here.
    fn create_consistent_backup(src: &Path, dst: &Path) -> Result<(), rusqlite::Error> {
        // Reopen the source read-only — we don't want our backup
        // path to be capable of mutating the database it's snapshotting.
        let src_conn = Connection::open_with_flags(
            src,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let mut dst_conn = Connection::open(dst)?;

        // SQLite's backup API has two entry points:
        //   * `step(num_pages)`: accepts -1 to mean "all pages, hold
        //     the source lock for the entire copy"
        //   * `run_to_completion(pages_per_step, sleep, progress)`:
        //     requires `pages_per_step > 0`; calls `step` in a loop
        //     so other writers can make progress between chunks
        //
        // We're snapshotting before opening, with no other writers in
        // the picture — `step(-1)` is the simplest and avoids the
        // panic from passing -1 to `run_to_completion`.
        let backup = rusqlite::backup::Backup::new(&src_conn, &mut dst_conn)?;
        // Loop because `step` returns More/Done — for our single-
        // shot use it should always return Done in one call, but
        // handling More keeps us safe if SQLite ever caps a single
        // step.
        loop {
            match backup.step(-1)? {
                rusqlite::backup::StepResult::Done => break,
                rusqlite::backup::StepResult::More => continue,
                rusqlite::backup::StepResult::Busy
                | rusqlite::backup::StepResult::Locked => {
                    // Backup file is being held by another reader of
                    // ourselves (rare, but possible during an upgrade
                    // race). Bail rather than spin-lock — the caller
                    // logs and continues without the backup.
                    return Err(rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                        Some("backup target busy".into()),
                    ));
                }
                // `StepResult` is `#[non_exhaustive]`; the rusqlite
                // crate can add new variants without a major bump.
                // We treat unknown future results as "bail conservatively"
                // — better than silently looping forever or panicking.
                _ => {
                    return Err(rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_INTERNAL),
                        Some("unexpected StepResult variant from backup.step".into()),
                    ));
                }
            }
        }
        Ok(())
    }

    fn open(path: &Path) -> Result<Self, rusqlite::Error> {
        // Create a *consistent* backup before opening for use.
        //
        // We deliberately use SQLite's online backup API (via
        // `Connection::backup`) instead of std::fs::copy. The bare
        // file copy doesn't include the `-wal` / `-journal` sidecars,
        // so a copy taken right after a crash can be missing the
        // most recent committed transactions or be partially
        // committed mid-row. The backup API knows about all three
        // files and produces a self-contained snapshot.
        if path.exists() {
            let backup_path = path.with_extension("db.backup");
            if let Err(e) = Self::create_consistent_backup(path, &backup_path) {
                eprintln!("⚠️  Warning: Could not create backup: {}", e);
                // Non-fatal — the user gets to keep the original DB
                // even if we couldn't snapshot it.
            } else {
                println!("✓ Created database backup: {}", backup_path.display());
            }
        }
        
        let conn = Connection::open(path)?;
        
        // Check database integrity
        match conn.pragma_query_value(None, "integrity_check", |row| row.get::<_, String>(0)) {
            Ok(result) => {
                if result != "ok" {
                    eprintln!("⚠️  Database integrity check failed: {}", result);
                    eprintln!("Attempting recovery...");
                    
                    // Try to recover by creating a new connection
                    drop(conn);
                    
                    // Check if backup exists
                    let backup_path = path.with_extension("db.backup");
                    if backup_path.exists() {
                        eprintln!("Restoring from backup...");
                        if let Err(e) = std::fs::copy(&backup_path, path) {
                            eprintln!("Failed to restore backup: {}", e);
                            return Err(rusqlite::Error::InvalidPath(path.to_path_buf()));
                        }
                        println!("✓ Restored from backup");
                        return Self::open(path); // Retry
                    }
                    
                    return Err(rusqlite::Error::InvalidPath(path.to_path_buf()));
                }
                println!("✓ Database integrity check passed");
            }
            Err(e) => {
                eprintln!("⚠️  Could not perform integrity check: {}", e);
                // Continue anyway - integrity check may not be critical
            }
        }
        
        // Enable WAL mode for existing databases
        // conn.execute("PRAGMA journal_mode=WAL", [])?;
        conn.pragma_update(None, "journal_mode", &"WAL")?;
        conn.execute("PRAGMA synchronous=NORMAL", [])?;
        conn.execute("PRAGMA cache_size=10000", [])?;
        // Same 2s built-in busy handler as Database::new — see there
        // for rationale.
        conn.busy_timeout(std::time::Duration::from_millis(2000))?;
        
        // Migrations: add `location_id` column to legacy tables.
        //
        // The previous version blindly ran `ALTER TABLE ... ADD COLUMN`
        // wrapped in `let _ = ...` — that "works" by ignoring errors
        // when the column already exists, but it dirties the WAL on
        // every startup. Querying `PRAGMA table_info` first lets us
        // skip the no-op alter entirely.
        Self::ensure_column(&conn, "readings", "location_id", "TEXT")?;
        Self::ensure_column(&conn, "calibrations", "location_id", "TEXT")?;
        
        println!("✓ Database opened: {}", path.display());
        Ok(Self { conn, path: path.to_path_buf() })
    }

    /// Add `column_name` of type `column_type` to `table_name` only if
    /// it doesn't already exist. Idempotent and avoids spamming the
    /// transaction log on every startup.
    fn ensure_column(
        conn: &Connection,
        table_name: &str,
        column_name: &str,
        column_type: &str,
    ) -> Result<(), rusqlite::Error> {
        // PRAGMA table_info returns one row per column. If we find
        // the target name, the migration is already done.
        let pragma = format!("PRAGMA table_info({})", table_name);
        let mut stmt = conn.prepare(&pragma)?;
        let exists = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(Result::ok)
            .any(|name| name == column_name);
        if exists {
            return Ok(());
        }

        let alter = format!(
            "ALTER TABLE {} ADD COLUMN {} {}",
            table_name, column_name, column_type
        );
        conn.execute(&alter, [])?;
        println!("✓ Migration: added {}.{}", table_name, column_name);
        Ok(())
    }

    fn get_calibration_count(&self) -> Result<usize, rusqlite::Error> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM calibrations",
            [],
            |row| row.get(0)
        )?;
        Ok(count as usize)
    }
    
    fn get_reading_count(&self) -> Result<usize, rusqlite::Error> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM readings",
            [],
            |row| row.get(0)
        )?;
        Ok(count as usize)
    }
    
    fn get_database_stats(&self) -> Result<(usize, usize, Option<i64>, Option<i64>), rusqlite::Error> {
        let cal_count = self.get_calibration_count()?;
        let read_count = self.get_reading_count()?;
        
        // Get oldest and newest calibration timestamps
        let oldest: Option<i64> = self.conn.query_row(
            "SELECT MIN(timestamp) FROM calibrations",
            [],
            |row| row.get(0)
        ).ok();
        
        let newest: Option<i64> = self.conn.query_row(
            "SELECT MAX(timestamp) FROM calibrations",
            [],
            |row| row.get(0)
        ).ok();
        
        Ok((cal_count, read_count, oldest, newest))
    }
    
    /// Reclaim free pages with `VACUUM`. **Expensive** — acquires an
    /// exclusive write lock on the database and rewrites every page
    /// to a temp file before swapping. Don't call this from the UI
    /// thread or while a poll is in flight.
    ///
    /// Typical use: scheduled maintenance via a separate thread, or
    /// from a CLI subcommand the user invokes manually.
    fn vacuum_database(&self) -> Result<(), rusqlite::Error> {
        println!("🧹 Running database VACUUM to reclaim space...");
        dbmaint::vacuum(&self.conn)?;
        println!("✓ VACUUM complete");
        Ok(())
    }
    
    fn analyze_database(&self) -> Result<(), rusqlite::Error> {
        println!("📊 Running database ANALYZE to update statistics...");
        dbmaint::analyze(&self.conn)?;
        println!("✓ ANALYZE complete");
        Ok(())
    }
    
    fn check_health(&self) -> Result<(), String> {
        println!("🏥 Running database health check...");
        
        let (validation, bad_sectors) = dbmaint::health_check(&self.conn)
            .map_err(|e| e.to_string())?;
        
        if !validation.is_valid {
            eprintln!("❌ Database validation failed!");
            for error in &validation.corruption_errors {
                eprintln!("  ⚠️  {}", error);
            }
            return Err("Database validation failed".to_string());
        }
        
        if !bad_sectors.is_empty() {
            println!("⚠️  Found {} corrupted records:", bad_sectors.len());
            for sector in bad_sectors.iter().take(5) {
                println!("  🔴 {} row {}: {}", sector.table, sector.row_id, sector.error);
            }
            if bad_sectors.len() > 5 {
                println!("  ... and {} more", bad_sectors.len() - 5);
            }
            return Err(format!("Found {} corrupted records", bad_sectors.len()));
        }
        
        println!("✅ Database health check passed");
        Ok(())
    }
    
    fn perform_maintenance(&self, keep_days: i64) -> Result<(), String> {
        println!("🔧 Starting database maintenance...");
        
        let report = dbmaint::full_maintenance(&self.conn, keep_days)
            .map_err(|e| e.to_string())?;
        
        println!("📊 Maintenance Report:");
        println!("  Before: {} calibrations, {} readings", 
            report.stats_before.calibration_count, 
            report.stats_before.reading_count);
        println!("  Bad sectors found: {}", report.bad_sectors_found);
        println!("  Bad sectors pruned: {}", report.bad_sectors_pruned);
        println!("  Old readings pruned: {}", report.old_readings_pruned);
        println!("  Duplicates removed: {}", report.duplicates_removed);
        println!("  After: {} calibrations, {} readings",
            report.stats_after.calibration_count,
            report.stats_after.reading_count);
        
        println!("✅ Maintenance complete");
        Ok(())
    }

    fn store_reading(&self, reading: &SensorReading, location_id: Option<&str>) -> Result<(), rusqlite::Error> {
        let json = match serde_json::to_string(reading) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("⚠️  store_reading: serialize failed: {}", e);
                return Err(rusqlite::Error::ToSqlConversionFailure(Box::new(e)));
            }
        };
        let timestamp: i64 = reading.time.parse().unwrap_or(0);

        // Cached prepared statement: rusqlite hashes the SQL once and
        // reuses the prepared statement on subsequent calls. Saves a
        // SQL parse per write (the dominant cost of one-shot inserts).
        let mut stmt = self.conn.prepare_cached(
            "INSERT OR REPLACE INTO readings (timestamp, data, location_id) VALUES (?1, ?2, ?3)",
        )?;

        // Exponential backoff with jitter on SQLITE_BUSY. Spreads the
        // retry wakeup so concurrent waiters don't lockstep.
        const RETRY_DELAYS_MS: &[u64] = &[10, 30, 90, 250];
        for (attempt, &base_ms) in RETRY_DELAYS_MS.iter().enumerate() {
            match stmt.execute(params![timestamp, json, location_id]) {
                Ok(_) => return Ok(()),
                Err(rusqlite::Error::SqliteFailure(err, _))
                    if err.code == rusqlite::ErrorCode::DatabaseBusy =>
                {
                    if attempt + 1 < RETRY_DELAYS_MS.len() {
                        let jitter_ms = (timestamp as u64).wrapping_mul(0x9E3779B1)
                            % (base_ms / 2 + 1);
                        std::thread::sleep(std::time::Duration::from_millis(base_ms + jitter_ms));
                        continue;
                    }
                    return Err(rusqlite::Error::SqliteFailure(err, None));
                }
                Err(e) => return Err(e),
            }
        }
        Err(rusqlite::Error::QueryReturnedNoRows)
    }

    pub fn store_calibration(&self, sensors: &[f32], target: f32, location_id: Option<&str>) -> Result<(), rusqlite::Error> {
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        let json = match serde_json::to_string(sensors) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("⚠️  store_calibration: serialize failed: {}", e);
                return Err(rusqlite::Error::ToSqlConversionFailure(Box::new(e)));
            }
        };

        let mut stmt = self.conn.prepare_cached(
            "INSERT INTO calibrations (timestamp, sensors, target, location_id) VALUES (?1, ?2, ?3, ?4)",
        )?;

        const RETRY_DELAYS_MS: &[u64] = &[10, 30, 90, 250];
        for (attempt, &base_ms) in RETRY_DELAYS_MS.iter().enumerate() {
            match stmt.execute(params![timestamp, json, target, location_id]) {
                Ok(_) => return Ok(()),
                Err(rusqlite::Error::SqliteFailure(err, _))
                    if err.code == rusqlite::ErrorCode::DatabaseBusy =>
                {
                    if attempt + 1 < RETRY_DELAYS_MS.len() {
                        let jitter_ms = (timestamp as u64).wrapping_mul(0x9E3779B1)
                            % (base_ms / 2 + 1);
                        std::thread::sleep(std::time::Duration::from_millis(base_ms + jitter_ms));
                        continue;
                    }
                    return Err(rusqlite::Error::SqliteFailure(err, None));
                }
                Err(e) => return Err(e),
            }
        }

        Err(rusqlite::Error::QueryReturnedNoRows)
    }
    
    /// Get all calibrations from the database for bootstrap training
    pub fn get_all_calibrations(&self) -> Result<Vec<(Vec<f32>, f32)>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT sensors, target FROM calibrations ORDER BY timestamp"
        )?;
        
        let calibrations: Vec<(Vec<f32>, f32)> = stmt.query_map([], |row| {
            let sensors_json: String = row.get(0)?;
            let target: f32 = row.get(1)?;
            
            // Parse JSON array of sensor values
            let sensors: Vec<f32> = serde_json::from_str(&sensors_json)
                .unwrap_or_default();
            
            Ok((sensors, target))
        })?
        .filter_map(Result::ok)
        .filter(|(sensors, _)| !sensors.is_empty())  // Filter out empty/invalid entries
        .collect();
        
        Ok(calibrations)
    }
    
    /// Get all calibrations with full metadata (for sync/export)
    pub fn get_all_calibrations_v1(&self) -> Result<Vec<(i64, Vec<f32>, f32, Option<String>)>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT timestamp, sensors, target, location_id FROM calibrations ORDER BY timestamp"
        )?;
        
        let calibrations: Vec<(i64, Vec<f32>, f32, Option<String>)> = stmt.query_map([], |row| {
            let timestamp: i64 = row.get(0)?;
            let sensors_json: String = row.get(1)?;
            let target: f32 = row.get(2)?;
            let location_id: Option<String> = row.get(3)?;
            
            // Parse JSON array of sensor values
            let sensors: Vec<f32> = serde_json::from_str(&sensors_json)
                .unwrap_or_default();
            
            Ok((timestamp, sensors, target, location_id))
        })?
        .filter_map(Result::ok)
        .filter(|(_, sensors, _, _)| !sensors.is_empty())  // Filter out empty/invalid entries
        .collect();
        
        Ok(calibrations)
    }

    fn get_daily_averages(&self, days_back: usize, num_days: usize, location_filter: Option<&str>) -> Vec<(String, f32)> {
        let mut result = Vec::new();

        let location_clause = if let Some(loc_id) = location_filter {
            format!("AND location_id = '{}' ", loc_id)
        } else {
            String::new()
        };

        let query = format!(
            "SELECT DATE(timestamp, 'unixepoch') as day, 
                    AVG(json_extract(data, '$.sensors[0]')) as avg_temp
             FROM readings 
             WHERE timestamp BETWEEN 
                strftime('%s', 'now', '-{} days') AND 
                strftime('%s', 'now', '-{} days')
                {}
             GROUP BY day
             ORDER BY day DESC",
            days_back + num_days,
            days_back,
            location_clause
        );

        if let Ok(mut stmt) = self.conn.prepare(&query) {
            if let Ok(rows) = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, f32>(1)?))) {
                for row in rows.flatten() {
                    result.push(row);
                }
            }
        }

        result
    }

    fn export_to_csv(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let query = "SELECT DATE(timestamp, 'unixepoch') as day, 
                            AVG(json_extract(data, '$.sensors[0]')) as avg_temp,
                            COUNT(*) as reading_count
                     FROM readings 
                     GROUP BY day
                     ORDER BY day ASC";

        let mut stmt = self.conn.prepare(query)?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, f32>(1)?,
                row.get::<_, i64>(2)?
            ))
        })?;

        let mut file = File::create(path)?;
        writeln!(file, "Date,Average Temperature (°C),Reading Count")?;

        for row in rows {
            if let Ok((day, temp, count)) = row {
                writeln!(file, "{},{:.2},{}", day, temp, count)?;
            }
        }

        Ok(())
    }
    
    /// Get calibration history for time series (timestamp, temperature)
    pub fn get_calibration_history(&self, days: i64) -> Result<Vec<(i64, f32)>, rusqlite::Error> {
        let cutoff = chrono::Utc::now().timestamp() - (days * 86400);
        
        let query = "SELECT timestamp, target_temp 
                     FROM calibrations 
                     WHERE timestamp > ? 
                     ORDER BY timestamp ASC";
        
        let mut stmt = self.conn.prepare(query)?;
        let rows = stmt.query_map([cutoff], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
        
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        
        Ok(result)
    }
}

fn get_timestamp() -> String {
    Local::now().format("%Y%m%d_%H%M%S").to_string()
}

fn get_iso_timestamp() -> String {
    Utc::now().to_rfc3339()
}

/// Gets a unique device identifier for sensor tracking
///
/// Combines CPU model, sensor count, and machine ID to create
/// a stable identifier that distinguishes between different devices.
///
/// This is critical for preventing sensor confusion when:
/// - Sharing databases between devices
/// - Same sensor names but different hardware
/// - Transfer learning across devices
///
/// # Returns
///
/// Device ID string like: "Snapdragon_8_Gen2_53s_a3f4b2c1"
///
/// # Format
///
/// `{cpu_model}_{sensor_count}s_{machine_id_prefix}`
///
/// # Examples
///
/// ```
/// Phone: "Snapdragon_8_Gen2_53s_a3f4b2c1"
/// Laptop: "Intel_Core_i7-1165G7_5s_81f60d58"
/// ```
fn get_device_id() -> String {
    // Get CPU model from /proc/cpuinfo
    let cpu_model = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split(':').nth(1))
                .map(|m| m.trim().to_string())
        })
        .unwrap_or_else(|| {
            // Fallback: try ARM hardware
            std::fs::read_to_string("/proc/cpuinfo")
                .ok()
                .and_then(|s| {
                    s.lines()
                        .find(|l| l.starts_with("Hardware"))
                        .and_then(|l| l.split(':').nth(1))
                        .map(|m| m.trim().to_string())
                })
                .unwrap_or_else(|| "Unknown_CPU".to_string())
        });
    
    // Sanitize CPU model (remove spaces, special chars)
    let cpu_sanitized: String = cpu_model
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    
    // Get sensor count
    let sensor_count = ThermalPoller::new().sensor_names().len();
    
    // Get machine ID (first 8 chars for brevity)
    let machine_id = std::fs::read_to_string("/etc/machine-id")
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| {
            // Fallback: generate from environment
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            
            let hostname = std::env::var("HOSTNAME")
                .or_else(|_| std::env::var("HOST"))
                .unwrap_or_else(|_| "unknown".to_string());
            
            let mut hasher = DefaultHasher::new();
            hostname.hash(&mut hasher);
            format!("{:016x}", hasher.finish())
        });
    
    let machine_id_short = &machine_id[..8.min(machine_id.len())];
    
    format!("{}_{}s_{}", 
        cpu_sanitized,
        sensor_count,
        machine_id_short
    )
}

fn get_latest_model() -> Option<PathBuf> {
    fs::create_dir_all(DATA_DIR).ok();
    let re = Regex::new(r"model_(\d{8}_\d{6})\.json").unwrap();
    
    let mut models: Vec<(String, PathBuf)> = fs::read_dir(DATA_DIR)
        .ok()?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            let filename = path.file_name()?.to_str()?;
            
            if let Some(caps) = re.captures(filename) {
                let timestamp = caps.get(1)?.as_str().to_string();
                Some((timestamp, path))
            } else {
                None
            }
        })
        .collect();
    
    models.sort_by(|a, b| b.0.cmp(&a.0));
    models.first().map(|(_, path)| path.clone())
}

fn get_latest_neural_field() -> Option<PathBuf> {
    fs::create_dir_all(DATA_DIR).ok();
    let re = Regex::new(r"neural_field_(\d{8}_\d{6})\.json").unwrap();
    
    let mut fields: Vec<(String, PathBuf)> = fs::read_dir(DATA_DIR)
        .ok()?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            let filename = path.file_name()?.to_str()?;
            
            if let Some(caps) = re.captures(filename) {
                let timestamp = caps.get(1)?.as_str().to_string();
                Some((timestamp, path))
            } else {
                None
            }
        })
        .collect();
    
    fields.sort_by(|a, b| b.0.cmp(&a.0));
    fields.first().map(|(_, path)| path.clone())
}

/// CLI override for the database path. Set once from `main()` if
/// `--db <PATH>` is passed; `get_latest_database()` consults this
/// before scanning the data directory.
///
/// We use `OnceLock` rather than a regular `Mutex<Option<...>>`
/// because the value is set exactly once at startup and never
/// changes; readers don't need to take a lock on the hot path.
static DB_PATH_OVERRIDE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Set the CLI database-path override. Idempotent — calling twice
/// with different paths is a programming error and the second call
/// will be silently ignored (OnceLock semantics). Should be called
/// exactly once from `main()` after CLI parsing.
fn set_db_override(path: PathBuf) {
    let _ = DB_PATH_OVERRIDE.set(path);
}

fn get_latest_database() -> Option<PathBuf> {
    // CLI override wins. We don't validate that the path exists
    // here — the caller will get a clear error when it tries to
    // open() the file. Returning the path anyway lets the user
    // see "failed to open: <the path I gave>" instead of the
    // misleading "no database found in weather_data/" when they
    // typo'd a path.
    if let Some(path) = DB_PATH_OVERRIDE.get() {
        return Some(path.clone());
    }

    fs::create_dir_all(DATA_DIR).ok();
    latest_database_in(Path::new(DATA_DIR))
}

/// Find the newest `sensors_<YYYYMMDD_HHMMSS>.db` file in `dir`.
///
/// Extracted from `get_latest_database` so it can be tested
/// without the process-global `DB_PATH_OVERRIDE` getting in the
/// way. This is the file-scan logic alone; the override layering
/// is handled by the caller.
fn latest_database_in(dir: &Path) -> Option<PathBuf> {
    let re = Regex::new(r"sensors_(\d{8}_\d{6})\.db").unwrap();

    let mut dbs: Vec<(String, PathBuf)> = fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            let filename = path.file_name()?.to_str()?;

            if let Some(caps) = re.captures(filename) {
                let timestamp = caps.get(1)?.as_str().to_string();
                Some((timestamp, path))
            } else {
                None
            }
        })
        .collect();

    dbs.sort_by(|a, b| b.0.cmp(&a.0));
    dbs.first().map(|(_, path)| path.clone())
}

#[cfg(test)]
mod latest_database_tests {
    use super::latest_database_in;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn empty_dir_returns_none() {
        let dir = tempdir().unwrap();
        assert!(latest_database_in(dir.path()).is_none());
    }

    #[test]
    fn picks_newest_by_timestamp_lex_sort() {
        // The timestamp format YYYYMMDD_HHMMSS sorts lexicographically
        // the same as it sorts temporally. Verify the newest wins.
        let dir = tempdir().unwrap();
        for name in &[
            "sensors_20250101_120000.db",
            "sensors_20240615_080000.db",
            "sensors_20251231_235959.db", // newest
            "sensors_20250615_080000.db",
        ] {
            fs::write(dir.path().join(name), b"").unwrap();
        }
        let got = latest_database_in(dir.path()).unwrap();
        assert!(
            got.file_name().unwrap()
                .to_str().unwrap()
                .ends_with("20251231_235959.db"),
            "got {:?}", got,
        );
    }

    #[test]
    fn ignores_files_that_dont_match_pattern() {
        // Files in the data dir that aren't `sensors_*.db` (config
        // files, lock files, backups with different naming) must
        // be ignored. Picking up `db_lock` as a database would
        // produce confusing failures later.
        let dir = tempdir().unwrap();
        for name in &[
            "sensors_20250101_120000.db",  // valid, should win
            "db_lock",                      // not a DB
            "sensors_old.db",              // wrong format
            "sensors_20250101.db",          // wrong format (missing time)
            "readings.csv",                 // wrong extension
        ] {
            fs::write(dir.path().join(name), b"").unwrap();
        }
        let got = latest_database_in(dir.path()).unwrap();
        assert!(got.to_str().unwrap().contains("sensors_20250101_120000.db"));
    }

    #[test]
    fn no_matching_files_returns_none() {
        // Dir exists but has no sensors_*.db files.
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("readme.txt"), b"hi").unwrap();
        assert!(latest_database_in(dir.path()).is_none());
    }

    #[test]
    fn nonexistent_dir_returns_none() {
        // Should not panic. `read_dir` returns an error which we
        // suppress with `.ok()?`.
        assert!(latest_database_in(std::path::Path::new(
            "/nonexistent-xyz-totally-not-a-real-dir-12345"
        )).is_none());
    }
}

/// Import an external database file into the app's data dir.
///
/// We use SQLite's online backup API (not a raw file copy) so that:
///   * Source `-wal` / `-journal` sidecars are correctly merged
///   * The integrity_check pragma can run on the imported file
///   * Schema compatibility is implicitly checked when we open it
///
/// The destination filename is `sensors_<ts>.db` with `<ts>` being
/// the current timestamp, so the imported DB becomes the new "latest"
/// for the next `get_latest_database()` call.
///
/// Returns the destination path on success.
fn import_database_cli(src: &Path) -> Result<PathBuf, String> {
    if !src.exists() {
        return Err(format!("source path does not exist: {}", src.display()));
    }
    if !src.is_file() {
        return Err(format!("source is not a file: {}", src.display()));
    }

    // Verify the source is actually a SQLite database by opening it
    // read-only and running integrity_check. Better to fail here
    // than 30 seconds in.
    let src_conn = Connection::open_with_flags(
        src,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|e| format!("cannot open source as SQLite: {e}"))?;
    let integrity: String = src_conn
        .pragma_query_value(None, "integrity_check", |r| r.get(0))
        .map_err(|e| format!("integrity_check failed on source: {e}"))?;
    if integrity != "ok" {
        return Err(format!(
            "source database failed integrity_check: {}",
            integrity
        ));
    }
    drop(src_conn);

    fs::create_dir_all(DATA_DIR)
        .map_err(|e| format!("cannot create {}: {e}", DATA_DIR))?;

    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let dst_filename = format!("sensors_{}.db", timestamp);
    let dst = PathBuf::from(DATA_DIR).join(&dst_filename);

    if dst.exists() {
        // Astronomically unlikely (sub-second collision with our own
        // run) but we should still refuse rather than clobber.
        return Err(format!(
            "destination already exists (try again in a second): {}",
            dst.display()
        ));
    }

    // Use the online backup API to copy. Snapshot-style — same
    // pattern as Database::create_consistent_backup.
    let src_conn = Connection::open_with_flags(
        src,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|e| format!("reopen source: {e}"))?;
    let mut dst_conn = Connection::open(&dst)
        .map_err(|e| format!("create destination: {e}"))?;
    let backup = rusqlite::backup::Backup::new(&src_conn, &mut dst_conn)
        .map_err(|e| format!("init backup: {e}"))?;
    loop {
        match backup.step(-1) {
            Ok(rusqlite::backup::StepResult::Done) => break,
            Ok(rusqlite::backup::StepResult::More) => continue,
            Ok(rusqlite::backup::StepResult::Busy)
            | Ok(rusqlite::backup::StepResult::Locked) => {
                return Err("source database is busy".into());
            }
            Err(e) => return Err(format!("backup step: {e}")),
            // `StepResult` is `#[non_exhaustive]`; future rusqlite
            // versions may add variants. Bail conservatively rather
            // than looping or panicking on an unknown result.
            Ok(_) => return Err("unexpected StepResult variant".into()),
        }
    }

    Ok(dst)
}

/// Snapshot the current "latest" database to an arbitrary path.
/// Uses the SQLite online backup API for consistency.
fn export_database_cli(dest: &Path) -> Result<(), String> {
    let src = get_latest_database()
        .ok_or_else(|| format!("no database found in {}", DATA_DIR))?;

    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("create dest parent: {e}"))?;
        }
    }

    let src_conn = Connection::open_with_flags(
        &src,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|e| format!("open source: {e}"))?;
    let mut dst_conn = Connection::open(dest)
        .map_err(|e| format!("create export destination: {e}"))?;
    let backup = rusqlite::backup::Backup::new(&src_conn, &mut dst_conn)
        .map_err(|e| format!("init backup: {e}"))?;
    loop {
        match backup.step(-1) {
            Ok(rusqlite::backup::StepResult::Done) => break,
            Ok(rusqlite::backup::StepResult::More) => continue,
            Ok(rusqlite::backup::StepResult::Busy)
            | Ok(rusqlite::backup::StepResult::Locked) => {
                return Err("database is busy".into());
            }
            Err(e) => return Err(format!("backup step: {e}")),
            // Same future-variant guard as the import path; see there
            // for rationale.
            Ok(_) => return Err("unexpected StepResult variant".into()),
        }
    }

    println!(
        "✅ Exported {} → {}",
        src.display(),
        dest.display()
    );
    Ok(())
}

fn list_all_databases() -> Vec<PathBuf> {
    fs::create_dir_all(DATA_DIR).ok();
    let re = Regex::new(r"sensors_(\d{8}_\d{6})\.db").unwrap();
    
    let mut dbs: Vec<(String, PathBuf)> = fs::read_dir(DATA_DIR)
        .ok()
        .map(|entries| {
            entries
                .filter_map(|entry| {
                    let entry = entry.ok()?;
                    let path = entry.path();
                    let filename = path.file_name()?.to_str()?;
                    
                    if let Some(caps) = re.captures(filename) {
                        let timestamp = caps.get(1)?.as_str().to_string();
                        Some((timestamp, path))
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    
    dbs.sort_by(|a, b| b.0.cmp(&a.0));
    dbs.into_iter().map(|(_, path)| path).collect()
}

fn cleanup_old_databases(keep_count: usize) {
    let all_dbs = list_all_databases();
    
    if all_dbs.len() <= keep_count {
        println!("📦 Database cleanup: {} databases (keeping all)", all_dbs.len());
        return;
    }
    
    let to_delete = &all_dbs[keep_count..];
    println!("🧹 Cleaning up {} old databases (keeping {})", to_delete.len(), keep_count);
    
    for db_path in to_delete {
        println!("  🗑️  Deleting: {}", db_path.display());
        
        // Delete main database file
        if let Err(e) = std::fs::remove_file(db_path) {
            eprintln!("    ⚠️  Failed to delete: {}", e);
            continue;
        }
        
        // Delete associated WAL and SHM files
        let wal_path = db_path.with_extension("db-wal");
        let shm_path = db_path.with_extension("db-shm");
        let backup_path = db_path.with_extension("db.backup");
        
        std::fs::remove_file(&wal_path).ok();
        std::fs::remove_file(&shm_path).ok();
        std::fs::remove_file(&backup_path).ok();
        
        println!("    ✅ Deleted successfully");
    }
}

fn get_timezone_cities() -> Vec<String> {
    let mut cities = Vec::new();
    
    // Source 1: timedatectl list-timezones
    if let Ok(output) = std::process::Command::new("timedatectl")
        .arg("list-timezones")
        .output()
    {
        if let Ok(list) = String::from_utf8(output.stdout) {
            cities.extend(list.lines()
                .filter_map(|tz| {
                    // Extract city from timezone like "America/Montreal"
                    tz.split('/').last().map(|s| s.replace('_', " "))
                })
            );
        }
    }
    
    // Source 2: /usr/share/zoneinfo
    if cities.is_empty() {
        if let Ok(entries) = std::fs::read_dir("/usr/share/zoneinfo") {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type() {
                    if file_type.is_dir() {
                        let _continent = entry.file_name();
                        let continent_path = entry.path();
                        
                        if let Ok(city_entries) = std::fs::read_dir(&continent_path) {
                            for city_entry in city_entries.flatten() {
                                if let Some(city_name) = city_entry.file_name().to_str() {
                                    cities.push(city_name.replace('_', " ").to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    
    // Source 3: localectl list-locales (for additional location data)
    if let Ok(output) = std::process::Command::new("localectl")
        .arg("list-locales")
        .output()
    {
        if let Ok(list) = String::from_utf8(output.stdout) {
            for locale in list.lines() {
                // Extract location from locales like "en_CA.UTF-8"
                if let Some(loc) = locale.split('.').next() {
                    if let Some(country) = loc.split('_').last() {
                        // Don't add generic country codes
                        if country.len() == 2 {
                            continue;
                        }
                    }
                }
            }
        }
    }
    
    // Add comprehensive list of major world cities
    let major_cities = vec![
        // North America
        "Montreal", "Toronto", "Vancouver", "Calgary", "Edmonton", "Ottawa", "Winnipeg",
        "Quebec City", "Halifax", "Victoria",
        "New York", "Los Angeles", "Chicago", "Houston", "Phoenix", "Philadelphia",
        "San Antonio", "San Diego", "Dallas", "San Jose", "Austin", "Jacksonville",
        "Fort Worth", "Columbus", "Charlotte", "San Francisco", "Indianapolis",
        "Seattle", "Denver", "Washington", "Boston", "Nashville", "Detroit",
        "Portland", "Las Vegas", "Memphis", "Baltimore", "Milwaukee", "Albuquerque",
        "Tucson", "Fresno", "Sacramento", "Kansas City", "Mesa", "Atlanta",
        "Miami", "Minneapolis", "Cleveland", "Tampa", "New Orleans",
        "Mexico City", "Guadalajara", "Monterrey", "Puebla", "Tijuana",
        
        // Europe
        "London", "Paris", "Berlin", "Madrid", "Rome", "Vienna", "Athens",
        "Amsterdam", "Brussels", "Copenhagen", "Dublin", "Helsinki", "Lisbon",
        "Oslo", "Prague", "Stockholm", "Warsaw", "Budapest", "Bucharest",
        "Barcelona", "Munich", "Milan", "Hamburg", "Manchester", "Lyon",
        "Marseille", "Valencia", "Seville", "Naples", "Turin", "Palermo",
        "Genoa", "Rotterdam", "The Hague", "Antwerp", "Zurich", "Geneva",
        
        // Asia
        "Tokyo", "Delhi", "Shanghai", "Mumbai", "Beijing", "Osaka", "Karachi",
        "Dhaka", "Manila", "Seoul", "Jakarta", "Bangkok", "Hong Kong",
        "Singapore", "Kuala Lumpur", "Taipei", "Bangalore", "Chennai",
        "Kolkata", "Hyderabad", "Pune", "Ahmedabad", "Hanoi", "Ho Chi Minh City",
        "Yangon", "Riyadh", "Dubai", "Abu Dhabi", "Tehran", "Baghdad",
        "Kabul", "Tashkent", "Almaty", "Jerusalem", "Tel Aviv", "Beirut",
        "Damascus", "Amman", "Kuwait City", "Doha",
        
        // South America
        "São Paulo", "Buenos Aires", "Rio de Janeiro", "Lima", "Bogotá",
        "Santiago", "Caracas", "Brasília", "Quito", "Montevideo",
        "Asunción", "La Paz", "Sucre", "Georgetown", "Paramaribo",
        
        // Africa
        "Cairo", "Lagos", "Kinshasa", "Johannesburg", "Khartoum", "Alexandria",
        "Abidjan", "Casablanca", "Nairobi", "Addis Ababa", "Cape Town",
        "Dar es Salaam", "Kampala", "Luanda", "Maputo", "Accra", "Dakar",
        "Algiers", "Tunis", "Rabat", "Lusaka", "Harare",
        
        // Oceania
        "Sydney", "Melbourne", "Brisbane", "Perth", "Adelaide", "Auckland",
        "Wellington", "Christchurch", "Canberra", "Gold Coast", "Newcastle",
        "Wollongong", "Hobart", "Darwin", "Cairns",
    ];
    
    cities.extend(major_cities.iter().map(|s| s.to_string()));
    
    // Clean and deduplicate
    cities.sort();
    cities.dedup();
    
    // Remove empty strings
    cities.retain(|c| !c.is_empty());
    
    if cities.is_empty() {
        // Ultimate fallback
        vec![
            "Montreal".to_string(),
            "Toronto".to_string(),
            "New York".to_string(),
            "London".to_string(),
            "Paris".to_string(),
            "Tokyo".to_string(),
        ]
    } else {
        cities
    }
}

#[cfg(feature = "dbus")]
use zbus::{Connection as ZbusConnection, Result as ZbusResult};
#[cfg(feature = "dbus")]
use async_io::block_on;

/// WiFi detection with timeout and fallback
/// 
/// Tries D-Bus first (fast), falls back to nmcli if D-Bus fails/times out
fn get_current_network_ssid_with_fallback() -> Option<String> {
    // Try D-Bus with 500ms timeout
    match get_current_network_ssid_async_with_timeout(Duration::from_millis(500)) {
        Some(ssid) => {
            println!("WiFi detected via D-Bus: {}", ssid);
            Some(ssid)
        }
        None => {
            // D-Bus failed, try nmcli fallback
            println!("D-Bus WiFi detection failed, trying nmcli fallback...");
            get_current_network_ssid_nmcli()
        }
    }
}

/// D-Bus WiFi detection with timeout
#[cfg(feature = "dbus")]
fn get_current_network_ssid_async_with_timeout(timeout: Duration) -> Option<String> {
    // Use a channel to implement timeout
    let (tx, rx) = std::sync::mpsc::channel();
    
    std::thread::spawn(move || {
        let result: Option<String> = block_on(async {
            get_current_network_ssid_dbus().await.ok().flatten()
        });
        let _ = tx.send(result);
    });
    
    // Wait for result with timeout
    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(_) => {
            println!("D-Bus WiFi detection timed out after {}ms", timeout.as_millis());
            None
        }
    }
}

#[cfg(not(feature = "dbus"))]
fn get_current_network_ssid_async_with_timeout(_timeout: Duration) -> Option<String> {
    None  // D-Bus not enabled, skip
}

/// Non-blocking network detection using D-Bus
/// 
/// This replaces the blocking nmcli approach with async D-Bus queries
#[cfg(feature = "dbus")]
fn get_current_network_ssid_async() -> Option<String> {
    block_on(async {
        let result: Option<String> = get_current_network_ssid_dbus().await.ok().flatten();
        result
    })
}

#[cfg(not(feature = "dbus"))]
fn get_current_network_ssid_async() -> Option<String> {
    None
}

#[cfg(feature = "dbus")]
async fn get_current_network_ssid_dbus() -> ZbusResult<Option<String>> {
    // Try to connect to system bus
    let connection = ZbusConnection::system().await?;
    
    // NetworkManager object path
    let nm_proxy: zbus::Proxy = zbus::Proxy::new(
        &connection,
        "org.freedesktop.NetworkManager",
        "/org/freedesktop/NetworkManager",
        "org.freedesktop.NetworkManager",
    ).await?;
    
    // Get active connections (return None if fails)
    let active_connection: zbus::zvariant::OwnedObjectPath = match nm_proxy
        .get_property("PrimaryConnection")
        .await
    {
        Ok(path) => path,
        Err(_) => return Ok(None),
    };
    
    if active_connection.as_str() == "/" {
        return Ok(None);
    }
    
    // Get connection proxy
    let conn_proxy: zbus::Proxy = zbus::Proxy::new(
        &connection,
        "org.freedesktop.NetworkManager",
        active_connection.as_str(),
        "org.freedesktop.NetworkManager.Connection.Active",
    ).await?;
    
    // Get connection ID (SSID for WiFi)
    let ssid: String = match conn_proxy.get_property("Id").await {
        Ok(id) => id,
        Err(_) => return Ok(None),
    };
    
    if ssid.is_empty() {
        Ok(None)
    } else {
        Ok(Some(ssid))
    }
}

/// Fallback nmcli WiFi detection (used when D-Bus fails)
fn get_current_network_ssid_nmcli() -> Option<String> {
    // Try to get current WiFi SSID on Linux
    let output = std::process::Command::new("nmcli")
        .args(["-t", "-f", "active,ssid", "dev", "wifi"])
        .output();
    
    if let Ok(output) = output {
        if let Ok(text) = String::from_utf8(output.stdout) {
            for line in text.lines() {
                if line.starts_with("yes:") {
                    let ssid = line[4..].trim().to_string();
                    if !ssid.is_empty() {
                        return Some(ssid);
                    }
                }
            }
        }
    }
    None
}

fn save_locations(locations: &[Location]) -> Result<(), Box<dyn std::error::Error>> {
    let path = PathBuf::from(DATA_DIR).join("locations.json");
    let json = serde_json::to_string_pretty(locations)?;
    let mut file = File::create(path)?;
    file.write_all(json.as_bytes())?;
    Ok(())
}

fn load_locations() -> Vec<Location> {
    let path = PathBuf::from(DATA_DIR).join("locations.json");
    if let Ok(mut file) = File::open(path) {
        let mut contents = String::new();
        if file.read_to_string(&mut contents).is_ok() {
            if let Ok(locations) = serde_json::from_str(&contents) {
                return locations;
            }
        }
    }
    
    // Default: Unknown location
    vec![Location::new("Unknown".to_string(), LocationType::Unknown)]
}

fn detect_current_location(locations: &[Location]) -> Option<usize> {
    // Try to detect by WiFi network with timeout and fallback
    if let Some(ssid) = get_current_network_ssid_with_fallback() {
        for (idx, location) in locations.iter().enumerate() {
            if location.matches_network(&ssid) {
                println!("Auto-detected location: {} (network: {})", location.name, ssid);
                return Some(idx);
            }
        }
    }
    
    // Default to Unknown if it exists, otherwise first location
    locations.iter().position(|l| matches!(l.location_type, LocationType::Unknown))
        .or(Some(0))
}

// Database cleanup and compression
fn compress_old_models() -> Result<(), Box<dyn std::error::Error>> {
    use flate2::Compression;
    use flate2::write::GzEncoder;
    
    let model_dir = PathBuf::from(DATA_DIR);
    let re = Regex::new(r"^model_(\d{6})(\d{2})_\d{6}\.json$")?;
    
    // Group models by month
    let mut by_month: HashMap<String, Vec<PathBuf>> = HashMap::new();
    
    if let Ok(entries) = fs::read_dir(&model_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                if let Some(caps) = re.captures(filename) {
                    let month = format!("{}-{}", &caps[1], &caps[2]);
                    by_month.entry(month).or_default().push(path);
                }
            }
        }
    }
    
    // Current month - don't compress
    let current_month = Local::now().format("%Y-%m").to_string();
    
    for (month, files) in by_month {
        if month == current_month || files.len() < 10 {
            continue; // Skip current month and small batches
        }
        
        let archive_path = model_dir.join(format!("models_{}.json.gz", month));
        if archive_path.exists() {
            // Already compressed - delete originals
            for file in files {
                let _ = fs::remove_file(file);
            }
            continue;
        }
        
        // Load and combine models
        let mut combined = Vec::new();
        for file in &files {
            if let Ok(contents) = fs::read_to_string(file) {
                combined.push(contents);
            }
        }
        
        if combined.is_empty() {
            continue;
        }
        
        // Create archive JSON
        let archive_json = format!("{{\"month\":\"{}\",\"models\":[{}]}}", 
            month, combined.join(","));
        
        // Compress
        let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(archive_json.as_bytes())?;
        let compressed = encoder.finish()?;
        
        // Save
        fs::write(&archive_path, compressed)?;
        
        // Delete originals
        for file in files {
            let _ = fs::remove_file(file);
        }
        
        println!("Compressed {} models for {}", combined.len(), month);
    }
    
    Ok(())
}

fn cleanup_old_data() {
    // Run compression in background
    thread::spawn(|| {
        thread::sleep(Duration::from_secs(10)); // Wait for app to start
        if let Err(e) = compress_old_models() {
            eprintln!("Compression error: {}", e);
        }
    });
}

fn temp_to_color(temp: f32) -> &'static str {
    if temp < 40.0 {
        "#4A90E2"
    } else if temp < 55.0 {
        "#7ED321"
    } else {
        "#F5A623"
    }
}

fn generate_network_svg(model: &Model, pulse: f32) -> String {
    let width = 800;   // Reduced from 1000
    let height = 1000; // Reduced from 1400
    let mut svg = format!(
        "<svg width=\"100%\" height=\"100%\" xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {} {}\" preserveAspectRatio=\"xMidYMid meet\">",
        width, height
    );

    // Gradient background for depth
    svg.push_str("<defs>");
    svg.push_str("<linearGradient id=\"bg-gradient\" x1=\"0%\" y1=\"0%\" x2=\"0%\" y2=\"100%\">");
    svg.push_str("<stop offset=\"0%\" style=\"stop-color:#1a1a2e;stop-opacity:1\" />");
    svg.push_str("<stop offset=\"100%\" style=\"stop-color:#0f0f1a;stop-opacity:1\" />");
    svg.push_str("</linearGradient>");
    
    // Glow filter for nodes
    svg.push_str("<filter id=\"glow\">");
    svg.push_str("<feGaussianBlur stdDeviation=\"3\" result=\"coloredBlur\"/>");
    svg.push_str("<feMerge><feMergeNode in=\"coloredBlur\"/><feMergeNode in=\"SourceGraphic\"/></feMerge>");
    svg.push_str("</filter>");
    
    // Animated pulse
    svg.push_str("<radialGradient id=\"pulse-gradient\">");
    svg.push_str("<stop offset=\"0%\" style=\"stop-color:#ffffff;stop-opacity:0.8\" />");
    svg.push_str("<stop offset=\"100%\" style=\"stop-color:#4A90E2;stop-opacity:0\" />");
    svg.push_str("</radialGradient>");
    svg.push_str("</defs>");

    svg.push_str("<rect width=\"100%\" height=\"100%\" fill=\"url(#bg-gradient)\"/>");
    
    // Title with glow
    svg.push_str("<text x=\"400\" y=\"35\" text-anchor=\"middle\" fill=\"#ffffff\" font-size=\"24\" font-weight=\"bold\" filter=\"url(#glow)\">Neural Network Model</text>");
    svg.push_str("<text x=\"400\" y=\"60\" text-anchor=\"middle\" fill=\"#888888\" font-size=\"12\">Temperature Prediction Network</text>");

    let num_sensors = model.stats.len();
    let use_double_column = num_sensors > 12;
    let sensors_per_column = if use_double_column {
        (num_sensors + 1) / 2
    } else {
        num_sensors
    };

    let node_spacing = if use_double_column {
        (600.0 / sensors_per_column as f32).min(35.0)  // Adjusted for smaller canvas
    } else {
        (600.0 / num_sensors as f32).min(40.0)
    };

    let start_y = 100.0;  // Adjusted
    let output_x = 400.0; // Centered
    let output_y = 800.0; // Adjusted for smaller canvas

    // Draw connections (behind nodes)
    for (i, stat) in model.stats.iter().enumerate() {
        let col = if use_double_column && i >= sensors_per_column {
            1
        } else {
            0
        };
        let row = if use_double_column && i >= sensors_per_column {
            i - sensors_per_column
        } else {
            i
        };

        let sensor_x = if col == 0 { 150.0 } else { 650.0 };  // Adjusted spacing
        let sensor_y = start_y + (row as f32 * node_spacing);

        let opacity = (stat.weight.abs() * stat.dropout_score * 0.8).min(0.95);
        let line_width = (stat.weight.abs() * 3.5 + 0.8).min(8.0);  // Thicker lines
        
        // Animated color based on pulse
        let pulse_factor = (pulse + i as f32 * 0.2).sin() * 0.5 + 0.5;
        let color = if stat.weight > 0.0 { 
            format!("rgba(74, 144, 226, {})", 0.5 + pulse_factor * 0.5)  // Animated blue
        } else { 
            format!("rgba(226, 74, 74, {})", 0.5 + pulse_factor * 0.5)  // Animated red
        };

        if opacity > 0.05 {
            svg.push_str(&format!(
                "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{}\" stroke-width=\"{}\" opacity=\"{}\" stroke-linecap=\"round\">",
                sensor_x, sensor_y, output_x, output_y, color, line_width, opacity
            ));
            // Animate line opacity
            svg.push_str(&format!(
                "<animate attributeName=\"opacity\" values=\"{};{};{}\" dur=\"3s\" repeatCount=\"indefinite\" />",
                opacity * 0.7, opacity, opacity * 0.7
            ));
            svg.push_str("</line>");
        }
    }

    // Draw sensor nodes
    for (i, stat) in model.stats.iter().enumerate() {
        let col = if use_double_column && i >= sensors_per_column {
            1
        } else {
            0
        };
        let row = if use_double_column && i >= sensors_per_column {
            i - sensors_per_column
        } else {
            i
        };

        let sensor_x = if col == 0 { 150.0 } else { 650.0 };
        let sensor_y = start_y + (row as f32 * node_spacing);

        let pulse_mult = 1.0 + 0.15 * (pulse + i as f32 * 0.3).sin();
        let node_radius = (8.0 + stat.dropout_score * 6.0) * pulse_mult;  // Larger nodes
        let node_opacity = 0.7 + 0.3 * stat.dropout_score;

        let node_color = if stat.dropout_score > 0.7 {
            "#7ED321"  // Green - good
        } else if stat.dropout_score > 0.3 {
            "#F8E71C"  // Yellow - okay
        } else {
            "#888888"  // Gray - weak
        };

        // Outer glow ring (animated)
        svg.push_str(&format!(
            "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"2\" opacity=\"0.3\">",
            sensor_x, sensor_y, node_radius * 1.5, node_color
        ));
        svg.push_str(&format!(
            "<animate attributeName=\"r\" values=\"{};{};{}\" dur=\"2s\" repeatCount=\"indefinite\" />",
            node_radius * 1.3, node_radius * 1.7, node_radius * 1.3
        ));
        svg.push_str("<animate attributeName=\"opacity\" values=\"0.5;0.1;0.5\" dur=\"2s\" repeatCount=\"indefinite\" />");
        svg.push_str("</circle>");

        // Main node
        svg.push_str(&format!(
            "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{}\" opacity=\"{}\" filter=\"url(#glow)\" />",
            sensor_x, sensor_y, node_radius, node_color, node_opacity
        ));

        let label = if stat.name.len() > 13 {
            format!("tz{}", stat.name.trim_start_matches("thermal_zone"))
        } else {
            stat.name.clone()
        };

        let text_x = sensor_x + 25.0;
        svg.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" fill=\"#cccccc\" font-size=\"12\" font-family=\"monospace\">{}</text>",
            text_x,
            sensor_y + 4.0,
            label
        ));

        if stat.weight.abs() > 0.01 {
            let weight_x = sensor_x + 25.0;
            svg.push_str(&format!(
                "<text x=\"{}\" y=\"{}\" fill=\"#999999\" font-size=\"10\" font-family=\"monospace\">{:.2}</text>",
                weight_x,
                sensor_y + 15.0,
                stat.weight
            ));
        }
    }

    // Output node (animated with pulse)
    let output_pulse = 18.0 + 4.0 * pulse.sin();
    
    // Output node outer glow
    svg.push_str(&format!(
        "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"url(#pulse-gradient)\" opacity=\"0.4\">",
        output_x, output_y, output_pulse * 1.8
    ));
    svg.push_str(&format!(
        "<animate attributeName=\"r\" values=\"{};{};{}\" dur=\"2s\" repeatCount=\"indefinite\" />",
        output_pulse * 1.5, output_pulse * 2.0, output_pulse * 1.5
    ));
    svg.push_str("</circle>");
    
    // Main output node
    svg.push_str(&format!(
        "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"#F5A623\" stroke=\"#ffffff\" stroke-width=\"3\" filter=\"url(#glow)\"/>",
        output_x, output_y, output_pulse
    ));
    
    svg.push_str(&format!(
        "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" fill=\"#1a1a1a\" font-weight=\"bold\" font-size=\"14\">OUTPUT</text>",
        output_x,
        output_y + 5.0
    ));
    
    svg.push_str(&format!(
        "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" fill=\"#ffffff\" font-size=\"12\" font-weight=\"bold\">Temperature</text>",
        output_x,
        output_y + 30.0
    ));

    // Model statistics panel (glass style)
    let stats_y = 880.0;  // Adjusted for smaller canvas
    let stats_x = 400.0;  // Centered
    
    // Background panel for stats
    svg.push_str(&format!(
        "<rect x=\"{}\" y=\"{}\" width=\"350\" height=\"100\" rx=\"12\" fill=\"rgba(255,255,255,0.05)\" stroke=\"rgba(255,255,255,0.2)\" stroke-width=\"1\"/>",
        stats_x - 175.0, stats_y - 20.0
    ));
    
    svg.push_str(&format!(
        "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" fill=\"#ffffff\" font-size=\"14\" font-weight=\"bold\">Model Statistics</text>",
        stats_x, stats_y + 5.0
    ));
    
    svg.push_str(&format!(
        "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" fill=\"#4A90E2\" font-size=\"12\">Bias: {:.3}</text>",
        stats_x, stats_y + 25.0, model.bias
    ));
    
    svg.push_str(&format!(
        "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" fill=\"#aaaaaa\" font-size=\"11\">Calibrations: {}</text>",
        stats_x, stats_y + 42.0,
        model.calibration_count
    ));

    let active_count = model.stats.iter().filter(|s| s.dropout_score > 0.3).count();
    svg.push_str(&format!(
        "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" fill=\"#7ED321\" font-size=\"11\">Active Sensors: {}/{}</text>",
        stats_x, stats_y + 59.0,
        active_count,
        model.stats.len()
    ));

    svg.push_str("</svg>");
    svg
}

fn generate_history_svg(daily_data: &[(String, f32)]) -> String {
    let width = 800;
    let height = 600;
    let mut svg = format!(
        "<svg width=\"{}\" height=\"{}\" xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {} {}\">",
        width, height, width, height
    );

    svg.push_str("<rect width=\"100%\" height=\"100%\" fill=\"#1a1a1a\"/>");

    if daily_data.is_empty() {
        svg.push_str(
            "<text x=\"400\" y=\"280\" text-anchor=\"middle\" fill=\"#aaaaaa\" font-size=\"24\">No historical data yet</text>",
        );
        svg.push_str(
            "<text x=\"400\" y=\"320\" text-anchor=\"middle\" fill=\"#666666\" font-size=\"18\">Data is saved hourly</text>",
        );
    } else {
        let temps: Vec<f32> = daily_data.iter().map(|(_, t)| *t).collect();
        let max_temp = temps.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));
        let min_temp = temps.iter().fold(f32::INFINITY, |a, &b| a.min(b));
        let temp_range = (max_temp - min_temp).max(5.0);

        let chart_left = 80.0;
        let chart_right = 760.0;
        let chart_top = 60.0;
        let chart_bottom = 480.0;
        let chart_height = chart_bottom - chart_top;

        for i in 0..5 {
            let y = chart_top + (i as f32 * chart_height / 4.0);
            let temp_value = max_temp - (i as f32 * temp_range / 4.0);

            svg.push_str(&format!(
                "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"#2a2a2a\" stroke-width=\"2\" stroke-dasharray=\"6,6\"/>",
                chart_left, y, chart_right, y
            ));

            svg.push_str(&format!(
                "<text x=\"{}\" y=\"{}\" text-anchor=\"end\" fill=\"#888888\" font-size=\"18\">{:.1}°C</text>",
                chart_left - 10.0,
                y + 6.0,
                temp_value
            ));
        }

        let num_bars = daily_data.len().min(7);
        let bar_width = ((chart_right - chart_left) / num_bars as f32 * 0.7).min(76.0);
        let bar_spacing = (chart_right - chart_left) / num_bars as f32;

        for (i, (day, temp)) in daily_data.iter().take(7).rev().enumerate() {
            let x = chart_left + (i as f32 * bar_spacing) + (bar_spacing - bar_width) / 2.0;

            let normalized = (max_temp - temp) / temp_range;
            let bar_top = chart_top + (normalized * chart_height);
            let bar_height = chart_bottom - bar_top;

            let color = if *temp < 10.0 {
                "#4A90E2"
            } else if *temp < 15.0 {
                "#50E3C2"
            } else if *temp < 20.0 {
                "#7ED321"
            } else if *temp < 25.0 {
                "#F8E71C"
            } else if *temp < 30.0 {
                "#F5A623"
            } else {
                "#E24A4A"
            };

            svg.push_str(&format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\" opacity=\"0.85\" rx=\"6\"/>",
                x, bar_top, bar_width, bar_height, color
            ));

            svg.push_str(&format!(
                "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" fill=\"#ffffff\" font-size=\"16\" font-weight=\"bold\">{:.1}°</text>",
                x + bar_width / 2.0,
                bar_top - 8.0,
                temp
            ));

            let day_label = if day.len() >= 5 { &day[day.len() - 5..] } else { day };

            svg.push_str(&format!(
                "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" fill=\"#888888\" font-size=\"16\">{}</text>",
                x + bar_width / 2.0,
                chart_bottom + 30.0,
                day_label
            ));
        }

        let avg_temp: f32 = temps.iter().sum::<f32>() / temps.len() as f32;
        svg.push_str(&format!(
            "<text x=\"400\" y=\"560\" text-anchor=\"middle\" fill=\"#aaaaaa\" font-size=\"18\">Avg: {:.1}°C  |  Range: {:.1}°C - {:.1}°C</text>",
            avg_temp, min_temp, max_temp
        ));
    }

    svg.push_str("</svg>");
    svg
}

/// Extract current device state for temperature modeling
/// 
/// Reads from Linux `/proc` and `/sys` filesystems:
/// - CPU load: derived from /proc/stat (idle vs total ticks delta)
/// - Battery level: /sys/class/power_supply/BAT*/capacity
/// - Charging: /sys/class/power_supply/BAT*/status
/// - Screen brightness: /sys/class/backlight/*/brightness vs max_brightness
/// 
/// Returns sensible defaults if any read fails (non-Linux or restricted filesystem).
pub fn get_device_state() -> DeviceState {
    DeviceState {
        cpu_load: read_cpu_load().unwrap_or(0.0),
        battery_level: read_battery_level().unwrap_or(0.5),
        charging: read_charging_status().unwrap_or(false),
        screen_brightness: read_screen_brightness().unwrap_or(0.5),
    }
}

/// Read CPU load (0.0 to 1.0) from /proc/stat
/// 
/// Uses a static cache of previous tick counts to compute delta over time.
fn read_cpu_load() -> Option<f32> {
    use std::sync::Mutex;
    use std::sync::OnceLock;
    
    static LAST_STAT: OnceLock<Mutex<Option<(u64, u64)>>> = OnceLock::new();
    
    let contents = std::fs::read_to_string("/proc/stat").ok()?;
    let line = contents.lines().next()?;
    if !line.starts_with("cpu ") { return None; }
    
    let parts: Vec<u64> = line.split_whitespace()
        .skip(1)
        .filter_map(|s| s.parse().ok())
        .collect();
    
    if parts.len() < 4 { return None; }
    
    let idle = parts[3] + parts.get(4).copied().unwrap_or(0); // idle + iowait
    let total: u64 = parts.iter().sum();
    
    let cache = LAST_STAT.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().ok()?;
    
    let load = if let Some((prev_idle, prev_total)) = *guard {
        let delta_total = total.saturating_sub(prev_total);
        let delta_idle = idle.saturating_sub(prev_idle);
        if delta_total == 0 {
            0.0
        } else {
            1.0 - (delta_idle as f32 / delta_total as f32)
        }
    } else {
        0.0  // First call - no delta yet
    };
    
    *guard = Some((idle, total));
    Some(load.clamp(0.0, 1.0))
}

/// Read battery level (0.0 to 1.0) from /sys/class/power_supply
fn read_battery_level() -> Option<f32> {
    let entries = std::fs::read_dir("/sys/class/power_supply").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().into_string().ok()?;
        if name.starts_with("BAT") {
            let capacity_path = entry.path().join("capacity");
            if let Ok(s) = std::fs::read_to_string(&capacity_path) {
                if let Ok(pct) = s.trim().parse::<f32>() {
                    return Some((pct / 100.0).clamp(0.0, 1.0));
                }
            }
        }
    }
    None
}

/// Read charging status from /sys/class/power_supply
fn read_charging_status() -> Option<bool> {
    let entries = std::fs::read_dir("/sys/class/power_supply").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().into_string().ok()?;
        if name.starts_with("BAT") || name == "AC" || name.starts_with("ADP") {
            let status_path = entry.path().join("status");
            if let Ok(s) = std::fs::read_to_string(&status_path) {
                let status = s.trim();
                if status == "Charging" || status == "Full" {
                    return Some(true);
                }
            }
            // For AC adapters, "online" file indicates if plugged in
            let online_path = entry.path().join("online");
            if let Ok(s) = std::fs::read_to_string(&online_path) {
                if s.trim() == "1" {
                    return Some(true);
                }
            }
        }
    }
    Some(false)
}

/// Read screen brightness (0.0 to 1.0) from /sys/class/backlight
fn read_screen_brightness() -> Option<f32> {
    let entries = std::fs::read_dir("/sys/class/backlight").ok()?;
    for entry in entries.flatten() {
        let dir = entry.path();
        let current = std::fs::read_to_string(dir.join("brightness")).ok()?;
        let max = std::fs::read_to_string(dir.join("max_brightness")).ok()?;
        
        let cur_val: f32 = current.trim().parse().ok()?;
        let max_val: f32 = max.trim().parse().ok()?;
        
        if max_val > 0.0 {
            return Some((cur_val / max_val).clamp(0.0, 1.0));
        }
    }
    None
}

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
            static mut LAST_SVG_HASH: u64 = 0;
            static mut CACHED_SVG: Option<String> = None;
            
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
            
            unsafe {
                // Only regenerate SVG if model changed
                if model_hash != LAST_SVG_HASH || CACHED_SVG.is_none() {
                    let svg = generate_network_svg(&m_clone, 0.0);
                    CACHED_SVG = Some(svg);
                    LAST_SVG_HASH = model_hash;
                    
                    if let Some(svg) = &CACHED_SVG {
                        let loader = PixbufLoader::new();
                        let _ = loader.write(svg.as_bytes());
                        let _ = loader.close();
                        if let Some(pixbuf) = loader.pixbuf() {
                            model_picture.set_pixbuf(Some(&pixbuf));
                        }
                    }
                }
            }

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
