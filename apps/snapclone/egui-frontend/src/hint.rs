//! hint — the one thing worth telling the user right now.
//!
//! The app already knows a great deal about its own situation: whether a
//! camera was found and whether the pipeline is live, whether the DHT has
//! anyone to talk to and whether anyone still answers, whether our public
//! address is known, whether a send is stuck. Each of those was logged to
//! stderr or parked in a note field, and none of it reached the screen
//! as a sentence. So a phone with no camera showed a blank viewfinder,
//! and a phone whose peers had all gone quiet showed yesterday's peer
//! count.
//!
//! This is the decision: given the state, what is the single most useful
//! line, if any. It is a pure function so the priority order is a test,
//! not a habit. The bar that shows it is the thinnest chrome the app
//! draws — one line, a colour for severity, tap to go where it helps.
//!
//! ## Priority, and why this order
//!
//! Blocking problems before degraded ones before status. A missing
//! identity stops everything, so it beats a missing camera; a missing
//! camera stops the app's reason to exist, so it beats a quiet network;
//! and a network that has gone quiet is worth more than a peer count.
//! Positive status is shown only when nothing is wrong and only when it
//! is new information — "reachable from outside" the first time, not
//! forever.
//!
//! ## Phone and desktop
//!
//! Same decision, different shell. On a phone the bar sits above the tab
//! bar and its tap goes to the page that fixes the problem. On a desktop
//! the same line sits in the status strip where a business user expects
//! the state of the system to be, and it says a little more (the count,
//! the address) because there is room.

/// How urgent the line is, which is also its colour.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level { Blocking, Degraded, Info }

/// Where a tap on the hint should take the user: a page index of the
/// nav, or nowhere.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GoTo { Camera, Peers, Settings, Inbox, Nowhere }

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Hint {
    pub level: Level,
    /// The phone line. Short: it shares a row with nothing but itself.
    pub short: String,
    /// The desktop line. May carry the number or the address.
    pub long: String,
    pub go: GoTo,
}

/// Everything the decision reads. A struct rather than the app, so a test
/// can state a situation in five lines and so the decision cannot reach
/// into state it was not given.
#[derive(Clone, Debug, Default)]
pub struct Situation {
    pub has_identity: bool,
    pub cameras: usize,
    /// `capture::Status::Exhausted` — every pipeline candidate failed.
    pub capture_exhausted: bool,
    /// `capture::Status::Trying` — a candidate is being opened now.
    pub capture_trying: bool,
    pub dht_running: bool,
    pub dht_peers: usize,
    /// The last keepalive round: (pinged, answered).
    pub keepalive: Option<(usize, usize)>,
    /// Our punch socket as seen from outside, once STUN answered.
    pub reflexive: Option<String>,
    /// STUN asked and nobody answered.
    pub stun_failed: bool,
    pub in_call: bool,
    /// Call participants with no known address.
    pub unreachable_participants: usize,
    pub sends_failed: usize,
    pub sends_pending: usize,
    pub unread: usize,
    /// The user has already seen the "reachable" line this session.
    pub reachable_shown: bool,
}

