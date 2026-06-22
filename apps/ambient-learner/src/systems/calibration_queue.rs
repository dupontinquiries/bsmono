// src/systems/calibration_queue.rs
// Thread-safe queue for calibration requests from the UI to the model worker.
//
// Why a queue instead of a flag:
// - The Qt/GTK UI can issue a calibration at any time
// - The model worker drains pending requests on its own schedule
// - Multiple back-to-back calibrations don't get dropped
// - Each request carries the target temperature plus optional metadata

use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;

/// A user-supplied calibration: "the actual ambient temperature right
/// now is `target_temp`, here is what the sensors said".
#[derive(Debug, Clone)]
pub struct CalibrationRequest {
    /// The user-reported actual temperature in °C.
    pub target_temp: f32,
    /// Optional location id this calibration applies to.
    pub location_id: Option<String>,
    /// Optional note / source ("user", "weather_api", etc.)
    pub source: Option<String>,
}

impl CalibrationRequest {
    pub fn new(target_temp: f32) -> Self {
        Self {
            target_temp,
            location_id: None,
            source: None,
        }
    }

    pub fn with_location(mut self, location_id: impl Into<String>) -> Self {
        self.location_id = Some(location_id.into());
        self
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }
}

/// Bounded queue of pending calibration requests.
///
/// Cloning gives a new handle to the same queue (it's `Arc` inside).
#[derive(Clone, Default)]
pub struct CalibrationQueue {
    inner: Arc<Mutex<VecDeque<CalibrationRequest>>>,
}

impl CalibrationQueue {
    /// Create an empty queue. Cheap; the storage is heap-allocated lazily.
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a calibration onto the queue.
    pub fn push(&self, request: CalibrationRequest) {
        self.inner.lock().push_back(request);
    }

    /// Pop the oldest calibration, if any.
    pub fn pop(&self) -> Option<CalibrationRequest> {
        self.inner.lock().pop_front()
    }

    /// Drain *all* pending calibrations into a Vec (newest last).
    /// Useful for batch-processing on the model worker.
    pub fn drain(&self) -> Vec<CalibrationRequest> {
        let mut guard = self.inner.lock();
        guard.drain(..).collect()
    }

    /// Number of pending requests (mostly for diagnostics / tests).
    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }

    /// `true` iff there are no pending requests.
    pub fn is_empty(&self) -> bool {
        self.inner.lock().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_by_default() {
        let q = CalibrationQueue::new();
        assert!(q.is_empty());
        assert_eq!(q.len(), 0);
        assert!(q.pop().is_none());
    }

    #[test]
    fn push_pop_fifo_order() {
        let q = CalibrationQueue::new();
        q.push(CalibrationRequest::new(20.0));
        q.push(CalibrationRequest::new(21.0));
        q.push(CalibrationRequest::new(22.0));

        assert_eq!(q.len(), 3);
        assert_eq!(q.pop().unwrap().target_temp, 20.0);
        assert_eq!(q.pop().unwrap().target_temp, 21.0);
        assert_eq!(q.pop().unwrap().target_temp, 22.0);
        assert!(q.is_empty());
    }

    #[test]
    fn drain_returns_everything_and_empties() {
        let q = CalibrationQueue::new();
        for t in [18.0, 19.0, 20.0] {
            q.push(CalibrationRequest::new(t));
        }
        let drained = q.drain();
        assert_eq!(drained.len(), 3);
        assert_eq!(drained[0].target_temp, 18.0);
        assert_eq!(drained[2].target_temp, 20.0);
        assert!(q.is_empty());
    }

    #[test]
    fn builder_fields_propagate() {
        let req = CalibrationRequest::new(22.5)
            .with_location("home")
            .with_source("user");
        assert_eq!(req.target_temp, 22.5);
        assert_eq!(req.location_id.as_deref(), Some("home"));
        assert_eq!(req.source.as_deref(), Some("user"));
    }

    #[test]
    fn cloning_handles_share_storage() {
        let a = CalibrationQueue::new();
        let b = a.clone();
        a.push(CalibrationRequest::new(20.0));
        assert_eq!(b.len(), 1);
        b.pop();
        assert!(a.is_empty());
    }
}
