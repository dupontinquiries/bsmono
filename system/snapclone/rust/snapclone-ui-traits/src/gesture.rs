//! Gesture contract — horizontal swipe navigation and touch events.
//!
//! # Philosophy
//!
//! The SnapClone swipe system (SwipeNavigator.cpp, ~993 lines) is one of
//! the most complex pieces of the codebase. By extracting the CONTRACT here,
//! a future egui or Slint implementation doesn't need to re-invent the
//! direction detection, rubber-band physics, or idle timer.
//! It just implements `GestureHost` and calls the provided callbacks.
//!
//! # Qt mapping
//!
//! ```text
//! SwipeNavigator::pageChanged   → GestureHost::on_page_change
//! SwipeNavigator::goToPage()    → GestureHost::go_to_page()
//! SwipeNavigator::eventFilter() → GestureHost::feed_event()
//! ```

/// Direction of a confirmed horizontal swipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwipeDirection { Left, Right }

/// A low-level gesture event fed into the gesture recogniser.
/// Backends produce these from touch / wheel / trackpad events.
#[derive(Debug, Clone)]
pub enum GestureEvent {
    /// Finger/pointer touched down at (x, y) in widget-local coordinates.
    Begin { x: f32, y: f32 },
    /// Finger/pointer moved to (x, y).
    Update { x: f32, y: f32 },
    /// Finger/pointer lifted at (x, y).
    End { x: f32, y: f32 },
    /// Gesture cancelled (e.g. incoming call, system gesture).
    Cancel,
    /// Trackpad/wheel horizontal delta (pixels, positive = right).
    WheelDelta { dx: f32, dy: f32, phase: WheelPhase },
}

/// Phase information for trackpad scroll events (matches Qt::ScrollPhase).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WheelPhase {
    #[default]
    NoPhase,    // traditional mouse wheel (no momentum)
    Begin,      // first event in a trackpad gesture (may be zero-delta)
    Update,     // mid-gesture movement
    End,        // finger lifted — momentum deceleration begins
    Momentum,   // inertia scroll after finger lift
}

/// Parameters that tune gesture feel. Matches the constants in SwipeNavigator.hpp.
#[derive(Debug, Clone)]
pub struct GestureConfig {
    /// Pixels to move before direction is locked (default: 12)
    pub lock_threshold_px:    f32,
    /// Pixels of drag required to commit a page switch (default: 28)
    pub commit_threshold_px:  f32,
    /// Velocity in px/ms required for a flick commit (default: 0.18)
    pub flick_threshold_px_ms: f32,
    /// Idle timeout before wheel gesture ends (default: 400ms)
    pub wheel_idle_ms:        u32,
    /// Direction ratio: reject if |hx| < |vy| × ratio (default: 0.75)
    pub direction_ratio:      f32,
}

impl Default for GestureConfig {
    fn default() -> Self {
        Self {
            lock_threshold_px:    12.0,
            commit_threshold_px:  28.0,
            flick_threshold_px_ms: 0.18,
            wheel_idle_ms:        400,
            direction_ratio:      0.75,
        }
    }
}

/// The swipe navigation host. Implement for your UI framework.
///
/// Manages a stack of pages with horizontal swipe navigation.
/// The implementation handles rubber-band physics, velocity-scaled animation,
/// and the WheelPhase::Pending state for zero-delta trackpad events.
pub trait GestureHost {
    /// Total number of pages.
    fn page_count(&self) -> usize;

    /// Current page index.
    fn current_page(&self) -> usize;

    /// Programmatically navigate to a page (animated).
    fn go_to_page(&mut self, index: usize);

    /// Feed a raw gesture event into the recogniser.
    /// Returns `true` if the gesture was consumed (don't propagate).
    fn feed_event(&mut self, event: GestureEvent) -> bool;

    /// Set the gesture configuration (thresholds, timings).
    fn set_config(&mut self, config: GestureConfig);

