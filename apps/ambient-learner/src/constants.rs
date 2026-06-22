//! Application constants and configuration
//!
//! Centralized location for all configuration values, magic numbers,
//! and platform-specific settings.

use std::time::Duration;

// ============================================================================
// Sensor Configuration
// ============================================================================

/// Sensor polling and calibration settings
pub mod sensors {
    use super::Duration;
    
    /// Interval for passive sensor polling
    pub const PASSIVE_INTERVAL: Duration = Duration::from_millis(1000);
    
    /// Interval for active burst reading during calibration
    pub const ACTIVE_INTERVAL: Duration = Duration::from_millis(100);
    
    /// Number of readings per calibration burst
    ///
    /// Reduced from 50 to 20 for mobile performance.
    /// At 100ms per reading, this takes 2 seconds total.
    pub const ACTIVE_BURST_COUNT: usize = 20;
    
    /// Seconds of history to keep in memory buffer
    pub const HISTORY_SECONDS: u64 = 15;
    
    /// Typical device sensor temperature (°C)
    ///
    /// Used as initial mean for sensor statistics
    pub const TYPICAL_SENSOR_TEMP: f32 = 35.0;
}

// ============================================================================
// Temperature Validation
// ============================================================================

/// Temperature bounds and defaults
pub mod temperature {
    /// Minimum valid ambient temperature (°C)
    ///
    /// Below this is considered a sensor error or extreme conditions
    pub const MIN: f32 = -40.0;
    
    /// Maximum valid ambient temperature (°C)
    ///
    /// Above this is considered a sensor error or extreme conditions
    pub const MAX: f32 = 40.0;
    
    /// Default fallback temperature (°C)
    ///
    /// Used when no valid prediction is available
    pub const DEFAULT: f32 = 20.0;
    
    /// Temperature change threshold for prediction smoothing (°C)
    ///
    /// Predictions changing more than this are damped to reduce noise
    pub const SMOOTHING_THRESHOLD: f32 = 5.0;
}

// ============================================================================
// Model Training
// ============================================================================

/// Machine learning model parameters
pub mod model {
    /// Learning rate for gradient descent weight updates
    ///
    /// Higher = faster learning but less stable
    /// Lower = slower learning but more stable
    pub const LEARNING_RATE: f32 = 0.05;
    
    /// Minimum dropout score to keep sensor active
    ///
    /// Sensors below this threshold are pruned from predictions
    pub const MIN_DROPOUT_SCORE: f32 = 0.1;
    
    /// Alpha for exponential moving average of sensor statistics
    pub const EMA_ALPHA: f32 = 0.05;
    
    /// Initial variance estimate for sensors
    pub const INITIAL_VARIANCE: f32 = 1.0;
}

// ============================================================================
// User Interface
// ============================================================================

/// UI display and interaction settings
pub mod ui {
    use super::Duration;
    
    /// Main window default width (pixels)
    pub const WINDOW_WIDTH: i32 = 400;
    
    /// Main window default height (pixels)
    pub const WINDOW_HEIGHT: i32 = 700;
    
    /// UI refresh interval
    ///
    /// Updates temperature display, model visualization, etc.
    /// 100ms = 10 FPS
    pub const UPDATE_INTERVAL: Duration = Duration::from_millis(100);
    
    /// Page margin (pixels)
    pub const PAGE_MARGIN: i32 = 20;
    
    /// Page margin for live temperature page (reduced for mobile)
    pub const PAGE_MARGIN_LIVE: i32 = 16;
    
    /// Temperature display font size (points)
    pub const TEMP_FONT_SIZE: i32 = 80;
    
    /// Confidence label font class
    pub const CONFIDENCE_FONT_CLASS: &str = "title-2";
    
    /// Status label font class
    pub const STATUS_FONT_CLASS: &str = "title-4";
}

// ============================================================================
// Database Configuration
// ============================================================================

/// SQLite database settings
pub mod database {
    use super::Duration;
    
    /// Maximum retry attempts for locked database
    pub const RETRY_ATTEMPTS: u32 = 3;
    
    /// Delay between retry attempts
    pub const RETRY_DELAY: Duration = Duration::from_millis(50);
    
    /// SQLite WAL mode cache size (pages)
    ///
    /// At default 4KB page size, this is ~40MB cache
    pub const WAL_CACHE_SIZE: i32 = 10000;
    
    /// Synchronous mode for WAL
    ///
    /// NORMAL = faster writes, FULL = safer but slower
    pub const SYNCHRONOUS_MODE: &str = "NORMAL";
    
    /// Journal mode
    ///
    /// WAL = Write-Ahead Logging for better concurrency
    pub const JOURNAL_MODE: &str = "WAL";
}

