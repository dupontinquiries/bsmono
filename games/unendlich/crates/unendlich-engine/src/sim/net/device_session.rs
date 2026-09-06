//! `sim::net::device_session` — MULTI-DEVICE MODE over the DHT: one player, several devices.
//!
//! The phone-controller design (`docs/design/phone_controller_lan-v1r2`) has a render box that
//! runs the game and any number of phones that are pure input surfaces (touch, IMU, voice,
//! buttons) or second screens, each holding a player slot. Its §2b adds DHT join: phones find the
//! render box by a six-character SESSION CODE shown on its screen, not by an IP, so pairing
//! survives a second router, a VPN, a VLAN and DHCP. Everything in this module is the pure half
//! of that: what the render box publishes, how a phone finds it, how it asks to join, and the
//! table of who is joined. No sockets, no clock, no mDNS — the bin supplies those through two
//! small seams:
//!
//! * [`SessionRecords`] — put/find-value on the DHT (a record, last-write-wins, NOT a drained
//!   mailbox): where the [`DeviceAdvert`] lives under [`session_key`].
//! * [`MailboxDht`](super::dht_mailbox::MailboxDht) — the existing drained mailbox: where a
//!   [`MailMsg::DeviceRequest`] waits for the host, under [`device_mailbox_key`], and where the
//!   host's `Accept`/`Deny` waits for the phone, under its own id's mailbox. A SEPARATE key from
//!   the world-join mailbox, because a `get` drains — two flows on one key would steal each
//!   other's post.
//!
//! The IP-protection invariant of `dht_mailbox` is kept: the advert carries the host's id, name
//! and free slots, never its address; the address travels only in the `Accept` to the device
//! that was admitted. The design's "IP:port in the DHT value" is the one thing NOT adopted from
//! it — a session code is shown on a screen and read across a room, which is not a secret.
//!
//! Layering, so each piece is testable and swappable on its own:
//!   [`SessionCode`] → [`DeviceAdvert`] (wire) → host [`DeviceSession`] (over
//!   [`Roster`](super::player_roster::Roster) for slots) · phone [`DiscoveryLadder`] →
//!   [`DeviceJoin`] → [`PairOutcome`] · [`DevicesView`] for the menu.

use super::dht_mailbox::{key_of, mailbox_key, MailMsg, MailboxDht, MailboxKey};
use super::identity::UnendlichId;
use super::player_roster::{ConnState, JoinError, PlayerId, Role, Roster, Slot};

// ═══════════════════════════════════════════════ session code ═══════════════════════════════════

/// The alphabet a code is drawn from: no 0/O, no 1/I/L — read across a room, typed on a phone.
pub const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
pub const CODE_LEN: usize = 6;

/// The six-character code a render box shows and a phone types.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct SessionCode(String);

impl SessionCode {
    /// A code from a seed (the bin seeds from the clock + machine id). Six draws from the
    /// alphabet by a 64-bit LCG — uniform enough for a code that is not a secret.
    pub fn from_seed(mut seed: u64) -> SessionCode {
        let mut s = String::with_capacity(CODE_LEN);
        for _ in 0..CODE_LEN {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let i = ((seed >> 33) % CODE_ALPHABET.len() as u64) as usize;
            s.push(CODE_ALPHABET[i] as char);
        }
        SessionCode(s)
    }

