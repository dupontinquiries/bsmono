//! Working out what this machine actually is.
//!
//! `DeviceCaps` has always been a set of hand-written reference profiles picked
//! by guessing from the target triple. That is wrong for two of the three
//! aarch64 shapes this player targets — Venus wants VP9, Asahi M1/M2 wants AV1
//! in software, Asahi M3 wants AV1 in hardware and has no usable GPU — and
//! guessing wrong costs battery on every frame.
//!
//! This reads what the kernel already knows. Every path is taken relative to a
//! `root`, so the whole thing is tested against fixture directories rather than
//! requiring the hardware: the sandbox this was written in has neither an AVD
//! nor a Venus, and a probe that can only be tested on the device it detects is
//! a probe nobody runs.
//!
//! # What it reads, and why those
//!
//! - `/proc/device-tree/compatible` — a NUL-separated list, most specific
//!   first. Apple SoCs appear as `apple,t8103` and friends, and that string is
//!   the only reliable way to tell an M1 from an M3: the marketing name is
//!   nowhere in the kernel.
//! - `/sys/class/video4linux/*/name` — the decoder driver's own name. This is
//!   how a Venus is distinguished from an AVD without opening either.
//! - `/sys/class/drm/*/device/uevent` — `DRIVER=` for the GPU. On M3 there is
//!   no `asahi` DRM driver bound, which is exactly the case `GpuClass` exists
//!   to carry.
//!
//! # It never guesses silently
//!
//! Anything it cannot identify comes back [`Detected::Unknown`] with the
//! evidence it did find, and the caller falls back to a documented default. A
//! probe that returns a confident wrong answer is worse than the target-triple
//! guess it replaces, because nobody checks a confident answer.

use std::path::{Path, PathBuf};

/// Which machine this looks like.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Detected {
    /// Apple silicon with the AVD but no AV1 in hardware: M1, M2 and their
    /// Pro/Max/Ultra variants.
    AsahiM1M2 { soc: String },
    /// Apple silicon whose AVD does AV1: M3 and later.
    AsahiM3Plus { soc: String },
    /// Qualcomm Venus, as on the Fairphone 5's QCM6490.
    QualcommVenus { driver: String },
    /// Something with VAAPI or Vulkan video and no special casing needed.
    Desktop,
    /// Not identified. Carries what was seen so a bug report is one paste.
    Unknown { evidence: Vec<String> },
}

/// The full result: what it is, and what the GPU is good for.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Probe {
    pub detected: Detected,
    pub gpu_driver: Option<String>,
    /// Names of every V4L2 node found, for the diagnostics panel.
    pub v4l2_names: Vec<String>,
}

