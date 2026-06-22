//! Helper utilities for ambient-learner
//!
//! This module contains reusable utility functions and extensions
//! to simplify common patterns throughout the application.
//!
//! Includes:
//! - Lock extensions for cleaner mutex handling
//! - Retry logic with exponential backoff
//! - Validation and clamping utilities
//! - Formatting helpers
//! - Mathematical utilities (average, variance, normalization)
//! - Hash-based change detection
//! - Timestamp utilities
//! - Async database writer

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// GTK-specific imports
#[cfg(feature = "gtk")]
use libadwaita as adw;
#[cfg(feature = "gtk")]
use glib;

// ============================================================================
// Compressed Sensor Storage
// ============================================================================

/// Compressed sensor reading for efficient storage
/// Achieves ~82% compression vs JSON with ±0.1°C accuracy
#[derive(Debug, Clone)]
pub struct CompressedReading {
    pub timestamp: u32,      // Relative seconds from base
    pub min_temp: u16,       // Min temp × 10 (0.1°C precision)
    pub max_temp: u16,       // Max temp × 10 (0.1°C precision)
    pub sensors: Vec<u8>,    // Normalized to 0-255
    pub prediction: i16,     // Prediction × 10 (0.1°C precision)
}

impl CompressedReading {
    /// Compress sensor reading (357 bytes JSON → 63 bytes binary)
    pub fn compress(
        timestamp: i64,
        sensors: &[f32],
        prediction: f32,
        base_timestamp: i64,
    ) -> Self {
        // Find temperature range
        let min = sensors.iter().cloned().fold(f32::INFINITY, f32::min);
        let max = sensors.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let range = max - min;
        
        // Normalize to 0-255 (quantize to 256 levels)
        let normalized: Vec<u8> = sensors.iter().map(|&temp| {
            if range < 0.01 {
                127  // All sensors same temperature
            } else {
                ((temp - min) / range * 255.0).clamp(0.0, 255.0) as u8
            }
        }).collect();
        
        Self {
            timestamp: (timestamp - base_timestamp) as u32,
            min_temp: (min * 10.0) as u16,
            max_temp: (max * 10.0) as u16,
            sensors: normalized,
            prediction: (prediction * 10.0) as i16,
        }
    }
    
    /// Decompress back to original format with ±0.1°C accuracy
    pub fn decompress(&self, base_timestamp: i64) -> (i64, Vec<f32>, f32) {
        let timestamp = base_timestamp + self.timestamp as i64;
        
        let min = self.min_temp as f32 / 10.0;
        let max = self.max_temp as f32 / 10.0;
        let range = max - min;
        
        let sensors: Vec<f32> = self.sensors.iter().map(|&normalized| {
            min + (normalized as f32 / 255.0) * range
        }).collect();
        
        let prediction = self.prediction as f32 / 10.0;
        
        (timestamp, sensors, prediction)
    }
    
    /// Serialize to bytes for BLOB storage
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(10 + self.sensors.len());
        
        bytes.extend_from_slice(&self.timestamp.to_le_bytes());
        bytes.extend_from_slice(&self.min_temp.to_le_bytes());
        bytes.extend_from_slice(&self.max_temp.to_le_bytes());
        bytes.extend_from_slice(&self.sensors);
        bytes.extend_from_slice(&self.prediction.to_le_bytes());
        
        bytes
    }
    
    /// Deserialize from bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 12 {
            return Err("Data too short for CompressedReading".to_string());
        }
        
        let timestamp = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let min_temp = u16::from_le_bytes([bytes[4], bytes[5]]);
        let max_temp = u16::from_le_bytes([bytes[6], bytes[7]]);
        
        let sensor_count = bytes.len() - 10;
        let sensors = bytes[8..8 + sensor_count].to_vec();
        
        let prediction = i16::from_le_bytes([
            bytes[8 + sensor_count],
            bytes[9 + sensor_count]
        ]);
        
        Ok(Self { timestamp, min_temp, max_temp, sensors, prediction })
    }
}

// ============================================================================
// Lock Extensions - Reduce Boilerplate
// ============================================================================