    /// Parse what a person typed: separators and case are forgiven; the ambiguous glyphs the
    /// alphabet leaves out are mapped to what someone reading a screen most likely saw
    /// (`0`→`O`?? no — `O` is not in the alphabet either; `0`/`O` → `Q`, `1`/`I`/`L` → `7`
    /// would be guessing). They are rejected instead: a code that cannot be right is better
    /// refused than joined to the wrong room.
    pub fn parse(s: &str) -> Option<SessionCode> {
        let mut out = String::with_capacity(CODE_LEN);
        for ch in s.chars() {
            if ch == ' ' || ch == '-' || ch == '_' || ch == '.' {
                continue;
            }
            let up = ch.to_ascii_uppercase();
            if !CODE_ALPHABET.contains(&(up as u8)) {
                return None;
            }
            out.push(up);
        }
        (out.len() == CODE_LEN).then_some(SessionCode(out))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `ABC-DEF` — how it is shown on screen.
    pub fn grouped(&self) -> String {
        format!("{}-{}", &self.0[..3], &self.0[3..])
    }
}

/// Where the render box's advert lives: `H("unendlich-session:" + code)`.
pub fn session_key(code: &SessionCode) -> MailboxKey {
    key_of(&format!("unendlich-session:{}", code.as_str()))
}

/// Where a device's join request waits for the host. Distinct from [`session_key`] (records
/// and mailboxes are different primitives) and from the host's world-join mailbox.
pub fn device_mailbox_key(code: &SessionCode) -> MailboxKey {
    key_of(&format!("unendlich-devices:{}", code.as_str()))
}

/// The DHT record primitive the advert needs: put-value / find-value, last write wins, never
/// drained. `LocalRecords` is the in-memory stand-in for tests and same-process play.
pub trait SessionRecords {
    fn publish(&self, key: MailboxKey, record: Vec<u8>);
    fn lookup(&self, key: MailboxKey) -> Option<Vec<u8>>;
}

#[derive(Default)]
pub struct LocalRecords {
    records: std::sync::Mutex<std::collections::HashMap<MailboxKey, Vec<u8>>>,
}

impl SessionRecords for LocalRecords {
    fn publish(&self, key: MailboxKey, record: Vec<u8>) {
        self.records.lock().unwrap().insert(key, record);
    }
    fn lookup(&self, key: MailboxKey) -> Option<Vec<u8>> {
        self.records.lock().unwrap().get(&key).cloned()
    }
}

// ═══════════════════════════════════════════ device capabilities ═══════════════════════════════

/// What a device brings. Mirrors the pairing handshake's `X-Device-Capabilities: touch,imu,voice,
/// buttons` header, plus `display` for a device that renders its own view (a second screen or a
/// co-player's laptop) rather than being a pure controller.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct DeviceCaps(pub u8);

impl DeviceCaps {
    pub const TOUCH: DeviceCaps = DeviceCaps(1);
    pub const IMU: DeviceCaps = DeviceCaps(2);
    pub const VOICE: DeviceCaps = DeviceCaps(4);
    pub const BUTTONS: DeviceCaps = DeviceCaps(8);
    pub const DISPLAY: DeviceCaps = DeviceCaps(16);
    const ALL: [(DeviceCaps, &'static str); 5] = [
        (DeviceCaps::TOUCH, "touch"),
        (DeviceCaps::IMU, "imu"),
        (DeviceCaps::VOICE, "voice"),
        (DeviceCaps::BUTTONS, "buttons"),
        (DeviceCaps::DISPLAY, "display"),
    ];

    pub fn has(self, c: DeviceCaps) -> bool {
        self.0 & c.0 == c.0
    }
    pub fn with(self, c: DeviceCaps) -> DeviceCaps {
        DeviceCaps(self.0 | c.0)
    }
    /// A pure controller: it renders nothing, so the render box owes it a splitscreen viewport.
    pub fn is_controller(self) -> bool {
        !self.has(DeviceCaps::DISPLAY) && self.0 != 0
    }
    /// Parse the header form: `touch,imu,voice`. Unknown words are ignored (a newer phone may
    /// send a capability an older render box has no use for); an empty result is `None`.
    pub fn parse(s: &str) -> Option<DeviceCaps> {
        let mut out = DeviceCaps(0);
        for word in s.split(',') {
            let w = word.trim().to_ascii_lowercase();
            if let Some((c, _)) = Self::ALL.iter().find(|(_, n)| *n == w) {
                out = out.with(*c);
            }
        }
        (out.0 != 0).then_some(out)
    }
    /// The header form back: `touch,imu`.
    pub fn label(self) -> String {
        Self::ALL.iter().filter(|(c, _)| self.has(*c)).map(|(_, n)| *n).collect::<Vec<_>>().join(",")
    }
}

// ═══════════════════════════════════════════════ the advert ════════════════════════════════════

/// What a render box publishes under [`session_key`]: who it is, what it is running, and how
/// many device slots are free. No address (see the module doc).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DeviceAdvert {
    pub host: UnendlichId,
    pub host_name: String,
    pub world_name: String,
    pub used: u8,
    pub max: u8,
    pub last_seen_ms: u64,
}

const ADVERT_TAG: u8 = 0xD1; // "device advert v1"

impl DeviceAdvert {
    pub fn is_full(&self) -> bool {
        self.max > 0 && self.used >= self.max
    }
    pub fn free(&self) -> u8 {
        self.max.saturating_sub(self.used)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut o = vec![ADVERT_TAG];
        put_str(&mut o, self.host.as_str());
        put_str(&mut o, &self.host_name);
        put_str(&mut o, &self.world_name);
        o.push(self.used);
        o.push(self.max);
        o.extend_from_slice(&self.last_seen_ms.to_le_bytes());
        o
    }

    /// `None` on any malformed, truncated or foreign record — the DHT is public.
    pub fn decode(b: &[u8]) -> Option<DeviceAdvert> {
        if *b.first()? != ADVERT_TAG {
            return None;
        }
        let mut p = 1usize;
        let host = UnendlichId::new(take_str(b, &mut p)?);
        let host_name = take_str(b, &mut p)?;
        let world_name = take_str(b, &mut p)?;
        let used = *b.get(p)?;
        let max = *b.get(p + 1)?;
        p += 2;
        let ts: [u8; 8] = b.get(p..p + 8)?.try_into().ok()?;
        Some(DeviceAdvert { host, host_name, world_name, used, max, last_seen_ms: u64::from_le_bytes(ts) })
    }
}

const MAX_FIELD: usize = 128;

fn put_str(o: &mut Vec<u8>, s: &str) {
    let n = s.len().min(MAX_FIELD);
    o.push(n as u8);
    o.extend_from_slice(&s.as_bytes()[..n]);
}

fn take_str(b: &[u8], p: &mut usize) -> Option<String> {
    let n = *b.get(*p)? as usize;
    if n > MAX_FIELD {
        return None;
    }
    let start = *p + 1;
    let slice = b.get(start..start + n)?;
    *p = start + n;
    Some(String::from_utf8_lossy(slice).into_owned())
}

/// A stable `PlayerId` for the roster from a device's id (FNV-1a over the id string).
pub fn player_id_of(id: &UnendlichId) -> PlayerId {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in id.as_str().as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

// ═══════════════════════════════════════════ host: the device table ════════════════════════════

/// What the host learns from one `pump`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DeviceEvent {
    /// A new device took `slot`. `name` is the name it was given (deduplicated if taken).
    Joined { slot: Slot, name: String, caps: DeviceCaps },
    /// A device the host already knew came back after a drop; it keeps its slot.
    Reconnected { slot: Slot, name: String },
    /// The request was refused — no free slot, or a request with no capabilities at all.
    Rejected { id: UnendlichId, reason: &'static str },
    /// A device went quiet past the TTL and its slot was released.
    Dropped { slot: Slot, name: String },
    /// The host removed it.
    Kicked { slot: Slot, name: String },
}

#[derive(Clone, Debug)]
struct DeviceInfo {
    id: UnendlichId,
    caps: DeviceCaps,
    last_seen_ms: u64,
}

/// One row of the device table, as the menu shows it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DeviceRow {
    pub slot: Slot,
    pub name: String,
    pub caps: DeviceCaps,
    /// Heard from within the freshness window (else it is on its way to being dropped).
    pub fresh: bool,
    pub connected: bool,
}

/// The render box's side: the code, the advert, the joined devices and their slots.
/// Slot 0 is the host itself (the render box is player one); devices take 1..max.
pub struct DeviceSession {
    code: SessionCode,
    host: UnendlichId,
    host_name: String,
    world_name: String,
    max_devices: u8,
    roster: Roster,
    devices: Vec<DeviceInfo>,
    announce_period_ms: u64,
    next_announce_ms: u64,
    /// The address the host hands to an admitted device — the bin sets it from its socket.
    addr: String,
}

/// How often the advert is refreshed and how long a device may go quiet.
pub const ANNOUNCE_PERIOD_MS: u64 = 5_000;
pub const DEVICE_TTL_MS: u64 = 15_000;
pub const FRESH_MS: u64 = 6_000;

impl DeviceSession {
    pub fn new(code: SessionCode, host: UnendlichId, host_name: &str, world_name: &str, max_devices: u8, addr: &str) -> DeviceSession {
        let mut roster = Roster::new();
        let _ = roster.join(player_id_of(&host), host_name, Role::Host);
        roster.set_state(player_id_of(&host), ConnState::Active);
        DeviceSession {
            code,
            host,
            host_name: host_name.to_string(),
            world_name: world_name.to_string(),
            max_devices: max_devices.max(1),
            roster,
            devices: Vec::new(),
            announce_period_ms: ANNOUNCE_PERIOD_MS,
            next_announce_ms: 0,
            addr: addr.to_string(),
        }
    }