fn read(root: &Path, rel: &str) -> Option<String> {
    std::fs::read(root.join(rel))
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// The device-tree `compatible` list, NUL-separated in the file.
pub fn compatible(root: &Path) -> Vec<String> {
    read(root, "proc/device-tree/compatible")
        .map(|s| {
            s.split('\0')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Does this Apple SoC's AVD decode AV1?
///
/// M3 and later. The SoC ids are the only thing the kernel exposes — there is
/// no "M3" string anywhere — so this is a list, and an Apple SoC that is not on
/// it is treated as PRE-M3 rather than assumed newer. Guessing newer would
/// claim hardware AV1 that is not there and pick streams the machine has to
/// decode in software, which is the expensive direction to be wrong in.
pub fn apple_soc_has_av1(soc: &str) -> bool {
    // M3 (t8122), M3 Pro (t6030), M3 Max (t6031), M4 (t8132) and the M4 Pro/Max
    // pair, which share the generation's media block.
    matches!(
        soc,
        "t8122" | "t6030" | "t6031" | "t6034" | "t8132" | "t6040" | "t6041"
    )
}

fn apple_soc(compat: &[String]) -> Option<String> {
    compat.iter().find_map(|c| {
        let rest = c.strip_prefix("apple,")?;
        // `apple,t8103` — an SoC id, not `apple,j274` (the board) or
        // `apple,arm-platform` (the family).
        let looks_like_soc = rest.starts_with('t')
            && rest.len() >= 5
            && rest[1..].chars().all(|ch| ch.is_ascii_digit());
        looks_like_soc.then(|| rest.to_string())
    })
}

/// Every V4L2 node's driver-reported name.
pub fn v4l2_names(root: &Path) -> Vec<String> {
    let dir = root.join("sys/class/video4linux");
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        // Sorted, or the order depends on the filesystem and the diagnostics
        // panel reorders itself between runs.
        paths.sort();
        for p in paths {
            if let Ok(name) = std::fs::read_to_string(p.join("name")) {
                let n = name.trim().to_string();
                if !n.is_empty() {
                    out.push(n);
                }
            }
        }
    }
    out
}

/// The bound DRM driver, if any.
pub fn gpu_driver(root: &Path) -> Option<String> {
    let dir = root.join("sys/class/drm");
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                // `card0`, not `card0-HDMI-A-1`: the connectors have no driver.
                .map(|n| n.starts_with("card") && !n.contains('-'))
                .unwrap_or(false)
        })
        .collect();
    entries.sort();
    for p in entries {
        if let Ok(uevent) = std::fs::read_to_string(p.join("device/uevent")) {
            if let Some(d) = uevent
                .lines()
                .find_map(|l| l.strip_prefix("DRIVER="))
                .map(|d| d.trim().to_string())
            {
                if !d.is_empty() {
                    return Some(d);
                }
            }
        }
    }
    None
}

/// Probe a filesystem root. Pass `/` on a real machine.
pub fn probe(root: &Path) -> Probe {
    let compat = compatible(root);
    let names = v4l2_names(root);
    let gpu = gpu_driver(root);

    let lower: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
    let has_avd = lower.iter().any(|n| n.contains("avd") || n.contains("apple"));
    // Venus, and Iris: the SC7280/QCM6490 video core moves from the Venus
    // driver to Iris ("flip the switch", 2026; default from 6.21 when both
    // are built). Iris reports as iris_driver / "Iris Decoder", which a
    // venus|qcom match misses -- and on a cold boot no decoder may be bound
    // at all, so the SoC's own compatible counts too.
    let qcom_video_soc = compat.iter().any(|c| c == "qcom,qcm6490" || c == "qcom,sc7280");
    let has_venus = qcom_video_soc
        || lower.iter().any(|n| n.contains("venus") || n.contains("qcom") || n.contains("iris"));

    let detected = if let Some(soc) = apple_soc(&compat) {
        // Keyed on the SoC, not on the decoder node: the AVD may not be bound
        // yet on a cold boot, and the machine is still what it is.
        if apple_soc_has_av1(&soc) {
            Detected::AsahiM3Plus { soc }
        } else {
            Detected::AsahiM1M2 { soc }
        }
    } else if has_venus {
        Detected::QualcommVenus {
            driver: names
                .iter()
                .find(|n| {
                    let l = n.to_lowercase();
                    l.contains("venus") || l.contains("qcom") || l.contains("iris")
                })
                .cloned()
                .unwrap_or_default(),
        }
    } else if gpu.is_some() && !has_avd {
        Detected::Desktop
    } else {
        Detected::Unknown {
            evidence: compat
                .iter()
                .cloned()
                .chain(names.iter().cloned())
                .chain(gpu.clone())
                .collect(),
        }
    };

    Probe {
        detected,
        gpu_driver: gpu,
        v4l2_names: names,
    }
}

/// Turn a probe into capabilities.
///
/// `Unknown` returns `None` rather than a plausible profile: the caller has a
/// documented default and a wrong confident answer is worse than an admitted
/// gap.
pub fn caps_for(p: &Probe) -> Option<vidkit_core::caps::DeviceCaps> {
    use vidkit_core::caps::*;
    let mut caps = match &p.detected {
        Detected::AsahiM1M2 { .. } => asahi_m_series(),
        Detected::AsahiM3Plus { .. } => asahi_m3(),
        Detected::QualcommVenus { .. } => fairphone_venus(),
        Detected::Desktop => desktop_full_hw(),
        Detected::Unknown { .. } => return None,
    };

    // The GPU is read from the machine rather than taken from the reference
    // profile. This is the M3 case and the reason the probe is worth having:
    // the profile says Unaccelerated because that was true when it was written,
    // and the day an `asahi` DRM driver binds on M3 this picks it up without
    // anyone editing a table.
    caps.gpu = match p.gpu_driver.as_deref() {
        None => GpuClass::None,
        Some("llvmpipe") | Some("swrast") | Some("vgem") => GpuClass::Unaccelerated,
        Some(_) => GpuClass::Vulkan,
    };
    // ...except that a bound driver on an M3 is not yet a usable one. Asahi's
    // own words: "do not expect performant or power-efficient 3D acceleration
    // right now". Trusting the bind here would switch on the GPU convert path
    // on the one machine that must not use it.
    if matches!(p.detected, Detected::AsahiM3Plus { .. }) {
        caps.gpu = caps.gpu.min(GpuClass::Unaccelerated);
    }
    Some(caps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Build a fake /sys and /proc. The point of the whole module: this is
    /// testable without an AVD, a Venus or a Mac.
    struct FakeRoot(PathBuf);

    impl FakeRoot {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "vidkit-probe-{tag}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn write(&self, rel: &str, content: &[u8]) -> &Self {
            let full = self.0.join(rel);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, content).unwrap();
            self
        }
        fn v4l2(&self, node: &str, name: &str) -> &Self {
            self.write(&format!("sys/class/video4linux/{node}/name"), name.as_bytes())
        }
        fn gpu(&self, card: &str, driver: &str) -> &Self {
            self.write(
                &format!("sys/class/drm/{card}/device/uevent"),
                format!("DRIVER={driver}\nOF_NAME=gpu\n").as_bytes(),
            )
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for FakeRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn an_m1_is_recognised_from_its_soc_id() {
        let r = FakeRoot::new("m1");
        r.write("proc/device-tree/compatible", b"apple,j274\0apple,t8103\0apple,arm-platform\0")
            .v4l2("video0", "apple-avd")
            .gpu("card0", "asahi");
        let p = probe(r.path());
        assert_eq!(p.detected, Detected::AsahiM1M2 { soc: "t8103".into() });
    }

    /// The whole reason the SoC id is read: there is no "M3" string anywhere in
    /// the kernel, and an M1 and an M3 are otherwise indistinguishable.
    #[test]
    fn an_m3_is_distinguished_from_an_m1() {
        let r = FakeRoot::new("m3");
        r.write("proc/device-tree/compatible", b"apple,j504\0apple,t8122\0apple,arm-platform\0")
            .v4l2("video0", "apple-avd");
        let p = probe(r.path());
        assert_eq!(p.detected, Detected::AsahiM3Plus { soc: "t8122".into() });

        let caps = caps_for(&p).expect("m3 has a profile");
        assert!(caps.name.contains("M3"), "wrong profile: {}", caps.name);
    }

    /// An Apple SoC not on the AV1 list is treated as PRE-M3. Guessing newer
    /// would claim hardware AV1 that is not there and pick streams the machine
    /// then decodes in software, which is the expensive direction.
    #[test]
    fn an_unlisted_apple_soc_is_assumed_older_not_newer() {
        let r = FakeRoot::new("future");
        r.write("proc/device-tree/compatible", b"apple,t9999\0apple,arm-platform\0")
            .v4l2("video0", "apple-avd");
        assert_eq!(
            probe(r.path()).detected,
            Detected::AsahiM1M2 { soc: "t9999".into() }
        );
    }

    /// The board id must not be mistaken for the SoC id.
    /// A `t`-prefixed compatible that is not an SoC id. `j274` never reaches
    /// the digit check -- it fails on the `t` -- so without this case the
    /// digit check was unexercised and deleting it broke nothing.
    #[test]
    fn a_t_prefixed_string_that_is_not_an_soc_id_is_rejected() {
        let r = FakeRoot::new("tstring");
        r.write("proc/device-tree/compatible", b"apple,touchbar\0apple,arm-platform\0");
        assert!(
            matches!(probe(r.path()).detected, Detected::Unknown { .. }),
            "a non-numeric t-string was read as an SoC"
        );
    }

    #[test]
    fn the_board_id_is_not_the_soc_id() {
        let r = FakeRoot::new("board");
        r.write("proc/device-tree/compatible", b"apple,j274\0apple,arm-platform\0");
        let p = probe(r.path());
        assert!(
            matches!(p.detected, Detected::Unknown { .. }),
            "j274 was read as an SoC: {:?}",
            p.detected
        );
    }

    #[test]
    fn an_iris_kernel_fp5_is_still_a_fairphone() {
        for name in ["qcom-iris-decoder", "Iris Decoder", "iris_driver"] {
            let r = FakeRoot::new(&format!("iris-{}", name.len()));
            r.write("proc/device-tree/compatible", b"fairphone,fp5\0qcom,qcm6490\0").v4l2("video0", name);
            let p = probe(r.path());
            assert!(matches!(p.detected, Detected::QualcommVenus { .. }), "{name}: {:?}", p.detected);
        }
        // No decoder bound yet (cold boot): the SoC compatible alone says it.
        let r = FakeRoot::new("iris-unbound");
        r.write("proc/device-tree/compatible", b"fairphone,fp5\0qcom,qcm6490\0");
        assert!(matches!(probe(r.path()).detected, Detected::QualcommVenus { .. }));
    }

    #[test]
    fn a_venus_is_recognised_by_its_driver_name() {
        let r = FakeRoot::new("venus");
        r.write("proc/device-tree/compatible", b"fairphone,fp5\0qcom,qcm6490\0")
            .v4l2("video0", "qcom-venus-decoder")
            .gpu("card0", "msm");
        let p = probe(r.path());
        assert!(matches!(p.detected, Detected::QualcommVenus { .. }), "{:?}", p.detected);
        let caps = caps_for(&p).unwrap();
        assert!(caps.name.to_lowercase().contains("fairphone") || caps.name.contains("Venus"));
    }

    /// The GPU comes from the machine, not the reference profile — so the day
    /// a driver binds where there was none, nobody has to edit a table.
    #[test]
    fn the_gpu_class_is_read_from_the_bound_driver() {
        let r = FakeRoot::new("gpu");
        r.write("proc/device-tree/compatible", b"fairphone,fp5\0qcom,qcm6490\0")
            .v4l2("video0", "qcom-venus-decoder");
        // No DRM node at all.
        let none = caps_for(&probe(r.path())).unwrap();
        assert_eq!(none.gpu, vidkit_core::caps::GpuClass::None);

        r.gpu("card0", "msm");
        let real = caps_for(&probe(r.path())).unwrap();
        assert_eq!(real.gpu, vidkit_core::caps::GpuClass::Vulkan);

        r.gpu("card0", "llvmpipe");
        let soft = caps_for(&probe(r.path())).unwrap();
        assert_eq!(soft.gpu, vidkit_core::caps::GpuClass::Unaccelerated);
    }

    /// M3 is the exception: a bound driver is not yet a usable one, and
    /// trusting the bind would switch on the GPU convert path on the one
    /// machine that must not use it.
    #[test]
    fn an_m3_never_reports_a_usable_gpu_even_with_a_driver_bound() {
        let r = FakeRoot::new("m3gpu");
        r.write("proc/device-tree/compatible", b"apple,t8122\0")
            .v4l2("video0", "apple-avd")
            .gpu("card0", "asahi");
        let caps = caps_for(&probe(r.path())).unwrap();
        assert!(
            !caps.gpu.worth_converting_on(),
            "m3 claimed a usable GPU because a driver was bound"
        );
    }

    /// A connector directory is not a card.
    #[test]
    fn drm_connectors_are_not_mistaken_for_the_gpu() {
        let r = FakeRoot::new("conn");
        r.write("sys/class/drm/card0-HDMI-A-1/device/uevent", b"DRIVER=nonsense\n");
        assert_eq!(gpu_driver(r.path()), None);
    }

    /// Nothing recognisable returns Unknown WITH evidence, not a guess. A
    /// confident wrong answer is worse than the target-triple guess this
    /// replaces, because nobody checks a confident answer.
    #[test]
    fn an_unrecognised_machine_admits_it_and_shows_its_working() {
        let r = FakeRoot::new("weird");
        r.write("proc/device-tree/compatible", b"acme,widget\0")
            .v4l2("video0", "some-unknown-codec");
        let p = probe(r.path());
        match &p.detected {
            Detected::Unknown { evidence } => {
                assert!(evidence.iter().any(|e| e.contains("acme")), "{evidence:?}");
                assert!(evidence.iter().any(|e| e.contains("unknown-codec")));
            }
            other => panic!("guessed {other:?}"),
        }
        assert!(caps_for(&p).is_none(), "Unknown produced a confident profile");
    }

    /// An empty root must not panic; a probe that dies on a machine it does
    /// not understand takes the player with it.
    #[test]
    fn an_empty_root_is_survivable() {
        let r = FakeRoot::new("empty");
        let p = probe(r.path());
        assert!(matches!(p.detected, Detected::Unknown { .. }));
        assert!(p.v4l2_names.is_empty() && p.gpu_driver.is_none());
    }

    /// Node order must not depend on the filesystem, or the diagnostics panel
    /// reorders itself between runs.
    #[test]
    fn v4l2_nodes_come_back_sorted() {
        let r = FakeRoot::new("order");
        r.v4l2("video3", "third").v4l2("video1", "first").v4l2("video2", "second");
        assert_eq!(v4l2_names(r.path()), vec!["first", "second", "third"]);
    }
}
