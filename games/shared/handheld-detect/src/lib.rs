//! handheld-detect — is this machine a phone?
//!
//! Extracted from `platform.rs`, which was byte-identical in
//! `games/blackstar-rs` and `games/skydribble` (396+379+124 lines across
//! three such files; this is the one with a clean seam in it).
//!
//! ## Where the seam is, and why it is not "share the whole file"
//!
//! The original module does two jobs. One is asking what machine this is:
//! read the device-tree model, the `/proc/version` build tag and the
//! hostname, and decide. That is the same question in every game, it has a
//! right answer, and getting it wrong on a Fairphone is a bug in both games
//! at once — so it belongs here.
//!
//! The other is `Profile::apply`, which reaches into `crate::config::Config`
//! and sets render scale, bloom, vsync, camera distance and gyro gating.
//! That is not shared knowledge, it is a game's opinion about how it should
//! look on a small GPU, and blackstar-rs is deliberately growing into its own
//! game. Sharing it would mean one crate holding two games' art direction and
//! a `Config` type it cannot name. It stays in each game, calling
//! [`detect`] for the answer to the first question only.
//!
//! So this crate has no `apply`, no `Config`, and no opinion about graphics.
//! That is the point: the boundary is drawn where the two games agree and
//! stops exactly where they are entitled to differ.
//!
//! ## The override variable is now game-agnostic
//!
//! The original read `SKYDRIBBLE_MOBILE` in BOTH games, so forcing handheld
//! mode in blackstar-rs meant setting a variable named after the other game
//! — undiscoverable unless you had read the source. [`detect`] takes the
//! variable name from the caller, so each game passes its own and both keep
//! working. `detect_with_override` is the testable form; nothing here reads
//! the environment except `detect`.

use std::fs;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Profile {
    Desktop,
    /// Touchscreen handheld: the caller decides what that means for it.
    Handheld,
}

/// What made us decide — surfaced in the boot toast so a wrong call is
/// diagnosable on-device without a debugger.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Detection {
    pub profile: Profile,
    pub why: String,
}

impl Detection {
    pub fn is_handheld(&self) -> bool {
        self.profile == Profile::Handheld
    }
}

fn read(path: &str) -> String {
    fs::read_to_string(path).unwrap_or_default().to_lowercase()
}

/// Device-tree model, with the trailing NUL that firmware strings carry.
fn dt_model() -> String {
    let raw = fs::read("/sys/firmware/devicetree/base/model")
        .or_else(|_| fs::read("/proc/device-tree/model"))
        .unwrap_or_default();
    String::from_utf8_lossy(&raw).trim_end_matches('\0').trim().to_lowercase()
}

/// The decision, as a pure function of the strings — so the exact target
/// device can be proven in a headless test without owning one, and so a
/// future device is one test case rather than a debug session.
pub fn classify(version: &str, host: &str, model: &str, arm: bool) -> Detection {
    let (version, host, model) = (version.to_lowercase(), host.to_lowercase(), model.to_lowercase());
    let mobile_os = version.contains("postmarketos");
    let mobile_soc = version.contains("qcom-sc")
        || version.contains("qcom-sdm")
        || version.contains("qcom-msm")
        || model.contains("fairphone")
        || model.contains("pinephone")
        || model.contains("librem");
    let mobile_host = host.starts_with("fp") || host.contains("phone") || host.contains("pmos");
    if mobile_os || mobile_soc || (arm && mobile_host) {
        let mut why = Vec::new();
        if mobile_os {
            why.push("postmarketOS");
        }
        if mobile_soc {
            why.push("mobile SoC");
        }
        if mobile_host {
            why.push("device name");
        }
        if arm {
            why.push("aarch64");
        }
        Detection { profile: Profile::Handheld, why: why.join(" + ") }
    } else {
        Detection { profile: Profile::Desktop, why: "no handheld signal".into() }
    }
}

/// [`detect`] with the environment supplied, so the override precedence is
/// testable without touching the real environment — which is process-global
/// and would make these tests order-dependent under a threaded runner.
pub fn detect_with_override(env_value: Option<&str>, env_name: &str) -> Option<Detection> {
    match env_value {
        Some("1") | Some("true") => {
            Some(Detection { profile: Profile::Handheld, why: format!("{env_name}=1") })
        }
        Some("0") | Some("false") => {
            Some(Detection { profile: Profile::Desktop, why: format!("{env_name}=0") })
        }
        _ => None,
    }
}