    pub fn code(&self) -> &SessionCode {
        &self.code
    }
    pub fn max_devices(&self) -> u8 {
        self.max_devices
    }
    /// Devices joined (the host itself is not a device).
    pub fn device_count(&self) -> usize {
        self.devices.len()
    }
    pub fn is_full(&self) -> bool {
        self.devices.len() >= self.max_devices as usize
    }

    /// The advert as of `now`.
    pub fn advert(&self, now_ms: u64) -> DeviceAdvert {
        DeviceAdvert {
            host: self.host.clone(),
            host_name: self.host_name.clone(),
            world_name: self.world_name.clone(),
            used: self.devices.len().min(255) as u8,
            max: self.max_devices,
            last_seen_ms: now_ms,
        }
    }

    /// Publish the advert when due (first call publishes immediately). Returns whether it did.
    pub fn announce<R: SessionRecords>(&mut self, records: &R, now_ms: u64) -> bool {
        if now_ms < self.next_announce_ms {
            return false;
        }
        records.publish(session_key(&self.code), self.advert(now_ms).encode());
        self.next_announce_ms = now_ms + self.announce_period_ms;
        true
    }

    /// Withdraw the advert: publish a full one, so a phone that still finds it knows not to ask.
    pub fn close<R: SessionRecords>(&self, records: &R, now_ms: u64) {
        let mut a = self.advert(now_ms);
        a.used = a.max;
        records.publish(session_key(&self.code), a.encode());
    }

    /// Drain the device mailbox: admit, reconnect or refuse each request, reply through the
    /// requester's own mailbox, and report what happened.
    pub fn pump<D: MailboxDht>(&mut self, dht: &D, now_ms: u64) -> Vec<DeviceEvent> {
        let mut events = Vec::new();
        for raw in dht.get(device_mailbox_key(&self.code)) {
            let Some(MailMsg::DeviceRequest { from, name, caps }) = MailMsg::decode(&raw) else { continue };
            let caps = DeviceCaps(caps);
            let pid = player_id_of(&from);
            if let Some(i) = self.devices.iter().position(|d| d.id == from) {
                // known device: back after a drop or a re-sent request — same slot, no second entry
                self.devices[i].last_seen_ms = now_ms;
                self.devices[i].caps = caps;
                self.roster.reconnect(pid);
                let slot = self.roster.slot_of(pid).unwrap_or(0);
                let assigned = self.roster.by_id(pid).map(|e| e.name.clone()).unwrap_or(name);
                dht.put(mailbox_key(&from), MailMsg::Accept { from: self.host.clone(), assigned_name: assigned.clone(), addr: self.addr.clone() }.encode());
                events.push(DeviceEvent::Reconnected { slot, name: assigned });
                continue;
            }
            if caps.0 == 0 {
                dht.put(mailbox_key(&from), MailMsg::Deny { from: self.host.clone() }.encode());
                events.push(DeviceEvent::Rejected { id: from, reason: "no capabilities" });
                continue;
            }
            if self.is_full() {
                dht.put(mailbox_key(&from), MailMsg::Deny { from: self.host.clone() }.encode());
                events.push(DeviceEvent::Rejected { id: from, reason: "session full" });
                continue;
            }
            let assigned = self.unique_name(&name);
            match self.roster.join(pid, assigned.clone(), Role::Player) {
                Ok(slot) => {
                    self.roster.set_state(pid, ConnState::Active);
                    self.devices.push(DeviceInfo { id: from.clone(), caps, last_seen_ms: now_ms });
                    dht.put(mailbox_key(&from), MailMsg::Accept { from: self.host.clone(), assigned_name: assigned.clone(), addr: self.addr.clone() }.encode());
                    events.push(DeviceEvent::Joined { slot, name: assigned, caps });
                }
                Err(JoinError::RosterFull) | Err(JoinError::DuplicateId) => {
                    dht.put(mailbox_key(&from), MailMsg::Deny { from: self.host.clone() }.encode());
                    events.push(DeviceEvent::Rejected { id: from, reason: "roster refused" });
                }
            }
        }
        events
    }

