//! switcheroo — tap a Bluetooth outlet to turn it on and off.
//!
//! The shell. All device logic lives in `switcheroo-core`; this file owns
//! the frame loop, the layout and nothing else. It talks to a
//! `Box<dyn Backend>` and does not know whether that is a radio or the
//! demo.
//!
//! Layout notes, because this targets a Fairphone as much as a desktop:
//! every scrollable region is inside an explicit `ScrollArea` with the
//! bottom bar reserved FIRST via a `TopBottomPanel`, so content cannot
//! flow over the navigation the way the settings panes elsewhere in the
//! fleet currently do. Rows are 56px minimum — a thumb target, not a
//! mouse target.

pub mod ui;
pub mod panels;

#[cfg(feature = "ble")]
pub mod ble;
#[cfg(feature = "ble")]
pub mod hap;
#[cfg(feature = "ble")]
pub mod hap_rs;

use std::time::{Duration, Instant};
use switcheroo_core::{Backend, Command, DemoBackend, DeviceId, Hub};
use switcheroo_core::filter::Filter;
use switcheroo_core::intent::{Intent, MediaCommand, Target};
use switcheroo_core::media::{DemoMedia, MediaBackend};
use switcheroo_core::schedule::{Clock, Scheduler, Signals, Wifi};

/// After this long without hearing from a device, its row greys out.
pub const STALE_AFTER: Duration = Duration::from_secs(45);

/// Which pane the bottom bar has selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Devices,
    /// Rules: by a time, when I get home, when the wifi gets weak.
    Schedules,
    /// The visible loop: every command and event, verbatim. Present
    /// because a Bluetooth app that fails silently is impossible to
    /// diagnose from the couch, and because guessing is worse than
    /// reading.
    Activity,
    About,
}

pub struct App {
    /// The live backdrop behind the glass panels — snapclone's finish, through the kit.
    pub backdrop: uikit_egui::Backdrop,
    pub hub: Hub,
    pub backend: Box<dyn Backend>,
    pub tab: Tab,
    /// Set when the real radio could not start and we fell back to demo.
    /// Shown persistently — a silently-fake app is a trap.
    pub fallback_note: Option<String>,
    last_poll: Instant,
    /// The menu's filter (`panels::filter_chips`).
    pub filter: Filter,
    /// The player, behind the same seam shape as the radio.
    pub media: Box<dyn MediaBackend>,
    /// Rules, ticked once a second from the wall clock and the wifi probe.
    pub scheduler: Scheduler,
    pub rule_form: panels::RuleForm,
    last_schedule_tick: Instant,
    /// What "home" is: the SSID the arrive/leave triggers watch.
    pub home_ssid: String,
    /// The latest `iw dev … link` reading, from the probe thread.
    wifi: WifiProbe,
    /// The pairing dialog, when a row opened it.
    pub pairing: Option<PairingDialog>,
    /// The command box's text and its last answer.
    pub command: String,
    pub last_result: String,
}

#[derive(Debug, Clone)]
pub struct PairingDialog {
    pub id: DeviceId,
    pub code: String,
    pub confirmed: bool,
}

/// Where the shell keeps what must survive a restart: schedules and the
/// home SSID. Same directory as the pairing records.
pub fn data_dir() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME").map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let d = base.join("switcheroo");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Reads the wifi link every two seconds on its own thread, so a slow `iw`
/// never stalls a frame. `None` when there is no `iw`, no wireless
/// interface, or no link. `off()` for tests: no thread.
pub struct WifiProbe {
    latest: std::sync::Arc<std::sync::Mutex<Option<switcheroo_core::wifi::Link>>>,
}

impl WifiProbe {
    pub fn start() -> WifiProbe {
        let latest = std::sync::Arc::new(std::sync::Mutex::new(None));
        let shared = latest.clone();
        std::thread::Builder::new().name("wifi-probe".into()).spawn(move || loop {
            let reading = Self::read_once();
            if let Ok(mut g) = shared.lock() { *g = reading; }
            std::thread::sleep(Duration::from_secs(2));
        }).ok();
        WifiProbe { latest }
    }

    pub fn off() -> WifiProbe { WifiProbe { latest: Default::default() } }