/// Detect, honouring `env_name` as an explicit override.
///
/// `env_name` is the CALLER's variable — `SKYDRIBBLE_MOBILE`,
/// `BLACKSTAR_MOBILE`, whatever the game is called. The original hardcoded
/// one game's name and both games read it.
pub fn detect(env_name: &str) -> Detection {
    if let Some(d) = detect_with_override(std::env::var(env_name).ok().as_deref(), env_name) {
        return d;
    }
    // /proc/version carries the build tag: "#1-postmarketos-qcom-sc7280"
    classify(
        &read("/proc/version"),
        &read("/proc/sys/kernel/hostname"),
        &dt_model(),
        cfg!(target_arch = "aarch64"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The machine this module was written for.
    const FP5: &str =
        "linux fp5-black 7.1.2 #1-postmarketos-qcom-sc7280 smp preempt aarch64";

    #[test]
    fn the_fairphone_5_that_prompted_this_is_detected() {
        let d = classify(FP5, "fp5-black", "fairphone 5", true);
        assert_eq!(d.profile, Profile::Handheld);
        // Every signal that fired is named, because the boot toast shows this
        // and "handheld" alone does not tell you which rule to fix when it is
        // wrong.
        assert!(d.why.contains("postmarketOS"));
        assert!(d.why.contains("mobile SoC"));
    }

    #[test]
    fn a_desktop_is_not_a_phone() {
        let d = classify(
            "linux workstation 6.8.0-generic #1 smp x86_64",
            "workstation",
            "to be filled by o.e.m.",
            false,
        );
        assert_eq!(d.profile, Profile::Desktop);
        assert_eq!(d.why, "no handheld signal");
    }

    #[test]
    fn a_hostname_alone_needs_arm_to_count() {
        // `host.starts_with("fp")` is a weak signal — "fpga-builder" and
        // "fp-render-01" are plausible desktop names. The original guards it
        // behind `arm &&` for that reason, and this pins it: a desktop x86
        // box called "fphost" must not be classified as a phone just because
        // of its name.
        let x86 = classify("linux fphost 6.8.0 #1 smp x86_64", "fphost", "", false);
        assert_eq!(x86.profile, Profile::Desktop, "hostname alone on x86 is not enough");
        let arm = classify("linux fphost 6.8.0 #1 smp aarch64", "fphost", "", true);
        assert_eq!(arm.profile, Profile::Handheld, "hostname on ARM does count");
    }

    #[test]
    fn an_arm_server_is_not_a_phone() {
        // aarch64 by itself must not trigger handheld mode, or every ARM
        // build machine and CI runner drops to half render scale.
        let d = classify("linux build-a1 6.8.0 #1 smp aarch64", "build-a1", "", true);
        assert_eq!(d.profile, Profile::Desktop);
    }

    #[test]
    fn the_override_wins_over_the_hardware_and_names_itself() {
        // Forcing the touch UI on a desktop is the whole point of the
        // variable, so it must beat a Desktop classification, and the
        // reason must say so rather than claiming a hardware signal.
        let on = detect_with_override(Some("1"), "BLACKSTAR_MOBILE").unwrap();
        assert_eq!(on.profile, Profile::Handheld);
        assert_eq!(on.why, "BLACKSTAR_MOBILE=1");
        let off = detect_with_override(Some("0"), "SKYDRIBBLE_MOBILE").unwrap();
        assert_eq!(off.profile, Profile::Desktop);
        assert_eq!(off.why, "SKYDRIBBLE_MOBILE=0");
    }

    #[test]
    fn each_game_gets_its_own_variable_name() {
        // The copy artifact this extraction fixes: the original hardcoded
        // SKYDRIBBLE_MOBILE and BOTH games read it, so forcing handheld mode
        // in blackstar-rs meant setting a variable named after a different
        // game. Unguessable without reading the source.
        assert_eq!(
            detect_with_override(Some("1"), "BLACKSTAR_MOBILE").unwrap().why,
            "BLACKSTAR_MOBILE=1"
        );
    }

    #[test]
    fn an_unset_or_junk_override_falls_through_to_hardware() {
        // `None` means "not set"; anything unrecognised must also fall
        // through rather than being read as a truthy value — a typo like
        // `MOBILE=yes` silently forcing handheld mode would be worse than
        // it being ignored.
        assert_eq!(detect_with_override(None, "X"), None);
        assert_eq!(detect_with_override(Some(""), "X"), None);
        assert_eq!(detect_with_override(Some("yes"), "X"), None);
        assert_eq!(detect_with_override(Some("2"), "X"), None);
    }
}