    /// A name no one at the table already has: `Phone`, `Phone (2)`, `Phone (3)`.
    fn unique_name(&self, wanted: &str) -> String {
        let base = if wanted.trim().is_empty() { "Device" } else { wanted.trim() };
        let taken = |n: &str| self.roster.iter().any(|e| e.name == n);
        if !taken(base) {
            return base.to_string();
        }
        (2..).map(|k| format!("{base} ({k})")).find(|n| !taken(n)).unwrap()
    }

    /// The device sent something (an input packet, a ping): it is alive.
    pub fn heartbeat(&mut self, id: &UnendlichId, now_ms: u64) -> bool {
        match self.devices.iter_mut().find(|d| &d.id == id) {
            Some(d) => {
                d.last_seen_ms = now_ms;
                self.roster.reconnect(player_id_of(id));
                true
            }
            None => false,
        }
    }

    /// Mark devices quiet past `FRESH_MS` as disconnected (slot kept), and release the slots of
    /// those quiet past `ttl_ms`.
    pub fn prune(&mut self, now_ms: u64, ttl_ms: u64) -> Vec<DeviceEvent> {
        let mut events = Vec::new();
        let mut i = 0;
        while i < self.devices.len() {
            let quiet = now_ms.saturating_sub(self.devices[i].last_seen_ms);
            let pid = player_id_of(&self.devices[i].id);
            if quiet > ttl_ms {
                let slot = self.roster.slot_of(pid).unwrap_or(0);
                let name = self.roster.by_id(pid).map(|e| e.name.clone()).unwrap_or_default();
                self.roster.leave(pid);
                self.devices.remove(i);
                events.push(DeviceEvent::Dropped { slot, name });
                continue;
            }
            if quiet > FRESH_MS {
                self.roster.disconnect(pid);
            }
            i += 1;
        }
        events
    }

    /// Remove the device in `slot`. It is told nothing over the DHT — its next input packet
    /// simply goes unanswered; a re-request would be admitted as a new device.
    pub fn kick(&mut self, slot: Slot) -> Option<DeviceEvent> {
        let entry = self.roster.by_slot(slot)?;
        if entry.role == Role::Host {
            return None;
        }
        let pid = entry.id;
        let name = entry.name.clone();
        self.devices.retain(|d| player_id_of(&d.id) != pid);
        self.roster.leave(pid);
        Some(DeviceEvent::Kicked { slot, name })
    }

    /// The table, host first, as the menu shows it.
    pub fn rows(&self, now_ms: u64) -> Vec<DeviceRow> {
        let mut rows: Vec<DeviceRow> = self
            .roster
            .iter()
            .map(|e| {
                let info = self.devices.iter().find(|d| player_id_of(&d.id) == e.id);
                let fresh = match info {
                    Some(d) => now_ms.saturating_sub(d.last_seen_ms) <= FRESH_MS,
                    None => true, // the host
                };
                DeviceRow {
                    slot: e.slot,
                    name: e.name.clone(),
                    caps: info.map(|d| d.caps).unwrap_or(DeviceCaps::DISPLAY),
                    fresh,
                    connected: e.state == ConnState::Active,
                }
            })
            .collect();
        rows.sort_by_key(|r| r.slot);
        rows
    }

    /// The menu's view of the session.
    pub fn view(&self, now_ms: u64) -> DevicesView {
        DevicesView {
            code: self.code.grouped(),
            qr: self.qr_payload(),
            rows: self.rows(now_ms),
            max: self.max_devices,
            discovery: None,
        }
    }

    /// The QR payload for step 4 of the ladder: code + host id, no address.
    pub fn qr_payload(&self) -> String {
        format!("unendlich://join?code={}&host={}", self.code.as_str(), self.host.as_str())
    }
}

// ═══════════════════════════════════════════ phone: finding the box ════════════════════════════

/// The four discovery methods, in the order tried — each slower and wider than the last.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    /// `_unendlich._tcp` on the local link, 200 ms.
    Mdns,
    /// The last few addresses that worked, probed, 50 ms each.
    Cache,
    /// FIND_VALUE for [`session_key`], up to 5 s.
    Dht,
    /// A scanned QR or typed code/address. Never times out.
    Manual,
}

impl Step {
    pub fn timeout_ms(self) -> u64 {
        match self {
            Step::Mdns => 200,
            Step::Cache => 200, // 4 probes × 50 ms
            Step::Dht => 5_000,
            Step::Manual => u64::MAX,
        }
    }
    fn next(self) -> Option<Step> {
        match self {
            Step::Mdns => Some(Step::Cache),
            Step::Cache => Some(Step::Dht),
            Step::Dht => Some(Step::Manual),
            Step::Manual => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Step::Mdns => "local network",
            Step::Cache => "last address",
            Step::Dht => "DHT",
            Step::Manual => "code / QR",
        }
    }
}

/// What a step found: a direct address (mDNS, cache, QR with address) or an advert to ask to
/// join through the DHT.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum HostTarget {
    Addr(String),
    Advert(DeviceAdvert),
}

