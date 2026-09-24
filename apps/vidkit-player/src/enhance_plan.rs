//! Which titles get enhanced, and how. Pure: no GPU, no ffmpeg.
//!
//! Enhancement has a price and a benefit, and both depend on the title:
//!
//! - **Benefit.** Upscaling helps a 480p lecture on a 1080p screen and does
//!   nothing for a 1080p film on the same screen. Sharpening helps a soft
//!   picture and adds grain to a crisp one.
//! - **Price.** The GPU shaders cost texture fetches per OUTPUT pixel per
//!   frame. bscale's CNN costs MACs per SOURCE pixel -- but only for the
//!   pixels that changed. bscale's analysis measures exactly that fraction:
//!   a lecture whose slides sit still for minutes needs the CNN on a few
//!   percent of each frame, a handheld vlog on most of it.
//!
//! So titles are ranked by cheap metadata, the best are analysed within a
//! time budget ("analysis mode"), and only analysed titles get a tier. The
//! tier is the most useful one whose cost fits the device's budget.

use std::time::Duration;

use vidkit_egui::glow_present::{Enhance, Scaler};

/// realesr-animevideov3, the model bscale vendors: MACs per input pixel for
/// x4. The x2 photo model is ~4.5 MMAC/px; the choice is the model's, and
/// this is the cheap one on purpose.
pub const CNN_MAC_PER_PX: f64 = 0.62e6;

/// What analysis learned about a title.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TitleFacts {
    pub src_w: u32,
    pub src_h: u32,
    pub fps: f64,
    /// bscale's `upscaled_pixel_fraction`: the share of pixels per frame the
    /// CNN would actually run on; the rest are reused blocks.
    pub residual_fraction: f64,
    /// 0 = crisp, 1 = very soft.
    pub softness: f32,
}

/// What this device can spend, per second of playback.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Device {
    pub out_w: u32,
    pub out_h: u32,
    /// Texture fetches per second the GPU can give video without the UI
    /// dropping frames. A deliberately practical figure, not the datasheet's.
    pub fetches_per_s: f64,
    /// Sustained CNN throughput from `bscale bench`, if measured.
    pub cnn_gmac_per_s: Option<f64>,
    /// Whether the player can run the CNN during playback at all. It cannot
    /// yet: the planner still costs the tier, so the report says which
    /// titles WOULD fit.
    pub cnn_in_player: bool,
    /// Share of the budget enhancement may take: the rest is decode, UI,
    /// thermal headroom.
    pub share: f64,
}

impl Device {
    /// Conservative defaults per machine class. A phone's GPU (Adreno 6xx)
    /// has tens of Gtexel/s on paper and a fraction of that sustained while
    /// the SoC also decodes and stays cool; a desktop dGPU has far more.
    pub fn default_for(class: GpuClass, out_w: u32, out_h: u32) -> Self {
        let gmac = std::env::var("VIDKIT_CNN_GMAC").ok().and_then(|v| v.trim().parse().ok());
        Self {
            out_w,
            out_h,
            fetches_per_s: class.fetches_per_s(),
            cnn_gmac_per_s: gmac,
            cnn_in_player: false,
            share: 0.25,
        }
    }
}

/// What kind of GPU this is. The budgets are practical sustained figures
/// for video alongside a UI, not datasheet peaks: an Adreno 6xx phone GPU,
/// a laptop iGPU (Intel Xe / AMD 680M class), a desktop dGPU.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum GpuClass {
    Phone,
    Integrated,
    Discrete,
}

impl GpuClass {
    pub fn fetches_per_s(self) -> f64 {
        match self {
            GpuClass::Phone => 8.0e9,
            GpuClass::Integrated => 2.5e10,
            GpuClass::Discrete => 1.5e11,
        }
    }
}

/// One DRM device as sysfs describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct DrmCard {
    /// PCI vendor, e.g. 0x10de NVIDIA, 0x1002 AMD, 0x8086 Intel. 0 for a
    /// platform device (phones, Apple silicon).
    pub vendor: u32,
    pub driver: String,
    /// Dedicated VRAM, if the driver reports it (amdgpu's mem_info_vram_total).
    pub vram: Option<u64>,
}