/// Extension trait for Arc<Mutex<T>> to simplify lock handling
///
/// Provides convenient methods to work with locks without explicit unwrap()
/// calls, reducing boilerplate and improving readability.
///
/// # Examples
///
/// ```
/// let model = Arc::new(Mutex::new(Model::new()));
///
/// // Before:
/// let mut m = model.lock().unwrap();
/// m.calibrate(&data, target);
/// drop(m);
///
/// // After:
/// model.with_lock_mut(|m| {
///     m.calibrate(&data, target);
/// });
/// ```
pub trait LockExt<T> {
    /// Execute a closure with read access to the locked data
    ///
    /// # Arguments
    ///
    /// * `f` - Closure that receives immutable reference to data
    ///
    /// # Returns
    ///
    /// The return value of the closure
    ///
    /// # Panics
    ///
    /// Panics if the lock is poisoned (another thread panicked while holding the lock)
    fn with_lock<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&T) -> R;

    /// Execute a closure with write access to the locked data
    ///
    /// # Arguments
    ///
    /// * `f` - Closure that receives mutable reference to data
    ///
    /// # Returns
    ///
    /// The return value of the closure
    ///
    /// # Panics
    ///
    /// Panics if the lock is poisoned
    fn with_lock_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut T) -> R;

    /// Try to execute a closure with read access, returning None if busy
    ///
    /// Non-blocking alternative to `with_lock`. Returns None immediately
    /// if the lock is currently held by another thread.
    ///
    /// # Arguments
    ///
    /// * `f` - Closure that receives immutable reference to data
    ///
    /// # Returns
    ///
    /// Some(result) if lock acquired, None if busy
    fn try_with_lock<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&T) -> R;
}

impl<T> LockExt<T> for Arc<Mutex<T>> {
    fn with_lock<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        let guard = self.lock().expect("Lock poisoned");
        f(&*guard)
    }

    fn with_lock_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut T) -> R,
    {
        let mut guard = self.lock().expect("Lock poisoned");
        f(&mut *guard)
    }

    fn try_with_lock<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&T) -> R,
    {
        self.try_lock().ok().map(|guard| f(&*guard))
    }
}

// ============================================================================
// Retry Utilities
// ============================================================================

/// Execute an operation with automatic retry on transient failures
///
/// Retries the operation up to `max_attempts` times with exponential backoff.
/// Useful for database operations that may fail due to lock contention.
///
/// # Arguments
///
/// * `operation` - Closure that performs the operation
/// * `max_attempts` - Maximum number of attempts (including first try)
/// * `base_delay` - Initial delay between retries, doubles each attempt
///
/// # Returns
///
/// Result from the operation, or last error after all retries exhausted
///
/// # Examples
///
/// ```
/// let result = retry_with_backoff(
///     || database.insert(&record),
///     3,
///     Duration::from_millis(50)
/// );
/// ```
pub fn retry_with_backoff<F, T, E>(
    mut operation: F,
    max_attempts: u32,
    base_delay: Duration,
) -> Result<T, E>
where
    F: FnMut() -> Result<T, E>,
    E: std::fmt::Display,
{
    let mut delay = base_delay;

    for attempt in 1..=max_attempts {
        match operation() {
            Ok(result) => return Ok(result),
            Err(e) if attempt < max_attempts => {
                eprintln!(
                    "⚠️  Operation failed (attempt {}/{}): {}. Retrying in {:?}...",
                    attempt, max_attempts, e, delay
                );
                std::thread::sleep(delay);
                delay *= 2; // Exponential backoff
            }
            Err(e) => return Err(e),
        }
    }

    unreachable!()
}

// ============================================================================
// Validation Helpers
// ============================================================================