/// The discovery ladder as a state machine: the bin runs each step's IO and reports; the ladder
/// decides when to move on and remembers which step succeeded (the pairing dialog's small icon).
#[derive(Clone, Debug)]
pub struct DiscoveryLadder {
    step: Step,
    step_started_ms: u64,
    found: Option<(Step, HostTarget)>,
    tried: Vec<Step>,
}

impl DiscoveryLadder {
    pub fn start(now_ms: u64) -> DiscoveryLadder {
        DiscoveryLadder { step: Step::Mdns, step_started_ms: now_ms, found: None, tried: vec![Step::Mdns] }
    }

    pub fn step(&self) -> Step {
        self.step
    }
    pub fn found(&self) -> Option<&(Step, HostTarget)> {
        self.found.as_ref()
    }
    pub fn is_done(&self) -> bool {
        self.found.is_some()
    }
    /// Steps tried so far, in order.
    pub fn tried(&self) -> &[Step] {
        &self.tried
    }

    /// The bin reports a step's result. A hit short-circuits: the remaining steps are never
    /// tried. A miss on the CURRENT step moves on at once rather than waiting out its timeout;
    /// a miss reported for a step already left is ignored (a slow mDNS answer arriving during
    /// the DHT lookup must not reset anything).
    pub fn report(&mut self, step: Step, result: Option<HostTarget>, now_ms: u64) {
        if self.found.is_some() || step != self.step {
            return;
        }
        match result {
            Some(t) => self.found = Some((step, t)),
            None => self.advance(now_ms),
        }
    }

    /// Time passes: a step past its timeout is abandoned for the next one.
    pub fn tick(&mut self, now_ms: u64) {
        if self.found.is_none() && now_ms.saturating_sub(self.step_started_ms) > self.step.timeout_ms() {
            self.advance(now_ms);
        }
    }

    fn advance(&mut self, now_ms: u64) {
        if let Some(n) = self.step.next() {
            self.step = n;
            self.step_started_ms = now_ms;
            self.tried.push(n);
        }
    }
}

/// The last few addresses that worked (step 2), most recent first, at most four.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AddressCache {
    addrs: Vec<String>,
}

impl AddressCache {
    pub const CAPACITY: usize = 4;
    pub fn remember(&mut self, addr: &str) {
        self.addrs.retain(|a| a != addr);
        self.addrs.insert(0, addr.to_string());
        self.addrs.truncate(Self::CAPACITY);
    }
    pub fn candidates(&self) -> &[String] {
        &self.addrs
    }
    /// One line per address for a config file; `load` is its inverse.
    pub fn save(&self) -> String {
        self.addrs.join("\n")
    }
    pub fn load(s: &str) -> AddressCache {
        let mut c = AddressCache::default();
        for line in s.lines().rev() {
            let l = line.trim();
            if !l.is_empty() {
                c.remember(l);
            }
        }
        c
    }
}

// ═══════════════════════════════════════════ phone: asking to join ═════════════════════════════

/// How the pairing ended.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PairOutcome {
    /// Admitted: connect to `addr` as `name`.
    Paired { name: String, addr: String },
    Refused,
}

/// The phone's join, once discovery found an advert: drop a request in the host's device
/// mailbox, then poll its own mailbox for the answer. A direct address (mDNS / cache / QR) needs
/// none of this — the bin connects.
#[derive(Clone, Debug)]
pub struct DeviceJoin {
    me: UnendlichId,
    name: String,
    caps: DeviceCaps,
    code: SessionCode,
    requested_ms: Option<u64>,
    attempts: u32,
    outcome: Option<PairOutcome>,
}

/// Re-send the request if no answer comes within this.
pub const REQUEST_RETRY_MS: u64 = 3_000;
pub const MAX_ATTEMPTS: u32 = 3;

impl DeviceJoin {
    pub fn new(me: UnendlichId, name: &str, caps: DeviceCaps, code: SessionCode) -> DeviceJoin {
        DeviceJoin { me, name: name.to_string(), caps, code, requested_ms: None, attempts: 0, outcome: None }
    }

    pub fn outcome(&self) -> Option<&PairOutcome> {
        self.outcome.as_ref()
    }
    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    /// Send (or re-send) the request. Returns false once the attempt budget is spent.
    pub fn request<D: MailboxDht>(&mut self, dht: &D, now_ms: u64) -> bool {
        if self.outcome.is_some() || self.attempts >= MAX_ATTEMPTS {
            return false;
        }
        dht.put(
            device_mailbox_key(&self.code),
            MailMsg::DeviceRequest { from: self.me.clone(), name: self.name.clone(), caps: self.caps.0 }.encode(),
        );
        self.requested_ms = Some(now_ms);
        self.attempts += 1;
        true
    }

    /// Poll for the host's answer; re-send when the last request has gone unanswered too long.
    /// After the attempt budget is spent with no answer, the outcome is `Refused`.
    pub fn poll<D: MailboxDht>(&mut self, dht: &D, now_ms: u64) -> Option<&PairOutcome> {
        if self.outcome.is_some() {
            return self.outcome.as_ref();
        }
        for raw in dht.get(mailbox_key(&self.me)) {
            match MailMsg::decode(&raw) {
                Some(MailMsg::Accept { assigned_name, addr, .. }) => {
                    self.outcome = Some(PairOutcome::Paired { name: assigned_name, addr });
                    return self.outcome.as_ref();
                }
                Some(MailMsg::Deny { .. }) => {
                    self.outcome = Some(PairOutcome::Refused);
                    return self.outcome.as_ref();
                }
                _ => {}
            }
        }
        if let Some(t) = self.requested_ms {
            if now_ms.saturating_sub(t) > REQUEST_RETRY_MS {
                if !self.request(dht, now_ms) {
                    self.outcome = Some(PairOutcome::Refused);
                }
            }
        }
        self.outcome.as_ref()
    }
}