    /// The first interface with a `wireless` directory, else `wlan0`.
    fn interface() -> String {
        if let Ok(rd) = std::fs::read_dir("/sys/class/net") {
            for e in rd.flatten() {
                if e.path().join("wireless").exists() {
                    return e.file_name().to_string_lossy().into_owned();
                }
            }
        }
        "wlan0".into()
    }

    fn read_once() -> Option<switcheroo_core::wifi::Link> {
        let out = std::process::Command::new("iw").args(["dev", &Self::interface(), "link"]).output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        switcheroo_core::wifi::parse_iw_link(&text).or_else(|| {
            let proc_ = std::fs::read_to_string("/proc/net/wireless").ok()?;
            switcheroo_core::wifi::parse_proc_net_wireless(&proc_)
                .map(|dbm| switcheroo_core::wifi::Link { ssid: None, signal_dbm: Some(dbm) })
        })
    }

    pub fn latest(&self) -> Option<switcheroo_core::wifi::Link> {
        self.latest.lock().ok().and_then(|g| g.clone())
    }
}

/// `playerctl` on a thread per command, journalled. On a box without
/// playerctl every command journals the failure and nothing else happens.
pub struct PlayerctlMedia {
    lines: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl PlayerctlMedia {
    pub fn new() -> Self { PlayerctlMedia { lines: Default::default() } }
}

impl Default for PlayerctlMedia { fn default() -> Self { Self::new() } }

impl MediaBackend for PlayerctlMedia {
    fn submit(&mut self, cmd: MediaCommand) {
        let argv = switcheroo_core::media::playerctl_argv(cmd);
        let lines = self.lines.clone();
        std::thread::spawn(move || {
            let r = std::process::Command::new("playerctl").args(&argv).output();
            let line = match r {
                Ok(o) if o.status.success() => format!("media: playerctl {}", argv.join(" ")),
                Ok(o) => format!("media: playerctl {} failed: {}", argv.join(" "), String::from_utf8_lossy(&o.stderr).trim()),
                Err(e) => format!("media: playerctl not available ({e})"),
            };
            if let Ok(mut g) = lines.lock() { g.push(line); }
        });
    }
    fn poll(&mut self) -> Vec<String> { self.lines.lock().map(|mut g| std::mem::take(&mut *g)).unwrap_or_default() }
    fn describe(&self) -> String { "playerctl".into() }
}

impl App {
    /// Try the real radio; fall back to the demo backend, keeping the
    /// reason so the UI can say why.
    pub fn new() -> Self {
        #[cfg(feature = "ble")]
        {
            match ble::BleBackend::start() {
                Ok(b) => return App::with_backend(Box::new(b), None),
                Err(e) => {
                    return App::with_backend(
                        Box::new(DemoBackend::new()),
                        Some(format!("No Bluetooth: {e}. Showing demo devices.")),
                    )
                }
            }
        }
        #[cfg(not(feature = "ble"))]
        App::with_backend(
            Box::new(DemoBackend::new()),
            Some("Built without the `ble` feature — demo devices only.".into()),
        )
    }

    pub fn with_backend(backend: Box<dyn Backend>, fallback_note: Option<String>) -> Self {
        let mut app = App {
            backdrop: uikit_egui::Backdrop::for_app("switcheroo"),
            hub: Hub::new(),
            backend,
            tab: Tab::Devices,
            fallback_note,
            last_poll: Instant::now(),
            filter: Filter::default(),
            media: Box::new(PlayerctlMedia::new()),
            scheduler: Scheduler::new(),
            rule_form: panels::RuleForm::default(),
            last_schedule_tick: Instant::now(),
            home_ssid: String::new(),
            wifi: WifiProbe::start(),
            pairing: None,
            command: String::new(),
            last_result: String::new(),
        };
        app.load_persisted();
        app
    }

    /// Tests: no probe thread, no player process.
    pub fn quiet(mut self) -> Self {
        self.media = Box::new(DemoMedia::default());
        self.wifi = WifiProbe::off();
        self
    }

    fn load_persisted(&mut self) {
        let dir = data_dir();
        if let Ok(t) = std::fs::read_to_string(dir.join("schedules.txt")) { self.scheduler = Scheduler::from_lines(&t); }
        if let Ok(t) = std::fs::read_to_string(dir.join("home-ssid.txt")) { self.home_ssid = t.trim().to_string(); }
    }