    /// Register a callback invoked when the active page changes.
    fn on_page_change(&mut self, f: Box<dyn Fn(usize) + Send + 'static>);

    /// Current visual offset in pixels (0 = at rest, negative = dragged left).
    /// Used for rubber-band preview rendering.
    fn slide_offset(&self) -> f32;
}

/// Shared gesture state — the pure-Rust implementation of the recognition
/// algorithm. UI backends can use this directly instead of re-implementing it.
///
/// Feed events via `process()` and read back the resulting page state.
#[derive(Debug, Default)]
pub struct GestureRecognizer {
    pub config:        GestureConfig,
    pub current_page:  usize,
    pub page_count:    usize,
    pub slide_offset:  f32,
    pub drag_distance: f32,
    pub locked:        bool,
    touch_start_x:     f32,
    touch_start_y:     f32,
    wheel_accum:       f32,
    wheel_phase:       WheelPhaseState,
}

#[derive(Debug, Default, PartialEq, Eq)]
enum WheelPhaseState { #[default] Idle, Pending, Active }

#[derive(Debug, Clone, PartialEq)]
pub enum GestureOutcome {
    /// Nothing significant yet — keep rendering current state
    Dragging { offset: f32 },
    /// A full page switch was committed
    PageChanged { to: usize },
    /// Gesture cancelled or bounced back — animate to rest
    BounceBack,
    /// Nothing — event ignored
    None,
}

impl GestureRecognizer {
    pub fn new(page_count: usize) -> Self {
        Self { page_count, config: GestureConfig::default(), ..Default::default() }
    }

    pub fn process(&mut self, event: &GestureEvent) -> GestureOutcome {
        match event {
            GestureEvent::Begin { x, y } => {
                self.touch_start_x = *x;
                self.touch_start_y = *y;
                self.drag_distance = 0.0;
                self.locked        = false;
                self.slide_offset  = 0.0;
                GestureOutcome::None
            }

            GestureEvent::Update { x, y } => {
                self.drag_distance = x - self.touch_start_x;
                let vert = (y - self.touch_start_y).abs();

                if !self.locked {
                    // Reject clearly vertical gestures
                    if self.drag_distance.abs() < vert.abs() * self.config.direction_ratio {
                        return GestureOutcome::None;
                    }
                    // Wait for lock threshold
                    if self.drag_distance.abs() < self.config.lock_threshold_px {
                        return GestureOutcome::None;
                    }
                    self.locked = true;
                }

                // Rubber-band at edges
                let at_start = self.current_page == 0 && self.drag_distance > 0.0;
                let at_end   = self.current_page + 1 >= self.page_count && self.drag_distance < 0.0;
                self.slide_offset = if at_start || at_end {
                    // sqrt rubber-band (matches SwipeNavigator.cpp)
                    let sign = self.drag_distance.signum();
                    sign * (self.drag_distance.abs() * 400.0 * 0.35).sqrt()
                } else {
                    self.drag_distance
                };

                GestureOutcome::Dragging { offset: self.slide_offset }
            }

            GestureEvent::End { .. } | GestureEvent::Cancel => {
                if !self.locked {
                    self.slide_offset = 0.0;
                    return GestureOutcome::BounceBack;
                }
                let committed = self.drag_distance.abs() > self.config.commit_threshold_px;
                if committed {
                    let to_next = self.drag_distance < 0.0;
                    let new_page = if to_next {
                        (self.current_page + 1).min(self.page_count.saturating_sub(1))
                    } else {
                        self.current_page.saturating_sub(1)
                    };
                    self.current_page = new_page;
                    self.slide_offset = 0.0;
                    self.locked       = false;
                    GestureOutcome::PageChanged { to: new_page }
                } else {
                    self.slide_offset = 0.0;
                    self.locked       = false;
                    GestureOutcome::BounceBack
                }
            }

            GestureEvent::WheelDelta { dx, dy, phase } => {
                self.process_wheel(*dx, *dy, phase)
            }
        }
    }