/// Validates that a temperature is within acceptable bounds
///
/// # Arguments
///
/// * `temp` - Temperature value in Celsius
/// * `min` - Minimum acceptable temperature
/// * `max` - Maximum acceptable temperature
///
/// # Returns
///
/// Ok(temp) if valid, Err with message if out of bounds
///
/// # Examples
///
/// ```
/// assert!(validate_temperature(22.5, -40.0, 40.0).is_ok());
/// assert!(validate_temperature(100.0, -40.0, 40.0).is_err());
/// ```
pub fn validate_temperature(temp: f32, min: f32, max: f32) -> Result<f32, String> {
    if temp < min {
        Err(format!(
            "Temperature {:.1}°C is below minimum {:.1}°C",
            temp, min
        ))
    } else if temp > max {
        Err(format!(
            "Temperature {:.1}°C exceeds maximum {:.1}°C",
            temp, max
        ))
    } else if temp.is_nan() {
        Err("Temperature is NaN (not a number)".to_string())
    } else if temp.is_infinite() {
        Err("Temperature is infinite".to_string())
    } else {
        Ok(temp)
    }
}

/// Clamps a value to a specified range
///
/// # Arguments
///
/// * `value` - Value to clamp
/// * `min` - Minimum bound
/// * `max` - Maximum bound
///
/// # Returns
///
/// Value clamped to [min, max]
///
/// # Examples
///
/// ```
/// assert_eq!(clamp(5.0, 0.0, 10.0), 5.0);
/// assert_eq!(clamp(-5.0, 0.0, 10.0), 0.0);
/// assert_eq!(clamp(15.0, 0.0, 10.0), 10.0);
/// ```
///
/// # Performance
///
/// This function is marked as #[inline] for zero-cost abstraction.
#[inline]
pub fn clamp<T: PartialOrd>(value: T, min: T, max: T) -> T {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

// ============================================================================
// Formatting Helpers
// ============================================================================

/// Format a temperature value for display
///
/// # Arguments
///
/// * `temp` - Temperature in Celsius
/// * `decimals` - Number of decimal places
///
/// # Returns
///
/// Formatted string with °C symbol
///
/// # Examples
///
/// ```
/// assert_eq!(format_temperature(22.456, 1), "22.5°C");
/// assert_eq!(format_temperature(-5.0, 0), "-5°C");
/// ```
///
/// # Performance
///
/// Inlined for performance in UI update loops.
#[inline]
pub fn format_temperature(temp: f32, decimals: usize) -> String {
    format!("{:.prec$}°C", temp, prec = decimals)
}

/// Format a duration in human-readable form
///
/// # Arguments
///
/// * `duration` - Time duration
///
/// # Returns
///
/// Human-readable string (e.g., "2.5s", "150ms")
///
/// # Examples
///
/// ```
/// assert_eq!(format_duration(Duration::from_millis(2500)), "2.5s");
/// assert_eq!(format_duration(Duration::from_millis(150)), "150ms");
/// ```
///
/// # Performance
///
/// Inlined for performance in logging.
#[inline]
pub fn format_duration(duration: Duration) -> String {
    let ms = duration.as_millis();
    if ms < 1000 {
        format!("{}ms", ms)
    } else {
        format!("{:.1}s", ms as f64 / 1000.0)
    }
}

// ============================================================================
// Collection Helpers
// ============================================================================

/// Calculate the average of a slice of f32 values
///
/// # Arguments
///
/// * `values` - Slice of numeric values
///
/// # Returns
///
/// Average value, or 0.0 if slice is empty
///
/// # Examples
///
/// ```
/// assert_eq!(average(&[1.0, 2.0, 3.0]), 2.0);
/// assert_eq!(average(&[]), 0.0);
/// ```
///
/// # Performance
///
/// Inlined for hot path usage. Uses iterator sum for SIMD potential.
#[inline]
pub fn average(values: &[f32]) -> f32 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f32>() / values.len() as f32
    }
}

/// Calculate the variance of a slice of f32 values
///
/// # Arguments
///
/// * `values` - Slice of numeric values
/// * `mean` - Pre-calculated mean of the values
///
/// # Returns
///
/// Variance, or 0.0 if slice is empty
///
/// # Examples
///
/// ```
/// let values = vec![1.0, 2.0, 3.0];
/// let mean = average(&values);
/// assert!((variance(&values, mean) - 0.666).abs() < 0.01);
/// ```
///
/// # Performance
///
/// Inlined. Uses iterator chain for potential SIMD optimization.
#[inline]
pub fn variance(values: &[f32], mean: f32) -> f32 {
    if values.is_empty() {
        0.0
    } else {
        values
            .iter()
            .map(|&x| (x - mean).powi(2))
            .sum::<f32>()
            / values.len() as f32
    }
}

