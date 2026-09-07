//! switcheroo-core — the headless half of a Bluetooth smart-outlet remote.
//!
//! No egui, no bluetooth crate, no threads, no async. Everything here is a
//! plain value you can construct in a test. The two seams:
//!
//! * [`Backend`] — where bytes actually leave the machine. Deliberately a
//!   SYNCHRONOUS, dyn-compatible trait built from `submit(Command)` +
//!   `poll() -> Vec<Event>` rather than `async fn`. An async trait method
//!   would make `dyn Backend` unusable without boxing every future, and the
//!   UI already has a frame loop to poll from, so there is nothing for
//!   async to buy here. A real radio backend owns a thread and a channel
//!   behind this interface; the UI never learns that.
//! * [`Hub`] — a pure state machine. Feed it events, ask it for devices.
//!   It never performs I/O, so every interesting transition (a plug that
//!   stops answering, a command that times out, two scans racing) is a
//!   unit test rather than something you can only see by standing in the
//!   kitchen toggling a lamp.
//!
//! The loop is meant to be watchable: [`Hub::journal`] keeps the last N
//! events verbatim, and the UI shows them. When a plug misbehaves the
//! question "what did we actually send, and what came back" has an answer
//! on screen instead of in a log file on a phone.


pub mod radio;
pub mod fastpath;

/// Is this a HomeKit setup code — eight digits, dashes and spaces allowed?
/// hap-core normalises to `XXX-XX-XXX`; this is the same rule for a form
/// to grey its button on, kept here so the core needs no hap-core.
pub fn setup_code_ok(s: &str) -> bool {
    let digits = s.chars().filter(|c| c.is_ascii_digit()).count();
    digits == 8 && s.chars().all(|c| c.is_ascii_digit() || c == '-' || c == ' ')
}
pub mod filter;
pub mod intent;
pub mod media;
pub mod schedule;
pub mod wifi;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

// ── identity ────────────────────────────────────────────────────────────────

/// A device's stable address. On BlueZ this is the MAC (`AA:BB:CC:DD:EE:FF`);
/// on macOS CoreBluetooth hands out an opaque per-host UUID instead. Treated
/// as an opaque string precisely so that difference stays inside the backend.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceId(pub String);

impl DeviceId {
    pub fn new(s: impl Into<String>) -> Self {
        DeviceId(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DeviceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ── what kind of thing we found ─────────────────────────────────────────────

/// How a discovered device expects to be talked to.
///
/// This is the axis that actually varies, so it is the axis the code splits
/// on. It is NOT a brand list — "Wemo" is not a variant, because a Wemo
/// WSP100 and any other HomeKit accessory need exactly the same protocol
/// work and differ only in the name they advertise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// Advertises the HomeKit Accessory Protocol (16-bit service UUID
    /// 0xFE4A, Apple). This covers the Wemo WSP100 "smart plug" sold in
    /// Apple stores — the one with the HomeKit logo printed on the side —
    /// and every other HomeKit-over-BLE accessory.
    ///
    /// Discovery works. **Control does not**, and cannot without real
    /// work: HAP requires a pairing handshake (SRP-6a password
    /// verification with the 8-digit setup code, then Ed25519 long-term
    /// key exchange, then a ChaCha20-Poly1305 encrypted session) before an
    /// accessory will honour a single characteristic write. See
    /// `HOMEKIT.md` in this crate's directory. Until that exists these
    /// show up in the list, correctly identified, and say so.
    HomeKit,
    /// A plain GATT device exposing a writable on/off characteristic — the
    /// shape most non-HomeKit BLE plugs and relays use. No pairing, no
    /// crypto: connect, write one byte. This is the kind switcheroo can
    /// actually switch today.
    GenericGatt,
    /// Seen and named, but nothing we recognise. Listed so the user can
    /// tell the app is working even when their device is not supported,
    /// rather than staring at an empty window and guessing.
    Unknown,
}

impl Kind {
    /// Whether a power command has any chance of working. Drives whether
    /// the UI offers a toggle or an explanation.
    pub fn is_controllable(self) -> bool {
        matches!(self, Kind::GenericGatt)
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::HomeKit => "HomeKit",
            Kind::GenericGatt => "GATT switch",
            Kind::Unknown => "unrecognised",
        }
    }

    /// Why this device can't be toggled, if it can't. `None` when it can.
    pub fn unsupported_reason(self) -> Option<&'static str> {
        match self {
            Kind::GenericGatt => None,
            Kind::HomeKit => Some(
                "HomeKit accessories need a HAP pairing (SRP + Ed25519 + an encrypted \
                 session) before they accept any write. Pair it with its setup code; \
                 see HOMEKIT.md.",
            ),
            Kind::Unknown => Some(
                "No recognised on/off service in this device's advertisement.",
            ),
        }
    }
}

