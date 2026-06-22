//! Error types for ambient-learner
//!
//! Provides structured error handling with descriptive messages
//! and proper error propagation throughout the application.

use std::fmt;

/// Application-specific errors
///
/// Encompasses all error conditions that can occur during operation.
/// Each variant provides context about what went wrong.
#[derive(Debug)]
pub enum AppError {
    /// Database operation failed
    Database(rusqlite::Error),
    
    /// File I/O operation failed
    Io(std::io::Error),
    
    /// JSON serialization/deserialization failed
    Serialization(serde_json::Error),
    
    /// Compression/decompression failed
    Compression(std::io::Error),
    
    /// Temperature value outside valid range
    InvalidTemperature {
        value: f32,
        min: f32,
        max: f32,
    },
    
    /// Cannot access thermal sensors
    SensorAccessDenied {
        reason: String,
    },
    
    /// Model file not found
    ModelNotFound {
        path: std::path::PathBuf,
    },
    
    /// Database file not found
    DatabaseNotFound {
        path: std::path::PathBuf,
    },
    
    /// Lock is poisoned (another thread panicked)
    LockPoisoned {
        resource: String,
    },
    
    /// Maximum retry attempts exceeded
    MaxRetriesExceeded {
        operation: String,
        attempts: u32,
    },
    
    /// Invalid location ID
    InvalidLocationId {
        id: String,
    },
    
    /// Network detection failed
    NetworkDetectionFailed {
        reason: String,
    },
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Database(e) => write!(f, "Database error: {}", e),
            AppError::Io(e) => write!(f, "I/O error: {}", e),
            AppError::Serialization(e) => write!(f, "Serialization error: {}", e),
            AppError::Compression(e) => write!(f, "Compression error: {}", e),
            AppError::InvalidTemperature { value, min, max } => {
                write!(
                    f,
                    "Invalid temperature {:.1}°C (must be between {:.1}°C and {:.1}°C)",
                    value, min, max
                )
            }
            AppError::SensorAccessDenied { reason } => {
                write!(f, "Cannot access thermal sensors: {}", reason)
            }
            AppError::ModelNotFound { path } => {
                write!(f, "Model file not found: {}", path.display())
            }
            AppError::DatabaseNotFound { path } => {
                write!(f, "Database file not found: {}", path.display())
            }
            AppError::LockPoisoned { resource } => {
                write!(f, "Lock poisoned on {}", resource)
            }
            AppError::MaxRetriesExceeded { operation, attempts } => {
                write!(
                    f,
                    "Maximum retry attempts ({}) exceeded for operation: {}",
                    attempts, operation
                )
            }
            AppError::InvalidLocationId { id } => {
                write!(f, "Invalid location ID: {}", id)
            }
            AppError::NetworkDetectionFailed { reason } => {
                write!(f, "Network detection failed: {}", reason)
            }
        }
    }
}

impl std::error::Error for AppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AppError::Database(e) => Some(e),
            AppError::Io(e) => Some(e),
            AppError::Serialization(e) => Some(e),
            AppError::Compression(e) => Some(e),
            _ => None,
        }
    }
}

// Conversions from standard library errors
impl From<rusqlite::Error> for AppError {
    fn from(err: rusqlite::Error) -> Self {
        AppError::Database(err)
    }
}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        AppError::Io(err)
    }
}

impl From<serde_json::Error> for AppError {
    fn from(err: serde_json::Error) -> Self {
        AppError::Serialization(err)
    }
}

/// Result type alias for application operations
///
/// Use this instead of `std::result::Result<T, Box<dyn Error>>` for
/// consistent error handling throughout the application.
///
/// # Examples
///
/// ```
/// fn load_model(path: &Path) -> Result<Model> {
///     let data = fs::read_to_string(path)?;
///     let model = serde_json::from_str(&data)?;
///     Ok(model)
/// }
/// ```
pub type Result<T> = std::result::Result<T, AppError>;

/// Helper for creating validation errors
///
/// # Examples
///
/// ```
/// if temp < TEMP_MIN || temp > TEMP_MAX {
///     return Err(invalid_temperature(temp, TEMP_MIN, TEMP_MAX));
/// }
/// ```
pub fn invalid_temperature(value: f32, min: f32, max: f32) -> AppError {
    AppError::InvalidTemperature { value, min, max }
}

/// Helper for creating sensor access errors
///
/// # Examples
///
/// ```
/// if !sensor_path.exists() {
///     return Err(sensor_access_denied("Thermal sensors not available"));
/// }
/// ```
pub fn sensor_access_denied(reason: impl Into<String>) -> AppError {
    AppError::SensorAccessDenied {
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_invalid_temperature_display() {
        let err = invalid_temperature(100.0, -40.0, 40.0);
        let msg = format!("{}", err);
        assert!(msg.contains("100.0"));
        assert!(msg.contains("-40.0"));
        assert!(msg.contains("40.0"));
    }
    
    #[test]
    fn test_error_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "test");
        let app_err: AppError = io_err.into();
        assert!(matches!(app_err, AppError::Io(_)));
    }
}