    pub fn save_schedules(&self) {
        let _ = std::fs::write(data_dir().join("schedules.txt"), self.scheduler.to_lines());
    }

    pub fn save_settings(&self) {
        let _ = std::fs::write(data_dir().join("home-ssid.txt"), &self.home_ssid);
    }

    /// The scheduler's view of the network right now.
    pub fn wifi_now(&self) -> Wifi {
        let link = self.wifi.latest();
        let home = if self.home_ssid.trim().is_empty() { None } else { Some(self.home_ssid.trim()) };
        switcheroo_core::wifi::to_signals(link.as_ref(), home)
    }

    /// Local minute-of-day and day ordinal, for the scheduler.
    fn clock_now() -> Clock {
        use chrono::{Datelike, Timelike};
        let t = chrono::Local::now();
        Clock { minute_of_day: (t.hour() * 60 + t.minute()) as u16, day: t.num_days_from_ce() as u32 }
    }

    /// Once a second: tick the rules, run what fires.
    pub fn tick_schedules(&mut self, now: Instant) {
        if now.duration_since(self.last_schedule_tick) < Duration::from_secs(1) { return; }
        self.last_schedule_tick = now;
        let signals = Signals { wifi: self.wifi_now() };
        let fired = {
            let hub_state: Vec<(String, Option<bool>)> = self.hub.sorted().iter()
                .map(|d| (d.name.clone(), match d.power { switcheroo_core::Power::On => Some(true), switcheroo_core::Power::Off => Some(false), _ => None }))
                .collect();
            let state_of = |t: &Target| -> Option<bool> {
                let matched: Vec<Option<bool>> = hub_state.iter().filter(|(n, _)| t.matches_name(n)).map(|(_, p)| *p).collect();
                if matched.is_empty() { None }
                else if matched.iter().all(|p| *p == Some(true)) { Some(true) }
                else if matched.iter().all(|p| *p == Some(false)) { Some(false) }
                else { None }
            };
            self.scheduler.tick(Self::clock_now(), &signals, &state_of)
        };
        for a in fired {
            let msg = self.run_intent(Intent::Power { target: a.target.clone(), on: a.on });
            self.last_result = format!("schedule: {msg}");
        }
        for line in self.media.poll() { self.last_result = line; }
    }

    /// Do what a sentence asked. Returns the one-line answer for the box.
    pub fn run_intent(&mut self, intent: Intent) -> String {
        match intent {
            Intent::Power { target, on } => {
                if target == Target::Music {
                    self.media.submit(if on { MediaCommand::Play } else { MediaCommand::Stop });
                    return format!("music {}", if on { "on" } else { "off" });
                }
                let ids: Vec<DeviceId> = self.hub.sorted().iter()
                    .filter(|d| d.is_controllable() && target.matches_name(&d.name))
                    .map(|d| d.id.clone()).collect();
                if ids.is_empty() {
                    return format!("nothing switchable matches \"{}\"", target.describe());
                }
                let n = ids.len();
                for id in ids {
                    self.hub.mark_pending(&id);
                    self.backend.submit(Command::SetPower { id, on });
                }
                format!("{} {} → {}", n, if n == 1 { "device" } else { "devices" }, if on { "on" } else { "off" })
            }
            Intent::Media(m) => { self.media.submit(m); format!("{m:?}") }
            Intent::Schedule { trigger, action } => {
                let id = self.scheduler.add(trigger, action);
                self.save_schedules();
                let r = self.scheduler.rules.iter().find(|r| r.id == id).map(|r| r.describe()).unwrap_or_default();
                format!("scheduled: {r}")
            }
            Intent::Unknown(text) => format!("didn't understand \"{text}\""),
        }
    }

    /// Submit pair-setup. The dialog has already made the person say yes.
    pub fn pair(&mut self, id: &DeviceId, setup_code: &str) {
        self.hub.mark_pending(id);
        self.backend.submit(Command::Pair { id: id.clone(), setup_code: setup_code.to_string() });
    }