    fn process_wheel(&mut self, dx: f32, dy: f32, phase: &WheelPhase) -> GestureOutcome {
        // Handle zero-delta ScrollBegin (common on Wayland)
        if dx.abs() < 0.5 && dy.abs() < 0.5 {
            if *phase == WheelPhase::Begin {
                self.wheel_phase = WheelPhaseState::Pending;
            }
            return GestureOutcome::None;
        }

        // Direction check: reject clearly vertical wheel
        if dx.abs() < dy.abs() * self.config.direction_ratio {
            self.wheel_phase = WheelPhaseState::Idle;
            return GestureOutcome::None;
        }

        if self.wheel_phase == WheelPhaseState::Idle
            || self.wheel_phase == WheelPhaseState::Pending {
            self.wheel_accum  = 0.0;
            self.touch_start_x = 0.0;
            self.wheel_phase  = WheelPhaseState::Active;
        }

        self.wheel_accum += dx;
        self.drag_distance = self.wheel_accum;
        self.slide_offset  = self.wheel_accum;

        if *phase == WheelPhase::End || *phase == WheelPhase::Momentum {
            // Commit or bounce
            let result = if self.wheel_accum.abs() > self.config.commit_threshold_px {
                let to_next = self.wheel_accum < 0.0;
                let new_page = if to_next {
                    (self.current_page + 1).min(self.page_count.saturating_sub(1))
                } else {
                    self.current_page.saturating_sub(1)
                };
                self.current_page = new_page;
                GestureOutcome::PageChanged { to: new_page }
            } else {
                GestureOutcome::BounceBack
            };
            self.wheel_accum  = 0.0;
            self.slide_offset = 0.0;
            self.wheel_phase  = WheelPhaseState::Idle;
            return result;
        }

        GestureOutcome::Dragging { offset: self.slide_offset }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swipe_right_commits() {
        let mut gr = GestureRecognizer::new(3);
        gr.current_page = 1;
        gr.process(&GestureEvent::Begin { x: 0.0, y: 0.0 });
        gr.process(&GestureEvent::Update { x: 50.0, y: 0.0 });
        let r = gr.process(&GestureEvent::End { x: 50.0, y: 0.0 });
        assert_eq!(r, GestureOutcome::PageChanged { to: 0 });
    }

    #[test]
    fn short_swipe_bounces() {
        let mut gr = GestureRecognizer::new(3);
        gr.current_page = 1;
        gr.process(&GestureEvent::Begin { x: 0.0, y: 0.0 });
        gr.process(&GestureEvent::Update { x: 10.0, y: 0.0 });
        let r = gr.process(&GestureEvent::End { x: 10.0, y: 0.0 });
        // 10px < 28px threshold, but also below lock threshold — BounceBack
        assert!(matches!(r, GestureOutcome::BounceBack | GestureOutcome::None));
    }

    #[test]
    fn vertical_gesture_ignored() {
        let mut gr = GestureRecognizer::new(3);
        gr.process(&GestureEvent::Begin { x: 0.0, y: 0.0 });
        let r = gr.process(&GestureEvent::Update { x: 5.0, y: 50.0 }); // clearly vertical
        assert_eq!(r, GestureOutcome::None);
    }

    #[test]
    fn wheel_pending_then_active() {
        let mut gr = GestureRecognizer::new(3);
        gr.current_page = 1;
        // Zero-delta begin (Wayland pattern)
        gr.process(&GestureEvent::WheelDelta { dx: 0.0, dy: 0.0, phase: WheelPhase::Begin });
        assert_eq!(gr.wheel_phase, WheelPhaseState::Pending);
        // Real delta
        let r = gr.process(&GestureEvent::WheelDelta { dx: -50.0, dy: 0.0, phase: WheelPhase::Update });
        assert!(matches!(r, GestureOutcome::Dragging { .. }));
    }
}