/// Classify from sysfs facts. Pure.
///
/// Any NVIDIA card is discrete. amdgpu reports VRAM for both: an APU's is a
/// small carve-out (512 MB - 2 GB), a card's is more. Intel Arc is discrete
/// on the `xe`/`i915` drivers but reports no VRAM this way, so Intel is
/// integrated unless told otherwise -- a stated limit, not a guess dressed
/// as a fact. A platform device on a phone SoC is the phone class.
pub fn classify_gpu(cards: &[DrmCard], phone_soc: bool) -> GpuClass {
    if phone_soc {
        return GpuClass::Phone;
    }
    let discrete = cards.iter().any(|c| {
        c.vendor == 0x10de || (c.driver == "amdgpu" && c.vram.is_some_and(|v| v > 2 << 30))
    });
    if discrete {
        GpuClass::Discrete
    } else {
        GpuClass::Integrated
    }
}

/// Read /sys/class/drm/card*/device.
pub fn drm_cards() -> Vec<DrmCard> {
    let Ok(rd) = std::fs::read_dir("/sys/class/drm") else { return Vec::new() };
    rd.flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.starts_with("card") && !n.contains('-')
        })
        .map(|e| {
            let dev = e.path().join("device");
            let vendor = std::fs::read_to_string(dev.join("vendor"))
                .ok()
                .and_then(|v| u32::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok())
                .unwrap_or(0);
            let driver = std::fs::read_link(dev.join("driver"))
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_default();
            let vram = std::fs::read_to_string(dev.join("mem_info_vram_total")).ok().and_then(|v| v.trim().parse().ok());
            DrmCard { vendor, driver, vram }
        })
        .collect()
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub enum Tier {
    Off,
    Sharpen,
    GpuUpscale,
    Cnn,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub tier: Tier,
    pub enhance: Enhance,
    /// Share of the device budget the chosen tier uses.
    pub cost: f64,
    /// 0..1: how much the title stands to gain.
    pub benefit: f32,
    /// What a CNN pass would cost, as a share of budget, when measurable.
    pub cnn_cost: Option<f64>,
    pub reason: String,
}

/// How much bigger the picture is shown than it was encoded, fit-to-screen.
pub fn upscale_factor(src_w: u32, src_h: u32, out_w: u32, out_h: u32) -> f64 {
    if src_w == 0 || src_h == 0 {
        return 1.0;
    }
    (out_w as f64 / src_w as f64).min(out_h as f64 / src_h as f64)
}

/// Below this bscale residual a title counts as a still picture.
pub const STILL_RESIDUAL: f64 = 0.15;

/// Shader cost as a share of budget: fetches per output pixel per frame,
/// plus the temporal pass's per SOURCE pixel.
pub fn shader_cost(e: &Enhance, fps: f64, d: &Device) -> f64 {
    shader_cost_src(e, fps, d, 0, 0)
}

pub fn shader_cost_src(e: &Enhance, fps: f64, d: &Device, src_w: u32, src_h: u32) -> f64 {
    let px = d.out_w as f64 * d.out_h as f64;
    let src = src_w as f64 * src_h as f64;
    (px * e.fetches_per_pixel() as f64 + src * e.fetches_per_source_pixel() as f64) * fps / (d.fetches_per_s * d.share)
}

/// CNN cost as a share of budget, or `None` with no measured throughput.
pub fn cnn_cost(t: &TitleFacts, d: &Device) -> Option<f64> {
    let gmac = d.cnn_gmac_per_s?;
    let macs = t.src_w as f64 * t.src_h as f64 * t.fps * t.residual_fraction.clamp(0.0, 1.0) * CNN_MAC_PER_PX;
    Some(macs / (gmac * 1e9 * d.share))
}

