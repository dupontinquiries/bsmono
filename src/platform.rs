//! Platform detection — is this a phone?
//!
//! The target that prompted this module:
//!
//! ```text
//! Linux fp5-black 7.1.2 #1-postmarketos-qcom-sc7280 SMP PREEMPT aarch64
//! ```
//!
//! A Fairphone 5 (Qualcomm SC7280, Adreno 644) running postmarketOS. That is
//! a real Linux box with a real Vulkan driver, so everything builds and runs
//! — but it has a tenth of a desktop GPU's fill rate, a touchscreen instead
//! of a keyboard, and a compositor that wants Fifo. Detection here drives all
//! three adaptations at boot (see `Profile::apply`).
//!
//! Detection is deliberately conservative and always overridable:
//! `SKYDRIBBLE_MOBILE=1` forces handheld mode on any machine (handy for
//! testing the touch UI on a desktop), `SKYDRIBBLE_MOBILE=0` forces it off.

use std::fs;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Profile {
    Desktop,
    /// Touchscreen handheld: low graphics, half render scale, on-screen pads.
    Handheld,
}

/// What made us decide — surfaced in the boot toast so a wrong call is
/// diagnosable on-device without a debugger.
pub struct Detection {
    pub profile: Profile,
    pub why: String,
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
/// device can be proven in a headless sim (see `--sim plat`) without owning
/// one, and so a future device is one test case rather than a debug session.
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

pub fn detect() -> Detection {
    // explicit override always wins
    match std::env::var("SKYDRIBBLE_MOBILE").as_deref() {
        Ok("1") | Ok("true") => {
            return Detection { profile: Profile::Handheld, why: "SKYDRIBBLE_MOBILE=1".into() }
        }
        Ok("0") | Ok("false") => {
            return Detection { profile: Profile::Desktop, why: "SKYDRIBBLE_MOBILE=0".into() }
        }
        _ => {}
    }

    // /proc/version carries the build tag: "#1-postmarketos-qcom-sc7280"
    classify(&read("/proc/version"), &read("/proc/sys/kernel/hostname"), &dt_model(), cfg!(target_arch = "aarch64"))
}

impl Profile {
    /// Adapt the config for the detected machine. Only ever called when no
    /// save file existed OR the user asked to re-apply, so a player who tunes
    /// their phone's settings keeps them.
    pub fn apply(self, cfg: &mut crate::config::Config) {
        // GYRO FLYING on a handheld: motion controls are the natural way to
        // fly a car you cannot get a second thumb onto, so they arrive on,
        // gated to the air where they help and cannot upset your driving.
        cfg.gyro.on = true;
        cfg.gyro.gate = 2;
        cfg.gyro.air_mode = 0;
        if self != Profile::Handheld {
            return;
        }
        // ---- low graphics ----
        cfg.video.quality = 0.30; // AO off, short shadows, cheap reflections
        cfg.video.rays = false; // no mirror marching on a phone GPU
        cfg.video.bloom = cfg.video.bloom.min(0.35);
        cfg.video.screen_door = 0.0;
        cfg.video.render_scale = 0.5; // HALF resolution scene, native UI
        // Fifo: the Wayland compositor paces us instead of fighting us. The
        // monorepo's own Fairphone notes call this the stutter fix.
        cfg.video.vsync = true;
        // ---- touch ----
        cfg.touch.enabled = true;
        // a phone screen is small: bring the camera in a little
        cfg.cam.dist = cfg.cam.dist.min(6.2);
    }
}
