//! Touch controls: the model, with no window in it.
//!
//! unendlich has no touch input at all — `InputCache` handles keys and a mouse,
//! and `grep -rn touch` across the engine finds only the word "untouched" in
//! comments. On a phone that means the game renders and cannot be played.
//!
//! This is the part that does not need a window: which finger is doing what,
//! what the stick reads, how far the camera turned this frame, and when a tap
//! becomes a hold. Pure, so the rules can be tested without a compositor and
//! without a device — which matters here because every one of them is wrong in
//! a way that is hard to see and easy to feel.
//!
//! # The rule that costs the most to get wrong
//!
//! **A finger's ROLE is decided when it goes down and never changes.** Walking
//! forward means holding the left stick and often dragging past the middle of
//! the screen; if the role were recomputed per frame from the current position,
//! crossing that line would hand the finger to the camera and the view would
//! snap round while you walk. Deciding once at touch-down is the difference
//! between a control that works and one that is haunted.
//!
//! # Look is a DELTA and must be consumed
//!
//! The camera turns by however far the look finger moved since the last frame,
//! so [`TouchState::take_look`] returns and clears. A reader that peeks without
//! clearing keeps applying the same delta every frame and the view spins on a
//! motionless finger — the single most common bug in a touch camera, and
//! invisible until the device is in your hand.

/// Where a touch is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Start,
    Move,
    End,
}

/// One touch, as the platform layer reports it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Touch {
    /// Stable for the life of the contact. Whatever the compositor calls it.
    pub id: u64,
    pub x: f32,
    pub y: f32,
    pub phase: Phase,
}

/// What a finger was assigned to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Left of the split: a virtual stick anchored where it went down.
    Move,
    /// Right of the split: drags the camera, taps and holds act.
    Look,
}

/// Which half a touch at `x` belongs to, on a screen `width` wide.
///
/// A plain half-and-half split rather than fixed zones. Fixed zones need to be
/// drawn to be findable and are wrong on every aspect ratio; a half is where
/// people's thumbs already are, and it needs no chrome over the world.
pub fn role_for(x: f32, width: f32) -> Role {
    if x < width * 0.5 { Role::Move } else { Role::Look }
}

/// How far from its anchor a stick has to move before it reads as anything.
///
/// A thumb resting on glass drifts by a pixel or two, and without a deadzone
/// the player walks slowly in a random direction whenever they hold still.
pub const STICK_DEADZONE_PX: f32 = 8.0;

/// The distance at which the stick reads full tilt.
///
/// Small enough to reach without lifting the thumb. Beyond it the vector is
/// clamped rather than scaled, so dragging further keeps running at full speed
/// instead of doing nothing extra.
pub const STICK_RANGE_PX: f32 = 90.0;

/// Longest a contact can last and still be a tap, in seconds.
pub const TAP_SECS: f32 = 0.25;

/// Furthest a contact can travel and still be a tap, in pixels.
///
/// Separate from the time limit and both must hold: a quick flick is a look,
/// not a tap, and treating it as a tap places a block every time the player
/// turns quickly.
pub const TAP_SLOP_PX: f32 = 16.0;

#[derive(Debug, Clone, Copy)]
struct Finger {
    role: Role,
    anchor: (f32, f32),
    at: (f32, f32),
    /// Where it was when the look delta was last taken.
    last_taken: (f32, f32),
    age: f32,
    /// Furthest it has been from its anchor, for the tap test. Tracked as a
    /// maximum rather than measured at release, because a finger that swings
    /// out and comes back has still travelled and is not a tap.
    max_travel: f32,
}

/// What the player is doing with their fingers.
#[derive(Debug, Clone, Default)]
pub struct TouchState {
    fingers: Vec<(u64, Finger)>,
    /// Taps resolved since the last drain.
    taps: Vec<(f32, f32)>,
    width: f32,
}

impl TouchState {
    pub fn new(width: f32) -> TouchState {
        TouchState { fingers: Vec::new(), taps: Vec::new(), width }
    }

    /// The screen can rotate; the split moves with it.
    pub fn set_width(&mut self, width: f32) {
        self.width = width;
    }

    pub fn active_fingers(&self) -> usize {
        self.fingers.len()
    }

    /// Feed one platform touch event.
    pub fn feed(&mut self, t: Touch) {
        match t.phase {
            Phase::Start => {
                // Role decided HERE and never revisited — see the module note.
                let role = role_for(t.x, self.width);
                self.fingers.retain(|(id, _)| *id != t.id);
                self.fingers.push((
                    t.id,
                    Finger {
                        role,
                        anchor: (t.x, t.y),
                        at: (t.x, t.y),
                        last_taken: (t.x, t.y),
                        age: 0.0,
                        max_travel: 0.0,
                    },
                ));
            }
            Phase::Move => {
                if let Some((_, f)) = self.fingers.iter_mut().find(|(id, _)| *id == t.id) {
                    f.at = (t.x, t.y);
                    let d = ((t.x - f.anchor.0).powi(2) + (t.y - f.anchor.1).powi(2)).sqrt();
                    f.max_travel = f.max_travel.max(d);
                }
            }
            Phase::End => {
                if let Some(i) = self.fingers.iter().position(|(id, _)| *id == t.id) {
                    let (_, f) = self.fingers.remove(i);
                    // A tap is short AND still. Either alone misfires: a slow
                    // press is a hold, a fast flick is a look.
                    if f.role == Role::Look && f.age <= TAP_SECS && f.max_travel <= TAP_SLOP_PX {
                        self.taps.push((t.x, t.y));
                    }
                }
            }
        }
    }

