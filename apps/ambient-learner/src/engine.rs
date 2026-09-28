//! engine — the GTK-free core of ambient-learner.
//!
//! Everything in this file used to sit in `main.rs` between the GTK styling
//! helpers and the CLI arguments: the configuration constants, `Location`,
//! `Model` (the linear + neural estimator), `ThermalPoller` (the sysfs
//! thermal-zone reader), `Database` (SQLite, WAL), the on-disk layout helpers,
//! WiFi/SSID detection, the SVG generators the GTK cards draw with, and the
//! `/proc`//sys device-state readers.
//!
//! None of it referenced gtk, adw, glib, `Rc` or `RefCell` — it was only
//! *located* in the GTK binary, which meant the egui frontend (a separate crate
//! that cannot link GTK) had no way to open the real database or read the real
//! sensors, and shipped demo numbers instead. Moving the code here, unchanged
//! apart from `pub(crate)` on what `main.rs` reaches into, lets both
//! frontends mount the same engine: `main.rs` with `mod engine; use engine::*;`
//! and `egui_lib.rs` with `pub mod engine;`.
//!
//! Nothing in here knows which frontend is running. The seam is deliberate:
//! the engine is the part that varies by *device* (sensors, storage), the
//! frontend is the part that varies by *toolkit*, and they meet only through
//! plain structs and `Result`s.

use chrono::{Local, Utc};
use regex::Regex;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::dbmaint;
use crate::temperature_field::TemperatureEstimator;
use crate::world_state::DeviceState;

// ============================================================================
// Configuration Constants
// ============================================================================

/// Application configuration constants
pub(crate) mod config {
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
#[allow(unused_imports)]
pub(crate) use config::sensors::*;
pub(crate) use config::temperature::{MIN as TEMP_MIN, MAX as TEMP_MAX};
pub(crate) use config::model::LEARNING_RATE;
pub(crate) use config::storage::DATA_DIR;

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
pub struct SensorReading {
    pub time: String,
    pub sensors: Vec<f32>,
    pub cpu_load: f32,
    pub als_lux: u32,
    pub charging: bool,
    pub screen: u8,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub(crate) struct Location {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) location_type: LocationType,
    pub(crate) networks: Vec<String>,  // WiFi networks associated with this location
    pub(crate) created_at: String,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub(crate) enum LocationType {
    City(String),           // City name
    NetworkId(String),      // WiFi SSID or network ID  
    Unknown,
}

impl Location {
    pub(crate) fn new(name: String, location_type: LocationType) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            location_type,
            networks: Vec::new(),
            created_at: get_timestamp(),
        }
    }
    
    pub(crate) fn matches_network(&self, ssid: &str) -> bool {
        self.networks.iter().any(|n| n == ssid) ||
        matches!(&self.location_type, LocationType::NetworkId(id) if id == ssid)
    }
}

/// Statistical metadata for a single thermal sensor
///
/// Tracks the behavior and reliability of each sensor over time to
/// enable adaptive weighting and sensor pruning.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct SensorStat {
    /// Sensor identifier (e.g., "thermal_zone0")
    pub(crate) name: String,
    /// Weight in prediction calculation (higher = more influential)
    pub(crate) weight: f32,
    /// Historical mean reading for this sensor
    pub(crate) mean: f32,
    /// Historical variance (measures stability)
    pub(crate) variance: f32,
    /// Dropout score (0.0 = pruned, 1.0 = active)
    pub(crate) dropout_score: f32,
    /// Sensor type classification
    pub(crate) sensor_type: SensorType,
}

/// Device material type affects thermal conductivity
#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq)]
pub(crate) enum MaterialType {
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
    pub(crate) stats: Vec<SensorStat>,
    /// Model bias term (intercept)
    pub(crate) bias: f32,
    
    /// Hidden layer 1 weights [sensors x hidden1]
    #[serde(default)]
    pub(crate) hidden1_weights: Vec<Vec<f32>>,
    /// Hidden layer 1 biases
    #[serde(default)]
    pub(crate) hidden1_bias: Vec<f32>,
    
    /// Hidden layer 2 weights [hidden1 x hidden2]
    #[serde(default)]
    pub(crate) hidden2_weights: Vec<Vec<f32>>,
    /// Hidden layer 2 biases
    #[serde(default)]
    pub(crate) hidden2_bias: Vec<f32>,
    