// ═══════════════════════════════════════════════ the menu's view ═══════════════════════════════

/// Everything the Devices screen shows. Built by the host from its session, or by a phone from
/// its ladder (`discovery` set, `rows` empty).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct DevicesView {
    /// `ABC-DEF`, or empty when this device is not hosting.
    pub code: String,
    pub qr: String,
    pub rows: Vec<DeviceRow>,
    pub max: u8,
    /// On a phone: the step in progress, or the step that succeeded.
    pub discovery: Option<(Step, bool)>,
}

impl DevicesView {
    /// A phone's view while it looks for a render box.
    pub fn searching(ladder: &DiscoveryLadder) -> DevicesView {
        DevicesView {
            discovery: Some(match ladder.found() {
                Some((step, _)) => (*step, true),
                None => (ladder.step(), false),
            }),
            ..DevicesView::default()
        }
    }
    /// One line of status for the screen.
    pub fn status(&self) -> String {
        match self.discovery {
            Some((step, true)) => format!("found via {}", step.label()),
            Some((step, false)) => format!("looking… ({})", step.label()),
            None if self.code.is_empty() => "not hosting".to_string(),
            None => format!("{} of {} devices", self.rows.len().saturating_sub(1), self.max),
        }
    }
}

// ═════════════════════════════════════════════════ tests ═══════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::super::dht_mailbox::LocalDht;
    use super::*;

    fn id(s: &str) -> UnendlichId {
        UnendlichId::new(s)
    }

    fn session(max: u8) -> DeviceSession {
        DeviceSession::new(SessionCode::from_seed(7), id("HOSTHOSTHOST"), "kessler", "Vale", max, "10.0.0.2:47832")
    }

    fn phone(name: &str, i: u64) -> DeviceJoin {
        DeviceJoin::new(UnendlichId::from_seed(1000 + i), name, DeviceCaps::TOUCH.with(DeviceCaps::IMU), SessionCode::from_seed(7))
    }

    // ── codes ──

    #[test]
    fn codes_are_six_unambiguous_chars_and_survive_a_person_typing_them() {
        for seed in 0..200u64 {
            let c = SessionCode::from_seed(seed);
            assert_eq!(c.as_str().len(), CODE_LEN);
            for ch in c.as_str().bytes() {
                assert!(CODE_ALPHABET.contains(&ch), "{ch} in {}", c.as_str());
                assert!(!b"0O1IL".contains(&ch));
            }
            let grouped = c.grouped();
            assert_eq!(grouped.len(), 7);
            assert_eq!(SessionCode::parse(&grouped.to_lowercase()).as_ref(), Some(&c));
            assert_eq!(SessionCode::parse(&format!(" {} ", grouped.replace('-', " "))).as_ref(), Some(&c));
        }
        // a spread: 200 seeds should not collapse onto a handful of codes
        let distinct: std::collections::HashSet<String> = (0..200u64).map(|s| SessionCode::from_seed(s).0).collect();
        assert!(distinct.len() >= 195, "{} distinct of 200", distinct.len());
    }

    #[test]
    fn a_code_that_cannot_be_right_is_refused_not_guessed() {
        assert!(SessionCode::parse("ABC-DE").is_none(), "too short");
        assert!(SessionCode::parse("ABCDEFG").is_none(), "too long");
        assert!(SessionCode::parse("ABC0EF").is_none(), "0 is not in the alphabet");
        assert!(SessionCode::parse("ABCIEF").is_none(), "I is not in the alphabet");
        assert!(SessionCode::parse("ABCLEF").is_none(), "L is not in the alphabet");
    }

    #[test]
    fn the_record_key_and_the_mailbox_key_never_collide() {
        let c = SessionCode::from_seed(3);
        assert_ne!(session_key(&c), device_mailbox_key(&c));
        assert_ne!(session_key(&c), session_key(&SessionCode::from_seed(4)));
    }

    // ── caps + advert wire ──

    #[test]
    fn capabilities_round_trip_the_header_form_and_ignore_unknown_words() {
        let c = DeviceCaps::parse("touch, IMU ,voice,hologram").unwrap();
        assert!(c.has(DeviceCaps::TOUCH) && c.has(DeviceCaps::IMU) && c.has(DeviceCaps::VOICE));
        assert!(!c.has(DeviceCaps::BUTTONS));
        assert_eq!(c.label(), "touch,imu,voice");
        assert!(c.is_controller(), "no display → a pure controller");
        assert!(!DeviceCaps::DISPLAY.is_controller());
        assert!(DeviceCaps::parse("hologram").is_none(), "nothing known → None, not an empty device");
    }

    #[test]
    fn advert_round_trips_and_rejects_garbage() {
        let a = DeviceAdvert { host: id("HOSTHOSTHOST"), host_name: "kessler".into(), world_name: "Vale".into(), used: 1, max: 4, last_seen_ms: 123_456 };
        let bytes = a.encode();
        assert_eq!(DeviceAdvert::decode(&bytes).as_ref(), Some(&a));
        assert_eq!(a.free(), 3);
        for cut in 0..bytes.len() {
            assert!(DeviceAdvert::decode(&bytes[..cut]).is_none(), "truncated at {cut} decoded");
        }
        assert!(DeviceAdvert::decode(&[1, 2, 3]).is_none(), "a world-join message is not an advert");
        assert!(DeviceAdvert::decode(&[]).is_none());
    }

    // ── host ──

    #[test]
    fn the_advert_is_published_on_cadence_and_withdrawn_as_full() {
        let recs = LocalRecords::default();
        let mut s = session(4);
        assert!(s.announce(&recs, 0), "first announce publishes at once");
        assert!(!s.announce(&recs, 100), "not due yet");
        assert!(s.announce(&recs, ANNOUNCE_PERIOD_MS + 1));
        let a = DeviceAdvert::decode(&recs.lookup(session_key(s.code())).unwrap()).unwrap();
        assert_eq!((a.used, a.max), (0, 4));
        assert!(!a.is_full());
        s.close(&recs, 9_000);
        let a = DeviceAdvert::decode(&recs.lookup(session_key(s.code())).unwrap()).unwrap();
        assert!(a.is_full(), "a closed session reads as full to a phone that still finds it");
    }

    #[test]
    fn a_phone_finds_the_advert_asks_and_is_paired_with_the_address_only_in_the_accept() {
        let recs = LocalRecords::default();
        let dht = LocalDht::default();
        let mut host = session(4);
        host.announce(&recs, 0);
        let advert = DeviceAdvert::decode(&recs.lookup(session_key(host.code())).unwrap()).unwrap();
        assert!(!advert.encode().windows(4).any(|w| w == b"10.0"), "the advert never carries the address");
        let mut p = phone("Phone", 1);
        assert!(p.request(&dht, 10));
        assert!(p.poll(&dht, 20).is_none(), "no answer yet");
        let ev = host.pump(&dht, 30);
        assert_eq!(ev, vec![DeviceEvent::Joined { slot: 1, name: "Phone".into(), caps: DeviceCaps::TOUCH.with(DeviceCaps::IMU) }]);
        assert_eq!(p.poll(&dht, 40), Some(&PairOutcome::Paired { name: "Phone".into(), addr: "10.0.0.2:47832".into() }));
        assert_eq!(host.device_count(), 1);
        let rows = host.rows(50);
        assert_eq!(rows.len(), 2, "host + phone");
        assert_eq!((rows[0].slot, rows[0].name.as_str()), (0, "kessler"));
        assert_eq!((rows[1].slot, rows[1].name.as_str(), rows[1].fresh), (1, "Phone", true));
    }

    #[test]
    fn two_phones_with_the_same_name_get_told_apart_and_a_full_session_refuses_a_third() {
        let dht = LocalDht::default();
        let mut host = session(2);
        let mut a = phone("Phone", 1);
        let mut b = phone("Phone", 2);
        let mut c = phone("Phone", 3);
        a.request(&dht, 0);
        b.request(&dht, 0);
        c.request(&dht, 0);
        let ev = host.pump(&dht, 1);
        assert_eq!(ev.len(), 3);
        assert!(matches!(&ev[0], DeviceEvent::Joined { slot: 1, name, .. } if name == "Phone"));
        assert!(matches!(&ev[1], DeviceEvent::Joined { slot: 2, name, .. } if name == "Phone (2)"));
        assert!(matches!(&ev[2], DeviceEvent::Rejected { reason: "session full", .. }));
        assert!(host.is_full());
        assert_eq!(c.poll(&dht, 2), Some(&PairOutcome::Refused));
        assert!(matches!(b.poll(&dht, 2), Some(PairOutcome::Paired { name, .. }) if name == "Phone (2)"));
    }

    #[test]
    fn a_device_that_re_asks_keeps_its_slot_and_a_quiet_one_is_dropped_after_the_ttl() {
        let dht = LocalDht::default();
        let mut host = session(4);
        let mut p = phone("Phone", 1);
        p.request(&dht, 0);
        host.pump(&dht, 1);
        // the phone's app restarts and asks again: same id, same slot, no second row
        let mut again = phone("Phone", 1);
        again.request(&dht, 100);
        let ev = host.pump(&dht, 101);
        assert_eq!(ev, vec![DeviceEvent::Reconnected { slot: 1, name: "Phone".into() }]);
        assert_eq!(host.device_count(), 1);
        // quiet: fresh → stale (slot kept, disconnected) → dropped (slot released)
        assert!(host.prune(101 + FRESH_MS + 1, DEVICE_TTL_MS).is_empty());
        let rows = host.rows(101 + FRESH_MS + 1);
        assert_eq!((rows[1].fresh, rows[1].connected), (false, false));
        assert!(host.heartbeat(&UnendlichId::from_seed(1001), 101 + FRESH_MS + 2), "an input packet revives it");
        assert!(host.rows(101 + FRESH_MS + 3)[1].connected);
        let ev = host.prune(101 + FRESH_MS + 3 + DEVICE_TTL_MS + 1, DEVICE_TTL_MS);
        assert_eq!(ev, vec![DeviceEvent::Dropped { slot: 1, name: "Phone".into() }]);
        assert_eq!(host.device_count(), 0);
        assert!(!host.heartbeat(&UnendlichId::from_seed(1001), 0), "gone is gone");
    }

    #[test]
    fn the_host_can_kick_a_device_but_not_itself_and_the_slot_is_reused() {
        let dht = LocalDht::default();
        let mut host = session(4);
        let mut p = phone("Phone", 1);
        p.request(&dht, 0);
        host.pump(&dht, 1);
        assert!(host.kick(0).is_none(), "slot 0 is the render box");
        assert_eq!(host.kick(1), Some(DeviceEvent::Kicked { slot: 1, name: "Phone".into() }));
        assert!(host.kick(1).is_none(), "already gone");
        let mut q = phone("Tablet", 2);
        q.request(&dht, 5);
        let ev = host.pump(&dht, 6);
        assert!(matches!(&ev[0], DeviceEvent::Joined { slot: 1, .. }), "lowest free slot again");
    }

    #[test]
    fn a_request_with_no_capabilities_is_refused_and_the_device_mailbox_ignores_world_join_post() {
        let dht = LocalDht::default();
        let mut host = session(4);
        let code = host.code().clone();
        dht.put(device_mailbox_key(&code), MailMsg::DeviceRequest { from: id("NOCAPSNOCAPS"), name: "x".into(), caps: 0 }.encode());
        dht.put(device_mailbox_key(&code), MailMsg::Request { from: id("WORLDJOINER1"), requested_name: "w".into() }.encode());
        let ev = host.pump(&dht, 1);
        assert_eq!(ev, vec![DeviceEvent::Rejected { id: id("NOCAPSNOCAPS"), reason: "no capabilities" }]);
        assert_eq!(host.device_count(), 0);
    }

    // ── phone: ladder ──

    #[test]
    fn the_ladder_short_circuits_on_the_first_hit_and_remembers_which_step() {
        let mut l = DiscoveryLadder::start(0);
        assert_eq!(l.step(), Step::Mdns);
        l.report(Step::Mdns, Some(HostTarget::Addr("192.168.1.10:47832".into())), 50);
        assert!(l.is_done());
        assert_eq!(l.found().map(|(s, _)| *s), Some(Step::Mdns));
        l.tick(10_000);
        assert_eq!(l.step(), Step::Mdns, "a hit freezes the ladder");
        assert_eq!(l.tried(), &[Step::Mdns]);
    }

    #[test]
    fn misses_and_timeouts_walk_the_ladder_and_a_late_answer_for_a_left_step_is_ignored() {
        let mut l = DiscoveryLadder::start(0);
        l.report(Step::Mdns, None, 10); // miss → cache at once
        assert_eq!(l.step(), Step::Cache);
        l.tick(10 + Step::Cache.timeout_ms() + 1); // timeout → dht
        assert_eq!(l.step(), Step::Dht);
        l.report(Step::Mdns, Some(HostTarget::Addr("late".into())), 300); // a late mDNS answer
        assert!(!l.is_done(), "an answer for a step already left is ignored");
        l.tick(10 + Step::Cache.timeout_ms() + 1 + Step::Dht.timeout_ms() + 1);
        assert_eq!(l.step(), Step::Manual);
        l.tick(u64::MAX / 2);
        assert_eq!(l.step(), Step::Manual, "manual never times out");
        assert_eq!(l.tried(), &[Step::Mdns, Step::Cache, Step::Dht, Step::Manual]);
        let adv = DeviceAdvert { host: id("HOSTHOSTHOST"), host_name: "k".into(), world_name: "V".into(), used: 0, max: 4, last_seen_ms: 0 };
        l.report(Step::Manual, Some(HostTarget::Advert(adv.clone())), 1);
        assert_eq!(l.found(), Some(&(Step::Manual, HostTarget::Advert(adv))));
    }

    #[test]
    fn the_address_cache_keeps_the_last_four_most_recent_first_and_round_trips_a_file() {
        let mut c = AddressCache::default();
        for a in ["a", "b", "c", "d", "e"] {
            c.remember(a);
        }
        assert_eq!(c.candidates(), &["e", "d", "c", "b"]);
        c.remember("c");
        assert_eq!(c.candidates(), &["c", "e", "d", "b"], "a repeat moves to the front, no duplicate");
        assert_eq!(AddressCache::load(&c.save()), c);
    }

    #[test]
    fn an_unanswered_request_is_re_sent_and_gives_up_after_the_budget() {
        let dht = LocalDht::default();
        let mut p = phone("Phone", 1);
        assert!(p.request(&dht, 0));
        assert!(p.poll(&dht, REQUEST_RETRY_MS + 1).is_none(), "re-sent, still waiting");
        assert_eq!(p.attempts(), 2);
        assert!(p.poll(&dht, 2 * (REQUEST_RETRY_MS + 1)).is_none());
        assert_eq!(p.attempts(), 3);
        assert_eq!(p.poll(&dht, 3 * (REQUEST_RETRY_MS + 1)), Some(&PairOutcome::Refused));
        assert!(!p.request(&dht, 99_999), "settled: no more sends");
        let code = SessionCode::from_seed(7);
        assert_eq!(dht.get(device_mailbox_key(&code)).len(), 3, "exactly the budget's worth of requests went out");
    }

    // ── the menu's view ──

    #[test]
    fn the_view_reads_as_hosting_searching_or_idle() {
        let mut host = session(4);
        let v = host.view(0);
        assert_eq!(v.code.len(), 7);
        assert!(v.qr.starts_with("unendlich://join?code=") && v.qr.contains("&host="));
        assert_eq!(v.status(), "0 of 4 devices");
        let dht = LocalDht::default();
        let mut p = phone("Phone", 1);
        p.request(&dht, 0);
        host.pump(&dht, 1);
        assert_eq!(host.view(2).status(), "1 of 4 devices");
        let mut l = DiscoveryLadder::start(0);
        assert_eq!(DevicesView::searching(&l).status(), "looking… (local network)");
        l.report(Step::Mdns, None, 1);
        l.report(Step::Cache, None, 2);
        assert_eq!(DevicesView::searching(&l).status(), "looking… (DHT)");
        l.report(Step::Dht, Some(HostTarget::Addr("x".into())), 3);
        assert_eq!(DevicesView::searching(&l).status(), "found via DHT");
        assert_eq!(DevicesView::default().status(), "not hosting");
    }
}