    /// Advance the clock. Call once a frame, before reading.
    pub fn tick(&mut self, dt: f32) {
        for (_, f) in self.fingers.iter_mut() {
            f.age += dt;
        }
    }

    /// The movement stick, as `[x, y]` in -1..=1. `[0, 0]` when no finger is on
    /// the left half or it is inside the deadzone.
    ///
    /// The deadzone is SUBTRACTED rather than gating: crossing it starts the
    /// player at a crawl instead of jumping straight to a fifth of full speed,
    /// which is what a bare `if d > deadzone { d / range }` does.
    pub fn move_axes(&self) -> [f32; 2] {
        let Some((_, f)) = self.fingers.iter().find(|(_, f)| f.role == Role::Move) else {
            return [0.0, 0.0];
        };
        let (dx, dy) = (f.at.0 - f.anchor.0, f.at.1 - f.anchor.1);
        let len = (dx * dx + dy * dy).sqrt();
        if len <= STICK_DEADZONE_PX {
            return [0.0, 0.0];
        }
        let usable = (STICK_RANGE_PX - STICK_DEADZONE_PX).max(1.0);
        let mag = ((len - STICK_DEADZONE_PX) / usable).min(1.0);
        [dx / len * mag, dy / len * mag]
    }

    /// How far the look finger moved since this was last called, in pixels.
    ///
    /// Returns AND CLEARS. See the module note: a peek that does not clear
    /// spins the camera forever on a motionless finger.
    pub fn take_look(&mut self) -> [f32; 2] {
        let Some((_, f)) = self.fingers.iter_mut().find(|(_, f)| f.role == Role::Look) else {
            return [0.0, 0.0];
        };
        let d = [f.at.0 - f.last_taken.0, f.at.1 - f.last_taken.1];
        f.last_taken = f.at;
        d
    }

    /// Taps resolved since the last call, and clears them.
    pub fn take_taps(&mut self) -> Vec<(f32, f32)> {
        std::mem::take(&mut self.taps)
    }

    /// Is a look finger being held still — the "keep mining" gesture?
    ///
    /// Held rather than tapped repeatedly, because breaking a block takes time
    /// and a player should not have to drum on the glass for it.
    pub fn look_held(&self) -> bool {
        self.fingers
            .iter()
            .any(|(_, f)| f.role == Role::Look && f.age > TAP_SECS && f.max_travel <= TAP_SLOP_PX)
    }

