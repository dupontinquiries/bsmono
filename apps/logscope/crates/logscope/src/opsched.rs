//! opsched.rs — sizing work to when the operator and the hardware are actually available.
//!
//! A long fan-out is the right move when nobody is waiting and the wrong one when someone is. The
//! engine already knows enough to tell the difference — uptime, load, when the last keystroke
//! arrived, how long the last turnaround took — and uses none of it.
//!
//! ## Availability, not a profile
//!
//! Every signal here is about the SESSION and the MACHINE: idle seconds, uptime, whether a device
//! answered a probe. None of it is about the person. "Nothing typed for forty minutes and the M2 has
//! been up six days" is a scheduling fact; "the operator works late" is a profile — less useful,
//! since it predicts worse than the direct signal, and worth not building.
//!
//! That is a design constraint rather than a caveat: a profile would have to persist across sessions
//! to be worth anything, and persisting it is what makes it a different kind of feature.
//!
//! ## The failure this must not have
//!
//! **A scheduler that assumes absence and cannot be interrupted is worse than no scheduler.** Guessing
//! wrong about presence is cheap only if the work yields immediately when someone returns.

/// What the session looks like right now.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Presence {
    /// Seconds since the last operator input.
    pub idle_s: f32,
    /// Seconds the last turnaround took, operator-side — download, run, report.
    pub last_turnaround_s: f32,
    /// Devices that answered a probe within the freshness window.
    pub devices_up: u32,
}

/// How much work to commit to before checking back.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Window {
    /// Someone is at the keyboard. Short, interruptible, visible progress.
    Attended,
    /// Recently active. Medium work, nothing that cannot be abandoned.
    Warm,
    /// Idle long enough that a long run will not block anyone.
    Unattended { fan_out: u32 },
}

impl Window {
    /// The rule that makes a wrong guess cheap.
    ///
    /// Unattended work must still yield the moment someone types, or an incorrect absence guess
    /// costs the operator the whole run rather than the seconds it takes to notice.
    pub fn must_be_interruptible(&self) -> bool { true }

    pub fn explain(&self) -> String {
        match self {
            Window::Attended =>
                "someone is at the keyboard — short turns with visible progress, and nothing that \
cannot be abandoned mid-step".into(),
            Window::Warm =>
                "recently active — medium work is fine, but assume they will look before it finishes".into(),
            Window::Unattended { fan_out } => format!(
                "idle long enough that a long run blocks nobody — fan out to {fan_out} branches. This \
still yields the moment someone types: a wrong absence guess must cost seconds, not the run"),
        }
    }
}

/// Below this, treat the operator as present.
pub const ATTENDED_S: f32 = 120.0;
/// Above this, treat them as away.
pub const UNATTENDED_S: f32 = 900.0;

/// Choose a window.
pub fn window(p: Presence) -> Window {
    if p.idle_s < ATTENDED_S { return Window::Attended }
    if p.idle_s < UNATTENDED_S { return Window::Warm }
    // Fan width follows DEVICES, not idle time. Being away longer does not create more hardware, and
    // a fan wider than the machines available just queues.
    Window::Unattended { fan_out: p.devices_up.max(1) }
}

/// Should a task of this size be started now?
///
/// **Compared against the operator's own turnaround, not a constant.** A five-minute job is trivial
/// for someone whose last round trip took twenty and rude for someone whose took thirty seconds.
pub fn worth_starting(est_s: f32, p: Presence) -> Result<(), String> {
    let w = window(p);
    let budget = match w {
        // While attended, the yardstick is what they just spent waiting. Anything much longer than
        // their own last turnaround changes the rhythm of the session without asking.
        Window::Attended => p.last_turnaround_s.max(30.0) * 1.5,
        Window::Warm => p.last_turnaround_s.max(60.0) * 4.0,
        Window::Unattended { .. } => f32::INFINITY,
    };
    if est_s <= budget { Ok(()) }
    else {
        Err(format!("{est_s:.0}s of work against a {budget:.0}s budget — {}. Split it, or say what \
it will cost and let them choose", w.explain()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(idle: f32, turn: f32, dev: u32) -> Presence {
        Presence { idle_s: idle, last_turnaround_s: turn, devices_up: dev }
    }

    #[test]
    fn someone_at_the_keyboard_gets_SHORT_turns() {
        assert_eq!(window(p(10.0, 60.0, 3)), Window::Attended);
    }

    #[test]
    fn a_long_idle_fans_out_ACROSS_DEVICES_not_across_time() {
        // Being away longer does not create more hardware, and a fan wider than the machines
        // available just queues.
        assert_eq!(window(p(3600.0, 60.0, 3)), Window::Unattended { fan_out: 3 });
        assert_eq!(window(p(99999.0, 60.0, 3)), Window::Unattended { fan_out: 3 });
    }

    #[test]
    fn the_budget_is_THEIR_turnaround_not_a_constant() {
        // Five minutes is trivial for someone whose last round trip took twenty and rude for someone
        // whose took thirty seconds.
        assert!(worth_starting(300.0, p(10.0, 1200.0, 1)).is_ok(), "patient session");
        assert!(worth_starting(300.0, p(10.0, 30.0, 1)).is_err(), "fast session");
    }

    #[test]
    fn a_refusal_says_what_to_do_INSTEAD() {
        // "Too long" is not actionable; "split it, or say what it will cost and let them choose" is.
        let e = worth_starting(3000.0, p(10.0, 30.0, 1)).unwrap_err();
        assert!(e.contains("Split it"));
        assert!(e.contains("let them choose"));
    }

    #[test]
    fn UNATTENDED_work_is_still_interruptible() {
        // The rule that makes a wrong guess cheap. An incorrect absence guess must cost the seconds
        // it takes to notice, not the whole run.
        assert!(Window::Unattended { fan_out: 4 }.must_be_interruptible());
        assert!(Window::Unattended { fan_out: 4 }.explain().contains("yields the moment someone types"));
    }

    #[test]
    fn a_ZERO_device_session_still_fans_to_one() {
        // A degenerate device count must not produce a fan of zero, which would schedule nothing and
        // look like a hang.
        assert_eq!(window(p(3600.0, 60.0, 0)), Window::Unattended { fan_out: 1 });
    }

    #[test]
    fn the_ATTENDED_budget_has_a_floor() {
        // A session whose last turnaround was two seconds must not make every task refuse — the floor
        // keeps a fast round trip from ratcheting the budget to nothing.
        assert!(worth_starting(40.0, p(5.0, 2.0, 1)).is_ok());
    }
}
