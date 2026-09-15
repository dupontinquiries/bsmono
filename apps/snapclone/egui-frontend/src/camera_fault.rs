//! camera_fault — why the camera did not open, and what to do about it.
//!
//! `camera_policy::CameraHealth` tracks a camera that WORKS and then degrades.
//! This is the other half: a camera that never opened at all, which on Linux
//! is where the platform spread actually bites. The same app runs on a
//! Fairphone under the XDG portal, on a Flatpak with no `/dev/video*` at all,
//! on a desktop whose portal is not installed, and on a PinePhone where the
//! ISP refuses a second stream — and "camera failed" is the wrong thing to
//! say to any of them.
//!
//! ## Consent is not a fallback
//!
//! The important rule here, and the reason this is a module rather than a
//! match arm: **a denied portal permission must never fall back to opening
//! `/dev/video0` directly.** The two routes are not interchangeable. Trying
//! the device node after the user said no is a consent bypass that happens to
//! compile, and on a non-sandboxed desktop it would even WORK, which is worse
//! — it would ship, and nobody would notice until someone read the code.
//!
//! [`Fault::may_fall_back`] encodes that: a MISSING portal is a routing
//! problem and may fall back; a REFUSED one is an answer and may not.
//!
//! ## Classification is from the platform's own words
//!
//! `ashpd`, PipeWire and V4L2 report failures as strings that differ per
//! distro, per portal backend and per kernel. Rather than matching exact
//! error types that only exist on one of them, [`classify`] matches on
//! substrings the platforms have in common, and everything unrecognised is
//! [`Fault::Unknown`] — which is honest, retryable, and says so.
//!
//! The one case the reference app (GNOME Snapshot) singles out is
//! `PortalError::NotAllowed`, for exactly the reason this whole module
//! exists: a denied permission is a different UI from a broken pipeline, and
//! conflating them tells a user to check their hardware when they need to
//! check their settings. See `docs/CAMERA_ACQUISITION.md`.

/// How we tried to get frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// `org.freedesktop.portal.Camera` → a PipeWire fd. The only route that
    /// works both sandboxed and not, so it is tried first everywhere.
    Portal,
    /// A PipeWire node reached without the portal — a host build on a desktop
    /// with no portal backend installed.
    PipeWireDirect,
    /// `/dev/video*` through V4L2. Last resort, host-only, and never reached
    /// from a refusal.
    V4l2,
}

impl Route {
    pub fn label(self) -> &'static str {
        match self {
            Route::Portal => "desktop portal",
            Route::PipeWireDirect => "PipeWire",
            Route::V4l2 => "/dev/video",
        }
    }

    /// The order to try. Portal first because it is the only one that works
    /// inside a Flatpak, and because it is the one that asks.
    pub fn ladder() -> [Route; 3] {
        [Route::Portal, Route::PipeWireDirect, Route::V4l2]
    }
}

/// Why acquisition failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    /// The user said no, or a policy said no for them.
    PermissionDenied,
    /// No portal backend on this system. A routing problem, not an answer.
    PortalUnavailable,
    /// The portal answered and reported no camera present.
    ///
    /// Checked AFTER access is granted, deliberately: on a device with no
    /// camera the access request still succeeds, so a build that checked
    /// presence first would report "denied" for "absent".
    NoCameraPresent,
    /// PipeWire itself is not running or not reachable.
    PipeWireUnavailable,
    /// The node existed at enumeration and is gone now — an unplugged USB
    /// camera, or a node torn down between listing and opening.
    NodeVanished,
    /// Another process holds the device.
    DeviceBusy { holder: Option<String> },
    /// The ISP cannot run this camera alongside one already streaming. The
    /// FP5 PIP limit; see `camera_select`'s capture groups.
    CaptureGroupConflict { blocking: Option<String> },
    /// The camera opened but offers no format we can consume.
    NoUsableFormat,
    /// Opened, but no frame arrived in time.
    Timeout { waited_ms: u32 },
    /// Unrecognised. Honest, and retryable.
    Unknown(String),
}

