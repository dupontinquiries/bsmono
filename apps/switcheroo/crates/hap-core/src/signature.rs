//! signature — what a characteristic IS, read from the accessory (spec §7.3.4.13), and a
//! capability probe over the ones switcheroo cares about.
//!
//! Pairing and `On` were enough to switch a plug. They are not enough to draw a row.
//!
//! * A **WSP100 is an outlet, not a switch**: it exposes `Outlet In Use` (0x26) as well as
//!   `On`, and the difference — "the socket is switched on" versus "something is plugged
//!   in and drawing" — is the whole reason HomeKit models outlets separately. `On` alone
//!   cannot say a lamp is unplugged.
//! * An accessory has a **Name** the owner set in Home. A row reading `C4:3C:B0:…` when the
//!   device knows it is "desk lamp" is a row the owner has to decode.
//! * Three identical plugs in a row are three identical rows. **Identify** (0x14) is the
//!   HAP answer: write 1 and the accessory blinks or beeps. It exists precisely so a
//!   controller can ask "which one are you".
//! * And the controller must not offer what is not there. Writing `Brightness` to an
//!   outlet is a protocol error the user sees as a failure they caused.
//!
//! ## The signature is the accessory's own answer
//!
//! `Characteristic-Signature-Read` returns a TLV describing one characteristic: its type,
//! its service, its **properties** (readable, writable, notifies, and whether a write must
//! be timed), its **format**, and optionally a valid range and step. This parses that,
//! which is what turns "the GATT characteristic exists" into "and this is what you may do
//! with it".
//!
//! Properties are the load-bearing part. Writing to a read-only characteristic is a
//! guaranteed error, and `Outlet In Use` is read-only on every accessory that has it — so
//! a UI that draws it as a toggle is offering a control that cannot work.
//!
//! Pure: the parsing and the capability summary are data in, data out. `Controller::probe`
//! is the one function that talks to an accessory.

use crate::ble::{characteristic, opcode, Controller, HapError};
use crate::tlv8::Tlv;

/// Signature-read TLV parameter types (spec table 7-11).
pub mod sig {
    pub const CHARACTERISTIC_TYPE: u8 = 0x04;
    pub const SERVICE_INSTANCE_ID: u8 = 0x06;
    pub const SERVICE_TYPE: u8 = 0x07;
    pub const PROPERTIES: u8 = 0x0A;
    pub const USER_DESCRIPTION: u8 = 0x0B;
    pub const FORMAT: u8 = 0x0C;
    pub const VALID_RANGE: u8 = 0x0D;
    pub const STEP_VALUE: u8 = 0x0E;
}

/// The property bits (spec table 7-12).
pub mod prop {
    pub const READ: u16 = 0x0001;
    pub const WRITE: u16 = 0x0002;
    pub const ADDITIONAL_AUTHORIZATION: u16 = 0x0004;
    pub const TIMED_WRITE: u16 = 0x0008;
    pub const SECURE_READ: u16 = 0x0010;
    pub const SECURE_WRITE: u16 = 0x0020;
    pub const HIDDEN: u16 = 0x0040;
    pub const NOTIFY_CONNECTED: u16 = 0x0080;
    pub const NOTIFY_DISCONNECTED: u16 = 0x0100;
    pub const NOTIFY_BROADCAST: u16 = 0x0200;
}

/// HAP value formats (spec table 7-13), the subset an outlet or a light uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Bool,
    Uint8,
    Uint16,
    Uint32,
    Int32,
    Float,
    String,
    Data,
    /// A format this build does not name. Kept rather than rejected: an unknown format is
    /// a characteristic we should still list, just not render a typed control for.
    Other(u8),
}

impl Format {
    pub fn from_byte(b: u8) -> Format {
        match b {
            0x01 => Format::Bool,
            0x04 => Format::Uint8,
            0x06 => Format::Uint16,
            0x08 => Format::Uint32,
            0x10 => Format::Int32,
            0x14 => Format::Float,
            0x19 => Format::String,
            0x1B => Format::Data,
            other => Format::Other(other),
        }
    }
}

/// One characteristic, as the accessory describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    /// The HAP type, short form (0x25 for On).
    pub char_type: u16,
    pub service_type: Option<u16>,
    pub properties: u16,
    pub format: Format,
    /// The owner's name for it, when the accessory supplies one.
    pub description: Option<String>,
}

impl Signature {
    pub fn readable(&self) -> bool {
        self.properties & (prop::READ | prop::SECURE_READ) != 0
    }
    pub fn writable(&self) -> bool {
        self.properties & (prop::WRITE | prop::SECURE_WRITE) != 0
    }
    pub fn notifies(&self) -> bool {
        self.properties & (prop::NOTIFY_CONNECTED | prop::NOTIFY_DISCONNECTED | prop::NOTIFY_BROADCAST) != 0
    }
    /// A write to this must go through Timed Write (prepare, then execute) rather than a
    /// plain write. Locks and doors use it; getting it wrong is a write the accessory
    /// silently drops.
    pub fn timed_write(&self) -> bool {
        self.properties & prop::TIMED_WRITE != 0
    }