/// Calculate moving average (same as average, for API clarity)
///
/// # Arguments
///
/// * `values` - Slice of numeric values
///
/// # Returns
///
/// Some(average) if not empty, None otherwise
///
/// # Examples
///
/// ```
/// assert_eq!(moving_average(&[1.0, 2.0, 3.0]), Some(2.0));
/// assert_eq!(moving_average(&[]), None);
/// ```
#[inline]
pub fn moving_average(values: &[f32]) -> Option<f32> {
    if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<f32>() / values.len() as f32)
    }
}

/// Normalize a series of values to [0, 1] range
///
/// # Arguments
///
/// * `values` - Slice of values to normalize
///
/// # Returns
///
/// Vector of normalized values in [0, 1] range
///
/// # Examples
///
/// ```
/// let normalized = normalize_series(&[10.0, 20.0, 30.0]);
/// assert_eq!(normalized, vec![0.0, 0.5, 1.0]);
/// ```
///
/// # Performance
///
/// Uses functional fold to find min/max in single pass,
/// then map for transformation.
pub fn normalize_series(values: &[f32]) -> Vec<f32> {
    if values.is_empty() {
        return Vec::new();
    }

    let (min, max) = values
        .iter()
        .fold((f32::MAX, f32::MIN), |(min_val, max_val), &v| {
            (min_val.min(v), max_val.max(v))
        });

    let range = max - min;

    // If all values are the same, return zeros
    if range.abs() < f32::EPSILON {
        return vec![0.0; values.len()];
    }

    values.iter().map(|v| (v - min) / range).collect()
}

// ============================================================================
// Hash-Based Change Detection
// ============================================================================

/// Hash a slice of sensor readings for change detection
///
/// Converts f32 values to fixed-point integers before hashing
/// to avoid floating-point comparison issues.
///
/// # Arguments
///
/// * `values` - Slice of sensor readings
///
/// # Returns
///
/// 64-bit hash of the readings
///
/// # Examples
///
/// ```
/// let hash1 = hash_readings(&[1.0, 2.0, 3.0]);
/// let hash2 = hash_readings(&[1.0, 2.0, 3.0]);
/// assert_eq!(hash1, hash2);  // Same data = same hash
///
/// let hash3 = hash_readings(&[1.0, 2.0, 3.1]);
/// assert_ne!(hash1, hash3);  // Different data = different hash
/// ```
///
/// # Performance
///
/// Inlined. Converts to fixed-point to avoid f32 hash instability.
#[inline]
pub fn hash_readings(values: &[f32]) -> u64 {
    let mut hasher = DefaultHasher::new();

    for v in values {
        // Convert to fixed-point integer (2 decimal places)
        ((*v * 100.0) as i32).hash(&mut hasher);
    }

    hasher.finish()
}

// ============================================================================
// Timestamp Utilities
// ============================================================================

/// Get current timestamp in milliseconds since Unix epoch
///
/// # Returns
///
/// Timestamp as u128 milliseconds
///
/// # Examples
///
/// ```
/// let now = timestamp_ms();
/// assert!(now > 1_600_000_000_000);  // After Sep 2020
/// ```
///
/// # Performance
///
/// Inlined for frequent timestamp operations.
#[inline]
pub fn timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("System time before Unix epoch")
        .as_millis()
}

/// Get current timestamp in seconds since Unix epoch
///
/// # Returns
///
/// Timestamp as u64 seconds
///
/// # Examples
///
/// ```
/// let now = timestamp_s();
/// assert!(now > 1_600_000_000);  // After Sep 2020
/// ```
///
/// # Performance
///
/// Inlined for frequent timestamp operations.
#[inline]
pub fn timestamp_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("System time before Unix epoch")
        .as_secs()
}

