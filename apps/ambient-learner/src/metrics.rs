use parking_lot::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub struct Metrics {
    // Counters
    readings_processed: AtomicU64,
    calibrations_performed: AtomicU64,
    database_writes: AtomicU64,
    database_errors: AtomicU64,
    lock_contentions: AtomicU64,

    // Gauges
    active_sensors: AtomicUsize,
    model_confidence: AtomicU64, // Store as u64, convert from f32

    // Timing
    last_reading: Mutex<Option<Instant>>,
    last_calibration: Mutex<Option<Instant>>,

    // Errors
    recent_errors: Mutex<Vec<String>>,
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            readings_processed: AtomicU64::new(0),
            calibrations_performed: AtomicU64::new(0),
            database_writes: AtomicU64::new(0),
            database_errors: AtomicU64::new(0),
            lock_contentions: AtomicU64::new(0),
            active_sensors: AtomicUsize::new(0),
            model_confidence: AtomicU64::new(0),
            last_reading: Mutex::new(None),
            last_calibration: Mutex::new(None),
            recent_errors: Mutex::new(Vec::new()),
        }
    }

    pub fn record_reading(&self) {
        self.readings_processed.fetch_add(1, Ordering::Relaxed);
        *self.last_reading.lock() = Some(Instant::now());
    }

    pub fn record_calibration(&self) {
        self.calibrations_performed
            .fetch_add(1, Ordering::Relaxed);
        *self.last_calibration.lock() = Some(Instant::now());
    }

    pub fn record_db_write(&self) {
        self.database_writes.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_db_error(&self, error: String) {
        self.database_errors.fetch_add(1, Ordering::Relaxed);

        let mut errors = self.recent_errors.lock();
        errors.push(error);

        // Keep only last 100 errors
        if errors.len() > 100 {
            errors.remove(0);
        }
    }

    pub fn record_lock_contention(&self) {
        self.lock_contentions.fetch_add(1, Ordering::Relaxed);
    }

    pub fn set_active_sensors(&self, count: usize) {
        self.active_sensors.store(count, Ordering::Relaxed);
    }

    pub fn set_confidence(&self, confidence: f32) {
        let as_u64 = (confidence * 1000.0) as u64;
        self.model_confidence.store(as_u64, Ordering::Relaxed);
    }

    pub fn get_snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            readings_processed: self.readings_processed.load(Ordering::Relaxed),
            calibrations_performed: self.calibrations_performed.load(Ordering::Relaxed),
            database_writes: self.database_writes.load(Ordering::Relaxed),
            database_errors: self.database_errors.load(Ordering::Relaxed),
            lock_contentions: self.lock_contentions.load(Ordering::Relaxed),
            active_sensors: self.active_sensors.load(Ordering::Relaxed),
            confidence: self.model_confidence.load(Ordering::Relaxed) as f32 / 1000.0,
            time_since_last_reading: self.last_reading.lock().as_ref().map(|t| t.elapsed()),
            time_since_last_calibration: self.last_calibration.lock().as_ref().map(|t| t.elapsed()),
            recent_errors: self.recent_errors.lock().clone(),
        }
    }

    pub fn print_summary(&self) {
        let snapshot = self.get_snapshot();

        println!("\n📊 === METRICS SUMMARY ===");
        println!("Readings processed: {}", snapshot.readings_processed);
        println!("Calibrations: {}", snapshot.calibrations_performed);
        println!("Database writes: {}", snapshot.database_writes);
        println!("Database errors: {}", snapshot.database_errors);
        println!("Lock contentions: {}", snapshot.lock_contentions);
        println!("Active sensors: {}", snapshot.active_sensors);
        println!("Model confidence: {:.2}%", snapshot.confidence * 100.0);

        if let Some(elapsed) = snapshot.time_since_last_reading {
            println!("Last reading: {:?} ago", elapsed);
        }

        if !snapshot.recent_errors.is_empty() {
            println!("\nRecent errors:");
            for (i, error) in snapshot.recent_errors.iter().rev().take(5).enumerate() {
                println!("  {}. {}", i + 1, error);
            }
        }

        println!("=========================\n");
    }
}

#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub readings_processed: u64,
    pub calibrations_performed: u64,
    pub database_writes: u64,
    pub database_errors: u64,
    pub lock_contentions: u64,
    pub active_sensors: usize,
    pub confidence: f32,
    pub time_since_last_reading: Option<Duration>,
    pub time_since_last_calibration: Option<Duration>,
    pub recent_errors: Vec<String>,
}

// Global metrics instance
use lazy_static::lazy_static;

lazy_static! {
    pub static ref METRICS: Metrics = Metrics::new();
}