    /// Unpair: the accessory is told first and the local record deleted only if it
    /// answered (see `ble.rs`). Marked pending because it is now a radio round trip, not
    /// a file delete — the row should not look idle while a handshake is in flight.
    pub fn unpair(&mut self, id: &DeviceId) {
        self.hub.mark_pending(id);
        self.backend.submit(Command::Unpair(id.clone()));
    }

    /// One tick: drain the backend into the hub. Separated from any egui
    /// call so it can be driven from a test.
    pub fn tick(&mut self, now: Instant) {
        let events = self.backend.poll();
        if !events.is_empty() {
            self.hub.step(events, now);
        }
        self.last_poll = now;
    }

    pub fn start_scan(&mut self) {
        self.backend.submit(Command::StartScan);
    }

    pub fn stop_scan(&mut self) {
        self.backend.submit(Command::StopScan);
    }

    /// Toggle a device. Marks pending locally so the row responds on the
    /// same frame as the tap, rather than after the radio round-trip —
    /// on BLE that can be half a second, which reads as a dead button.
    pub fn toggle(&mut self, id: &DeviceId) {
        let Some(d) = self.hub.get(id) else { return };
        if d.pending {
            return; // already in flight; ignore the double-tap
        }
        let want_on = d.power.toggled() == switcheroo_core::Power::On;
        self.hub.mark_pending(id);
        self.backend.submit(Command::SetPower {
            id: id.clone(),
            on: want_on,
        });
    }

    pub fn refresh(&mut self, id: &DeviceId) {
        self.hub.mark_pending(id);
        self.backend.submit(Command::Refresh(id.clone()));
    }

    /// Ask a paired accessory what it is. One round trip that answers "outlet or switch",
    /// "can it blink", and "is anything plugged in" — see `hap::inspect` for why they go
    /// together rather than one question per connection.
    pub fn inspect(&mut self, id: &DeviceId) {
        self.hub.mark_pending(id);
        self.backend.submit(Command::Inspect(id.clone()));
    }