/// Classify from the service UUIDs in an advertisement.
///
/// Kept as a free function over plain strings so it can be tested against
/// captured advertisement data without a radio present.
pub fn classify(service_uuids: &[String]) -> Kind {
    let has = |needle: &str| {
        service_uuids
            .iter()
            .any(|u| u.to_ascii_lowercase().contains(needle))
    };
    // 0xFE4A — Apple's assigned 16-bit UUID for HAP over BLE. Matched in
    // both the short form and the 128-bit expansion, because BlueZ and
    // CoreBluetooth disagree about which one they hand you.
    if has("fe4a") || has("0000fe4a-0000-1000-8000-00805f9b34fb") {
        return Kind::HomeKit;
    }
    // 0x1815 Automation IO / 0x1816 is cycling, not this — 1815 is the
    // assigned service for generic digital output, which is what the cheap
    // relay plugs expose.
    if has("1815") || has("00001815-0000-1000-8000-00805f9b34fb") {
        return Kind::GenericGatt;
    }
    Kind::Unknown
}

// ── power ───────────────────────────────────────────────────────────────────

/// Outlet state. `Unknown` is a real, common state — a device we have seen
/// advertise but never successfully read — and conflating it with `Off`
/// would make the UI lie about a plug that is actually on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Power {
    On,
    Off,
    Unknown,
}

impl Power {
    pub fn toggled(self) -> Power {
        match self {
            Power::On => Power::Off,
            // An unknown plug is most likely off in the user's mind when
            // they reach for the button, and "on" is the useful default
            // action either way — you press the button because you want
            // the lamp lit.
            Power::Off | Power::Unknown => Power::On,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Power::On => "on",
            Power::Off => "off",
            Power::Unknown => "unknown",
        }
    }
}

// ── the device record ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Device {
    pub id: DeviceId,
    /// Advertised local name, or a fallback built from the address. Never
    /// empty — an unnamed row is unclickable in practice.
    pub name: String,
    pub kind: Kind,
    /// Signal strength in dBm, if the backend reported one. Shown because
    /// on a phone it is the difference between "the plug is broken" and
    /// "you are two rooms away".
    pub rssi: Option<i16>,
    pub power: Power,
    /// A command is in flight. The UI shows a spinner and refuses to queue
    /// a second toggle, which is what stops an impatient double-tap from
    /// turning a lamp on and then straight back off.
    pub pending: bool,
    /// When we last heard anything at all from this device.
    pub last_seen: Instant,
    /// Set when the last command failed; cleared on the next success.
    pub last_error: Option<String>,
    /// User-pinned favourite. Sorts to the top and survives a rescan.
    pub favourite: bool,
    /// A HomeKit accessory we hold a pairing for (see `hap-core`). Set by
    /// `Event::Paired`; the backend that reported it owns the record.
    pub paired: bool,
    /// What a paired accessory said it can do, in its own words: one line from
    /// `hap_core::Capabilities::describe` ("outlet · identify"). `None` until it has been
    /// asked. A row that says "switch" for a plug that reports load is a row that guessed.
    pub abilities: Option<String>,
    /// Can this accessory be asked to blink? Only then is the button worth drawing.
    pub can_identify: bool,
    /// An OUTLET's load: something is plugged in and drawing. `None` for an accessory that
    /// does not report it, which is not the same as `Some(false)` — "nothing plugged in"
    /// and "cannot tell" are different things to show.
    pub in_use: Option<bool>,
    /// The accessory advertised a change since the last read, so `power` may be wrong.
    /// Cleared by the next successful read.
    pub state_stale: bool,
}

impl Device {
    pub fn new(id: DeviceId, name: impl Into<String>, kind: Kind, now: Instant) -> Self {
        let name = name.into();
        let name = if name.trim().is_empty() {
            format!("device {}", id.as_str())
        } else {
            name
        };
        Device {
            id,
            name,
            kind,
            rssi: None,
            power: Power::Unknown,
            pending: false,
            last_seen: now,
            last_error: None,
            favourite: false,
            paired: false,
            abilities: None,
            can_identify: false,
            in_use: None,
            state_stale: false,
        }
    }

    /// A device we have not heard from in a while. Greyed rather than
    /// removed: a plug behind a wall drops in and out of range constantly,
    /// and a list that deletes rows out from under a thumb is unusable.
    pub fn is_stale(&self, now: Instant, after: Duration) -> bool {
        now.duration_since(self.last_seen) > after
    }

    /// Whether a power command has any chance of working on THIS device:
    /// its kind is switchable outright, or it is a HomeKit accessory we are
    /// paired with. Replaces `Kind::is_controllable` as the thing the UI
    /// asks, because pairing is per-device state, not per-kind.
    pub fn is_controllable(&self) -> bool {
        self.kind.is_controllable() || (self.kind == Kind::HomeKit && self.paired)
    }

