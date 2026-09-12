//! HomeKit over the real radio: `hap_core::Transport` on a btleplug
//! peripheral, the pairing store on disk, and the three things the radio
//! loop asks for (pair, set, read).
//!
//! # Sync inside async
//!
//! hap-core is synchronous on purpose (see its `ble` module); btleplug is
//! async. The bridge is `tokio::task::spawn_blocking` around the whole
//! procedure, with the transport calling `Handle::block_on` for each GATT
//! op. That is the sanctioned shape: `block_on` from a blocking-pool
//! thread, never from the reactor thread — on the `current_thread` runtime
//! the radio loop uses, the latter panics.
//!
//! # MTU
//!
//! btleplug does not expose the negotiated ATT MTU. HAP fragments at its
//! own layer and accessories reject a PDU larger than what they told the
//! stack, so the fragment size is `SWITCHEROO_HAP_MTU` (default 100):
//! small enough for any negotiated MTU, and the cost is a few extra
//! round trips in pair-setup, once. Raise it in the environment if a plug
//! is slow to pair and the journal shows many fragments.
//!
//! # Where the keys live
//!
//! `$XDG_DATA_HOME/switcheroo/` (or `~/.local/share/switcheroo/`):
//! `controller.hap` is this install's identity (36-byte id + 32-byte
//! Ed25519 secret), one `pairing-<address>.hap` per accessory in
//! `hap_core::Pairing`'s byte format. Losing `controller.hap` means every
//! plug must be reset and re-paired (HOMEKIT.md); back the directory up.

use btleplug::api::{Characteristic, Peripheral as _, WriteType};
use btleplug::platform::Peripheral;
use hap_core::ble::{characteristic as ch, hap_uuid};
use hap_core::{Capabilities, Controller, Pairing, Transport, TransportError};
use std::path::PathBuf;
use switcheroo_core::{DeviceId, Power};
use tokio::runtime::Handle;

pub const DEFAULT_MTU: usize = 100;

fn mtu() -> usize {
    std::env::var("SWITCHEROO_HAP_MTU").ok().and_then(|v| v.parse().ok()).filter(|m| (23..=512).contains(m)).unwrap_or(DEFAULT_MTU)
}

// ── the transport ──────────────────────────────────────────────────────────

struct BtleTransport {
    p: Peripheral,
    handle: Handle,
    chars: Vec<Characteristic>,
    mtu: usize,
    /// Instance ids read once per characteristic; they do not change
    /// within a connection.
    iids: std::collections::BTreeMap<u16, u16>,
}

impl BtleTransport {
    fn characteristic(&self, char_type: u16) -> Result<&Characteristic, TransportError> {
        let want = uuid::Uuid::parse_str(&hap_uuid(char_type)).expect("HAP UUIDs are well-formed");
        self.chars.iter().find(|c| c.uuid == want)
            .ok_or_else(|| TransportError(format!("accessory has no HAP characteristic {char_type:#04x} ({})", hap_uuid(char_type))))
    }
}

impl Transport for BtleTransport {
    fn write(&mut self, char_type: u16, data: &[u8]) -> Result<(), TransportError> {
        let c = self.characteristic(char_type)?.clone();
        self.handle.block_on(self.p.write(&c, data, WriteType::WithResponse))
            .map_err(|e| TransportError(format!("write {char_type:#04x}: {e}")))
    }

    fn read(&mut self, char_type: u16) -> Result<Vec<u8>, TransportError> {
        let c = self.characteristic(char_type)?.clone();
        self.handle.block_on(self.p.read(&c)).map_err(|e| TransportError(format!("read {char_type:#04x}: {e}")))
    }

    fn instance_id(&mut self, char_type: u16) -> Result<u16, TransportError> {
        if let Some(i) = self.iids.get(&char_type) { return Ok(*i); }
        let c = self.characteristic(char_type)?.clone();
        let want = uuid::Uuid::parse_str(ch::INSTANCE_ID_DESCRIPTOR).expect("well-formed");
        let d = c.descriptors.iter().find(|d| d.uuid == want)
            .ok_or_else(|| TransportError(format!("characteristic {char_type:#04x} has no HAP Instance ID descriptor")))?;
        let v = self.handle.block_on(self.p.read_descriptor(d)).map_err(|e| TransportError(format!("read instance id: {e}")))?;
        let iid = match v.as_slice() {
            [lo, hi, ..] => u16::from_le_bytes([*lo, *hi]),
            _ => return Err(TransportError(format!("instance id of {char_type:#04x} is {} bytes", v.len()))),
        };
        self.iids.insert(char_type, iid);
        Ok(iid)
    }

    fn mtu(&self) -> usize { self.mtu }
}

// ── the store ──────────────────────────────────────────────────────────────

pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn open() -> Store {
        let base = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .unwrap_or_else(|| PathBuf::from("."));
        let dir = base.join("switcheroo");
        let _ = std::fs::create_dir_all(&dir);
        Store { dir }
    }

    pub fn dir(&self) -> &std::path::Path { &self.dir }

    /// This install's controller identity, created on first use.
    pub fn controller(&self) -> Result<(String, [u8; 32]), String> {
        let path = self.dir.join("controller.hap");
        if let Ok(b) = std::fs::read(&path) {
            if b.len() == 36 + 32 {
                let id = String::from_utf8(b[..36].to_vec()).map_err(|_| "controller.hap: id is not UTF-8")?;
                let mut sk = [0u8; 32];
                sk.copy_from_slice(&b[36..]);
                return Ok((id, sk));
            }
            return Err(format!("{}: unexpected length {}", path.display(), b.len()));
        }
        let (id, sk) = hap_core::keys::new_controller_identity();
        let mut b = id.as_bytes().to_vec();
        b.extend_from_slice(&sk);
        std::fs::write(&path, &b).map_err(|e| format!("write {}: {e}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok((id, sk))
    }

    fn pairing_path(&self, id: &DeviceId) -> PathBuf {
        let safe: String = id.as_str().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
        self.dir.join(format!("pairing-{safe}.hap"))
    }

    pub fn load(&self, id: &DeviceId) -> Option<Pairing> {
        std::fs::read(self.pairing_path(id)).ok().and_then(|b| Pairing::from_bytes(&b))
    }

    pub fn has(&self, id: &DeviceId) -> bool { self.pairing_path(id).exists() }

    pub fn save(&self, id: &DeviceId, p: &Pairing) -> Result<(), String> {
        let path = self.pairing_path(id);
        std::fs::write(&path, p.to_bytes()).map_err(|e| format!("write {}: {e}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn forget(&self, id: &DeviceId) -> bool {
        std::fs::remove_file(self.pairing_path(id)).is_ok()
    }
}

// ── the procedures ─────────────────────────────────────────────────────────

async fn transport(p: Peripheral) -> Result<BtleTransport, String> {
    if !p.is_connected().await.unwrap_or(false) {
        p.connect().await.map_err(|e| format!("connect: {e}"))?;
    }
    p.discover_services().await.map_err(|e| format!("discover services: {e}"))?;
    let chars: Vec<Characteristic> = p.characteristics().into_iter().collect();
    let pairing_svc = uuid::Uuid::parse_str(&hap_uuid(hap_core::ble::service::PAIRING)).unwrap();
    if !chars.iter().any(|c| c.service_uuid == pairing_svc) {
        return Err("connected, but the device exposes no HAP Pairing service (0x55) — not a HomeKit accessory after all?".into());
    }
    Ok(BtleTransport { p, handle: Handle::current(), chars, mtu: mtu(), iids: Default::default() })
}

/// Pair Setup. Returns the record, which the caller has already asked the
/// store to keep; the journal lines are hap-core's PDU trace.
pub async fn pair(p: Peripheral, store: &Store, id: &DeviceId, setup_code: String) -> Result<(Pairing, Vec<String>), String> {
    let (cid, csk) = store.controller()?;
    let mut t = transport(p).await?;
    let out = tokio::task::spawn_blocking(move || {
        let mut c = Controller::new(&mut t);
        let r = c.pair_setup(&cid, csk, &setup_code).map_err(|e| e.to_string());
        let trace = std::mem::take(&mut c.trace);
        (r, trace)
    }).await.map_err(|e| format!("pairing task: {e}"))?;
    let (r, trace) = out;
    let pairing = r?;
    store.save(id, &pairing)?;
    Ok((pairing, trace))
}

/// Verify with the stored pairing, then write On. Both in one blocking
/// task so the session never crosses an await point.
pub async fn set_power(
    p: Peripheral,
    pairing: Pairing,
    on: bool,
) -> Result<(bool, Vec<String>), String> {
    let mut t = transport(p).await?;
    tokio::task::spawn_blocking(move || {
        let mut c = Controller::new(&mut t);
        c.pair_verify(&pairing).map_err(|e| e.to_string())?;
        c.set_on(on).map_err(|e| e.to_string())?;
        // READ BACK, and report what the ACCESSORY says rather than what we asked for.
        //
        // A successful write means the accessory accepted the PDU, not that the outlet
        // changed: a plug with a child lock, a firmware in a fault state, or one that clamps
        // a value all ACK and do nothing. Reporting the requested value then leaves the row
        // showing On over a socket that is off, and the next `AccessoryChanged` advert looks
        // like someone else touched it.
        //
        // In the SAME session, for the reason `inspect` gives below: each `pair_verify` is a
        // Curve25519 exchange and two signature round trips over BLE, so a second session to
        // confirm one bit would double the cost of every toggle.
        let actual = c.get_on().map_err(|e| e.to_string())?;
        Ok((actual, std::mem::take(&mut c.trace)))
    }).await.map_err(|e| format!("write task: {e}"))?
}

/// Verify, then ask the accessory what it has and read the outlet's live state in one
/// session: the capabilities, whether it is on, and whether anything is plugged in.
///
/// One session for all three because each `pair_verify` is a Curve25519 exchange and two
/// signature round trips over BLE — doing it per question turns opening a row into four
/// handshakes. The whole thing is one blocking task so the session never crosses an await.
pub async fn inspect(p: Peripheral, pairing: Pairing) -> Result<(Capabilities, Power, Option<bool>, Vec<String>), String> {
    let mut t = transport(p).await?;
    tokio::task::spawn_blocking(move || {
        let mut c = Controller::new(&mut t);
        c.pair_verify(&pairing).map_err(|e| e.to_string())?;
        let caps = c.probe().map_err(|e| e.to_string())?;
        // Only read what the accessory said it has. Reading On from something whose
        // signature does not list it is a round trip that can only fail.
        let power = if caps.get(hap_core::ble::characteristic::ON).is_some_and(|s| s.readable()) {
            match c.get_on().map_err(|e| e.to_string())? {
                true => Power::On,
                false => Power::Off,
            }
        } else {
            Power::Unknown
        };
        let in_use = if caps.reports_load() { Some(c.outlet_in_use().map_err(|e| e.to_string())?) } else { None };
        Ok((caps, power, in_use, std::mem::take(&mut c.trace)))
    })
    .await
    .map_err(|e| format!("inspect task: {e}"))?
}

/// Tell the accessory to forget us, then report whether it did.
///
/// The caller deletes the local record AFTER this returns, and the order is the whole
/// point: with the record gone first, a radio failure leaves the controller without the
/// keys to ask again and the accessory claimed by a pairing nobody holds — recoverable
/// only by the factory reset the pairing dialog warns about. In this order a failure
/// leaves both sides paired, which is a state the user can retry from.
pub async fn remove_pairing(p: Peripheral, pairing: Pairing) -> Result<Vec<String>, String> {
    let mut t = transport(p).await?;
    tokio::task::spawn_blocking(move || {
        let mut c = Controller::new(&mut t);
        c.pair_verify(&pairing).map_err(|e| e.to_string())?;
        c.remove_pairing(&pairing.controller_id).map_err(|e| e.to_string())?;
        Ok(std::mem::take(&mut c.trace))
    })
    .await
    .map_err(|e| format!("remove-pairing task: {e}"))?
}

/// Ask the accessory to blink or beep — "which of these three plugs are you".
pub async fn identify(p: Peripheral, pairing: Pairing) -> Result<Vec<String>, String> {
    let mut t = transport(p).await?;
    tokio::task::spawn_blocking(move || {
        let mut c = Controller::new(&mut t);
        c.pair_verify(&pairing).map_err(|e| e.to_string())?;
        c.identify().map_err(|e| e.to_string())?;
        Ok(std::mem::take(&mut c.trace))
    })
    .await
    .map_err(|e| format!("identify task: {e}"))?
}

pub async fn read_power(p: Peripheral, pairing: Pairing) -> Result<(Power, Vec<String>), String> {
    let mut t = transport(p).await?;
    tokio::task::spawn_blocking(move || {
        let mut c = Controller::new(&mut t);
        c.pair_verify(&pairing).map_err(|e| e.to_string())?;
        let on = c.get_on().map_err(|e| e.to_string())?;
        Ok((if on { Power::On } else { Power::Off }, std::mem::take(&mut c.trace)))
    }).await.map_err(|e| format!("read task: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_store_round_trips_a_pairing_and_makes_one_identity() {
        let tmp = std::env::temp_dir().join(format!("sw-hap-store-{}", std::process::id()));
        std::env::set_var("XDG_DATA_HOME", &tmp);
        let s = Store::open();
        let (id1, sk1) = s.controller().unwrap();
        let (id2, sk2) = s.controller().unwrap();
        assert_eq!((id1.clone(), sk1), (id2, sk2), "identity is created once");
        let dev = DeviceId::new("AA:BB:CC:DD:EE:FF");
        assert!(!s.has(&dev));
        let p = Pairing { controller_id: id1, controller_ltsk: sk1, accessory_id: "acc".into(), accessory_ltpk: [7; 32] };
        s.save(&dev, &p).unwrap();
        assert!(s.has(&dev));
        assert_eq!(s.load(&dev).unwrap(), p);
        assert!(s.forget(&dev));
        assert!(!s.has(&dev));
        std::env::remove_var("XDG_DATA_HOME");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn mtu_comes_from_the_environment_within_bounds() {
        std::env::set_var("SWITCHEROO_HAP_MTU", "9999");
        assert_eq!(mtu(), DEFAULT_MTU);
        std::env::set_var("SWITCHEROO_HAP_MTU", "180");
        assert_eq!(mtu(), 180);
        std::env::remove_var("SWITCHEROO_HAP_MTU");
        assert_eq!(mtu(), DEFAULT_MTU);
    }
}