    /// Drop every contact. For a focus loss or a mode change.
    ///
    /// Necessary because touch has no equivalent of a key-up you can rely on:
    /// a compositor that takes the surface away mid-drag never sends `End`, and
    /// the player comes back to a game walking into a wall by itself.
    pub fn clear(&mut self) {
        self.fingers.clear();
        self.taps.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn down(id: u64, x: f32, y: f32) -> Touch {
        Touch { id, x, y, phase: Phase::Start }
    }
    fn mv(id: u64, x: f32, y: f32) -> Touch {
        Touch { id, x, y, phase: Phase::Move }
    }
    fn up(id: u64, x: f32, y: f32) -> Touch {
        Touch { id, x, y, phase: Phase::End }
    }

    #[test]
    fn a_fingers_role_is_fixed_when_it_goes_down() {
        // The one that makes touch controls feel haunted. Walking forward drags
        // the thumb past the middle; if the role were recomputed the camera
        // would snap round mid-stride.
        let mut s = TouchState::new(1000.0);
        s.feed(down(1, 100.0, 500.0)); // left half → Move
        s.feed(mv(1, 900.0, 500.0));   // dragged well into the right half
        assert_eq!(s.take_look(), [0.0, 0.0], "the move finger started steering the camera");
        assert!(s.move_axes()[0] > 0.9, "the move finger stopped moving the player");
    }

    #[test]
    fn the_look_delta_is_consumed_and_a_still_finger_stops_the_camera() {
        // Peeking without clearing keeps applying the same delta every frame,
        // so the view spins on a motionless thumb. Invisible until it is in
        // your hand, so it gets a test.
        let mut s = TouchState::new(1000.0);
        s.feed(down(1, 900.0, 500.0));
        s.feed(mv(1, 930.0, 480.0));
        assert_eq!(s.take_look(), [30.0, -20.0]);
        assert_eq!(s.take_look(), [0.0, 0.0], "the camera kept turning on a still finger");
        s.feed(mv(1, 940.0, 480.0));
        assert_eq!(s.take_look(), [10.0, 0.0], "the delta is not relative to the last read");
    }

    #[test]
    fn the_stick_has_a_deadzone_and_eases_out_of_it() {
        // Subtracted, not gated: a bare `if d > deadzone { d / range }` jumps
        // the player from a standstill to a fifth of full speed the instant
        // they cross it.
        let mut s = TouchState::new(1000.0);
        s.feed(down(1, 100.0, 500.0));
        s.feed(mv(1, 100.0 + STICK_DEADZONE_PX - 1.0, 500.0));
        assert_eq!(s.move_axes(), [0.0, 0.0], "a resting thumb walked the player");

        s.feed(mv(1, 100.0 + STICK_DEADZONE_PX + 1.0, 500.0));
        let a = s.move_axes()[0];
        assert!(a > 0.0 && a < 0.1, "crossing the deadzone jumped to {a}");

        // Clamped at the range, not scaled past it: dragging further keeps
        // running rather than doing nothing extra.
        s.feed(mv(1, 100.0 + STICK_RANGE_PX * 3.0, 500.0));
        assert!((s.move_axes()[0] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn a_tap_must_be_both_short_and_still() {
        // Either test alone misfires: a slow press is a hold, and a fast flick
        // is a look — treating that as a tap places a block every time the
        // player turns quickly.
        let mut s = TouchState::new(1000.0);
        s.feed(down(1, 900.0, 500.0));
        s.tick(0.05);
        s.feed(up(1, 902.0, 501.0));
        assert_eq!(s.take_taps().len(), 1, "a short still touch was not a tap");

        // Too slow.
        s.feed(down(2, 900.0, 500.0));
        s.tick(TAP_SECS + 0.1);
        s.feed(up(2, 900.0, 500.0));
        assert!(s.take_taps().is_empty(), "a long press registered as a tap");

        // Too far.
        s.feed(down(3, 900.0, 500.0));
        s.tick(0.05);
        s.feed(mv(3, 900.0 + TAP_SLOP_PX + 10.0, 500.0));
        s.feed(up(3, 900.0 + TAP_SLOP_PX + 10.0, 500.0));
        assert!(s.take_taps().is_empty(), "a flick registered as a tap");
    }

    #[test]
    fn a_finger_that_swings_out_and_back_is_not_a_tap() {
        // Travel is a MAXIMUM, not the distance at release. Measuring at
        // release would call a wide arc that happens to end where it started a
        // tap.
        let mut s = TouchState::new(1000.0);
        s.feed(down(1, 900.0, 500.0));
        s.tick(0.05);
        s.feed(mv(1, 900.0 + TAP_SLOP_PX * 4.0, 500.0));
        s.feed(mv(1, 900.0, 500.0));
        s.feed(up(1, 900.0, 500.0));
        assert!(s.take_taps().is_empty(), "an arc back to the start counted as a tap");
    }

    #[test]
    fn holding_still_on_the_right_is_a_hold_not_a_tap() {
        // Breaking a block takes time; a player should not have to drum on the
        // glass. And the two must be exclusive, or a hold fires an action every
        // time it ends.
        let mut s = TouchState::new(1000.0);
        s.feed(down(1, 900.0, 500.0));
        assert!(!s.look_held(), "held immediately, before the tap window elapsed");
        s.tick(TAP_SECS + 0.05);
        assert!(s.look_held());
        s.feed(up(1, 900.0, 500.0));
        assert!(s.take_taps().is_empty(), "a hold also fired a tap on release");
    }

    #[test]
    fn two_thumbs_work_at_once() {
        // The whole point of tracking by id. Walking while looking is the
        // normal case, not an edge one.
        let mut s = TouchState::new(1000.0);
        s.feed(down(1, 100.0, 500.0));
        s.feed(down(2, 900.0, 500.0));
        s.feed(mv(1, 100.0 + STICK_RANGE_PX, 500.0));
        s.feed(mv(2, 950.0, 500.0));
        assert!((s.move_axes()[0] - 1.0).abs() < 1e-5);
        assert_eq!(s.take_look(), [50.0, 0.0]);
        assert_eq!(s.active_fingers(), 2);
    }

    #[test]
    fn clearing_stops_the_player_walking_by_itself() {
        // A compositor that takes the surface away mid-drag never sends `End`,
        // so without this the player returns to a game already walking.
        let mut s = TouchState::new(1000.0);
        s.feed(down(1, 100.0, 500.0));
        s.feed(mv(1, 100.0 + STICK_RANGE_PX, 500.0));
        assert!(s.move_axes()[0] > 0.9);
        s.clear();
        assert_eq!(s.move_axes(), [0.0, 0.0]);
        assert_eq!(s.active_fingers(), 0);
    }

    #[test]
    fn the_split_follows_a_rotated_screen() {
        // x = 600 is the right half of a 1000-wide screen and the LEFT half of
        // a 1400-wide one. A split cached at startup puts both thumbs on the
        // camera in landscape.
        assert_eq!(role_for(600.0, 1000.0), Role::Look);
        assert_eq!(role_for(600.0, 1400.0), Role::Move);
        let mut s = TouchState::new(1000.0);
        s.set_width(1400.0);
        s.feed(down(1, 600.0, 500.0));
        s.feed(mv(1, 700.0, 500.0));
        assert!(s.move_axes()[0] > 0.0, "after rotation the finger still steered the camera");
    }
}