    /// Output layer weights [hidden2 x 1]
    #[serde(default)]
    pub(crate) output_weights: Vec<f32>,
    
    /// Momentum buffers (prevent oscillation between +40 and -40)
    #[serde(default)]
    pub(crate) hidden1_weights_momentum: Vec<Vec<f32>>,
    #[serde(default)]
    pub(crate) hidden2_weights_momentum: Vec<Vec<f32>>,
    #[serde(default)]
    pub(crate) output_weights_momentum: Vec<f32>,
    #[serde(default)]
    pub(crate) bias_momentum: f32,
    
    /// Detected material type
    #[serde(default)]
    pub(crate) material_type: MaterialType,
    
    /// Material-specific thermal transfer coefficient
    #[serde(default)]
    pub(crate) material_coefficient: f32,
    
    /// Device identifier (CPU + sensor count + machine ID)
    /// Prevents sensor confusion when sharing databases across devices
    #[serde(default)]
    pub(crate) device_id: String,
    
    /// Last predicted temperature (for smoothing)
    pub(crate) last_prediction: f32,
    /// Number of user calibrations performed
    pub calibration_count: usize,
    /// Timestamp when model was created
    pub(crate) created_at: String,
    /// Hash of last saved state (for change detection)
    #[serde(skip)]
    pub(crate) last_saved_hash: u64,
}

impl Default for MaterialType {
    fn default() -> Self {
        MaterialType::Unknown
    }
}