    /// Why this device can't be toggled right now, if it can't.
    pub fn unsupported_reason(&self) -> Option<&'static str> {
        if self.is_controllable() { return None; }
        match self.kind {
            Kind::HomeKit => Some("HomeKit accessory: pair it first (needs the 8-digit setup code printed on the plug)."),
            other => other.unsupported_reason(),
        }
    }
}

// ── the protocol between UI and radio ───────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    StartScan,
    StopScan,
    /// Ask for a device's current power state.
    Refresh(DeviceId),
    SetPower { id: DeviceId, on: bool },
    /// HomeKit pair-setup with the 8-digit code off the plug. Destructive if
    /// something else holds the pairing (HOMEKIT.md), so the UI confirms
    /// loudly before submitting this.
    Pair { id: DeviceId, setup_code: String },
    /// Forget our pairing record for this accessory.
    Unpair(DeviceId),
    /// Ask a paired accessory what it can do (`hap-core`'s capability probe) and read the
    /// outlet's load while the session is up. Answered with `Event::Abilities`.
    Inspect(DeviceId),
    /// Ask a paired accessory to blink or beep, so a person can tell which row it is.
    Identify(DeviceId),
}

#[derive(Debug, Clone)]
pub enum Event {
    ScanStarted,
    ScanStopped,
    /// A device was seen. Sent repeatedly as advertisements arrive; the hub
    /// treats a repeat as an update rather than a duplicate row.
    Seen {
        id: DeviceId,
        name: String,
        kind: Kind,
        rssi: Option<i16>,
    },
    PowerIs {
        id: DeviceId,
        power: Power,
    },
    /// The accessory's advertisement says something changed — its Global State Number
    /// moved, or it broadcast an encrypted notification we cannot read.
    ///
    /// Not a state: a NOTICE. It says a broadcast-enabled characteristic is different, not
    /// which one or what it now is, so the row goes stale and the app reads once instead of
    /// polling every accessory on a timer. `reconfigured` means the attribute database
    /// changed — firmware update or factory reset — so cached instance ids are stale and
    /// the pairing may be gone with them.
    AccessoryChanged { id: DeviceId, reconfigured: bool },
    /// What a paired accessory reported about itself, from `hap-core`'s capability probe.
    /// Separate from `PowerIs` because it is asked once per connection and answers a
    /// different question: not "is it on" but "what is it".
    Abilities {
        id: DeviceId,
        /// `Capabilities::describe`, already rendered — core does not depend on hap-core.
        describes: String,
        can_identify: bool,
        in_use: Option<bool>,
    },
    /// A command failed. Carries the id so the row can show it in place
    /// instead of a global error bar that does not say which plug broke.
    Failed {
        id: Option<DeviceId>,
        message: String,
    },
    /// Pair-setup succeeded (or a stored pairing was found at startup).
    Paired { id: DeviceId },
    /// Pair-setup failed; `message` is hap-core's error, which names the
    /// accessory's own error code when it sent one.
    PairingFailed { id: DeviceId, message: String },
    /// A pairing was forgotten.
    Unpaired { id: DeviceId },
}

/// The radio. See the module docs for why this is sync and dyn-compatible.
pub trait Backend {
    /// Queue a command. Must not block — a backend that talks to a radio
    /// pushes onto a channel here and does the work on its own thread.
    fn submit(&mut self, cmd: Command);
    /// Take everything that has happened since the last call. Called once
    /// per UI frame.
    fn poll(&mut self) -> Vec<Event>;
    /// Human-readable name for the status bar, e.g. "btleplug" or "demo".
    fn describe(&self) -> String;
}

// ── the state machine ───────────────────────────────────────────────────────

/// How many events the journal keeps. Enough to cover a scan plus a few
/// toggles, small enough that it costs nothing to keep on a phone.
pub const JOURNAL_CAP: usize = 200;

/// Pure state. No I/O — [`Hub::step`] takes the events a backend already
/// produced.
#[derive(Debug, Default)]
pub struct Hub {
    devices: BTreeMap<DeviceId, Device>,
    pub scanning: bool,
    /// Last N events, newest last, for the visible-loop pane.
    journal: Vec<String>,
}

impl Hub {
    pub fn new() -> Self {
        Hub::default()
    }