/// The decision.
pub fn hint(s: &Situation) -> Option<Hint> {
    let h = |level, short: &str, long: String, go| Some(Hint { level, short: short.into(), long, go });

    if !s.has_identity {
        return h(Level::Blocking, "no identity — set one up",
                 "No identity yet: nothing can be published or received until one is set up.".into(),
                 GoTo::Settings);
    }
    if s.cameras == 0 {
        return h(Level::Blocking, "no camera found",
                 "No camera found. On a phone, check that PipeWire is running and libcamera sees the sensors; \
                  on a desktop, plug one in.".into(),
                 GoTo::Settings);
    }
    if s.capture_exhausted {
        return h(Level::Blocking, "camera could not be opened",
                 "Every way of opening the camera failed. Another app may hold it; the log in Settings says \
                  what was tried.".into(),
                 GoTo::Settings);
    }
    if s.in_call && s.unreachable_participants > 0 {
        let n = s.unreachable_participants;
        return h(Level::Degraded, &format!("{n} in the call cannot be reached"),
                 format!("{n} participant{} in the call {} no known address yet — they will not hear this until \
                          their record arrives or a punch lands.",
                         if n == 1 { "" } else { "s" }, if n == 1 { "has" } else { "have" }),
                 GoTo::Peers);
    }
    if s.sends_failed > 0 {
        let n = s.sends_failed;
        return h(Level::Degraded, &format!("{n} send{} failed — tap to retry", if n == 1 { "" } else { "s" }),
                 format!("{n} snap{} could not be delivered. Retry from the inbox; the peer may be offline or \
                          behind a network that needs the path opened first.", if n == 1 { "" } else { "s" }),
                 GoTo::Inbox);
    }
    if let Some((pinged, 0)) = s.keepalive {
        if pinged > 0 {
            return h(Level::Degraded, "network changed — reconnecting",
                     format!("None of the {pinged} peers we were keeping warm answered. The network probably \
                              changed under us; the node is re-bootstrapping."),
                     GoTo::Peers);
        }
    }
    if s.dht_running && s.dht_peers == 0 {
        return h(Level::Degraded, "looking for peers",
                 "No DHT peer yet. Bootstrap servers and every remembered peer are being tried; on a network \
                  that blocks them, a peer on the same LAN is enough.".into(),
                 GoTo::Peers);
    }
    if s.stun_failed {
        return h(Level::Degraded, "public address unknown",
                 "No STUN server answered, so peers outside this network cannot be told where to reach us. \
                  LAN peers are unaffected.".into(),
                 GoTo::Settings);
    }
    if s.capture_trying {
        return h(Level::Info, "opening camera…", "Opening the camera.".into(), GoTo::Camera);
    }
    if let (Some(r), false) = (&s.reflexive, s.reachable_shown) {
        return h(Level::Info, "reachable from outside",
                 format!("Reachable from outside at {r}; peers on other networks can open a path to us."),
                 GoTo::Nowhere);
    }
    if s.unread > 0 {
        let n = s.unread;
        return h(Level::Info, &format!("{n} new"), format!("{n} unread snap{}.", if n == 1 { "" } else { "s" }),
                 GoTo::Inbox);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fine() -> Situation {
        Situation { has_identity: true, cameras: 1, dht_running: true, dht_peers: 3,
                    keepalive: Some((3, 3)), reflexive: Some("203.0.113.7:51234".into()),
                    reachable_shown: true, ..Default::default() }
    }

    #[test]
    fn a_healthy_app_says_nothing() {
        assert_eq!(hint(&fine()), None);
    }

    #[test]
    fn blocking_beats_degraded_beats_status() {
        let mut s = fine();
        s.unread = 2;                       // status
        s.keepalive = Some((3, 0));         // degraded
        s.cameras = 0;                      // blocking
        s.has_identity = false;             // blocking, and first
        assert_eq!(hint(&s).unwrap().go, GoTo::Settings);
        assert!(hint(&s).unwrap().short.contains("identity"));
        s.has_identity = true;
        assert!(hint(&s).unwrap().short.contains("camera"), "no camera beats everything after identity");
        s.cameras = 1;
        assert_eq!(hint(&s).unwrap().level, Level::Degraded);
        assert!(hint(&s).unwrap().short.contains("network changed"));
        s.keepalive = Some((3, 3));
        assert_eq!(hint(&s).unwrap().short, "2 new");
    }

    #[test]
    fn the_keepalive_only_speaks_when_somebody_was_pinged_and_nobody_answered() {
        let mut s = fine();
        s.keepalive = Some((0, 0));
        assert_eq!(hint(&s), None, "a node with nobody to ping is not a changed network");
        s.keepalive = Some((2, 1));
        assert_eq!(hint(&s), None, "one answer is a live network");
    }

    #[test]
    fn reachable_is_said_once() {
        let mut s = fine();
        s.reachable_shown = false;
        assert_eq!(hint(&s).unwrap().short, "reachable from outside");
        assert!(hint(&s).unwrap().long.contains("203.0.113.7:51234"), "the desktop line carries the address");
        s.reachable_shown = true;
        assert_eq!(hint(&s), None);
    }

    #[test]
    fn a_call_with_an_unreachable_participant_outranks_a_failed_send() {
        let mut s = fine();
        s.sends_failed = 1;
        s.in_call = true;
        s.unreachable_participants = 2;
        let h = hint(&s).unwrap();
        assert_eq!(h.go, GoTo::Peers);
        assert_eq!(h.short, "2 in the call cannot be reached");
        s.in_call = false;
        assert_eq!(hint(&s).unwrap().go, GoTo::Inbox);
        assert_eq!(hint(&s).unwrap().short, "1 send failed — tap to retry");
    }

    #[test]
    fn a_fresh_node_says_it_is_looking_and_a_silent_stun_says_so_after() {
        let mut s = fine();
        s.dht_peers = 0;
        s.keepalive = None;
        s.stun_failed = true;
        assert_eq!(hint(&s).unwrap().short, "looking for peers");
        s.dht_peers = 1;
        assert_eq!(hint(&s).unwrap().short, "public address unknown");
    }
}
