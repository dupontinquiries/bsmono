//! hap_rs — HomeKit through the published `hap-rust` crates.
//!
//! `crate::hap` drives our own `hap-core` over btleplug. That was written
//! because nothing else existed in this tree, and it carries a caveat I
//! could never discharge: two SRP conventions (`H(g)` unpadded in the
//! client proof, `K = H(S)` as the HKDF input) that the in-process
//! simulator cannot falsify, because both halves were written from the
//! same reading of the spec.
//!
//! `hap-controller` and the crates under it (`hap-crypto`, `hap-ble`,
//! `hap-pairing`) do not have that problem. `hap-crypto` states it
//! cross-verifies every SRP-6a intermediate byte-for-byte against captured
//! `aiohomekit` traces and the HAP spec's own vectors, and pairs
//! interoperably against real accessories. That is the hardware
//! verification this app cannot do for itself.
//!
//! It also brings things `hap-core` does not have and would each be a
//! project: mDNS discovery for IP accessories, the accessory database,
//! setup-payload (`X-HM://`) parsing, event subscription, and Thread
//! provisioning — which matters because the WSP100 is a Thread accessory
//! that speaks BLE when no Thread network is present.
//!
//! # Two BLE stacks
//!
//! `hap-ble` uses `bluest` (BlueZ over D-Bus on Linux); switcheroo's
//! scanner uses `btleplug` (also BlueZ over D-Bus). BlueZ permits several
//! clients, but two of them scanning at once can starve each other's
//! discovery. The scanner should be paused around a pairing run. Not yet
//! enforced — named here because it is the first thing to suspect if
//! discovery goes quiet during a pair.
//!
//! Nothing here has run against hardware. It compiles, and the call
//! sequence was checked against the published API rather than guessed.

use std::time::Duration;

use hap_controller::{
    CharValue, CharacteristicType, Discovered, HapController, JsonFileStore, ServiceType,
};

/// Where the controller identity and every pairing are kept. One file, in
/// the same directory as the rest of switcheroo's state.
pub fn store_path() -> std::path::PathBuf {
    crate::hap::Store::open().dir().join("homekit-pairings.json")
}

async fn controller() -> hap_controller::Result<HapController> {
    HapController::new(JsonFileStore::new(store_path())).await
}

/// What a scan turned up, as (index, label). The index is what
/// [`pair_index`] takes.
///
/// Indices rather than addresses because `Discovered::Ble`'s fields are not
/// part of the API surface this module reads, and matching on a formatted
/// debug string would be precision this does not have. The UI shows the
/// labels and the person picks one, which is what the Home app does too.
pub async fn discover(secs: u64) -> Result<Vec<(usize, String)>, String> {
    let mut c = controller().await.map_err(|e| e.to_string())?;
    let found = c
        .discover(Duration::from_secs(secs))
        .await
        .map_err(|e| e.to_string())?;
    Ok(found
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let kind = match d {
                Discovered::Ble(_) => "BLE",
                _ => "IP",
            };
            (i, format!("{kind}  {d:?}"))
        })
        .collect())
}

/// Pair with the `index`-th result of a fresh scan, using the 8-digit
/// setup code. Returns the accessory's pairing id when the transport
/// reports one (BLE does; IP does not).
///
/// Rescans rather than holding the previous scan's results: a
/// `Discovered` borrows the controller, and a stale one is worse than a
/// second scan.
pub async fn pair_index(index: usize, secs: u64, setup_code: &str) -> Result<String, String> {
    let mut c = controller().await.map_err(|e| e.to_string())?;
    let found = c
        .discover(Duration::from_secs(secs))
        .await
        .map_err(|e| e.to_string())?;
    let target = found
        .get(index)
        .ok_or_else(|| format!("no accessory at index {index}; rescan and try again"))?;
    let handle = c.pair(target, setup_code).await.map_err(|e| e.to_string())?;
    Ok(handle.pairing_id().unwrap_or_default().to_string())
}

/// Connect to a stored pairing and switch its Outlet's On characteristic.
pub async fn set_power(pairing_id: &str, on: bool) -> Result<(), String> {
    let mut c = controller().await.map_err(|e| e.to_string())?;
    let mut h = c.connect(pairing_id).await.map_err(|e| e.to_string())?;
    let (aid, iid) = on_characteristic(&mut h).await?;
    h.write(aid, iid, CharValue::Bool(on))
        .await
        .map_err(|e| e.to_string())
}

/// Read the Outlet's On characteristic from a stored pairing.
pub async fn read_power(pairing_id: &str) -> Result<bool, String> {
    let mut c = controller().await.map_err(|e| e.to_string())?;
    let mut h = c.connect(pairing_id).await.map_err(|e| e.to_string())?;
    let (aid, iid) = on_characteristic(&mut h).await?;
    match h.read(aid, iid).await.map_err(|e| e.to_string())? {
        CharValue::Bool(b) => Ok(b),
        other => Err(format!("On read back as {other:?}, not a bool")),
    }
}

/// The (aid, iid) of the outlet's On switch.
///
/// `accessories()` first because on IP the database is fetched lazily and
/// `find` reads the cache; on BLE it is already populated and the call is
/// free. Doing it unconditionally keeps the two transports on one path.
///
/// Outlet, then Switch: a plug is usually the former, but the Wemo family
/// and several others declare the latter, and a lamp module declares
/// Lightbulb. Trying in order beats making the caller know.
async fn on_characteristic(
    h: &mut hap_controller::AccessoryHandle,
) -> Result<(u64, u64), String> {
    h.accessories().await.map_err(|e| e.to_string())?;
    for svc in [ServiceType::Outlet, ServiceType::Switch, ServiceType::LightBulb] {
        if let Ok(ids) = h.find(svc, CharacteristicType::On) {
            return Ok(ids);
        }
    }
    Err("this accessory exposes no On characteristic on an outlet, switch or lightbulb".into())
}