    /// Apply one batch of events. `now` is passed in rather than read from
    /// the clock so staleness is testable.
    pub fn step(&mut self, events: Vec<Event>, now: Instant) {
        for ev in events {
            self.note(&ev);
            match ev {
                Event::ScanStarted => self.scanning = true,
                Event::ScanStopped => self.scanning = false,
                Event::Seen { id, name, kind, rssi } => {
                    let e = self
                        .devices
                        .entry(id.clone())
                        .or_insert_with(|| Device::new(id, name.clone(), kind, now));
                    e.last_seen = now;
                    if rssi.is_some() {
                        e.rssi = rssi;
                    }
                    // A later advertisement can carry a name the first one
                    // omitted (BLE splits adv/scan-response), so upgrade —
                    // but never downgrade a real name back to a placeholder.
                    if !name.trim().is_empty() {
                        e.name = name;
                    }
                    if e.kind == Kind::Unknown && kind != Kind::Unknown {
                        e.kind = kind;
                    }
                }
                Event::PowerIs { id, power } => {
                    if let Some(d) = self.devices.get_mut(&id) {
                        d.power = power;
                        d.pending = false;
                        d.last_error = None;
                        d.last_seen = now;
                        // A fresh read is what staleness was waiting for.
                        d.state_stale = false;
                    }
                }
                Event::Failed { id, message } => {
                    if let Some(d) = id.and_then(|i| self.devices.get_mut(&i)) {
                        d.pending = false;
                        d.last_error = Some(message);
                    }
                }
                Event::AccessoryChanged { id, reconfigured } => {
                    if let Some(d) = self.devices.get_mut(&id) {
                        // The power we last read is no longer trustworthy. Marked stale
                        // rather than cleared: a row that blanks on every advert flickers,
                        // and "was on, may have changed" is more use than "unknown".
                        d.state_stale = true;
                        if reconfigured {
                            // The database moved under us. Everything the probe learned is
                            // about a layout that may no longer exist, and a factory reset
                            // takes the pairing with it.
                            d.abilities = None;
                            d.can_identify = false;
                            d.in_use = None;
                        }
                    }
                }
                Event::Abilities { id, describes, can_identify, in_use } => {
                    if let Some(d) = self.devices.get_mut(&id) {
                        d.abilities = Some(describes);
                        d.can_identify = can_identify;
                        d.in_use = in_use;
                        d.pending = false;
                    }
                }
                Event::Paired { id } => {
                    if let Some(d) = self.devices.get_mut(&id) {
                        d.paired = true;
                        d.pending = false;
                        d.last_error = None;
                    }
                }
                Event::PairingFailed { id, message } => {
                    if let Some(d) = self.devices.get_mut(&id) {
                        d.pending = false;
                        d.last_error = Some(message);
                    }
                }
                Event::Unpaired { id } => {
                    if let Some(d) = self.devices.get_mut(&id) {
                        d.paired = false;
                        d.power = Power::Unknown;
                        // Unpairing is a round trip now (the accessory is told before the
                        // record is deleted), so the row was left pending by `unpair`.
                        d.pending = false;
                        // And everything the probe learned goes with the pairing: those
                        // answers came from a session this controller can no longer open.
                        d.abilities = None;
                        d.can_identify = false;
                        d.in_use = None;
                    }
                }
            }
        }
    }

    fn note(&mut self, ev: &Event) {
        let line = match ev {
            Event::ScanStarted => "scan started".to_string(),
            Event::ScanStopped => "scan stopped".to_string(),
            Event::Seen { id, name, kind, rssi } => format!(
                "seen {id} \"{name}\" [{}]{}",
                kind.label(),
                rssi.map(|r| format!(" {r}dBm")).unwrap_or_default()
            ),
            Event::PowerIs { id, power } => format!("{id} is {}", power.label()),
            Event::Failed { id, message } => match id {
                Some(i) => format!("FAILED {i}: {message}"),
                None => format!("FAILED: {message}"),
            },
            Event::AccessoryChanged { id, reconfigured } => {
                if *reconfigured {
                    format!("{id} re-configured (firmware or factory reset) — re-read it")
                } else {
                    format!("{id} advertised a change")
                }
            }
            Event::Abilities { id, describes, in_use, .. } => format!(
                "{id} is a {describes}{}",
                match in_use {
                    Some(true) => " · in use",
                    Some(false) => " · nothing plugged in",
                    None => "",
                }
            ),
            Event::Paired { id } => format!("paired {id}"),
            Event::PairingFailed { id, message } => format!("PAIRING FAILED {id}: {message}"),
            Event::Unpaired { id } => format!("unpaired {id}"),
        };
        self.journal.push(line);
        if self.journal.len() > JOURNAL_CAP {
            let overflow = self.journal.len() - JOURNAL_CAP;
            self.journal.drain(..overflow);
        }
    }

    /// Record locally that a command went out, so the row can show pending
    /// immediately rather than after the radio round-trip.
    pub fn mark_pending(&mut self, id: &DeviceId) {
        if let Some(d) = self.devices.get_mut(id) {
            d.pending = true;
            d.last_error = None;
        }
    }

    pub fn toggle_favourite(&mut self, id: &DeviceId) {
        if let Some(d) = self.devices.get_mut(id) {
            d.favourite = !d.favourite;
        }
    }

    pub fn get(&self, id: &DeviceId) -> Option<&Device> {
        self.devices.get(id)
    }

