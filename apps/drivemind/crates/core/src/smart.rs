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

    #[test]
    fn a_hot_nvme_drive_produces_a_critical_temperature_finding() {
        // Real gap: NVME_BAD (used by usb_bridge_ladder_reaches_sntjmicron_
        // and_flags_failure) already carries temperature 83C, which is
        // ABOVE the NVMe critical threshold of 80 -- but that test only
        // ever checked overall worst() and the critical_warning bit
        // count, never the temperature finding specifically. This is the
        // exact class of gap that lets a real bug hide behind an
        // already-passing test: the finding could silently stop being
        // produced (a regression in the temp branch) and worst() would
        // still read Critical because of the OTHER findings in that same
        // fixture, masking it completely. Ladder wired in full, same
        // shape as usb_bridge_ladder_reaches_sntjmicron_and_flags_failure,
        // since NVME_BAD only resolves at the sntjmicron rung.
        let sys = FixtureRunner::new()
            .on("smartctl -a --json=c /dev/sda", 2, BRIDGE_EMPTY)
            .on("smartctl -a --json=c -d sat /dev/sda", 2, BRIDGE_EMPTY)
            .on("smartctl -a --json=c -d sntjmicron /dev/sda", 8, NVME_BAD);
        let mut sink = null_sink();
        let rep = probe(&sys, "/dev/sda", &mut sink).unwrap();
        let temp = rep.findings.iter().find(|f| f.key == "temp");
        assert!(temp.is_some(), "83C on an NVMe device (critical >= 80) must produce a temp finding: {:?}", rep.findings);
        assert_eq!(temp.unwrap().severity, Severity::Critical);
    }

    #[test]
    fn temperature_severity_uses_different_thresholds_for_nvme_vs_ata() {
        // The warn/crit pair is (70, 80) for NVMe and (55, 65) for ATA --
        // untested that these are genuinely DIFFERENT, not the same
        // numbers duplicated. 60C is below NVMe's warn threshold (70)
        // but above ATA's WARN threshold (55): the same temperature
        // must produce no finding at all on one protocol and a Warning on
        // the other.
        let nvme_60 = r#"{"device":{"name":"/dev/nvme0","type":"nvme","protocol":"NVMe"},
            "smart_status":{"passed":true},"temperature":{"current":60},
            "nvme_smart_health_information_log":{"critical_warning":0,"percentage_used":0}}"#;
        let ata_60 = r#"{"device":{"name":"/dev/sdb","type":"sat","protocol":"ATA"},
            "smart_status":{"passed":true},"temperature":{"current":60}}"#;
        let sys_n = FixtureRunner::new().on("smartctl -a --json=c /dev/nvme0n1", 0, nvme_60);
        let sys_a = FixtureRunner::new().on("smartctl -a --json=c /dev/sdb", 0, ata_60);
        let mut sink = null_sink();
        let rep_n = probe(&sys_n, "/dev/nvme0n1", &mut sink).unwrap();
        let rep_a = probe(&sys_a, "/dev/sdb", &mut sink).unwrap();
        assert!(!rep_n.findings.iter().any(|f| f.key == "temp"), "60C on NVMe is below its own warn threshold of 70");
        let ata_temp = rep_a.findings.iter().find(|f| f.key == "temp");
        // Warning, not Critical. The first version of this asserted Critical
        // with the message "60C on ATA is above ITS critical threshold of 65"
        // -- which contradicts itself, since 60 is below 65. The code was
        // right; the test was wrong, and it was written in a session that
        // never compiled it. ATA is (warn 55, crit 65), so 60C lands in the
        // warn band. The POINT of the test survives: the same temperature is
        // no finding at all on NVMe and a real one on ATA.
        assert_eq!(ata_temp.map(|f| f.severity), Some(Severity::Warning),
            "60C on ATA is above its warn threshold of 55 but below its critical 65");

        // ...and the critical side of the ATA split, which nothing covered:
        // the bug that matters is a threshold pair applied to the wrong bus,
        // and only checking the warn band would miss a crit that never fires.
        let ata_70 = r#"{"device":{"name":"/dev/sdb","type":"sat","protocol":"ATA"},
            "smart_status":{"passed":true},"temperature":{"current":70}}"#;
        let sys_hot = FixtureRunner::new().on("smartctl -a --json=c /dev/sdb", 0, ata_70);
        let rep_hot = probe(&sys_hot, "/dev/sdb", &mut sink).unwrap();
        let hot = rep_hot.findings.iter().find(|f| f.key == "temp");
        assert_eq!(hot.map(|f| f.severity), Some(Severity::Critical),
            "70C on ATA is at or above its critical threshold of 65");
    }

    #[test]
    fn a_severe_ata_damage_attribute_is_critical_not_just_warning() {
        // ATA_DAMAGE_ATTRS' `if a.raw >= 50 { Critical } else { Warning }`
        // split had only ever been tested on the Warning side (attr 5,
        // raw=8, in the existing ata_attrs_judged test). The Critical
        // side -- the one that should actually change which clone tool
        // gets selected -- had zero coverage.
        let severe = r#"{"device":{"name":"/dev/sdb","type":"sat","protocol":"ATA"},
            "smart_status":{"passed":true},"temperature":{"current":38},
            "ata_smart_attributes":{"table":[
                {"id":5,"name":"Reallocated_Sector_Ct","value":50,"thresh":10,"when_failed":"","raw":{"value":75}}
            ]}}"#;
        let sys = FixtureRunner::new().on("smartctl -a --json=c /dev/sdb", 0, severe);
        let mut sink = null_sink();
        let rep = probe(&sys, "/dev/sdb", &mut sink).unwrap();
        let f = rep.findings.iter().find(|f| f.key == "ata.attr.5").expect("finding must exist");
        assert_eq!(f.severity, Severity::Critical, "raw=75 is >= the 50 threshold for Critical");
        assert_eq!(rep.worst(), Severity::Critical);
    }

    #[test]
    fn available_spare_below_threshold_is_its_own_critical_finding() {
        // NVME_BAD already has available_spare=4 < threshold=10, and the
        // existing ladder test's assertions never isolated this specific
        // finding -- only the aggregate worst() and the unrelated
        // critical_warning bit count. A regression removing the spare
        // check entirely would not have been caught, since percentage_
        // used=101 alone already makes worst() Critical in that fixture.
        let low_spare = r#"{"device":{"name":"/dev/nvme0","type":"nvme","protocol":"NVMe"},
            "smart_status":{"passed":true},"temperature":{"current":40},
            "nvme_smart_health_information_log":{"critical_warning":0,
                "available_spare":4,"available_spare_threshold":10,"percentage_used":5}}"#;
        let sys = FixtureRunner::new().on("smartctl -a --json=c /dev/nvme0n1", 0, low_spare);
        let mut sink = null_sink();
        let rep = probe(&sys, "/dev/nvme0n1", &mut sink).unwrap();
        assert!(rep.findings.iter().any(|f| f.key == "nvme.spare" && f.severity == Severity::Critical),
            "available_spare (4) below its own threshold (10) must be Critical on its own, isolated from percentage_used: {:?}", rep.findings);
    }

    #[test]
    fn percentage_used_severity_tiers_are_distinct_at_each_boundary() {
        // Only 3% (Ok) and 101% (Critical, via >100) had ever been used.
        // The three OTHER tiers (70/90/100 boundaries, and the Advisory/
        // Warning bands between them) had zero coverage -- a mis-ordered
        // or off-by-one comparison chain (`>=100` before `>=90` matters,
        // since they're checked in that order) would not be caught by
        // only testing the extremes.
        let at = |pct: i64| -> Severity {
            let fix = format!(r#"{{"device":{{"name":"/dev/nvme0","type":"nvme","protocol":"NVMe"}},
                "smart_status":{{"passed":true}},"temperature":{{"current":40}},
                "nvme_smart_health_information_log":{{"critical_warning":0,"percentage_used":{pct}}}}}"#);
            let sys = FixtureRunner::new().on("smartctl -a --json=c /dev/nvme0n1", 0, &fix);
            let mut sink = null_sink();
            let rep = probe(&sys, "/dev/nvme0n1", &mut sink).unwrap();
            rep.findings.iter().find(|f| f.key == "nvme.percentage_used").unwrap().severity
        };
        assert_eq!(at(69), Severity::Ok, "just under the advisory line");
        assert_eq!(at(70), Severity::Advisory, "exactly at the advisory line");
        assert_eq!(at(89), Severity::Advisory, "just under the warning line");
        assert_eq!(at(90), Severity::Warning, "exactly at the warning line");
        assert_eq!(at(99), Severity::Warning, "just under the critical line");
        assert_eq!(at(100), Severity::Critical, "exactly at the critical line");
    }

    #[test]
    fn no_findings_at_all_reports_ok_not_a_default_that_looks_like_success() {
        // worst()'s `.unwrap_or(Severity::Ok)` on a truly empty findings
        // list -- untested with a report that has NOTHING to judge (no
        // smart_status, no nvme section, no ata attributes, no
        // temperature). Distinguishing this from smart_passed: Some(true)
        // (which DOES add an explicit Ok finding) matters: this is the
        // "we could not assess anything" case, not "we assessed and it's
        // fine" -- both currently report Severity::Ok from worst(), which
        // is worth pinning explicitly as a known shape rather than
        // assuming a frontend can tell them apart some other way.
        // The fixture must satisfy usable()'s check (presence of the
        // /smart_status/passed pointer) or probe() rejects it and walks
        // the rest of the ladder looking for a usable rung -- but the
        // VALUE at that pointer being null rather than a bool still
        // makes smart_passed itself None (as_bool() on null is None),
        // which is the actual thing this test needs: a report usable()
        // accepts but judge() has nothing to say about.
        let bare = r#"{"device":{"name":"/dev/sdc","type":"scsi"},"smart_status":{"passed":null}}"#;
        let sys = FixtureRunner::new().on("smartctl -a --json=c /dev/sdc", 0, bare);
        let mut sink = null_sink();
        let rep = probe(&sys, "/dev/sdc", &mut sink).unwrap();
        assert!(rep.findings.is_empty());
        assert_eq!(rep.worst(), Severity::Ok);
    }
}