/// The tier for one analysed title. Pure.
pub fn plan_title(t: &TitleFacts, d: &Device) -> Plan {
    let factor = upscale_factor(t.src_w, t.src_h, d.out_w, d.out_h);
    let fps = if t.fps.is_finite() && t.fps > 0.0 { t.fps } else { 30.0 };
    // A factor of 3 (480p on a 1440p screen) is as much as upscaling gives.
    let up_benefit = ((factor - 1.0) / 2.0).clamp(0.0, 1.0) as f32;
    let benefit = up_benefit.max(t.softness.clamp(0.0, 1.0) * 0.6);
    // Stronger sharpening for softer pictures: 0.2 stops (strong) at the
    // softest, 1.2 (gentle) at the threshold.
    let stops = 1.2 - t.softness.clamp(0.0, 1.0);
    let sharpen = |on: bool| on.then_some(stops);
    let cnn = cnn_cost(t, d);

    // A mostly static picture -- a lecture, a blackboard, slides -- gets the
    // temporal denoise: the compression noise round each chalk stroke
    // settles, and RCAS sharpens a clean edge rather than the noise. bscale's
    // residual is the measure: under 15% of pixels change per frame. Game
    // footage and handheld video measure far higher and never get it (it
    // would pass nearly everything through anyway, at a cost).
    let still = t.residual_fraction < STILL_RESIDUAL;
    let candidates = [
        (Tier::Cnn, Enhance { scaler: Scaler::Bicubic, sharpen: sharpen(true), temporal: still }),
        (Tier::GpuUpscale, Enhance { scaler: Scaler::Bicubic, sharpen: sharpen(t.softness > 0.25 || still), temporal: still }),
        (Tier::Sharpen, Enhance { scaler: Scaler::Bilinear, sharpen: sharpen(true), temporal: still }),
    ];
    for (tier, e) in candidates {
        let (fits, cost, why) = match tier {
            Tier::Cnn => match (cnn, d.cnn_in_player) {
                (Some(c), true) if factor >= 1.5 => (c + shader_cost_src(&e, fps, d, t.src_w, t.src_h) <= 1.0, c, "CNN on the changed blocks"),
                _ => (false, 0.0, ""),
            },
            Tier::GpuUpscale if factor >= 1.2 => {
                let c = shader_cost_src(&e, fps, d, t.src_w, t.src_h);
                (c <= 1.0, c, "bicubic upscale on the GPU")
            }
            Tier::Sharpen if t.softness > 0.25 || still => {
                let c = shader_cost_src(&e, fps, d, t.src_w, t.src_h);
                (c <= 1.0, c, "RCAS sharpening only")
            }
            _ => (false, 0.0, ""),
        };
        if fits {
            return Plan {
                tier,
                enhance: e,
                cost,
                benefit,
                cnn_cost: cnn,
                reason: format!("{why}: {:.1}x on screen, {:.0}% soft, {:.0}% of budget", factor, t.softness * 100.0, cost * 100.0),
            };
        }
    }
    Plan {
        tier: Tier::Off,
        enhance: Enhance::default(),
        cost: 0.0,
        benefit,
        cnn_cost: cnn,
        reason: if factor < 1.2 && t.softness <= 0.25 {
            format!("already sharp at {:.1}x: nothing to gain", factor)
        } else {
            "over budget on this device".to_string()
        },
    }
}

/// A title before analysis: only what a probe and the history say.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub key: String,
    pub src_w: u32,
    pub src_h: u32,
    /// Unix seconds last watched.
    pub watched: u64,
    pub duration: Option<Duration>,
}

/// Order candidates for analysis: most upscale headroom first, recently
/// watched breaking ties. Titles already at screen size go last -- analysis
/// could still find them soft, but that is the smaller win. Pure.
pub fn rank(cands: &mut [Candidate], out_w: u32, out_h: u32, now: u64) {
    let score = |c: &Candidate| {
        let f = upscale_factor(c.src_w, c.src_h, out_w, out_h).min(4.0);
        let age_days = now.saturating_sub(c.watched) as f64 / 86_400.0;
        let recency = 1.0 / (1.0 + age_days / 7.0);
        f + 0.5 * recency
    };
    cands.sort_by(|a, b| score(b).partial_cmp(&score(a)).unwrap_or(std::cmp::Ordering::Equal));
}

/// How many of the ranked candidates to analyse within `budget`, given the
/// measured cost of the ones done so far. Pure. Analysis cost scales with
/// the frames analysed, which is fixed per title, so a running mean is a
/// fair predictor; with nothing measured yet, one is always allowed.
pub fn within_budget(spent: Duration, done: usize, budget: Duration) -> bool {
    if done == 0 {
        return true;
    }
    let mean = spent / done as u32;
    spent + mean <= budget
}

/// One line per analysed title, beside its transcript in the seen store:
/// `v1 <w> <h> <fps> <residual> <softness>`. Pure codec.
pub fn facts_to_line(t: &TitleFacts) -> String {
    format!("v1 {} {} {:.3} {:.4} {:.3}", t.src_w, t.src_h, t.fps, t.residual_fraction, t.softness)
}

pub fn facts_from_line(l: &str) -> Option<TitleFacts> {
    let mut p = l.split_whitespace();
    if p.next()? != "v1" {
        return None;
    }
    Some(TitleFacts {
        src_w: p.next()?.parse().ok()?,
        src_h: p.next()?.parse().ok()?,
        fps: p.next()?.parse().ok()?,
        residual_fraction: p.next()?.parse().ok()?,
        softness: p.next()?.parse().ok()?,
    })
}

pub fn facts_path(dir: &std::path::Path, url: &str) -> std::path::PathBuf {
    dir.join(format!("{:016x}.enh", crate::transcribe::fnv1a(url.trim().as_bytes())))
}