    pub fn len(&self) -> usize {
        self.devices.len()
    }

    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }

    pub fn journal(&self) -> &[String] {
        &self.journal
    }

    /// Devices in display order: favourites first, then controllable ones
    /// (a plug you can actually switch is more useful than one you can
    /// only look at), then strongest signal, then name for stability.
    /// Name last matters — without a total order the list jitters every
    /// frame as RSSI wobbles, which on a touch screen means mis-taps.
    pub fn sorted(&self) -> Vec<&Device> {
        let mut v: Vec<&Device> = self.devices.values().collect();
        v.sort_by(|a, b| {
            b.favourite
                .cmp(&a.favourite)
                .then(b.kind.is_controllable().cmp(&a.kind.is_controllable()))
                .then(b.rssi.unwrap_or(i16::MIN).cmp(&a.rssi.unwrap_or(i16::MIN)))
                .then(a.name.cmp(&b.name))
                .then(a.id.cmp(&b.id))
        });
        v
    }
}

// ── demo backend ────────────────────────────────────────────────────────────

/// A fake radio with three plugs on it.
///
/// Not just a testing convenience: this is how the UI is developed and how
/// the app behaves on a machine with no Bluetooth at all, which includes
/// most CI and every sandbox. It models the two things that actually make
/// BLE UIs hard — devices appearing gradually rather than all at once, and
/// commands taking long enough that you need a pending state — so a UI that
/// looks right against this is not obviously wrong against a real radio.
pub struct DemoBackend {
    queue: Vec<(Instant, Event)>,
    scanning: bool,
    started: Option<Instant>,
    latency: Duration,
    /// Fixtures the demo has 'paired' this session.
    paired: Vec<DeviceId>,
}

impl Default for DemoBackend {
    fn default() -> Self {
        DemoBackend::new()
    }
}

impl DemoBackend {
    pub fn new() -> Self {
        DemoBackend {
            queue: Vec::new(),
            scanning: false,
            started: None,
            latency: Duration::from_millis(450),
            paired: Vec::new(),
        }
    }

    /// The one setup code the demo's HomeKit fixture accepts.
    pub const DEMO_SETUP_CODE: &'static str = "03145154";

    fn fixtures() -> Vec<(DeviceId, &'static str, Kind, i16)> {
        vec![
            (DeviceId::new("AA:BB:CC:00:00:01"), "Wemo Mini 1F2", Kind::HomeKit, -52),
            (DeviceId::new("AA:BB:CC:00:00:02"), "Desk lamp relay", Kind::GenericGatt, -61),
            (DeviceId::new("AA:BB:CC:00:00:03"), "BT-SPEAKER-9", Kind::Unknown, -80),
        ]
    }
}

impl Backend for DemoBackend {
    fn submit(&mut self, cmd: Command) {
        let now = Instant::now();
        match cmd {
            Command::StartScan => {
                self.scanning = true;
                self.started = Some(now);
                self.queue.push((now, Event::ScanStarted));
                // Staggered, like real advertisements.
                for (i, (id, name, kind, rssi)) in Self::fixtures().into_iter().enumerate() {
                    self.queue.push((
                        now + Duration::from_millis(300 * (i as u64 + 1)),
                        Event::Seen {
                            id,
                            name: name.to_string(),
                            kind,
                            rssi: Some(rssi),
                        },
                    ));
                }
            }
            Command::StopScan => {
                self.scanning = false;
                self.queue.push((now, Event::ScanStopped));
            }
            Command::Refresh(id) => {
                self.queue
                    .push((now + self.latency, Event::PowerIs { id, power: Power::Off }));
            }
            // The demo accessory is an OUTLET with nothing plugged into it, which is the
            // more interesting of the two states to see in the UI: "switch" and "outlet"
            // look the same until one of them tells you the socket is empty.
            Command::Inspect(id) => {
                self.queue.push((
                    now + self.latency,
                    Event::Abilities {
                        id,
                        describes: "outlet · identify".into(),
                        can_identify: true,
                        in_use: Some(false),
                    },
                ));
            }
            // Nothing to report: the accessory blinks, and the person looking at the
            // plug is the one who receives the answer.
            Command::Identify(_) => {}
            Command::Pair { id, setup_code } => {
                // The demo accepts exactly one code, so the wrong-code path
                // is visible in the UI without a plug.
                let digits: String = setup_code.chars().filter(|c| c.is_ascii_digit()).collect();
                let ev = if digits.len() != 8 {
                    Event::PairingFailed { id, message: "setup code must be 8 digits (XXX-XX-XXX)".into() }
                } else if digits == DemoBackend::DEMO_SETUP_CODE {
                    self.paired.push(id.clone());
                    Event::Paired { id }
                } else {
                    Event::PairingFailed { id, message: "accessory refused at M4: authentication failed (wrong setup code, or a stale pairing)".into() }
                };
                self.queue.push((now + self.latency * 3, ev));
            }
            Command::Unpair(id) => {
                self.paired.retain(|p| *p != id);
                self.queue.push((now, Event::Unpaired { id }));
            }
            Command::SetPower { id, on } => {
                let controllable = Self::fixtures()
                    .into_iter()
                    .find(|(f, _, _, _)| *f == id)
                    .map(|(_, _, k, _)| k.is_controllable() || (k == Kind::HomeKit && self.paired.contains(&id)))
                    .unwrap_or(false);
                let ev = if controllable {
                    Event::PowerIs {
                        id,
                        power: if on { Power::On } else { Power::Off },
                    }
                } else {
                    Event::Failed {
                        id: Some(id),
                        message: "demo: this device is not controllable".to_string(),
                    }
                };
                self.queue.push((now + self.latency, ev));
            }
        }
    }