    /// Parse a signature-read reply body.
    pub fn parse(char_type: u16, body: &[u8]) -> Result<Signature, HapError> {
        let tlv = Tlv::decode(body)?;
        // The type comes back as a full 16-byte UUID or as its short form; take whichever
        // is there and fall back to the type we asked about, which is what a reply that
        // omits it means.
        let ty = tlv
            .get(sig::CHARACTERISTIC_TYPE)
            .and_then(short_uuid)
            .unwrap_or(char_type);
        let service_type = tlv.get(sig::SERVICE_TYPE).and_then(short_uuid);
        let properties = tlv
            .get(sig::PROPERTIES)
            .and_then(|v| match v {
                [lo, hi, ..] => Some(u16::from_le_bytes([*lo, *hi])),
                [lo] => Some(*lo as u16),
                _ => None,
            })
            .unwrap_or(0);
        // The format TLV is [format, unit(2), ...]; the first byte is what matters here.
        let format = tlv
            .get(sig::FORMAT)
            .and_then(|v| v.first().copied())
            .map(Format::from_byte)
            .unwrap_or(Format::Other(0));
        let description = tlv
            .get(sig::USER_DESCRIPTION)
            .map(|v| String::from_utf8_lossy(v).trim_end_matches('\0').to_string())
            .filter(|s| !s.is_empty());
        Ok(Signature { char_type: ty, service_type, properties, format, description })
    }
}

/// A 2-byte little-endian short type, or the short form embedded in a HAP base UUID
/// (`0000XXXX-0000-1000-8000-0026BB765291`, little-endian on the wire).
fn short_uuid(v: &[u8]) -> Option<u16> {
    match v.len() {
        2 => Some(u16::from_le_bytes([v[0], v[1]])),
        // 16 bytes, reversed: the short form sits at bytes 12..14 of the big-endian UUID,
        // which is 2..4 from the end of the little-endian wire form.
        16 => Some(u16::from_le_bytes([v[12], v[13]])),
        1 => Some(v[0] as u16),
        _ => None,
    }
}

/// What this accessory can do, as far as switcheroo cares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// Every characteristic that answered a signature read, in probe order.
    pub found: Vec<Signature>,
    /// The accessory's own name, when it has one.
    pub name: Option<String>,
}

impl Capabilities {
    pub fn get(&self, char_type: u16) -> Option<&Signature> {
        self.found.iter().find(|s| s.char_type == char_type)
    }
    pub fn has(&self, char_type: u16) -> bool {
        self.get(char_type).is_some()
    }
    /// Can this be switched? `On` present AND writable — a characteristic that is present
    /// and read-only is a status light, not a switch.
    pub fn switchable(&self) -> bool {
        self.get(characteristic::ON).is_some_and(|s| s.writable())
    }
    /// Is this an outlet rather than a plain switch — does it report whether something is
    /// plugged in and drawing?
    pub fn reports_load(&self) -> bool {
        self.get(characteristic::OUTLET_IN_USE).is_some_and(|s| s.readable())
    }
    /// Can we ask it to blink? The answer to "which of these three plugs is the lamp".
    pub fn identifiable(&self) -> bool {
        self.get(characteristic::IDENTIFY).is_some_and(|s| s.writable())
    }
    pub fn dimmable(&self) -> bool {
        self.get(characteristic::BRIGHTNESS).is_some_and(|s| s.writable())
    }
    /// One line for the row: what this thing is.
    pub fn describe(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.reports_load() {
            parts.push("outlet");
        } else if self.switchable() {
            parts.push("switch");
        }
        if self.dimmable() {
            parts.push("dimmable");
        }
        if self.identifiable() {
            parts.push("identify");
        }
        if parts.is_empty() {
            "no controls found".to_string()
        } else {
            parts.join(" · ")
        }
    }
}

/// The characteristics worth asking about, in the order the probe asks.
pub const PROBED: &[u16] = &[
    characteristic::ON,
    characteristic::OUTLET_IN_USE,
    characteristic::BRIGHTNESS,
    characteristic::IDENTIFY,
    characteristic::NAME,
];

