//! `app::device_svc` — the render box's MULTI-DEVICE service: runs a
//! [`DeviceSession`] per tick over the two DHT seams and answers the title menu's
//! Devices screen (`live_menu` picks 10 / 11 and `device_kick`).
//!
//! Same doctrine as [`multiplayer_svc`](super::multiplayer_svc): the shell holds one
//! of these, calls [`DeviceHostSvc::tick`] once a frame and [`DeviceHostSvc::on_menu`]
//! with the menu's output, and hands [`DeviceHostSvc::view`] back to the menu. All of
//! that is asserted here headless over the in-process DHT stand-ins; the bin swaps in
//! its real node through the same generics. What the bin still owes is listed in
//! `docs/design/multi_device_dht.md`.

use crate::sim::net::device_session::{
    DeviceEvent, DeviceSession, DevicesView, LocalRecords, SessionCode, SessionRecords, DEVICE_TTL_MS,
};
use crate::sim::net::dht_mailbox::{LocalDht, MailboxDht};
use crate::sim::net::identity::UnendlichId;

/// The menu picks this service answers (`live_menu` contract, do not renumber).
pub const PICK_DEVICES: u32 = 10;
pub const PICK_STOP_HOSTING: u32 = 11;

pub struct DeviceHostSvc<R: SessionRecords, D: MailboxDht> {
    records: R,
    dht: D,
    me: UnendlichId,
    me_name: String,
    world_name: String,
    max_devices: u8,
    addr: String,
    seed: u64,
    session: Option<DeviceSession>,
    /// Human status line for the shell ("hosting ABC-DEF · 1 of 4", "device Phone joined slot 1", …).
    pub status: String,
    /// Everything that happened since the shell last drained it.
    pub events: Vec<DeviceEvent>,
}

impl DeviceHostSvc<LocalRecords, LocalDht> {
    /// Same-process stand-ins for both seams: tests, and local play where the phone runs in
    /// the same process (the loopback analogue of `transport::loopback_pair`).
    pub fn local(me: UnendlichId, me_name: &str, world_name: &str, max_devices: u8, addr: &str, seed: u64) -> Self {
        DeviceHostSvc::over(LocalRecords::default(), LocalDht::default(), me, me_name, world_name, max_devices, addr, seed)
    }
}

impl<R: SessionRecords, D: MailboxDht> DeviceHostSvc<R, D> {
    /// The real constructor: the bin's DHT node implements both seams.
    #[allow(clippy::too_many_arguments)]
    pub fn over(records: R, dht: D, me: UnendlichId, me_name: &str, world_name: &str, max_devices: u8, addr: &str, seed: u64) -> Self {
        DeviceHostSvc {
            records,
            dht,
            me,
            me_name: me_name.to_string(),
            world_name: world_name.to_string(),
            max_devices,
            addr: addr.to_string(),
            seed,
            session: None,
            status: "not hosting".to_string(),
            events: Vec::new(),
        }
    }

    pub fn records(&self) -> &R {
        &self.records
    }
    pub fn dht(&self) -> &D {
        &self.dht
    }
    pub fn is_hosting(&self) -> bool {
        self.session.is_some()
    }
    pub fn session(&self) -> Option<&DeviceSession> {
        self.session.as_ref()
    }

    /// Start hosting: a fresh code each time (the seed advances), advert published at the next tick.
    pub fn start(&mut self, now_ms: u64) -> &SessionCode {
        self.seed = self.seed.wrapping_mul(6364136223846793005).wrapping_add(now_ms | 1);
        let code = SessionCode::from_seed(self.seed);
        let s = DeviceSession::new(code, self.me.clone(), &self.me_name, &self.world_name, self.max_devices, &self.addr);
        self.status = format!("hosting {} · 0 of {}", s.code().grouped(), self.max_devices);
        self.session = Some(s);
        self.session.as_ref().unwrap().code()
    }

    /// Stop hosting: the advert is withdrawn as full, the table dropped.
    pub fn stop(&mut self, now_ms: u64) {
        if let Some(s) = self.session.take() {
            s.close(&self.records, now_ms);
        }
        self.status = "not hosting".to_string();
    }

    /// Once a frame: announce, admit, prune. Events accumulate in `events`.
    pub fn tick(&mut self, now_ms: u64) {
        let Some(s) = self.session.as_mut() else { return };
        s.announce(&self.records, now_ms);
        let mut ev = s.pump(&self.dht, now_ms);
        ev.extend(s.prune(now_ms, DEVICE_TTL_MS));
        for e in &ev {
            self.status = match e {
                DeviceEvent::Joined { slot, name, .. } => format!("{name} joined slot {slot}"),
                DeviceEvent::Reconnected { slot, name } => format!("{name} back in slot {slot}"),
                DeviceEvent::Rejected { reason, .. } => format!("refused a device: {reason}"),
                DeviceEvent::Dropped { name, .. } => format!("{name} dropped"),
                DeviceEvent::Kicked { name, .. } => format!("{name} kicked"),
            };
        }
        if ev.is_empty() {
            self.status = format!("hosting {} · {} of {}", s.code().grouped(), s.device_count(), s.max_devices());
        }
        self.events.extend(ev);
    }

