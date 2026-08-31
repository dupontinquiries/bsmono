// smart — talk to smartctl, survive USB bridges, and turn raw telemetry
// into verdicts a person can act on.
//
// smartctl (smartmontools ≥ 7.0) gives us `--json=c`: one stable JSON doc
// for both protocol families. We model only the fields the verdict engine
// uses; everything else stays available to frontends via `raw`.
//
// THE USB-NVMe BRIDGE LADDER. External NVMe enclosures hide the drive
// behind a bridge chip; plain probing yields nothing (or a lying SCSI
// shim). smartctl grew per-vendor tunnels: `-d sntjmicron`, `-d
// sntasmedia`, `-d sntrealtek` (and `-d sat` for plain SATA-behind-USB).
// We try, in order: auto → sat → the three snt* tunnels, and stop at the
// first probe that yields a usable health section. Each attempt is
// surfaced as an Event so the frontend shows exactly what was tried —
// when a new enclosure chip appears, the user sees where the ladder
// ended instead of a silent "no SMART".
//
// Exit-code note: smartctl's exit status is a BITMASK (bit 0 cmdline, bit
// 1 device open failed, bit 2 identify/SMART command failed, bit 3 disk
// failing, bits 4..7 various prefail/error-log conditions). A non-zero
// exit is often exactly the information we came for, so we parse stdout
// regardless and only treat "no JSON at all" as a failed attempt.

