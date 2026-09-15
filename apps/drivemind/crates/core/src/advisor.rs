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

    #[test]
    fn all_four_priority_levels_sort_correctly_together() {
        // Both existing tests only ever confirmed "the Emergency one is
        // first" -- never that Now, Soon, and Info also land in the
        // right relative order with each other. Built one Inputs that
        // produces all four levels at once (critical SMART = Emergency,
        // a changed file = Now, reclaimable cleanup space = Soon) plus
        // confirms nothing has snuck in front of Emergency or behind
        // Soon. A reversed sort direction, or Priority's derived Ord
        // reordered by a careless enum edit, would only be caught by
        // checking the FULL order, not just one end of it.
        let smart = vec![smart_with("/dev/sda", vec![Finding {
            severity: Severity::Critical, key: "smart.overall".into(), message: "FAILED".into(),
        }])];
        let mut verify = VerifyReport::default();
        verify.changed.push(("a.txt".into(), crate::integrity::FileStatus::Changed {
            expected: "aaa".into(), actual: "bbb".into() }));
        let cleanup = vec![Suggestion {
            kind: SuggestionKind::RebuildableDir, path: "/x/target".into(),
            paths: vec!["/x/target".into()], reclaim_bytes: 1000,
            note: "n".into(), command: Some("rm -rf /x/target".into()),
        }];
        let recs = advise(&Inputs { smart: &smart, verify: Some(&verify), cleanup: &cleanup, ..Default::default() });
        let prios: Vec<Priority> = recs.iter().map(|r| r.priority).collect();
        let mut sorted = prios.clone();
        sorted.sort_by(|a, b| b.cmp(a));
        assert_eq!(prios, sorted, "recommendations must already be in descending-priority order: {prios:?}");
        assert_eq!(prios[0], Priority::Emergency);
        assert!(prios.contains(&Priority::Now), "the changed-file recommendation must be present: {prios:?}");
        assert!(prios.contains(&Priority::Soon), "the cleanup rollup must be present: {prios:?}");
    }

    #[test]
    fn crc_errors_on_a_non_usb_device_do_not_trigger_the_bridge_warning() {
        // The disambiguation exists specifically to blame the cable/
        // enclosure INSTEAD of the media on a USB bridge -- on a native
        // SATA/NVMe device there is no bridge to blame, and CRC-like
        // findings there mean something else entirely. Neither existing
        // test ever checked the `on_usb` guard in isolation; both of
        // them always used a UsbBridge device.
        let smart = vec![smart_with("/dev/sda", vec![Finding {
            severity: Severity::Advisory, key: "ata.attr.199".into(), message: "UDMA CRC errors: 9".into(),
        }])];
        let devices = vec![BlockDevice {
            name: "sda".into(), path: "/dev/sda".into(), size_bytes: 1, model: None, serial: None,
            vendor: None, transport: Some("sata".into()), rotational: false, removable: false,
            attach: Attach::NativeSata, mountpoints: vec![],
        }];
        let recs = advise(&Inputs { devices: &devices, smart: &smart, ..Default::default() });
        assert!(!recs.iter().any(|r| r.title.contains("USB bridge")),
            "a native SATA device must never get the USB-bridge disambiguation: {:?}", recs.iter().map(|r| &r.title).collect::<Vec<_>>());
    }

    #[test]
    fn usb_crc_without_verify_errors_is_only_soon_priority_not_now() {
        // The existing bridge test only ever exercised the "WITH verify
        // read errors" branch (Now priority, extra sentence about a
        // failing bridge). The other half of that if/else -- CRC errors
        // present but nothing wrong turned up in verification -- must
        // stay at the lower Soon priority with the plain message, not
        // escalate speculatively.
        let devices = vec![BlockDevice {
            name: "sda".into(), path: "/dev/sda".into(), size_bytes: 1, model: None, serial: None,
            vendor: None, transport: Some("usb".into()), rotational: false, removable: true,
            attach: Attach::UsbBridge, mountpoints: vec![],
        }];
        let smart = vec![smart_with("/dev/sda", vec![Finding {
            severity: Severity::Advisory, key: "ata.attr.199".into(), message: "UDMA CRC errors: 9".into(),
        }])];
        let recs = advise(&Inputs { devices: &devices, smart: &smart, ..Default::default() });
        let bridge = recs.iter().find(|r| r.title.contains("USB bridge")).unwrap();
        assert_eq!(bridge.priority, Priority::Soon);
        assert!(!bridge.detail.contains("failing bridge/cable"), "{}", bridge.detail);
    }

    #[test]
    fn cleanup_reclaim_rolls_up_into_one_recommendation_not_one_per_suggestion() {
        let cleanup = vec![
            Suggestion { kind: SuggestionKind::RebuildableDir, path: "/a".into(), paths: vec!["/a".into()],
                reclaim_bytes: 1_000_000, note: "n".into(), command: Some("rm -rf /a".into()) },
            Suggestion { kind: SuggestionKind::DuplicateGroup, path: "/b1".into(), paths: vec!["/b1".into(), "/b2".into()],
                reclaim_bytes: 2_000_000, note: "n".into(), command: None },
            // A BigFile suggestion must NOT count toward reclaimable
            // bytes here -- it isn't reclaimable without a human
            // decision to actually delete it, unlike a cache dir or a
            // confirmed exact duplicate.
            Suggestion { kind: SuggestionKind::BigFile, path: "/huge".into(), paths: vec!["/huge".into()],
                reclaim_bytes: 9_999_999, note: "n".into(), command: None },
        ];
        let recs = advise(&Inputs { cleanup: &cleanup, ..Default::default() });
        assert_eq!(recs.len(), 1, "{recs:?}");
        assert_eq!(recs[0].priority, Priority::Soon);
        assert!(recs[0].title.contains("2.9 MiB"),
            "must reflect only the RebuildableDir + DuplicateGroup bytes (3,000,000 = 2.9 MiB per human_bytes' binary units), not the BigFile's: {}", recs[0].title);
    }

    #[test]
    fn video_candidates_produce_one_handoff_recommendation_with_the_batch_command() {
        let scan = VideoScan {
            candidates: vec![crate::video::VideoCandidate {
                path: "/x.avi".into(), size: 1_000_000, codec: Some("mpeg4".into()),
                bit_rate: None, why: "legacy codec mpeg4".into(), est_savings_ratio: 0.6,
            }],
            skipped_modern: 0, total_candidate_bytes: 1_000_000,
        };
        let recs = advise(&Inputs { videos: Some(&scan), video_batch_path: Some("/tmp/batch.txt"), ..Default::default() });
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].priority, Priority::Soon);
        assert!(recs[0].command.as_ref().unwrap().contains("/tmp/batch.txt"), "{:?}", recs[0].command);
    }

    #[test]
    fn an_empty_video_scan_produces_no_recommendation_at_all() {
        // The `if !v.candidates.is_empty()` guard -- untested that a
        // scan finding NOTHING correctly produces zero noise rather
        // than an empty or misleading recommendation.
        let scan = VideoScan::default();
        let recs = advise(&Inputs { videos: Some(&scan), ..Default::default() });
        assert!(recs.is_empty());
    }

    #[test]
    fn migration_percentage_is_computed_and_zero_tracked_bytes_does_not_divide_by_zero() {
        let plan_normal = MigrationPlan {
            repo: "/repo".into(),
            candidates: vec![crate::deltahostile::Candidate {
                path: "big.bin".into(), size: 250, reason: crate::deltahostile::Reason::HighEntropy(7.9),
            }],
            total_bytes: 250,
            tracked_bytes: 1000,
        };
        let recs = advise(&Inputs { migration: Some(&plan_normal), ..Default::default() });
        assert!(recs[0].title.contains("25%"), "{}", recs[0].title);

        // tracked_bytes: 0 is a real shape (a repo scan that could not
        // read any file sizes at all) -- the `if m.tracked_bytes > 0`
        // guard exists specifically to avoid a float division by zero
        // here, and had no test forcing that exact input.
        let plan_zero = MigrationPlan {
            repo: "/repo".into(), candidates: plan_normal.candidates.clone(),
            total_bytes: 250, tracked_bytes: 0,
        };
        let recs2 = advise(&Inputs { migration: Some(&plan_zero), ..Default::default() });
        assert!(recs2[0].title.contains("0%"), "{}", recs2[0].title); // must not panic or produce NaN/inf
    }
}