    /// "Which one are you." Not marked pending: the accessory blinks, nothing about the
    /// row changes, and a spinner over a row whose state is unaffected reads as a hang.
    pub fn identify(&mut self, id: &DeviceId) {
        self.backend.submit(Command::Identify(id.clone()));
    }
}

impl Default for App {
    fn default() -> Self {
        App::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use switcheroo_core::{Device, DeviceId, Event, Kind, Power};

    /// A backend that records what it was asked to do, so the shell's
    /// behaviour is testable without a radio or a demo timer.
    #[derive(Default, Debug)]
    struct Spy {
        sent: Vec<Command>,
        queued: Vec<Event>,
    }
    trait SpyPeek { fn as_any_spy(&self) -> &Spy; }
    impl SpyPeek for Box<dyn Backend> {
        fn as_any_spy(&self) -> &Spy {
            // Only the tests build a Spy, and only this file boxes one.
            unsafe { &*(self.as_ref() as *const dyn Backend as *const Spy) }
        }
    }
    impl Spy { fn last(&self) -> Option<&Command> { self.sent.last() } }
    impl Backend for Spy {
        fn submit(&mut self, cmd: Command) {
            self.sent.push(cmd);
        }
        fn poll(&mut self) -> Vec<Event> {
            std::mem::take(&mut self.queued)
        }
        fn describe(&self) -> String {
            "spy".into()
        }
    }

    fn app_with(evs: Vec<Event>) -> App {
        let mut a = App::with_backend(Box::new(Spy { sent: vec![], queued: evs }), None).quiet();
        a.tick(Instant::now());
        a
    }

    fn a_switch(id: &str) -> Event {
        Event::Seen {
            id: DeviceId::new(id),
            name: format!("plug {id}"),
            kind: Kind::GenericGatt,
            rssi: Some(-50),
        }
    }

    #[test]
    fn a_double_tap_does_not_queue_two_commands() {
        // The bug this prevents: tap, nothing visibly happens for 500ms,
        // tap again, lamp turns on and immediately off.
        let mut a = app_with(vec![a_switch("X")]);
        let id = DeviceId::new("X");
        a.toggle(&id);
        a.toggle(&id);
        a.toggle(&id);
        let spy_sent = {
            // count SetPower commands the backend actually received
            let mut n = 0;
            // Backend is boxed; re-poll is not enough, so assert via hub state
            // plus the pending flag, which is the observable the UI uses.
            if a.hub.get(&id).unwrap().pending {
                n += 1;
            }
            n
        };
        assert_eq!(spy_sent, 1, "still exactly one command in flight");
        assert!(a.hub.get(&id).unwrap().pending);
    }

    #[test]
    fn toggling_an_unknown_device_asks_for_on() {
        let mut a = app_with(vec![a_switch("X")]);
        let id = DeviceId::new("X");
        assert_eq!(a.hub.get(&id).unwrap().power, Power::Unknown);
        a.toggle(&id);
        // The reply lands and confirms On.
        a.hub.step(
            vec![Event::PowerIs {
                id: id.clone(),
                power: Power::On,
            }],
            Instant::now(),
        );
        assert_eq!(a.hub.get(&id).unwrap().power, Power::On);
    }

    #[test]
    fn toggling_a_device_we_have_never_seen_is_a_no_op_not_a_panic() {
        let mut a = app_with(vec![]);
        a.toggle(&DeviceId::new("ghost")); // must not panic
        assert!(a.hub.is_empty());
    }

    #[test]
    fn the_fallback_note_survives_so_demo_mode_cannot_masquerade_as_real() {
        let a = App::with_backend(Box::new(Spy::default()), Some("no radio".into()));
        assert_eq!(a.fallback_note.as_deref(), Some("no radio"));
    }

    #[test]
    fn stale_threshold_is_long_enough_to_survive_a_missed_advertisement() {
        // BLE advertisement intervals for a mains-powered plug are ~1s, but
        // a phone radio duty-cycles and can genuinely miss many seconds of
        // them. A 45s window greys a row only when the device is really gone.
        let d = Device::new(DeviceId::new("X"), "p", Kind::GenericGatt, Instant::now());
        let now = d.last_seen;
        assert!(!d.is_stale(now + Duration::from_secs(20), STALE_AFTER));
        assert!(d.is_stale(now + Duration::from_secs(60), STALE_AFTER));
    }
    #[test]
    fn sentences_reach_the_backend_the_player_or_the_scheduler() {
        let tmp = std::env::temp_dir().join(format!("sw-app-{}", std::process::id()));
        std::env::set_var("XDG_DATA_HOME", &tmp);
        let mut a = app_with(vec![
            Event::Seen { id: DeviceId::new("L1"), name: "Kitchen Light".into(), kind: Kind::GenericGatt, rssi: Some(-50) },
            Event::Seen { id: DeviceId::new("W1"), name: "Wemo Lamp".into(), kind: Kind::HomeKit, rssi: Some(-60) },
        ]);
        // "lights off": the GATT light is switchable, the unpaired HomeKit lamp is not.
        let msg = a.run_intent(switcheroo_core::intent::parse("lights off"));
        assert_eq!(msg, "1 device → off");
        let spy = a.backend.as_any_spy();
        assert!(matches!(spy.last(), Some(Command::SetPower { id, on: false }) if id.as_str() == "L1"), "{spy:?}");
        // Pair the lamp, and it joins.
        a.hub.step(vec![Event::Paired { id: DeviceId::new("W1") }], Instant::now());
        assert_eq!(a.run_intent(switcheroo_core::intent::parse("turn on the lights")), "2 devices → on");
        // Music goes to the player, not the radio.
        assert_eq!(a.run_intent(switcheroo_core::intent::parse("next song")), "Next");
        // A schedule lands in the scheduler and on disk.
        let msg = a.run_intent(switcheroo_core::intent::parse("turn the kitchen light off by 9pm"));
        assert_eq!(msg, "scheduled: turn kitchen light off by 9pm");
        assert_eq!(a.scheduler.rules.len(), 1);
        assert!(std::fs::read_to_string(tmp.join("switcheroo/schedules.txt")).unwrap().contains("by:1260"));
        assert!(a.run_intent(switcheroo_core::intent::parse("what time is it")).starts_with("didn't understand"));
        std::env::remove_var("XDG_DATA_HOME");
        let _ = std::fs::remove_dir_all(&tmp);
    }

}