use crate::{Event, Sink, SysRunner};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    Ok,
    Advisory,
    Warning,
    Critical,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Finding {
    pub severity: Severity,
    /// Stable machine key ("nvme.percentage_used", "ata.attr.197", …).
    pub key: String,
    /// Human sentence with the evidence baked in.
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SmartReport {
    pub device: String,
    /// The -d type that finally worked (None = auto).
    pub device_type_used: Option<String>,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub firmware: Option<String>,
    /// "NVMe" | "ATA" | other, from smartctl's device.protocol.
    pub protocol: Option<String>,
    pub smart_passed: Option<bool>,
    pub temperature_c: Option<i64>,
    pub power_on_hours: Option<i64>,
    pub nvme: Option<NvmeHealth>,
    pub ata_attrs: Vec<AtaAttr>,
    pub findings: Vec<Finding>,
    /// Full smartctl JSON for frontends that want to drill down.
    pub raw: serde_json::Value,
}

impl SmartReport {
    pub fn worst(&self) -> Severity {
        self.findings.iter().map(|f| f.severity).max().unwrap_or(Severity::Ok)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NvmeHealth {
    pub critical_warning: u8,
    pub available_spare: Option<i64>,
    pub available_spare_threshold: Option<i64>,
    pub percentage_used: Option<i64>,
    pub media_errors: Option<i64>,
    pub unsafe_shutdowns: Option<i64>,
    pub data_units_written: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AtaAttr {
    pub id: u16,
    pub name: String,
    pub value: i64,
    pub thresh: i64,
    pub raw: i64,
    pub when_failed: String,
}

/// The ladder. Order matters: cheap/likely first. `None` = let smartctl
/// autodetect (covers native NVMe/SATA and many bridges on 7.3+).
pub const DEVICE_TYPE_LADDER: &[Option<&str>] =
    &[None, Some("sat"), Some("sntjmicron"), Some("sntasmedia"), Some("sntrealtek")];

/// Probe one device, walking the ladder until a usable report appears.
pub fn probe(sys: &dyn SysRunner, dev: &str, sink: Sink) -> Result<SmartReport, String> {
    let mut last_err = String::new();
    for dtype in DEVICE_TYPE_LADDER {
        let mut args: Vec<&str> = vec!["-a", "--json=c"];
        if let Some(t) = dtype {
            args.push("-d");
            args.push(t);
        }
        args.push(dev);
        sink(Event::Exec(format!("smartctl {}", args.join(" "))));
        let out = match sys.run("smartctl", &args) {
            Ok(o) => o,
            Err(e) => return Err(e), // smartctl not installed: no ladder can help
        };
        match serde_json::from_str::<serde_json::Value>(&out.stdout) {
            Ok(v) if usable(&v) => {
                let mut rep = extract(dev, dtype.map(str::to_string), v);
                rep.findings = judge(&rep);
                return Ok(rep);
            }
            Ok(v) => {
                last_err = v
                    .pointer("/smartctl/messages/0/string")
                    .and_then(|s| s.as_str())
                    .unwrap_or("no health data in reply")
                    .to_string();
                sink(Event::Note(format!(
                    "{dev}: -d {} gave no health data ({last_err}); trying next",
                    dtype.unwrap_or("auto")
                )));
            }
            Err(e) => {
                last_err = format!("unparseable smartctl output: {e}");
                sink(Event::Note(format!("{dev}: {last_err}")));
            }
        }
    }
    Err(format!(
        "{dev}: exhausted device-type ladder (auto/sat/sntjmicron/sntasmedia/sntrealtek): {last_err}"
    ))
}

/// Kick off a short self-test. Fire-and-forget: results land in the next
/// `probe` (self-test log section of the raw JSON).
pub fn start_short_self_test(sys: &dyn SysRunner, dev: &str, sink: Sink) -> Result<(), String> {
    sink(Event::Exec(format!("smartctl -t short {dev}")));
    let out = sys.run("smartctl", &["-t", "short", dev])?;
    if out.code & 0b11 != 0 {
        return Err(format!("smartctl -t short failed: {}", out.stderr));
    }
    Ok(())
}

/// A reply is usable if any health-bearing section is present.
fn usable(v: &serde_json::Value) -> bool {
    v.get("nvme_smart_health_information_log").is_some()
        || v.get("ata_smart_attributes").is_some()
        || v.pointer("/smart_status/passed").is_some()
}

fn i64_at(v: &serde_json::Value, ptr: &str) -> Option<i64> {
    v.pointer(ptr).and_then(|x| x.as_i64())
}

fn str_at(v: &serde_json::Value, ptr: &str) -> Option<String> {
    v.pointer(ptr).and_then(|x| x.as_str()).map(str::to_string)
}

fn extract(dev: &str, dtype: Option<String>, v: serde_json::Value) -> SmartReport {
    let nvme = v.get("nvme_smart_health_information_log").map(|n| NvmeHealth {
        critical_warning: n.get("critical_warning").and_then(|x| x.as_u64()).unwrap_or(0) as u8,
        available_spare: n.get("available_spare").and_then(|x| x.as_i64()),
        available_spare_threshold: n.get("available_spare_threshold").and_then(|x| x.as_i64()),
        percentage_used: n.get("percentage_used").and_then(|x| x.as_i64()),
        media_errors: n.get("media_errors").and_then(|x| x.as_i64()),
        unsafe_shutdowns: n.get("unsafe_shutdowns").and_then(|x| x.as_i64()),
        data_units_written: n.get("data_units_written").and_then(|x| x.as_i64()),
    });

    let mut ata_attrs = Vec::new();
    if let Some(table) = v.pointer("/ata_smart_attributes/table").and_then(|t| t.as_array()) {
        for a in table {
            ata_attrs.push(AtaAttr {
                id: a.get("id").and_then(|x| x.as_u64()).unwrap_or(0) as u16,
                name: a.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                value: a.get("value").and_then(|x| x.as_i64()).unwrap_or(0),
                thresh: a.get("thresh").and_then(|x| x.as_i64()).unwrap_or(0),
                raw: a.pointer("/raw/value").and_then(|x| x.as_i64()).unwrap_or(0),
                when_failed: a
                    .get("when_failed")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
            });
        }
    }

    SmartReport {
        device: dev.to_string(),
        device_type_used: dtype,
        model: str_at(&v, "/model_name"),
        serial: str_at(&v, "/serial_number"),
        firmware: str_at(&v, "/firmware_version"),
        protocol: str_at(&v, "/device/protocol"),
        smart_passed: v.pointer("/smart_status/passed").and_then(|x| x.as_bool()),
        temperature_c: i64_at(&v, "/temperature/current"),
        power_on_hours: i64_at(&v, "/power_on_time/hours"),
        nvme,
        ata_attrs,
        findings: Vec::new(),
        raw: v,
    }
}

/// NVMe critical_warning bit meanings (NVMe spec, Get Log Page 02h byte 0).
const NVME_CW_BITS: &[(u8, &str)] = &[
    (0x01, "available spare below threshold"),
    (0x02, "temperature above/below operating threshold"),
    (0x04, "NVM subsystem reliability degraded (media errors)"),
    (0x08, "media placed in read-only mode"),
    (0x10, "volatile memory backup device failed"),
];

/// ATA attributes whose raw value directly counts damage. 199 is split out
/// below because on a USB bridge it usually means cable/bridge, not media.
const ATA_DAMAGE_ATTRS: &[(u16, &str)] = &[
    (5, "reallocated sectors"),
    (187, "reported uncorrectable errors"),
    (197, "current pending sectors"),
    (198, "offline uncorrectable sectors"),
];

fn judge(rep: &SmartReport) -> Vec<Finding> {
    let mut f = Vec::new();
    let push = |f: &mut Vec<Finding>, severity, key: &str, message: String| {
        f.push(Finding { severity, key: key.to_string(), message })
    };

    match rep.smart_passed {
        Some(false) => push(
            &mut f,
            Severity::Critical,
            "smart.overall",
            "SMART overall-health self-assessment: FAILED — back up now".into(),
        ),
        Some(true) => push(
            &mut f,
            Severity::Ok,
            "smart.overall",
            "SMART overall-health self-assessment: passed".into(),
        ),
        None => {}
    }

    if let Some(n) = &rep.nvme {
        for (bit, meaning) in NVME_CW_BITS {
            if n.critical_warning & bit != 0 {
                push(
                    &mut f,
                    Severity::Critical,
                    &format!("nvme.critical_warning.{bit:#04x}"),
                    format!("NVMe critical warning: {meaning}"),
                );
            }
        }
        if let Some(p) = n.percentage_used {
            let sev = if p >= 100 {
                Severity::Critical
            } else if p >= 90 {
                Severity::Warning
            } else if p >= 70 {
                Severity::Advisory
            } else {
                Severity::Ok
            };
            push(
                &mut f,
                sev,
                "nvme.percentage_used",
                format!("endurance used: {p}% of rated write life"),
            );
        }
        if let (Some(s), Some(t)) = (n.available_spare, n.available_spare_threshold) {
            if s < t {
                push(
                    &mut f,
                    Severity::Critical,
                    "nvme.spare",
                    format!("available spare {s}% is below threshold {t}%"),
                );
            }
        }
        if let Some(m) = n.media_errors {
            if m > 0 {
                push(
                    &mut f,
                    Severity::Warning,
                    "nvme.media_errors",
                    format!("{m} media and data integrity error(s) logged"),
                );
            }
        }
    }

    for a in &rep.ata_attrs {
        if !a.when_failed.is_empty() {
            push(
                &mut f,
                Severity::Critical,
                &format!("ata.attr.{}.failed", a.id),
                format!("attribute {} ({}) marked FAILED ({})", a.id, a.name, a.when_failed),
            );
        }
        for (id, what) in ATA_DAMAGE_ATTRS {
            if a.id == *id && a.raw > 0 {
                let sev = if a.raw >= 50 { Severity::Critical } else { Severity::Warning };
                push(
                    &mut f,
                    sev,
                    &format!("ata.attr.{id}"),
                    format!("{}: raw count {}", what, a.raw),
                );
            }
        }
        if a.id == 199 && a.raw > 0 {
            push(
                &mut f,
                Severity::Advisory,
                "ata.attr.199",
                format!(
                    "UDMA CRC errors: {} — usually cable, port, or USB bridge, not the media",
                    a.raw
                ),
            );
        }
    }

    if let Some(t) = rep.temperature_c {
        let is_nvme = rep.nvme.is_some();
        let (warn, crit) = if is_nvme { (70, 80) } else { (55, 65) };
        if t >= crit {
            push(&mut f, Severity::Critical, "temp", format!("temperature {t}°C — critical"));
        } else if t >= warn {
            push(&mut f, Severity::Warning, "temp", format!("temperature {t}°C — high"));
        }
    }

    f
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{null_sink, FixtureRunner};

    const NVME_OK: &str = r#"{
      "device": {"name":"/dev/nvme0","type":"nvme","protocol":"NVMe"},
      "model_name":"WD_BLACK SN850X 1000GB","serial_number":"A1","firmware_version":"620311WD",
      "smart_status":{"passed":true},
      "temperature":{"current":42},
      "power_on_time":{"hours":1234},
      "nvme_smart_health_information_log":{
        "critical_warning":0,"temperature":42,"available_spare":100,
        "available_spare_threshold":10,"percentage_used":3,
        "data_units_written":11122233,"media_errors":0,"unsafe_shutdowns":17}
    }"#;

    // A worn, failing NVMe stick behind a JMicron bridge: auto and sat give
    // nothing, sntjmicron delivers.
    const BRIDGE_EMPTY: &str = r#"{"smartctl":{"messages":[{"string":"Unknown USB bridge","severity":"error"}]}}"#;
    const NVME_BAD: &str = r#"{
      "device": {"name":"/dev/sda","type":"sntjmicron","protocol":"NVMe"},
      "smart_status":{"passed":false},
      "temperature":{"current":83},
      "nvme_smart_health_information_log":{
        "critical_warning":5,"available_spare":4,"available_spare_threshold":10,
        "percentage_used":101,"media_errors":12,"unsafe_shutdowns":900}
    }"#;

    const ATA_WORN: &str = r#"{
      "device":{"name":"/dev/sdb","type":"sat","protocol":"ATA"},
      "model_name":"Samsung SSD 870 EVO","smart_status":{"passed":true},
      "temperature":{"current":38},
      "ata_smart_attributes":{"table":[
        {"id":5,"name":"Reallocated_Sector_Ct","value":95,"thresh":10,"when_failed":"","raw":{"value":8}},
        {"id":197,"name":"Current_Pending_Sector","value":100,"thresh":0,"when_failed":"","raw":{"value":0}},
        {"id":199,"name":"UDMA_CRC_Error_Count","value":100,"thresh":0,"when_failed":"","raw":{"value":3}}
      ]}
    }"#;

    #[test]
    fn nvme_healthy_report() {
        let sys = FixtureRunner::new().on("smartctl -a --json=c /dev/nvme0n1", 0, NVME_OK);
        let mut sink = null_sink();
        let rep = probe(&sys, "/dev/nvme0n1", &mut sink).unwrap();
        assert_eq!(rep.worst(), Severity::Ok);
        assert_eq!(rep.nvme.as_ref().unwrap().percentage_used, Some(3));
        assert_eq!(rep.device_type_used, None);
        assert_eq!(rep.protocol.as_deref(), Some("NVMe"));
    }

    #[test]
    fn usb_bridge_ladder_reaches_sntjmicron_and_flags_failure() {
        let sys = FixtureRunner::new()
            .on("smartctl -a --json=c /dev/sda", 2, BRIDGE_EMPTY)
            .on("smartctl -a --json=c -d sat /dev/sda", 2, BRIDGE_EMPTY)
            .on("smartctl -a --json=c -d sntjmicron /dev/sda", 8, NVME_BAD);
        let mut events = Vec::new();
        let mut sink = |e: Event| events.push(e);
        let rep = probe(&sys, "/dev/sda", &mut sink).unwrap();
        assert_eq!(rep.device_type_used.as_deref(), Some("sntjmicron"));
        assert_eq!(rep.worst(), Severity::Critical);
        // critical_warning=5 = bits 0x01|0x04 → two decoded findings.
        let cw: Vec<_> =
            rep.findings.iter().filter(|f| f.key.starts_with("nvme.critical_warning")).collect();
        assert_eq!(cw.len(), 2);
        // The ladder was visible: two Notes about failed rungs, three Execs.
        assert_eq!(events.iter().filter(|e| matches!(e, Event::Note(_))).count(), 2);
        assert_eq!(events.iter().filter(|e| matches!(e, Event::Exec(_))).count(), 3);
    }

    #[test]
    fn ata_attrs_judged() {
        let sys = FixtureRunner::new().on("smartctl -a --json=c /dev/sdb", 0, ATA_WORN);
        let mut sink = null_sink();
        let rep = probe(&sys, "/dev/sdb", &mut sink).unwrap();
        assert_eq!(rep.worst(), Severity::Warning); // 8 reallocated
        assert!(rep.findings.iter().any(|f| f.key == "ata.attr.5"));
        // CRC errors are advisory (cable-class), and pending=0 adds nothing.
        assert!(rep
            .findings
            .iter()
            .any(|f| f.key == "ata.attr.199" && f.severity == Severity::Advisory));
        assert!(!rep.findings.iter().any(|f| f.key == "ata.attr.197"));
    }

    #[test]
    fn exhausted_ladder_is_an_error_not_a_lie() {
        let sys = FixtureRunner::new()
            .on("smartctl -a --json=c /dev/sdz", 2, BRIDGE_EMPTY)
            .on("smartctl -a --json=c -d sat /dev/sdz", 2, BRIDGE_EMPTY)
            .on("smartctl -a --json=c -d sntjmicron /dev/sdz", 2, BRIDGE_EMPTY)
            .on("smartctl -a --json=c -d sntasmedia /dev/sdz", 2, BRIDGE_EMPTY)
            .on("smartctl -a --json=c -d sntrealtek /dev/sdz", 2, BRIDGE_EMPTY);
        let mut sink = null_sink();
        let err = probe(&sys, "/dev/sdz", &mut sink).unwrap_err();
        assert!(err.contains("exhausted"), "{err}");
    }
}