    /// An input packet or ping arrived from `id`.
    pub fn heard_from(&mut self, id: &UnendlichId, now_ms: u64) -> bool {
        self.session.as_mut().map(|s| s.heartbeat(id, now_ms)).unwrap_or(false)
    }

    /// Route the title menu's output: pick 10 starts (or, when already hosting, is the screen
    /// itself and does nothing), pick 11 stops, a kick removes that slot.
    pub fn on_menu(&mut self, menu_pick: u32, device_kick: i32, now_ms: u64) {
        match menu_pick {
            PICK_DEVICES if self.session.is_none() => {
                self.start(now_ms);
            }
            PICK_STOP_HOSTING => self.stop(now_ms),
            _ => {}
        }
        if device_kick >= 0 {
            if let Some(s) = self.session.as_mut() {
                if let Some(e) = s.kick(device_kick as usize) {
                    self.status = match &e {
                        DeviceEvent::Kicked { name, .. } => format!("{name} kicked"),
                        _ => self.status.clone(),
                    };
                    self.events.push(e);
                }
            }
        }
    }

    /// What the Devices screen shows; `None` when not hosting (the screen then offers to start).
    pub fn view(&self, now_ms: u64) -> Option<DevicesView> {
        self.session.as_ref().map(|s| s.view(now_ms))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::net::device_session::{session_key, DeviceAdvert, DeviceCaps, DeviceJoin, PairOutcome};

    fn svc() -> DeviceHostSvc<LocalRecords, LocalDht> {
        DeviceHostSvc::local(UnendlichId::new("HOSTHOSTHOST"), "kessler", "Vale", 3, "10.0.0.2:47832", 42)
    }

    #[test]
    fn the_menu_starts_and_stops_hosting_and_the_advert_follows() {
        let mut s = svc();
        assert!(s.view(0).is_none(), "not hosting → the screen offers to start");
        s.on_menu(PICK_DEVICES, -1, 100);
        assert!(s.is_hosting());
        let code = s.session().unwrap().code().clone();
        assert!(s.status.starts_with("hosting ") && s.status.contains("0 of 3"), "{}", s.status);
        s.tick(101);
        let a = DeviceAdvert::decode(&s.records().lookup(session_key(&code)).unwrap()).unwrap();
        assert_eq!((a.used, a.max), (0, 3));
        // pick 10 while hosting is just the screen: the code does not change under the player
        s.on_menu(PICK_DEVICES, -1, 200);
        assert_eq!(s.session().unwrap().code(), &code);
        s.on_menu(PICK_STOP_HOSTING, -1, 300);
        assert!(!s.is_hosting());
        let a = DeviceAdvert::decode(&s.records().lookup(session_key(&code)).unwrap()).unwrap();
        assert!(a.is_full(), "stopping withdraws the advert as full");
        // a new session is a new code
        s.on_menu(PICK_DEVICES, -1, 400);
        assert_ne!(s.session().unwrap().code(), &code);
    }

    #[test]
    fn a_phone_joins_through_the_ticks_and_the_screen_and_kick_are_wired() {
        let mut s = svc();
        s.start(0);
        let code = s.session().unwrap().code().clone();
        let mut phone = DeviceJoin::new(UnendlichId::from_seed(9), "Phone", DeviceCaps::TOUCH, code);
        phone.request(s.dht(), 1);
        s.tick(2);
        assert_eq!(s.events.len(), 1);
        assert!(matches!(s.events[0], DeviceEvent::Joined { slot: 1, .. }));
        assert_eq!(s.status, "Phone joined slot 1");
        assert!(matches!(phone.poll(s.dht(), 3), Some(PairOutcome::Paired { addr, .. }) if addr == "10.0.0.2:47832"));
        let v = s.view(4).unwrap();
        assert_eq!(v.rows.len(), 2);
        assert_eq!(v.status(), "1 of 3 devices");
        // quiet tick: status returns to the summary
        s.tick(5);
        assert!(s.status.contains("1 of 3"), "{}", s.status);
        // an input packet keeps it alive across the TTL; silence drops it
        assert!(s.heard_from(&UnendlichId::from_seed(9), DEVICE_TTL_MS));
        s.tick(DEVICE_TTL_MS + 1);
        assert_eq!(s.view(DEVICE_TTL_MS + 1).unwrap().rows.len(), 2, "heard from → kept");
        s.tick(2 * DEVICE_TTL_MS + 2);
        assert!(matches!(s.events.last(), Some(DeviceEvent::Dropped { slot: 1, .. })));
        assert_eq!(s.view(2 * DEVICE_TTL_MS + 2).unwrap().rows.len(), 1);
        // rejoin, then the menu kicks it
        phone = DeviceJoin::new(UnendlichId::from_seed(9), "Phone", DeviceCaps::TOUCH, s.session().unwrap().code().clone());
        phone.request(s.dht(), 3 * DEVICE_TTL_MS);
        s.tick(3 * DEVICE_TTL_MS + 1);
        s.on_menu(0, 1, 3 * DEVICE_TTL_MS + 2);
        assert!(matches!(s.events.last(), Some(DeviceEvent::Kicked { slot: 1, .. })));
        assert_eq!(s.status, "Phone kicked");
        assert_eq!(s.view(3 * DEVICE_TTL_MS + 3).unwrap().rows.len(), 1);
        assert!(!s.heard_from(&UnendlichId::from_seed(9), 0), "a kicked device is unknown");
    }
}