    fn poll(&mut self) -> Vec<Event> {
        let now = Instant::now();
        let mut out = Vec::new();
        self.queue.retain(|(at, ev)| {
            if *at <= now {
                out.push(ev.clone());
                false
            } else {
                true
            }
        });
        out
    }

    fn describe(&self) -> String {
        "demo (no radio)".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn homekit_is_recognised_from_either_uuid_form() {
        // BlueZ hands back the short form, CoreBluetooth the long one. Both
        // are the same service and both must classify the same, or the same
        // plug reads as HomeKit on one host and Unknown on another.
        assert_eq!(classify(&["FE4A".into()]), Kind::HomeKit);
        assert_eq!(
            classify(&["0000fe4a-0000-1000-8000-00805f9b34fb".into()]),
            Kind::HomeKit
        );
    }

    #[test]
    fn an_unrecognised_device_is_listed_not_hidden() {
        // The failure mode this guards: a user opens the app, their plug is
        // unsupported, the list is empty, and they cannot tell the app from
        // a broken radio.
        assert_eq!(classify(&["180f".into()]), Kind::Unknown);
        assert!(Kind::Unknown.unsupported_reason().is_some());
    }

    #[test]
    fn a_repeat_advertisement_updates_the_row_instead_of_adding_one() {
        let mut hub = Hub::new();
        let id = DeviceId::new("X");
        let seen = |rssi| Event::Seen {
            id: DeviceId::new("X"),
            name: "plug".into(),
            kind: Kind::GenericGatt,
            rssi: Some(rssi),
        };
        hub.step(vec![seen(-40), seen(-70)], t0());
        assert_eq!(hub.len(), 1, "one device, seen twice");
        assert_eq!(hub.get(&id).unwrap().rssi, Some(-70), "latest signal wins");
    }

    #[test]
    fn a_later_advertisement_can_supply_a_name_the_first_one_lacked() {
        // BLE splits the advertisement and the scan response; the name is
        // often only in the second. Before this, such a device kept its
        // "device AA:BB.." placeholder forever.
        let mut hub = Hub::new();
        hub.step(
            vec![Event::Seen {
                id: DeviceId::new("X"),
                name: String::new(),
                kind: Kind::Unknown,
                rssi: None,
            }],
            t0(),
        );
        assert!(hub.get(&DeviceId::new("X")).unwrap().name.contains("X"));
        hub.step(
            vec![Event::Seen {
                id: DeviceId::new("X"),
                name: "Desk lamp".into(),
                kind: Kind::GenericGatt,
                rssi: None,
            }],
            t0(),
        );
        let d = hub.get(&DeviceId::new("X")).unwrap();
        assert_eq!(d.name, "Desk lamp");
        assert_eq!(d.kind, Kind::GenericGatt, "kind upgrades off Unknown too");
    }

    #[test]
    fn a_failure_clears_pending_so_the_row_does_not_spin_forever() {
        let mut hub = Hub::new();
        let id = DeviceId::new("X");
        hub.step(
            vec![Event::Seen {
                id: id.clone(),
                name: "p".into(),
                kind: Kind::GenericGatt,
                rssi: None,
            }],
            t0(),
        );
        hub.mark_pending(&id);
        assert!(hub.get(&id).unwrap().pending);
        hub.step(
            vec![Event::Failed {
                id: Some(id.clone()),
                message: "out of range".into(),
            }],
            t0(),
        );
        let d = hub.get(&id).unwrap();
        assert!(!d.pending, "a failed command must stop the spinner");
        assert_eq!(d.last_error.as_deref(), Some("out of range"));
    }

    #[test]
    fn a_success_clears_a_previous_error() {
        let mut hub = Hub::new();
        let id = DeviceId::new("X");
        hub.step(
            vec![
                Event::Seen {
                    id: id.clone(),
                    name: "p".into(),
                    kind: Kind::GenericGatt,
                    rssi: None,
                },
                Event::Failed {
                    id: Some(id.clone()),
                    message: "nope".into(),
                },
                Event::PowerIs {
                    id: id.clone(),
                    power: Power::On,
                },
            ],
            t0(),
        );
        let d = hub.get(&id).unwrap();
        assert_eq!(d.power, Power::On);
        assert!(d.last_error.is_none(), "a later success clears the error");
    }

    #[test]
    fn unknown_toggles_to_on_because_that_is_why_you_pressed_it() {
        assert_eq!(Power::Unknown.toggled(), Power::On);
        assert_eq!(Power::On.toggled(), Power::Off);
        assert_eq!(Power::Off.toggled(), Power::On);
    }

    #[test]
    fn sort_is_a_total_order_so_the_list_cannot_jitter() {
        // Two devices identical on every ranked field except id. Without the
        // final id tiebreak their relative order would be unspecified and the
        // rows could swap between frames — which on a touch screen means
        // tapping the wrong plug.
        let mut hub = Hub::new();
        for id in ["B", "A"] {
            hub.step(
                vec![Event::Seen {
                    id: DeviceId::new(id),
                    name: "same".into(),
                    kind: Kind::GenericGatt,
                    rssi: Some(-50),
                }],
                t0(),
            );
        }
        let first: Vec<String> = hub.sorted().iter().map(|d| d.id.0.clone()).collect();
        let again: Vec<String> = hub.sorted().iter().map(|d| d.id.0.clone()).collect();
        assert_eq!(first, again);
        assert_eq!(first, vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn favourites_outrank_controllable_which_outranks_signal() {
        let mut hub = Hub::new();
        let ev = |id: &str, kind, rssi| Event::Seen {
            id: DeviceId::new(id),
            name: id.into(),
            kind,
            rssi: Some(rssi),
        };
        hub.step(
            vec![
                ev("loud-unknown", Kind::Unknown, -30),
                ev("weak-switch", Kind::GenericGatt, -90),
                ev("fav", Kind::Unknown, -95),
            ],
            t0(),
        );
        hub.toggle_favourite(&DeviceId::new("fav"));
        let order: Vec<String> = hub.sorted().iter().map(|d| d.id.0.clone()).collect();
        assert_eq!(order[0], "fav", "pinned first even though it is weakest");
        assert_eq!(order[1], "weak-switch", "controllable beats a louder unknown");
    }

    #[test]
    fn a_quiet_device_goes_stale_rather_than_disappearing() {
        let mut hub = Hub::new();
        let now = Instant::now();
        hub.step(
            vec![Event::Seen {
                id: DeviceId::new("X"),
                name: "p".into(),
                kind: Kind::GenericGatt,
                rssi: None,
            }],
            now,
        );
        let d = hub.get(&DeviceId::new("X")).unwrap();
        assert!(!d.is_stale(now, Duration::from_secs(30)));
        assert!(d.is_stale(now + Duration::from_secs(31), Duration::from_secs(30)));
        assert_eq!(hub.len(), 1, "stale, but still listed");
    }

    #[test]
    fn the_journal_is_capped_and_keeps_the_newest() {
        let mut hub = Hub::new();
        for i in 0..(JOURNAL_CAP + 25) {
            hub.step(
                vec![Event::Failed {
                    id: None,
                    message: format!("{i}"),
                }],
                t0(),
            );
        }
        assert_eq!(hub.journal().len(), JOURNAL_CAP);
        assert!(
            hub.journal().last().unwrap().contains(&format!("{}", JOURNAL_CAP + 24)),
            "the newest event must survive the trim"
        );
    }

    #[test]
    fn demo_backend_reports_a_homekit_plug_as_a_failure_not_a_lie() {
        // The important behaviour: asking a HomeKit plug to switch must
        // surface an honest failure. A demo that pretended it worked would
        // make the UI look finished when the protocol is not written.
        let mut b = DemoBackend::new();
        b.submit(Command::SetPower {
            id: DeviceId::new("AA:BB:CC:00:00:01"),
            on: true,
        });
        std::thread::sleep(Duration::from_millis(500));
        let evs = b.poll();
        assert!(
            matches!(evs.first(), Some(Event::Failed { .. })),
            "got {evs:?}"
        );
    }
}

#[cfg(test)]
mod ability_tests {
    use super::*;

    fn now() -> Instant { Instant::now() }

    fn hub_with_one() -> (Hub, DeviceId) {
        let mut h = Hub::new();
        let id = DeviceId::new("AA:BB:CC:DD:EE:FF");
        h.step(vec![Event::Seen { id: id.clone(), name: "plug".into(), kind: Kind::HomeKit, rssi: None }], now());
        (h, id)
    }

    /// A row says what the accessory said, and "cannot tell" is not "nothing plugged in".
    #[test]
    fn abilities_reach_the_row_and_an_absent_load_sensor_stays_absent() {
        let (mut h, id) = hub_with_one();
        let d = h.get(&id).unwrap();
        assert!(d.abilities.is_none() && !d.can_identify && d.in_use.is_none());

        h.step(vec![Event::Abilities {
            id: id.clone(),
            describes: "outlet · identify".into(),
            can_identify: true,
            in_use: Some(false),
        }], now());
        let d = h.get(&id).unwrap();
        assert_eq!(d.abilities.as_deref(), Some("outlet · identify"));
        assert!(d.can_identify);
        assert_eq!(d.in_use, Some(false), "the plug says nothing is drawing");
        assert!(!d.pending, "the answer ends the in-flight state");

        // a switch, which has no load sensor at all
        h.step(vec![Event::Abilities { id: id.clone(), describes: "switch".into(), can_identify: false, in_use: None }], now());
        let d = h.get(&id).unwrap();
        assert_eq!(d.in_use, None, "no sensor is not the same as nothing plugged in");
        assert!(!d.can_identify);
    }

    /// What the probe learned belongs to the pairing. Keeping "outlet · identify" on a row
    /// this controller can no longer open a session with would offer a blink button that
    /// cannot work.
    #[test]
    fn unpairing_forgets_what_the_probe_learned() {
        let (mut h, id) = hub_with_one();
        h.step(vec![Event::Paired { id: id.clone() }], now());
        h.step(vec![Event::Abilities { id: id.clone(), describes: "outlet · identify".into(), can_identify: true, in_use: Some(true) }], now());
        assert!(h.get(&id).unwrap().can_identify);
        h.mark_pending(&id);
        h.step(vec![Event::Unpaired { id: id.clone() }], now());
        let d = h.get(&id).unwrap();
        assert!(!d.paired && !d.pending);
        assert!(d.abilities.is_none() && !d.can_identify && d.in_use.is_none());
        assert_eq!(d.power, Power::Unknown);
    }

    /// A plug switched at the wall advertises it. The row must stop asserting a power it
    /// has been told is out of date — and must not blank, because "was on, may have
    /// changed" is more use than "unknown" and a row that clears on every advert flickers.
    #[test]
    fn an_advertised_change_makes_the_last_reading_stale_without_erasing_it() {
        let (mut h, id) = hub_with_one();
        h.step(vec![Event::PowerIs { id: id.clone(), power: Power::On }], now());
        let d = h.get(&id).unwrap();
        assert_eq!(d.power, Power::On);
        assert!(!d.state_stale);

        h.step(vec![Event::AccessoryChanged { id: id.clone(), reconfigured: false }], now());
        let d = h.get(&id).unwrap();
        assert!(d.state_stale, "the advert said it changed and the row did not notice");
        assert_eq!(d.power, Power::On, "the last reading was erased rather than flagged");

        // A fresh read is what staleness was waiting for.
        h.step(vec![Event::PowerIs { id: id.clone(), power: Power::Off }], now());
        let d = h.get(&id).unwrap();
        assert!(!d.state_stale);
        assert_eq!(d.power, Power::Off);
    }

    /// A re-configuration is a different event: the attribute database moved, so everything
    /// the capability probe learned describes a layout that may no longer exist — and a
    /// factory reset takes the pairing with it.
    #[test]
    fn a_reconfiguration_drops_what_the_probe_learned() {
        let (mut h, id) = hub_with_one();
        h.step(vec![Event::Abilities {
            id: id.clone(),
            describes: "outlet · identify".into(),
            can_identify: true,
            in_use: Some(true),
        }], now());
        assert!(h.get(&id).unwrap().can_identify);

        h.step(vec![Event::AccessoryChanged { id: id.clone(), reconfigured: true }], now());
        let d = h.get(&id).unwrap();
        assert!(d.abilities.is_none() && !d.can_identify && d.in_use.is_none());
        assert!(d.state_stale);
        assert!(h.journal().last().unwrap().contains("re-configured"), "{:?}", h.journal().last());

        // A plain change leaves the capabilities alone — they are still true.
        h.step(vec![Event::Abilities {
            id: id.clone(),
            describes: "outlet".into(),
            can_identify: false,
            in_use: Some(false),
        }], now());
        h.step(vec![Event::AccessoryChanged { id: id.clone(), reconfigured: false }], now());
        assert_eq!(h.get(&id).unwrap().abilities.as_deref(), Some("outlet"));
    }

    #[test]
    fn the_journal_says_what_was_learned() {
        let (mut h, id) = hub_with_one();
        h.step(vec![Event::Abilities { id: id.clone(), describes: "outlet".into(), can_identify: false, in_use: Some(true) }], now());
        let last = h.journal().last().unwrap();
        assert!(last.contains("is a outlet") && last.contains("in use"), "{last}");
        h.step(vec![Event::Abilities { id, describes: "outlet".into(), can_identify: false, in_use: Some(false) }], now());
        assert!(h.journal().last().unwrap().contains("nothing plugged in"));
    }
}