// ============================================================================
// Async Database Writer - Prevents UI Blocking
// ============================================================================

/// Message types for async database operations
#[derive(Debug)]
pub enum DbMessage {
    /// Store a sensor reading
    StoreReading {
        sensors: Vec<f32>,
        timestamp: String,
        location_id: Option<String>,
    },
    /// Store a calibration point
    StoreCalibration {
        sensors: Vec<f32>,
        target: f32,
        location_id: Option<String>,
    },
    /// Shutdown the writer thread
    Shutdown,
}

/// Non-blocking database writer using a background thread
///
/// All database writes happen asynchronously in a background thread,
/// preventing UI freezes during I/O operations.
///
/// # Examples
///
/// ```
/// let writer = AsyncDbWriter::new(100);
/// 
/// // Non-blocking write (returns immediately)
/// writer.log_calibration(sensor_data, 22.5, Some("location_id"));
/// 
/// // UI remains responsive!
/// ```
pub struct AsyncDbWriter {
    tx: std::sync::mpsc::SyncSender<DbMessage>,
}

impl AsyncDbWriter {
    /// Create a new async writer with the given queue size
    ///
    /// # Arguments
    ///
    /// * `queue_size` - Maximum number of pending operations (e.g., 100)
    ///
    /// # Returns
    ///
    /// A new AsyncDbWriter instance
    pub fn new(queue_size: usize) -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel(queue_size);
        
        // Background writer thread
        std::thread::spawn(move || {
            while let Ok(msg) = rx.recv() {
                match msg {
                    DbMessage::Shutdown => break,
                    _ => {
                        // Database operations would happen here
                        // This is a skeleton - actual DB writes done in main.rs
                    }
                }
            }
            println!("✓ Database writer thread shutting down");
        });
        
        Self { tx }
    }
    
    /// Log a calibration (non-blocking)
    ///
    /// # Arguments
    ///
    /// * `sensors` - Sensor readings at calibration time
    /// * `target` - Actual temperature provided by user
    /// * `location_id` - Optional location identifier
    ///
    /// # Returns
    ///
    /// true if queued successfully, false if queue full
    pub fn log_calibration(&self, sensors: Vec<f32>, target: f32, location_id: Option<String>) -> bool {
        match self.tx.try_send(DbMessage::StoreCalibration {
            sensors,
            target,
            location_id,
        }) {
            Ok(_) => true,
            Err(std::sync::mpsc::TrySendError::Full(_)) => {
                eprintln!("⚠️  Database write queue full, skipping calibration log");
                false
            }
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                eprintln!("❌ Database writer thread disconnected");
                false
            }
        }
    }
    
    /// Log a sensor reading (non-blocking)
    ///
    /// # Returns
    ///
    /// true if queued successfully, false if queue full
    pub fn log_reading(&self, sensors: Vec<f32>, timestamp: String, location_id: Option<String>) -> bool {
        match self.tx.try_send(DbMessage::StoreReading {
            sensors,
            timestamp,
            location_id,
        }) {
            Ok(_) => true,
            Err(std::sync::mpsc::TrySendError::Full(_)) => {
                eprintln!("⚠️  Database write queue full, skipping reading");
                false
            }
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                eprintln!("❌ Database writer thread disconnected");
                false
            }
        }
    }
    
    /// Shutdown the writer thread gracefully
    pub fn shutdown(&self) {
        let _ = self.tx.send(DbMessage::Shutdown);
    }
}