impl Fault {
    /// Is trying the SAME route again worth anything?
    ///
    /// A refusal and an absent camera are answers; retrying them just asks
    /// the same question. Busy and timeout are states that pass.
    pub fn retry_same_route(&self) -> bool {
        match self {
            Fault::PermissionDenied | Fault::NoCameraPresent | Fault::NoUsableFormat => false,
            Fault::PortalUnavailable | Fault::PipeWireUnavailable => false,
            Fault::DeviceBusy { .. }
            | Fault::CaptureGroupConflict { .. }
            | Fault::NodeVanished
            | Fault::Timeout { .. }
            | Fault::Unknown(_) => true,
        }
    }

    /// May we try a DIFFERENT route after this?
    ///
    /// **`PermissionDenied` is false and that is the point.** Falling back to
    /// `/dev/video0` after the user refused the portal is a consent bypass.
    /// It would work on a non-sandboxed desktop, which is precisely why it
    /// has to be refused in code rather than left to whoever writes the
    /// retry loop.
    ///
    /// `NoCameraPresent` is also false: the platform has told us there is no
    /// camera, and a different route will not conjure one.
    pub fn may_fall_back(&self) -> bool {
        !matches!(self, Fault::PermissionDenied | Fault::NoCameraPresent)
    }

    /// Is this the user's to fix, rather than ours or the machine's?
    ///
    /// Drives whether the UI offers a settings link or a retry button —
    /// the distinction the reference app makes and the one worth copying.
    pub fn is_user_actionable(&self) -> bool {
        matches!(self, Fault::PermissionDenied | Fault::DeviceBusy { .. }
                     | Fault::CaptureGroupConflict { .. })
    }

    /// One line for the person, naming the fix rather than the fault.
    pub fn remedy(&self) -> String {
        match self {
            Fault::PermissionDenied =>
                "Camera access was refused. Allow it in your system's privacy settings — \
                 this is a permission, not a hardware problem.".into(),
            Fault::PortalUnavailable =>
                "No desktop portal on this system, so the sandboxed camera route is \
                 unavailable. Trying the camera directly.".into(),
            Fault::NoCameraPresent =>
                "This device reports no camera.".into(),
            Fault::PipeWireUnavailable =>
                "PipeWire is not running, so no camera can be opened. Start it (or \
                 the pipewire user service) and try again.".into(),
            Fault::NodeVanished =>
                "The camera disappeared while opening it — unplugged, or claimed by \
                 something else. Reconnect it and try again.".into(),
            Fault::DeviceBusy { holder } => match holder {
                Some(h) => format!("The camera is already in use by {h}. Close it and try again."),
                None => "The camera is already in use by another program.".into(),
            },
            Fault::CaptureGroupConflict { blocking } => match blocking {
                Some(b) => format!("This camera cannot run at the same time as {b} — \
                                    the image processor handles one of them at a time."),
                None => "This camera cannot run alongside the one already streaming — \
                         the image processor handles one of them at a time.".into(),
            },
            Fault::NoUsableFormat =>
                "The camera offers no video format this build can read.".into(),
            Fault::Timeout { waited_ms } =>
                format!("The camera opened but sent no frames within {waited_ms} ms."),
            Fault::Unknown(why) =>
                format!("The camera could not be opened: {why}"),
        }
    }

    /// A short tag for logs and health records.
    pub fn tag(&self) -> &'static str {
        match self {
            Fault::PermissionDenied => "denied",
            Fault::PortalUnavailable => "no-portal",
            Fault::NoCameraPresent => "no-camera",
            Fault::PipeWireUnavailable => "no-pipewire",
            Fault::NodeVanished => "node-gone",
            Fault::DeviceBusy { .. } => "busy",
            Fault::CaptureGroupConflict { .. } => "capture-group",
            Fault::NoUsableFormat => "no-format",
            Fault::Timeout { .. } => "timeout",
            Fault::Unknown(_) => "unknown",
        }
    }
}