// ============================================================================
// Chart Rendering
// ============================================================================

/// Visualization chart dimensions and styling
pub mod chart {
    /// Network visualization width (pixels)
    pub const NETWORK_WIDTH: u32 = 800;
    
    /// Network visualization height (pixels)
    pub const NETWORK_HEIGHT: u32 = 1200;
    
    /// History chart width (pixels)
    pub const HISTORY_WIDTH: u32 = 800;
    
    /// History chart height (pixels)
    pub const HISTORY_HEIGHT: u32 = 600;
    
    /// Chart grid line width (pixels)
    pub const GRID_LINE_WIDTH: u32 = 2;
    
    /// Chart bar corner radius (pixels)
    pub const BAR_CORNER_RADIUS: u32 = 6;
    
    /// Font size for chart labels (points)
    pub const LABEL_FONT_SIZE: u32 = 16;
    
    /// Font size for chart axis text (points)
    pub const AXIS_FONT_SIZE: u32 = 14;
}

// ============================================================================
// File Storage
// ============================================================================

/// File paths and storage settings
pub mod storage {
    use super::Duration;
    
    /// Data directory name
    pub const DATA_DIR: &str = "weather_data";
    
    /// Model save interval
    ///
    /// Checks every minute if model has changed, saves if so
    pub const MODEL_SAVE_INTERVAL: Duration = Duration::from_secs(60);
    
    /// Hourly reading save interval
    ///
    /// Saves sensor reading snapshot every hour
    pub const READING_SAVE_INTERVAL: Duration = Duration::from_secs(3600);
    
    /// Cleanup startup delay
    ///
    /// Waits this long after app start before running compression
    pub const CLEANUP_DELAY: Duration = Duration::from_secs(10);
    
    /// Model file name format
    pub const MODEL_FILE_FORMAT: &str = "model_%Y%m%d_%H%M%S.json";
    
    /// Database file name format
    pub const DB_FILE_FORMAT: &str = "sensors_%Y%m%d_%H%M%S.db";
    
    /// Compressed model archive format
    pub const MODEL_ARCHIVE_FORMAT: &str = "models_%Y-%m.json.gz";
}

// ============================================================================
// Platform Detection
// ============================================================================

/// Platform-specific behavior
pub mod platform {
    /// Detect if running on mobile platform
    ///
    /// Checks for Termux (Android) or ARM architecture
    pub fn is_mobile() -> bool {
        std::env::var("TERMUX_VERSION").is_ok()
            || cfg!(target_arch = "aarch64")
            || cfg!(target_arch = "arm")
    }
    
    /// Get optimal burst count for current platform
    pub fn optimal_burst_count() -> usize {
        if is_mobile() {
            10 // Faster on mobile
        } else {
            super::sensors::ACTIVE_BURST_COUNT
        }
    }
    
    /// Get optimal UI update interval for current platform
    pub fn optimal_update_interval() -> std::time::Duration {
        if is_mobile() {
            std::time::Duration::from_millis(200) // Lower FPS on mobile
        } else {
            super::ui::UPDATE_INTERVAL
        }
    }
}

// ============================================================================
// Presets for Common Scenarios
// ============================================================================

/// Preset configurations for different use cases
pub mod presets {
    use super::*;
    
    /// High performance desktop configuration
    pub struct Desktop;
    
    impl Desktop {
        pub const BURST_COUNT: usize = 50;
        pub const UPDATE_INTERVAL: Duration = Duration::from_millis(100);
        pub const CHART_RESOLUTION: (u32, u32) = (800, 1200);
    }
    
    /// Mobile/battery optimized configuration
    pub struct Mobile;
    
    impl Mobile {
        pub const BURST_COUNT: usize = 10;
        pub const UPDATE_INTERVAL: Duration = Duration::from_millis(200);
        pub const CHART_RESOLUTION: (u32, u32) = (400, 600);
    }
    
    /// Development/testing configuration
    pub struct Debug;
    
    impl Debug {
        pub const BURST_COUNT: usize = 5;
        pub const UPDATE_INTERVAL: Duration = Duration::from_millis(500);
        pub const VERBOSE_LOGGING: bool = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_temperature_bounds() {
        assert!(temperature::MIN < temperature::DEFAULT);
        assert!(temperature::DEFAULT < temperature::MAX);
    }
    
    #[test]
    fn test_burst_count_reasonable() {
        assert!(sensors::ACTIVE_BURST_COUNT > 0);
        assert!(sensors::ACTIVE_BURST_COUNT < 100);
    }
    
    #[test]
    fn test_learning_rate_valid() {
        assert!(model::LEARNING_RATE > 0.0);
        assert!(model::LEARNING_RATE < 1.0);
    }
}