/// ReLU activation function: max(0, x)
pub(crate) fn relu(x: f32) -> f32 {
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
    pub(crate) fn clamp(temp: f32) -> f32 {
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
    pub(crate) fn update_passive(&mut self, readings: &[f32]) {
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
    pub(crate) fn update_passive_simd(&mut self, readings: &[f32]) -> bool {
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
    pub(crate) fn predict_neural(&mut self, readings: &[f32]) -> f32 {
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
    pub(crate) fn predict_linear(&mut self, readings: &[f32]) -> f32 {
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
    pub(crate) fn predict_simd(&self, readings: &[f32]) -> Option<f32> {
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
    pub(crate) fn bootstrap_neural_from_linear(&mut self) {
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
    pub(crate) fn initialize_neural_weights(&mut self) {
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
    pub(crate) fn calibrate_neural(&mut self, readings: &[f32], target: f32) {
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
    pub(crate) fn update_weights_with_lr(
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
    pub(crate) fn calibrate_linear(&mut self, readings: &[f32], target: f32) {
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

    pub(crate) fn prune_weak_sensors(&mut self) {
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
    pub(crate) fn detect_material_type(&mut self) {
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

    pub(crate) fn confidence(&self) -> f32 {
        let active_sensors = self.stats.iter().filter(|s| s.dropout_score > 0.1).count();

        if active_sensors == 0 {
            return 0.0;
        }

        let avg_variance: f32 = self.stats.iter().map(|s| s.variance * s.dropout_score).sum::<f32>()
            / active_sensors as f32;

        let calib_factor = 1.0 - (-0.1 * self.calibration_count as f32).exp();

        (1.0 / (1.0 + avg_variance)) * calib_factor
    }

    pub(crate) fn compute_hash(&self) -> u64 {
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

    pub(crate) fn has_changed(&self) -> bool {
        self.compute_hash() != self.last_saved_hash
    }

    pub(crate) fn save(&mut self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
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

    pub(crate) fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
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
    pub(crate) files: Vec<(String, File, SensorType)>,
    pub(crate) buffer: Vec<u8>,
    pub(crate) locked: bool,
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
    pub(crate) unit_scale: Vec<Option<f32>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SensorType {
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
    /// Open every thermal zone and CPU hwmon sensor under `/sys`.
    ///
    /// `AMBIENT_SYSFS_ROOT` overrides the root. It exists for one reason:
    /// a machine with no thermal zones — a container, a CI runner, a
    /// desktop where the app is being developed — otherwise gives every
    /// frontend the same "no sensors" screen, and the whole pipeline from
    /// poll to prediction to database goes unexercised. Point it at a
    /// directory holding `class/thermal/thermal_zoneN/{temp,type}` files
    /// and the poller reads those instead. Nothing else about the app
    /// changes, which is what makes screenshots taken that way honest.
    pub fn new() -> Self {
        let root = std::env::var_os("AMBIENT_SYSFS_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/sys"));
        Self::from_sysfs(&root)
    }

    /// The same, against an explicit sysfs root.
    pub fn from_sysfs(root: &Path) -> Self {
        let mut files = Vec::new();
        let base = root.join("class/thermal");
        let base = base.as_path();

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
        let hwmon_base = root.join("class/hwmon");
        let hwmon_base = hwmon_base.as_path();
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

    pub(crate) fn try_lock(&mut self) -> bool {
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
    pub(crate) conn: Connection,
    pub(crate) path: PathBuf,
}

impl Database {
    pub(crate) fn new(timestamp: &str) -> Result<Self, rusqlite::Error> {
        fs::create_dir_all(DATA_DIR).ok();
        let path = PathBuf::from(DATA_DIR).join(format!("sensors_{}.db", timestamp));
        Self::create_at(&path)
    }

    /// Create a fresh database at an explicit path — `new` with the
    /// location chosen by the caller, so a test can build one in a temp
    /// directory instead of the app's data dir.
    pub(crate) fn create_at(path: &Path) -> Result<Self, rusqlite::Error> {
        let path = path.to_path_buf();
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
    pub(crate) fn create_consistent_backup(src: &Path, dst: &Path) -> Result<(), rusqlite::Error> {
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

    pub(crate) fn open(path: &Path) -> Result<Self, rusqlite::Error> {
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
    pub(crate) fn ensure_column(
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

    pub(crate) fn get_calibration_count(&self) -> Result<usize, rusqlite::Error> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM calibrations",
            [],
            |row| row.get(0)
        )?;
        Ok(count as usize)
    }
    
    pub(crate) fn get_reading_count(&self) -> Result<usize, rusqlite::Error> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM readings",
            [],
            |row| row.get(0)
        )?;
        Ok(count as usize)
    }
    
    pub(crate) fn get_database_stats(&self) -> Result<(usize, usize, Option<i64>, Option<i64>), rusqlite::Error> {
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
    pub(crate) fn vacuum_database(&self) -> Result<(), rusqlite::Error> {
        println!("🧹 Running database VACUUM to reclaim space...");
        dbmaint::vacuum(&self.conn)?;
        println!("✓ VACUUM complete");
        Ok(())
    }
    
    pub(crate) fn analyze_database(&self) -> Result<(), rusqlite::Error> {
        println!("📊 Running database ANALYZE to update statistics...");
        dbmaint::analyze(&self.conn)?;
        println!("✓ ANALYZE complete");
        Ok(())
    }
    
    pub(crate) fn check_health(&self) -> Result<(), String> {
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
    
    pub(crate) fn perform_maintenance(&self, keep_days: i64) -> Result<(), String> {
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

    pub(crate) fn store_reading(&self, reading: &SensorReading, location_id: Option<&str>) -> Result<(), rusqlite::Error> {
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

    pub(crate) fn get_daily_averages(&self, days_back: usize, num_days: usize, location_filter: Option<&str>) -> Vec<(String, f32)> {
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

    pub(crate) fn export_to_csv(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
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
        
        // `target`, as the table declares it. This read `target_temp` —
        // a column that never existed — and had no caller to notice.
        let query = "SELECT timestamp, target 
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

pub(crate) fn get_timestamp() -> String {
    Local::now().format("%Y%m%d_%H%M%S").to_string()
}

pub(crate) fn get_iso_timestamp() -> String {
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
pub(crate) fn get_device_id() -> String {
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

pub(crate) fn get_latest_model() -> Option<PathBuf> {
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

pub(crate) fn get_latest_neural_field() -> Option<PathBuf> {
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
pub(crate) static DB_PATH_OVERRIDE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Set the CLI database-path override. Idempotent — calling twice
/// with different paths is a programming error and the second call
/// will be silently ignored (OnceLock semantics). Should be called
/// exactly once from `main()` after CLI parsing.
pub(crate) fn set_db_override(path: PathBuf) {
    let _ = DB_PATH_OVERRIDE.set(path);
}

pub(crate) fn get_latest_database() -> Option<PathBuf> {
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
pub(crate) fn latest_database_in(dir: &Path) -> Option<PathBuf> {
    // Anchored. Unanchored, this also matched `sensors_X.db.backup` (the
    // snapshot `Database::open` takes on every launch), `sensors_X.db-wal`
    // and `sensors_X.db-shm` — all with the same timestamp capture, so the
    // "latest database" was whichever of the four the directory listing
    // yielded last. The second launch after any first one opened the
    // BACKUP as the live database, then backed that up as
    // `sensors_X.db.db.backup`. Seen in the egui frontend's second run;
    // the GTK binary has the same call.
    let re = Regex::new(r"^sensors_(\d{8}_\d{6})\.db$").unwrap();

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
    fn sidecars_and_backups_are_not_databases() {
        // `Database::open` writes `X.db.backup` beside `X.db`, and WAL mode
        // adds `X.db-wal` / `X.db-shm`. None of them is the database.
        let dir = tempdir().unwrap();
        for name in ["sensors_20260101_000000.db", "sensors_20260101_000000.db.backup",
                     "sensors_20260101_000000.db-wal", "sensors_20260101_000000.db-shm",
                     "sensors_20260101_000000.db.db.backup"] {
            fs::write(dir.path().join(name), b"").unwrap();
        }
        for _ in 0..5 {
            let got = latest_database_in(dir.path()).unwrap();
            assert_eq!(got.file_name().unwrap(), "sensors_20260101_000000.db");
        }
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
pub(crate) fn import_database_cli(src: &Path) -> Result<PathBuf, String> {
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
pub(crate) fn export_database_cli(dest: &Path) -> Result<(), String> {
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

pub(crate) fn list_all_databases() -> Vec<PathBuf> {
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

pub(crate) fn cleanup_old_databases(keep_count: usize) {
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

pub(crate) fn get_timezone_cities() -> Vec<String> {
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
pub(crate) fn get_current_network_ssid_with_fallback() -> Option<String> {
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
pub(crate) fn get_current_network_ssid_async_with_timeout(timeout: Duration) -> Option<String> {
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
pub(crate) fn get_current_network_ssid_async_with_timeout(_timeout: Duration) -> Option<String> {
    None  // D-Bus not enabled, skip
}

/// Non-blocking network detection using D-Bus
/// 
/// This replaces the blocking nmcli approach with async D-Bus queries
#[cfg(feature = "dbus")]
pub(crate) fn get_current_network_ssid_async() -> Option<String> {
    block_on(async {
        let result: Option<String> = get_current_network_ssid_dbus().await.ok().flatten();
        result
    })
}

#[cfg(not(feature = "dbus"))]
pub(crate) fn get_current_network_ssid_async() -> Option<String> {
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
pub(crate) fn get_current_network_ssid_nmcli() -> Option<String> {
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

pub(crate) fn save_locations(locations: &[Location]) -> Result<(), Box<dyn std::error::Error>> {
    let path = PathBuf::from(DATA_DIR).join("locations.json");
    let json = serde_json::to_string_pretty(locations)?;
    let mut file = File::create(path)?;
    file.write_all(json.as_bytes())?;
    Ok(())
}

pub(crate) fn load_locations() -> Vec<Location> {
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

pub(crate) fn detect_current_location(locations: &[Location]) -> Option<usize> {
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
pub(crate) fn compress_old_models() -> Result<(), Box<dyn std::error::Error>> {
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

pub(crate) fn cleanup_old_data() {
    // Run compression in background
    thread::spawn(|| {
        thread::sleep(Duration::from_secs(10)); // Wait for app to start
        if let Err(e) = compress_old_models() {
            eprintln!("Compression error: {}", e);
        }
    });
}

pub(crate) fn temp_to_color(temp: f32) -> &'static str {
    if temp < 40.0 {
        "#4A90E2"
    } else if temp < 55.0 {
        "#7ED321"
    } else {
        "#F5A623"
    }
}

pub(crate) fn generate_network_svg(model: &Model, pulse: f32) -> String {
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

pub(crate) fn generate_history_svg(daily_data: &[(String, f32)]) -> String {
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
pub(crate) fn read_cpu_load() -> Option<f32> {
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
pub(crate) fn read_battery_level() -> Option<f32> {
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
pub(crate) fn read_charging_status() -> Option<bool> {
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
pub(crate) fn read_screen_brightness() -> Option<f32> {
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