impl Controller<'_> {
    /// Read one characteristic's signature. Needs a session, like every other read.
    pub fn signature(&mut self, char_type: u16) -> Result<Signature, HapError> {
        let body = self.exchange(char_type, opcode::CHARACTERISTIC_SIGNATURE_READ, Vec::new())?;
        Signature::parse(char_type, &body)
    }

    /// Ask the accessory what it has.
    ///
    /// A characteristic the accessory does not expose is NOT an error: the transport says
    /// it has no such GATT characteristic, or the accessory answers `invalid instance id`,
    /// and both mean "not present". Anything else IS an error and stops the probe.
    ///
    /// The distinction is the whole correctness of this function. An unverified controller
    /// gets `insufficient authentication` from every characteristic, and treating that as
    /// absence returns a cheerful "this accessory has no controls" for a plug that has
    /// them all — the caller then draws an empty row and nobody learns the session was the
    /// problem. Found by the test that probes before pair-verify. Hence the guard first
    /// and the narrow skip second.
    pub fn probe(&mut self) -> Result<Capabilities, HapError> {
        if !self.is_secure() {
            return Err(HapError::NotVerified);
        }
        let mut caps = Capabilities::default();
        for ty in PROBED {
            match self.signature(*ty) {
                Ok(s) => caps.found.push(s),
                // not present: the GATT characteristic is missing, or the accessory has
                // no such instance
                Err(HapError::Transport(_)) => continue,
                Err(HapError::Status { status, .. }) if status == crate::ble::status::INVALID_INSTANCE_ID => continue,
                Err(e) => return Err(e),
            }
        }
        // The name is a value, not a signature. Read it only if it is there and readable.
        if caps.get(characteristic::NAME).is_some_and(|s| s.readable()) {
            if let Ok(v) = self.read_value(characteristic::NAME) {
                let n = String::from_utf8_lossy(&v).trim_end_matches('\0').trim().to_string();
                if !n.is_empty() {
                    caps.name = Some(n);
                }
            }
        }
        Ok(caps)
    }

    /// Is something plugged in and drawing? `None` when the accessory does not say.
    pub fn outlet_in_use(&mut self) -> Result<bool, HapError> {
        let v = self.read_value(characteristic::OUTLET_IN_USE)?;
        match v.as_slice() {
            [b] => Ok(*b != 0),
            _ => Err(HapError::Protocol(format!("Outlet In Use is {} bytes, expected 1", v.len()))),
        }
    }

    /// Blink or beep, so a person can tell which accessory this row is.
    pub fn identify(&mut self) -> Result<(), HapError> {
        self.write_value(characteristic::IDENTIFY, &[1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig_body(props: u16, format: u8, desc: Option<&str>) -> Vec<u8> {
        let mut t = Tlv::new()
            .with(sig::CHARACTERISTIC_TYPE, 0x25u16.to_le_bytes())
            .with(sig::PROPERTIES, props.to_le_bytes())
            .with(sig::FORMAT, [format, 0x00, 0x00]);
        if let Some(d) = desc {
            t = t.with(sig::USER_DESCRIPTION, d.as_bytes());
        }
        t.encode()
    }

    #[test]
    fn a_signature_says_what_may_be_done_with_a_characteristic() {
        let s = Signature::parse(
            characteristic::ON,
            &sig_body(prop::SECURE_READ | prop::SECURE_WRITE | prop::NOTIFY_CONNECTED, 0x01, Some("Power")),
        )
        .unwrap();
        assert_eq!(s.char_type, characteristic::ON);
        assert_eq!(s.format, Format::Bool);
        assert!(s.readable() && s.writable() && s.notifies());
        assert!(!s.timed_write());
        assert_eq!(s.description.as_deref(), Some("Power"));
        // read-only is the case that matters: Outlet In Use is never writable
        let ro = Signature::parse(characteristic::OUTLET_IN_USE, &sig_body(prop::SECURE_READ, 0x01, None)).unwrap();
        assert!(ro.readable() && !ro.writable());
        assert!(ro.description.is_none());
    }

    #[test]
    fn a_type_arrives_as_a_short_form_or_a_full_uuid() {
        // 0000_0025-0000-1000-8000-0026BB765291, little-endian on the wire
        let mut le = [0u8; 16];
        le[12] = 0x25;
        assert_eq!(short_uuid(&le), Some(0x25));
        assert_eq!(short_uuid(&[0x25, 0x00]), Some(0x25));
        assert_eq!(short_uuid(&[0x25]), Some(0x25));
        assert_eq!(short_uuid(&[]), None);
        // a reply that omits the type means "the one you asked about"
        let s = Signature::parse(0x26, &Tlv::new().with(sig::PROPERTIES, [0x10, 0x00]).encode()).unwrap();
        assert_eq!(s.char_type, 0x26);
        assert_eq!(s.format, Format::Other(0), "no format TLV is not a guess at Bool");
    }

    #[test]
    fn capabilities_describe_an_outlet_a_switch_and_a_thing_with_nothing_on_it() {
        let ro = |t: u16| Signature { char_type: t, service_type: None, properties: prop::SECURE_READ, format: Format::Bool, description: None };
        let rw = |t: u16| Signature { char_type: t, service_type: None, properties: prop::SECURE_READ | prop::SECURE_WRITE, format: Format::Bool, description: None };

        let outlet = Capabilities {
            found: vec![rw(characteristic::ON), ro(characteristic::OUTLET_IN_USE), rw(characteristic::IDENTIFY)],
            name: Some("desk lamp".into()),
        };
        assert!(outlet.switchable() && outlet.reports_load() && outlet.identifiable());
        assert!(!outlet.dimmable());
        assert_eq!(outlet.describe(), "outlet · identify");

        let switch = Capabilities { found: vec![rw(characteristic::ON)], name: None };
        assert!(switch.switchable() && !switch.reports_load());
        assert_eq!(switch.describe(), "switch");

        // present but read-only is a status light, not a switch
        let readout = Capabilities { found: vec![ro(characteristic::ON)], name: None };
        assert!(!readout.switchable());
        assert_eq!(readout.describe(), "no controls found");
        assert_eq!(Capabilities::default().describe(), "no controls found");
    }
}
