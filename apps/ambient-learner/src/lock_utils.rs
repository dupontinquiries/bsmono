use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Lock with timeout to prevent indefinite blocking
pub fn lock_with_timeout<T>(
    mutex: &Mutex<T>,
    timeout: Duration,
) -> Result<MutexGuard<T>, LockError> {
    let start = Instant::now();
    let mut attempts = 0;

    loop {
        match mutex.try_lock() {
            Ok(guard) => {
                if attempts > 10 {
                    eprintln!(
                        "⚠️  Lock acquired after {} attempts ({:?})",
                        attempts,
                        start.elapsed()
                    );
                }
                return Ok(guard);
            }
            Err(_) if start.elapsed() >= timeout => {
                return Err(LockError::Timeout {
                    elapsed: start.elapsed(),
                    attempts,
                });
            }
            Err(_) => {
                attempts += 1;

                // Exponential backoff: 1ms, 2ms, 4ms, 8ms, 16ms, 32ms, 64ms, 100ms
                let sleep_ms = (2_u64.pow(attempts.min(6))).min(100);
                std::thread::sleep(Duration::from_millis(sleep_ms));
            }
        }
    }
}

#[derive(Debug)]
pub enum LockError {
    Timeout { elapsed: Duration, attempts: u32 },
    Poisoned,
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::Timeout { elapsed, attempts } => {
                write!(
                    f,
                    "Lock timeout after {:?} ({} attempts)",
                    elapsed, attempts
                )
            }
            LockError::Poisoned => write!(f, "Lock poisoned"),
        }
    }
}

impl std::error::Error for LockError {}