/// Classify a platform error string.
///
/// Substrings rather than typed errors on purpose: the same condition is
/// spelled differently by `ashpd`, by PipeWire and by the kernel, and differs
/// again between portal backends. Matching the words they share covers more
/// platforms than matching one of their type hierarchies, and anything
/// unmatched becomes [`Fault::Unknown`] with the original text kept.
///
/// Order matters — the checks run most-specific first, because "no such
/// device" appears inside several longer messages that mean something more
/// precise.
pub fn classify(route: Route, raw: &str) -> Fault {
    let s = raw.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| s.contains(n));

    // Permission first: it is the one that must never be confused with a
    // hardware fault, and some backends wrap it in generic wording.
    if has(&["notallowed", "not allowed", "permission denied", "access denied",
             "org.freedesktop.portal.error.notallowed", "cancelled by user",
             "request was denied"]) {
        return Fault::PermissionDenied;
    }
    // A portal that is not there reports as an unknown service/interface, NOT
    // as a refusal -- distinguishing these two is the module's main job.
    if route == Route::Portal
        && has(&["serviceunknown", "service unknown", "no such interface",
                 "was not provided by any .service", "name org.freedesktop.portal"]) {
        return Fault::PortalUnavailable;
    }
    if has(&["no camera", "no cameras", "is_present", "no camera present",
             "no video device", "no capture device"]) {
        return Fault::NoCameraPresent;
    }
    if has(&["pipewire is not running", "failed to connect to pipewire",
             "pw_context", "core error: connection refused", "no pipewire"]) {
        return Fault::PipeWireUnavailable;
    }
    if has(&["capture group", "capture-group", "isp busy", "concurrent stream",
             "cannot stream both"]) {
        return Fault::CaptureGroupConflict { blocking: None };
    }
    if has(&["busy", "ebusy", "resource busy", "device or resource busy",
             "already in use"]) {
        return Fault::DeviceBusy { holder: None };
    }
    if has(&["node removed", "node destroyed", "no such node", "node vanished",
             "enodev", "no such device"]) {
        return Fault::NodeVanished;
    }
    if has(&["no usable format", "unsupported format", "no matching caps",
             "caps intersection is empty", "einval: format"]) {
        return Fault::NoUsableFormat;
    }
    if has(&["timed out", "timeout", "etimedout"]) {
        return Fault::Timeout { waited_ms: 0 };
    }
    Fault::Unknown(raw.trim().to_string())
}

/// One attempt's outcome, for the log and the health record.
#[derive(Debug, Clone, PartialEq)]
pub struct Attempt {
    pub route: Route,
    pub fault: Fault,
}

/// Walk the route ladder, respecting what each fault permits.
///
/// `open` is called per route and returns `Ok(())` or the platform's error
/// text. Returns the attempts made, in order; empty means the first route
/// worked.
///
/// Stops early — rather than exhausting the ladder — whenever a fault says
/// the answer will not change. That is not an optimisation: continuing past a
/// `PermissionDenied` is the consent bypass described in the module header,
/// and continuing past `NoCameraPresent` produces two more misleading errors
/// for a machine that simply has no camera.
pub fn acquire<F>(mut open: F) -> Result<Route, Vec<Attempt>>
where
    F: FnMut(Route) -> Result<(), String>,
{
    let mut log = Vec::new();
    for route in Route::ladder() {
        match open(route) {
            Ok(()) => return Ok(route),
            Err(raw) => {
                let fault = classify(route, &raw);
                let stop = !fault.may_fall_back();
                log.push(Attempt { route, fault });
                if stop {
                    break;
                }
            }
        }
    }
    Err(log)
}