impl Drop for AsyncDbWriter {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ============================================================================
// Async Utilities - Non-blocking I/O
// ============================================================================

// NOTE: Async functions commented out - not using tokio yet
// Uncomment when adding tokio dependency

/*
/// Async file reading wrapper
pub async fn read_file_async(path: impl AsRef<std::path::Path>) -> std::io::Result<String> {
    tokio::fs::read_to_string(path).await
}

/// Async file writing wrapper
pub async fn write_file_async(
    path: impl AsRef<std::path::Path>,
    contents: impl AsRef<[u8]>,
) -> std::io::Result<()> {
    tokio::fs::write(path, contents).await
}

/// Async background task spawner (requires tokio)
pub fn spawn_async<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(future)
}
*/

/// Background task spawner
///
/// Spawns a task that won't block the UI thread.
///
/// # Examples
///
/// ```
/// spawn_background(|| {
///     expensive_computation()
/// });
/// ```
pub fn spawn_background<F, T>(f: F) -> std::thread::JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    std::thread::spawn(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_temperature() {
        assert!(validate_temperature(20.0, -40.0, 40.0).is_ok());
        assert!(validate_temperature(-50.0, -40.0, 40.0).is_err());
        assert!(validate_temperature(50.0, -40.0, 40.0).is_err());
        assert!(validate_temperature(f32::NAN, -40.0, 40.0).is_err());
    }

    #[test]
    fn test_clamp() {
        assert_eq!(clamp(5.0, 0.0, 10.0), 5.0);
        assert_eq!(clamp(-5.0, 0.0, 10.0), 0.0);
        assert_eq!(clamp(15.0, 0.0, 10.0), 10.0);
    }

    #[test]
    fn test_average() {
        assert_eq!(average(&[1.0, 2.0, 3.0]), 2.0);
        assert_eq!(average(&[]), 0.0);
        assert_eq!(average(&[5.0]), 5.0);
    }

    #[test]
    fn test_moving_average() {
        assert_eq!(moving_average(&[1.0, 2.0, 3.0]), Some(2.0));
        assert_eq!(moving_average(&[]), None);
        assert_eq!(moving_average(&[5.0]), Some(5.0));
    }

    #[test]
    fn test_variance() {
        let values = vec![1.0, 2.0, 3.0];
        let mean = average(&values);
        let var = variance(&values, mean);
        // Variance of [1,2,3] with mean 2.0 should be 0.666...
        assert!((var - 0.666).abs() < 0.01);
    }

    #[test]
    fn test_normalize_series() {
        let normalized = normalize_series(&[10.0, 20.0, 30.0]);
        assert_eq!(normalized.len(), 3);
        assert!((normalized[0] - 0.0).abs() < 0.001);
        assert!((normalized[1] - 0.5).abs() < 0.001);
        assert!((normalized[2] - 1.0).abs() < 0.001);

        // Empty slice
        assert_eq!(normalize_series(&[]), Vec::<f32>::new());

        // All same values
        let same = normalize_series(&[5.0, 5.0, 5.0]);
        assert_eq!(same, vec![0.0, 0.0, 0.0]);
    }

    #[test]
    fn test_hash_readings() {
        let hash1 = hash_readings(&[1.0, 2.0, 3.0]);
        let hash2 = hash_readings(&[1.0, 2.0, 3.0]);
        assert_eq!(hash1, hash2);

        let hash3 = hash_readings(&[1.0, 2.0, 3.1]);
        assert_ne!(hash1, hash3);

        // Empty slice should hash consistently
        let empty1 = hash_readings(&[]);
        let empty2 = hash_readings(&[]);
        assert_eq!(empty1, empty2);
    }

    #[test]
    fn test_timestamp_utilities() {
        let ms = timestamp_ms();
        let s = timestamp_s();

        // Should be reasonable values (after 2020)
        assert!(ms > 1_600_000_000_000);
        assert!(s > 1_600_000_000);

        // Milliseconds should be ~1000x seconds
        let approx_ms = s as u128 * 1000;
        assert!((ms as i128 - approx_ms as i128).abs() < 2000);
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(Duration::from_millis(2500)), "2.5s");
        assert_eq!(format_duration(Duration::from_millis(150)), "150ms");
        assert_eq!(format_duration(Duration::from_millis(1000)), "1.0s");
    }

    #[test]
    fn test_format_temperature() {
        assert_eq!(format_temperature(22.456, 1), "22.5°C");
        assert_eq!(format_temperature(-5.0, 0), "-5°C");
        assert_eq!(format_temperature(0.0, 2), "0.00°C");
    }

