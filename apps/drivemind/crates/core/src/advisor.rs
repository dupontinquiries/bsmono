// advisor — where the subsystems' facts become one prioritized list.
//
// Each module reports in isolation; the advisor is the only place allowed
// to REASON ACROSS them, because the valuable calls are cross-signal:
//
//   * SMART says the drive is dying           → "clone it, here's the plan"
//   * USB bridge + CRC advisories + integrity  → "suspect the cable/enclosure
//     read errors on the same device             before you suspect the data"
//   * cleanup found N GiB of video + av1tuner  → one batch handoff, not N
//     candidates                                  individual nags
//   * repo scan found delta-hostile files      → the `gin migrate` command
//
// Output is a flat Vec<Recommendation> sorted by priority. Every entry has
// an actionable `command` where one exists. The advisor never executes.

use crate::cleanup::{Suggestion, SuggestionKind};
use crate::deltahostile::MigrationPlan;
use crate::device::{Attach, BlockDevice};
use crate::smart::{Severity, SmartReport};
use crate::video::VideoScan;
use crate::{human_bytes, integrity::VerifyReport};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Priority {
    Info,
    Soon,
    Now,
    Emergency,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Recommendation {
    pub priority: Priority,
    pub title: String,
    pub detail: String,
    pub command: Option<String>,
}

/// Everything the advisor may consider. All optional — advise with what
/// you have; the CLI's `dvm advise` fills what it collected this run.
#[derive(Default)]
pub struct Inputs<'a> {
    pub devices: &'a [BlockDevice],
    pub smart: &'a [SmartReport],
    pub verify: Option<&'a VerifyReport>,
    pub cleanup: &'a [Suggestion],
    pub migration: Option<&'a MigrationPlan>,
    pub videos: Option<&'a VideoScan>,
    /// Path the video batch file was (or would be) written to.
    pub video_batch_path: Option<&'a str>,
}