/// What to show after a failed [`acquire`].
///
/// The LAST fault is not the right one to report: the ladder ends on
/// `/dev/video`, so a machine with a denied portal and no device node would
/// tell the user about a missing file when the actual answer was the refusal
/// three steps earlier. The most user-actionable fault wins, and a refusal
/// outranks everything.
pub fn primary_fault(log: &[Attempt]) -> Option<&Fault> {
    log.iter()
        .find(|a| a.fault == Fault::PermissionDenied)
        .or_else(|| log.iter().find(|a| a.fault.is_user_actionable()))
        .or_else(|| log.iter().find(|a| !matches!(a.fault, Fault::Unknown(_))))
        .or_else(|| log.first())
        .map(|a| &a.fault)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE CONSENT RULE. Falling back to /dev/video after the user refused
    /// the portal would WORK on a non-sandboxed desktop, which is exactly why
    /// it has to be refused in code.
    #[test]
    fn a_refusal_never_falls_back_to_the_device_node() {
        let mut tried = Vec::new();
        let r = acquire(|route| {
            tried.push(route);
            Err("org.freedesktop.portal.Error.NotAllowed".to_string())
        });
        assert_eq!(tried, vec![Route::Portal], "the ladder must stop at the refusal");
        let log = r.unwrap_err();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].fault, Fault::PermissionDenied);
        assert!(!Fault::PermissionDenied.may_fall_back());
    }

    /// A MISSING portal is a routing problem and must fall through, which is
    /// the distinction that makes the rule above safe to hold.
    #[test]
    fn a_missing_portal_falls_through_to_the_other_routes() {
        let mut tried = Vec::new();
        let r = acquire(|route| {
            tried.push(route);
            if route == Route::Portal {
                Err("The name org.freedesktop.portal.Desktop was not provided by any .service files".into())
            } else {
                Ok(())
            }
        });
        assert_eq!(r, Ok(Route::PipeWireDirect));
        assert_eq!(tried, vec![Route::Portal, Route::PipeWireDirect]);
    }

    /// No camera is an answer, not a routing problem. Continuing produces two
    /// more misleading errors for a machine that simply has none.
    #[test]
    fn an_absent_camera_stops_the_ladder() {
        let r = acquire(|_| Err("no cameras available".into()));
        let log = r.unwrap_err();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].fault, Fault::NoCameraPresent);
    }

    /// The reference app's one singled-out case, and the reason for the
    /// module: a refusal must not read as a hardware fault.
    #[test]
    fn a_refusal_is_reported_as_a_permission_not_as_hardware() {
        for raw in ["org.freedesktop.portal.Error.NotAllowed",
                    "Access denied",
                    "Permission denied",
                    "the request was denied",
                    "cancelled by user"] {
            assert_eq!(classify(Route::Portal, raw), Fault::PermissionDenied, "{raw}");
        }
        let f = Fault::PermissionDenied;
        assert!(f.is_user_actionable());
        let r = f.remedy();
        assert!(r.contains("permission"), "must name it as a permission: {r}");
        assert!(r.contains("privacy settings"), "must point at the fix: {r}");
        assert!(r.contains("not a hardware problem"),
            "must actively deny the hardware reading, since that is the wrong \
             place this sends people: {r}");
    }

    /// Classification has to work across the spellings three different
    /// subsystems use for the same condition.
    #[test]
    fn the_platforms_different_wordings_land_on_the_same_fault() {
        assert_eq!(classify(Route::V4l2, "Device or resource busy (EBUSY)"),
                   Fault::DeviceBusy { holder: None });
        assert_eq!(classify(Route::PipeWireDirect, "camera is already in use"),
                   Fault::DeviceBusy { holder: None });
        assert_eq!(classify(Route::PipeWireDirect, "failed to connect to pipewire"),
                   Fault::PipeWireUnavailable);
        assert_eq!(classify(Route::V4l2, "ENODEV: no such device"), Fault::NodeVanished);
        assert_eq!(classify(Route::PipeWireDirect, "caps intersection is empty"),
                   Fault::NoUsableFormat);
    }

    /// "Denied" must win over "busy" even when the words co-occur, because
    /// the wrong one sends the user to the wrong place entirely.
    #[test]
    fn permission_outranks_every_other_match_in_one_message() {
        let f = classify(Route::Portal, "device busy: access denied by portal policy");
        assert_eq!(f, Fault::PermissionDenied, "permission must be checked first");
    }

    /// An unrecognised failure keeps the platform's own words rather than
    /// inventing a category.
    #[test]
    fn an_unrecognised_error_is_unknown_and_keeps_its_text() {
        let f = classify(Route::V4l2, "  weird vendor ioctl 0x5150 failed  ");
        assert_eq!(f, Fault::Unknown("weird vendor ioctl 0x5150 failed".into()));
        assert!(f.retry_same_route(), "unknown is worth retrying");
        assert!(f.may_fall_back());
        assert!(f.remedy().contains("0x5150"), "the original text survives to the user");
    }

    /// Retrying a refusal or an absence just asks the same question again.
    #[test]
    fn answers_are_not_retried_but_states_are() {
        for f in [Fault::PermissionDenied, Fault::NoCameraPresent, Fault::NoUsableFormat] {
            assert!(!f.retry_same_route(), "{f:?} is an answer");
        }
        for f in [Fault::DeviceBusy { holder: None },
                  Fault::Timeout { waited_ms: 5000 },
                  Fault::NodeVanished,
                  Fault::CaptureGroupConflict { blocking: None }] {
            assert!(f.retry_same_route(), "{f:?} is a passing state");
        }
    }

    /// THE REPORTING RULE. The ladder ends on /dev/video, so the LAST fault
    /// is the wrong one to show: a denied portal plus a missing device node
    /// would tell the user about a missing file.
    #[test]
    fn the_reported_fault_is_the_actionable_one_not_the_last_one() {
        let log = vec![
            Attempt { route: Route::Portal, fault: Fault::PermissionDenied },
            Attempt { route: Route::V4l2, fault: Fault::NodeVanished },
        ];
        assert_eq!(primary_fault(&log), Some(&Fault::PermissionDenied));

        // With nothing user-actionable, a classified fault beats an Unknown.
        let log = vec![
            Attempt { route: Route::Portal, fault: Fault::Unknown("odd".into()) },
            Attempt { route: Route::PipeWireDirect, fault: Fault::PipeWireUnavailable },
        ];
        assert_eq!(primary_fault(&log), Some(&Fault::PipeWireUnavailable));

        assert_eq!(primary_fault(&[]), None);
    }

    /// The capture-group limit is ours to carry (the reference app shows one
    /// camera at a time and never meets it), and it is user-actionable: the
    /// fix is closing the other stream.
    #[test]
    fn a_capture_group_conflict_explains_itself_and_is_actionable() {
        let f = classify(Route::Portal, "cannot stream both: capture group in use");
        assert!(matches!(f, Fault::CaptureGroupConflict { .. }));
        assert!(f.is_user_actionable());
        let named = Fault::CaptureGroupConflict { blocking: Some("the rear camera".into()) };
        assert!(named.remedy().contains("the rear camera"), "{}", named.remedy());
    }

    /// Every fault must say something useful; a blank remedy is a dialog with
    /// no text in it.
    #[test]
    fn every_fault_has_a_remedy_and_a_tag() {
        let all = [
            Fault::PermissionDenied, Fault::PortalUnavailable, Fault::NoCameraPresent,
            Fault::PipeWireUnavailable, Fault::NodeVanished,
            Fault::DeviceBusy { holder: None },
            Fault::CaptureGroupConflict { blocking: None },
            Fault::NoUsableFormat, Fault::Timeout { waited_ms: 1 },
            Fault::Unknown("x".into()),
        ];
        let mut tags = Vec::new();
        for f in &all {
            assert!(f.remedy().len() > 12, "{f:?} has no useful remedy");
            assert!(!tags.contains(&f.tag()), "duplicate tag {}", f.tag());
            tags.push(f.tag());
        }
    }

    /// The happy path costs nothing and logs nothing.
    #[test]
    fn a_working_first_route_returns_immediately() {
        let mut calls = 0;
        let r = acquire(|_| { calls += 1; Ok(()) });
        assert_eq!(r, Ok(Route::Portal));
        assert_eq!(calls, 1, "no other route is touched");
    }
}