pub fn load_facts(dir: &std::path::Path, url: &str) -> Option<TitleFacts> {
    facts_from_line(std::fs::read_to_string(facts_path(dir, url)).ok()?.lines().next()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phone() -> Device {
        Device { out_w: 1080, out_h: 2340, fetches_per_s: 8.0e9, cnn_gmac_per_s: None, cnn_in_player: false, share: 0.25 }
    }
    fn laptop() -> Device {
        Device { out_w: 1920, out_h: 1080, fetches_per_s: 2.0e10, cnn_gmac_per_s: Some(400.0), cnn_in_player: false, share: 0.25 }
    }
    fn lecture_480p() -> TitleFacts {
        TitleFacts { src_w: 854, src_h: 480, fps: 30.0, residual_fraction: 0.04, softness: 0.5 }
    }

    #[test]
    fn a_480p_lecture_on_a_laptop_is_upscaled_and_sharpened() {
        let p = plan_title(&lecture_480p(), &laptop());
        assert_eq!(p.tier, Tier::GpuUpscale, "{}", p.reason);
        assert_eq!(p.enhance.scaler, Scaler::Bicubic);
        assert!(p.enhance.sharpen.is_some(), "soft, so sharpened too");
        assert!(p.cost < 0.2, "{}", p.cost);
    }

    #[test]
    fn a_crisp_1080p_film_on_a_1080p_screen_is_left_alone() {
        let film = TitleFacts { src_w: 1920, src_h: 1080, fps: 24.0, residual_fraction: 0.6, softness: 0.1 };
        let p = plan_title(&film, &laptop());
        assert_eq!(p.tier, Tier::Off);
        assert!(p.reason.contains("nothing to gain"), "{}", p.reason);
    }

    /// The CNN is costed from the residual fraction bscale measures: a still
    /// lecture costs a small share of what a busy vlog does.
    #[test]
    fn cnn_cost_follows_what_actually_changes() {
        let still = cnn_cost(&lecture_480p(), &laptop()).unwrap();
        let busy = cnn_cost(&TitleFacts { residual_fraction: 0.8, ..lecture_480p() }, &laptop()).unwrap();
        assert!((busy / still - 20.0).abs() < 1e-6, "{still} vs {busy}");
        // 854x480x30x0.04x0.62M = 305 GMAC/s against 100 GMAC/s of share:
        // three times over even for a still lecture on a 400 GMAC/s GPU.
        // The realtime CNN wants a big GPU; that is the arithmetic.
        assert!((still - 3.05).abs() < 0.01, "{still}");
        assert_eq!(cnn_cost(&lecture_480p(), &phone()), None, "unmeasured device: no guess");
    }

    /// Costed, never chosen, while the player cannot run the CNN.
    #[test]
    fn the_cnn_tier_is_only_chosen_when_the_player_can_run_it() {
        let d = laptop();
        let p = plan_title(&lecture_480p(), &d);
        assert!(p.cnn_cost.is_some());
        assert_ne!(p.tier, Tier::Cnn);
        // At 400 GMAC/s even the still lecture does not fit (3x over).
        let able = Device { cnn_in_player: true, ..d };
        assert_ne!(plan_title(&lecture_480p(), &able).tier, Tier::Cnn);
        // A big GPU (1600 GMAC/s sustained): the still lecture's CNN takes
        // 76% and its denoise + bicubic + RCAS passes ~19% -- it fits; at
        // 1500 the sum crossed 100%. A busy title (25x the residual) does not.
        let big = Device { cnn_gmac_per_s: Some(1600.0), ..able };
        assert_eq!(plan_title(&lecture_480p(), &big).tier, Tier::Cnn);
        let busy = TitleFacts { residual_fraction: 1.0, ..lecture_480p() };
        assert_ne!(plan_title(&busy, &big).tier, Tier::Cnn);
    }

    /// A phone's full-screen output is 2.5 Mpx: bicubic + RCAS at 60 fps is
    /// 15 fetches x 2.5M x 60 = 2.3 G/s, over a quarter of 8 G/s; sharpening
    /// alone (7 fetches) is 1.1 G/s and fits. At 30 fps both fit.
    #[test]
    fn a_phone_budget_is_respected_at_high_frame_rates() {
        let t = TitleFacts { fps: 60.0, ..lecture_480p() };
        let p = plan_title(&t, &phone());
        assert!(p.cost <= 1.0, "{p:?}");
        assert!(p.tier < Tier::GpuUpscale, "bicubic+RCAS at 60 fps does not fit: {}", p.reason);
        assert_eq!(p.tier, Tier::Sharpen, "{}", p.reason);
        let p30 = plan_title(&lecture_480p(), &phone());
        assert_eq!(p30.tier, Tier::GpuUpscale, "{}", p30.reason);
    }

    #[test]
    fn analysis_goes_to_the_titles_with_the_most_to_gain() {
        let c = |k: &str, w, h, watched| Candidate { key: k.into(), src_w: w, src_h: h, watched, duration: None };
        let mut cs = vec![c("film-1080", 1920, 1080, 1000), c("lecture-360", 640, 360, 900), c("talk-720", 1280, 720, 1000)];
        rank(&mut cs, 1920, 1080, 1000);
        let order: Vec<&str> = cs.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(order, vec!["lecture-360", "talk-720", "film-1080"]);
    }

    #[test]
    fn the_analysis_budget_stops_before_it_would_be_exceeded() {
        let s = |n| Duration::from_secs(n);
        assert!(within_budget(s(0), 0, s(1)), "the first is always allowed");
        assert!(within_budget(s(40), 2, s(120)), "mean 20: 60 <= 120");
        assert!(!within_budget(s(110), 2, s(120)), "mean 55: 165 > 120");
    }

    #[test]
    fn facts_round_trip() {
        let t = lecture_480p();
        assert_eq!(facts_from_line(&facts_to_line(&t)), Some(t));
        assert_eq!(facts_from_line("v2 1 2 3 4 5"), None);
    }

    #[test]
    fn gpus_are_classified_from_sysfs_facts() {
        let card = |vendor, driver: &str, vram| DrmCard { vendor, driver: driver.into(), vram };
        assert_eq!(classify_gpu(&[card(0x8086, "i915", None)], false), GpuClass::Integrated);
        assert_eq!(classify_gpu(&[card(0x8086, "i915", None), card(0x10de, "nvidia", None)], false), GpuClass::Discrete, "hybrid laptop");
        assert_eq!(classify_gpu(&[card(0x1002, "amdgpu", Some(512 << 20))], false), GpuClass::Integrated, "APU carve-out");
        assert_eq!(classify_gpu(&[card(0x1002, "amdgpu", Some(16 << 30))], false), GpuClass::Discrete);
        assert_eq!(classify_gpu(&[card(0, "msm", None)], true), GpuClass::Phone);
        assert_eq!(classify_gpu(&[], false), GpuClass::Integrated, "unknown: the cautious class");
    }

    /// Blackboard lecture on a laptop iGPU: static, soft, 720p on a 1080p
    /// panel. Denoise + upscale + sharpen, well within budget.
    #[test]
    fn a_blackboard_lecture_on_an_igpu_gets_denoise_upscale_and_sharpen() {
        let board = TitleFacts { src_w: 1280, src_h: 720, fps: 30.0, residual_fraction: 0.05, softness: 0.4 };
        let d = Device::default_for(GpuClass::Integrated, 1920, 1080);
        let p = plan_title(&board, &d);
        assert_eq!(p.tier, Tier::GpuUpscale, "{}", p.reason);
        assert!(p.enhance.temporal && p.enhance.sharpen.is_some(), "{:?}", p.enhance);
        assert!(p.cost < 0.2, "{}", p.cost);
        // Even at native size it is denoised and sharpened.
        let native = TitleFacts { src_w: 1920, src_h: 1080, ..board };
        let p = plan_title(&native, &d);
        assert_eq!(p.tier, Tier::Sharpen, "{}", p.reason);
        assert!(p.enhance.temporal);
    }

    /// Smite on a gaming PC: 1080p60 of constant motion shown on a 1440p
    /// panel. Shader upscale, no temporal pass; and the CNN arithmetic says
    /// why not the CNN even on a 1500 GMAC/s card.
    #[test]
    fn game_footage_on_a_dgpu_gets_the_shader_upscale_not_the_denoise() {
        let smite = TitleFacts { src_w: 1920, src_h: 1080, fps: 60.0, residual_fraction: 0.9, softness: 0.1 };
        let d = Device { cnn_gmac_per_s: Some(1500.0), cnn_in_player: true, ..Device::default_for(GpuClass::Discrete, 2560, 1440) };
        let p = plan_title(&smite, &d);
        assert_eq!(p.tier, Tier::GpuUpscale, "{}", p.reason);
        assert!(!p.enhance.temporal);
        // 1920x1080x60x0.9x0.62M = 69 TMAC/s against 375 GMAC/s: ~185x over.
        let c = p.cnn_cost.unwrap();
        assert!(c > 150.0 && c < 220.0, "{c}");
    }
}