pub fn advise(inp: &Inputs) -> Vec<Recommendation> {
    let mut recs = Vec::new();

    // --- per-device SMART verdicts, with clone escalation ---
    for rep in inp.smart {
        match rep.worst() {
            Severity::Critical => {
                let crits: Vec<&str> = rep
                    .findings
                    .iter()
                    .filter(|f| f.severity == Severity::Critical)
                    .map(|f| f.message.as_str())
                    .collect();
                recs.push(Recommendation {
                    priority: Priority::Emergency,
                    title: format!("{}: drive health CRITICAL", rep.device),
                    detail: crits.join("; "),
                    command: Some(format!(
                        "dvm clone {} <replacement-device> --run   # ddrescue path, plan shown first",
                        rep.device
                    )),
                });
            }
            Severity::Warning => recs.push(Recommendation {
                priority: Priority::Now,
                title: format!("{}: drive health degraded", rep.device),
                detail: rep
                    .findings
                    .iter()
                    .filter(|f| f.severity >= Severity::Warning)
                    .map(|f| f.message.clone())
                    .collect::<Vec<_>>()
                    .join("; "),
                command: Some(format!("dvm smart {}", rep.device)),
            }),
            _ => {}
        }
    }

    // --- the bridge-vs-media disambiguation ---
    // CRC/interface errors on a USB-attached device + read errors during
    // verification on that mount = the classic dying-enclosure signature.
    // Naming it saves people from reformatting a healthy drive.
    for rep in inp.smart {
        let crc = rep.findings.iter().any(|f| f.key == "ata.attr.199");
        let on_usb = inp
            .devices
            .iter()
            .any(|d| d.path == rep.device && d.attach == Attach::UsbBridge);
        let verify_errs =
            inp.verify.map(|v| !v.read_errors.is_empty()).unwrap_or(false);
        if crc && on_usb {
            let (priority, extra) = if verify_errs {
                (Priority::Now, " — and this run's verify hit read errors, consistent with a failing bridge/cable, not failing media")
            } else {
                (Priority::Soon, "")
            };
            recs.push(Recommendation {
                priority,
                title: format!("{}: interface errors on a USB bridge", rep.device),
                detail: format!(
                    "CRC errors usually mean cable, port, or enclosure{extra}. Swap the cable first, re-verify, and only then judge the drive."
                ),
                command: Some(format!("dvm smart {}", rep.device)),
            });
        }
    }

    // --- integrity outcome ---
    if let Some(v) = inp.verify {
        if !v.read_errors.is_empty() {
            recs.push(Recommendation {
                priority: Priority::Emergency,
                title: format!("{} file(s) UNREADABLE during verify", v.read_errors.len()),
                detail: "Unreadable files mean the storage path is failing right now. Copy everything readable off this medium before anything else."
                    .into(),
                command: None,
            });
        }
        if !v.changed.is_empty() {
            recs.push(Recommendation {
                priority: Priority::Now,
                title: format!("{} file(s) changed since manifest", v.changed.len()),
                detail: "Content hash mismatches. Expected if you edited them; silent corruption if you didn't."
                    .into(),
                command: None,
            });
        }
    }

    // --- cleanup rollup (one entry, not a nag per directory) ---
    let reclaim: u64 = inp
        .cleanup
        .iter()
        .filter(|s| matches!(s.kind, SuggestionKind::RebuildableDir | SuggestionKind::DuplicateGroup))
        .map(|s| s.reclaim_bytes)
        .sum();
    if reclaim > 0 {
        recs.push(Recommendation {
            priority: Priority::Soon,
            title: format!("{} reclaimable without losing data", human_bytes(reclaim)),
            detail: format!(
                "{} suggestion(s): rebuildable caches and exact duplicates. `dvm cleanup` prints each with its command.",
                inp.cleanup.len()
            ),
            command: Some("dvm cleanup <path>".into()),
        });
    }

    // --- video → av1tuner handoff ---
    if let Some(v) = inp.videos {
        if !v.candidates.is_empty() {
            recs.push(Recommendation {
                priority: Priority::Soon,
                title: format!(
                    "{} video(s) worth re-encoding — est. {} back",
                    v.candidates.len(),
                    human_bytes(v.est_savings_bytes())
                ),
                detail: "Legacy codecs / high-bitrate h264 found. av1tuner owns the encode pipeline (scene-split, parallel, quality-targeted)."
                    .into(),
                command: inp
                    .video_batch_path
                    .map(|p| format!("av1tuner --batch '{p}'   # or: Add → From list in the app")),
            });
        }
    }

    // --- git → gitmind migration ---
    if let Some(m) = inp.migration {
        if !m.candidates.is_empty() {
            let pct = if m.tracked_bytes > 0 {
                (m.total_bytes as f64 / m.tracked_bytes as f64 * 100.0).round()
            } else {
                0.0
            };
            recs.push(Recommendation {
                priority: Priority::Soon,
                title: format!(
                    "{} delta-hostile file(s) in {} ({}% of tracked bytes)",
                    m.candidates.len(),
                    m.repo,
                    pct
                ),
                detail: format!(
                    "{} that git stores near-whole per version. gitmind's chunk store dedups these before compression.",
                    human_bytes(m.total_bytes)
                ),
                command: Some(m.gin_command()),
            });
        }
    }

    recs.sort_by(|a, b| b.priority.cmp(&a.priority));
    recs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smart::{Finding, SmartReport};

    fn smart_with(dev: &str, findings: Vec<Finding>) -> SmartReport {
        SmartReport {
            device: dev.into(),
            device_type_used: None,
            model: None,
            serial: None,
            firmware: None,
            protocol: None,
            smart_passed: None,
            temperature_c: None,
            power_on_hours: None,
            nvme: None,
            ata_attrs: vec![],
            findings,
            raw: serde_json::Value::Null,
        }
    }

    #[test]
    fn critical_smart_outranks_everything_and_carries_clone_command() {
        let smart = vec![smart_with(
            "/dev/sda",
            vec![Finding {
                severity: Severity::Critical,
                key: "smart.overall".into(),
                message: "FAILED".into(),
            }],
        )];
        let recs = advise(&Inputs { smart: &smart, ..Default::default() });
        assert_eq!(recs[0].priority, Priority::Emergency);
        assert!(recs[0].command.as_ref().unwrap().contains("dvm clone /dev/sda"));
    }

    #[test]
    fn usb_crc_plus_verify_read_errors_names_the_bridge() {
        use crate::device::{Attach, BlockDevice};
        let devices = vec![BlockDevice {
            name: "sda".into(),
            path: "/dev/sda".into(),
            size_bytes: 1,
            model: None,
            serial: None,
            vendor: None,
            transport: Some("usb".into()),
            rotational: false,
            removable: true,
            attach: Attach::UsbBridge,
            mountpoints: vec![],
        }];
        let smart = vec![smart_with(
            "/dev/sda",
            vec![Finding {
                severity: Severity::Advisory,
                key: "ata.attr.199".into(),
                message: "UDMA CRC errors: 9".into(),
            }],
        )];
        let mut verify = VerifyReport::default();
        verify.read_errors.push(("x".into(), "Input/output error".into()));
        let recs = advise(&Inputs {
            devices: &devices,
            smart: &smart,
            verify: Some(&verify),
            ..Default::default()
        });
        let bridge = recs.iter().find(|r| r.title.contains("USB bridge")).unwrap();
        assert_eq!(bridge.priority, Priority::Now);
        assert!(bridge.detail.contains("failing bridge/cable"));
        // The unreadable-files emergency is also present and sorted first.
        assert_eq!(recs[0].priority, Priority::Emergency);
    }
}