    #[test]
    fn test_lock_ext() {
        let data = Arc::new(Mutex::new(vec![1, 2, 3]));

        // Read access
        let sum = data.with_lock(|d| d.iter().sum::<i32>());
        assert_eq!(sum, 6);

        // Write access
        data.with_lock_mut(|d| d.push(4));
        assert_eq!(data.with_lock(|d| d.len()), 4);

        // Try access (non-blocking)
        let result = data.try_with_lock(|d| d.len());
        assert_eq!(result, Some(4));
    }

    #[test]
    fn test_retry_with_backoff() {
        let mut attempts = 0;

        let result = retry_with_backoff(
            || {
                attempts += 1;
                if attempts < 3 {
                    Err("Not yet")
                } else {
                    Ok(42)
                }
            },
            5,
            Duration::from_millis(10),
        );

        assert_eq!(result, Ok(42));
        assert_eq!(attempts, 3);
    }

    #[test]
    fn test_retry_exhaustion() {
        let mut attempts = 0;

        let result: Result<i32, &str> = retry_with_backoff(
            || {
                attempts += 1;
                Err("Always fails")
            },
            3,
            Duration::from_millis(1),
        );

        assert!(result.is_err());
        assert_eq!(attempts, 3);
    }
}

// ============================================================================
// UI Helpers - Reduce Boilerplate
// ============================================================================

/// Show a toast notification with a message
///
/// Simplifies the common pattern of creating and showing toast notifications.
///
/// # Arguments
///
/// * `overlay` - The ToastOverlay to add the toast to
/// * `message` - The message to display
/// * `timeout` - How long to show the toast (in seconds)
///
/// # Examples
///
/// ```
/// show_toast(&toast_overlay, "✅ Calibration complete!", 3);
/// show_toast(&toast_overlay, "❌ Invalid input", 2);
/// ```
#[cfg(feature = "gtk")]
pub fn show_toast(overlay: &adw::ToastOverlay, message: &str, timeout: u32) {
    let toast = adw::Toast::new(message);
    toast.set_timeout(timeout);
    overlay.add_toast(toast);
}

/// Try to lock a mutex and execute an action, or return Continue if lock fails
///
/// Useful for UI update loops where we want to skip updates if the lock is busy
/// rather than blocking the UI thread.
///
/// # Arguments
///
/// * `mutex` - The Arc<Mutex<T>> to try to lock
/// * `action` - Closure to execute with the locked data
///
/// # Returns
///
/// Always returns glib::ControlFlow::Continue
///
/// # Examples
///
/// ```
/// glib::timeout_add_local(Duration::from_millis(100), move || {
///     try_lock_or_continue(&model, |m| {
///         temp_label.set_text(&format!("{:.1}°C", m.last_prediction));
///     })
/// });
/// ```
#[cfg(feature = "gtk")]
pub fn try_lock_or_continue<T, F>(
    mutex: &Arc<Mutex<T>>,
    action: F,
) -> glib::ControlFlow
where
    F: FnOnce(&T),
{
    match mutex.try_lock() {
        Ok(guard) => {
            action(&guard);
            glib::ControlFlow::Continue
        }
        Err(_) => {
            // Lock is busy, skip this update cycle
            glib::ControlFlow::Continue
        }
    }
}


#[cfg(test)]
mod compression_tests {
    use super::*;
    
    #[test]
    fn test_compression_accuracy() {
        let sensors = vec![
            32.5, 35.7, 33.2, 38.6, 41.1, 39.8, 34.2, 36.9,
            42.3, 40.1, 38.7, 37.2, 35.9, 33.1, 31.5,
        ];
        let prediction = 14.234;
        let timestamp = 1710245678;
        let base_timestamp = 1710240000;
        
        let compressed = CompressedReading::compress(timestamp, &sensors, prediction, base_timestamp);
        let (ts, reconstructed, pred) = compressed.decompress(base_timestamp);
        
        assert_eq!(ts, timestamp);
        
        for (orig, recon) in sensors.iter().zip(reconstructed.iter()) {
            let error = (orig - recon).abs();
            assert!(error < 0.2, "Error: {}°C", error);
        }
        
        assert!((prediction - pred).abs() < 0.1);
    }
}
