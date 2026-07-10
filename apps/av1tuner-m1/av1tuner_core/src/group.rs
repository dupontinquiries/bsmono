//! # group — merge groups (tabs), output templates, resolution policy
//!
//! A `MergeGroup` is one tab in the UI: a named, ordered list of source 
//! videos that concatenate into a single output file, with chapters 
//! marking each source clip's boundary. Each group can override the 
//! global encoder settings and carries its own output-name template + 
//! resolution policy.
//!
//! This module is the data model + planning logic. Execution (encode 
//! each clip, concat into one file with chapters) lives in the batch 
//! runner, reusing the chunk/scheduler/concat machinery a level down.

use std::path::PathBuf;
use serde::{Serialize, Deserialize};

use crate::encoder::EncoderSettings;

/// How to handle a group whose source clips have different resolutions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolutionPolicy {
    /// Scale every clip to the smallest resolution present (least common
    /// denominator). Default — avoids upscaling, keeps the merge clean.
    ScaleToSmallest,
    /// Scale every clip to the largest resolution present (upscales the
    /// smaller ones; can look soft but preserves the big clips' detail).
    ScaleToLargest,
    /// Leave clips at native resolution. Concat may fail or produce a
    /// variable-resolution output that some players dislike. The UI
    /// flags this as risky.
    KeepNative,
}

impl Default for ResolutionPolicy {
    fn default() -> Self { ResolutionPolicy::ScaleToSmallest }
}

impl ResolutionPolicy {
    pub fn label(&self) -> &'static str {
        match self {
            ResolutionPolicy::ScaleToSmallest => "Scale to smallest (recommended)",
            ResolutionPolicy::ScaleToLargest => "Scale to largest",
            ResolutionPolicy::KeepNative => "Keep native (risky)",
        }
    }
}

/// One source clip within a group, with the metadata the UI shows + 
/// the merge planner needs.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GroupClip {
    pub path: PathBuf,
    /// Bytes on disk (display + savings calc).
    pub bytes: u64,
    /// Width/height if known (from probe). None until probed.
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Frames per second if known (from probe). None until probed. 
    /// Used to detect framerate mismatch across a group's clips — the 
    /// resolution policy equalizes size but NOT fps, so mixed-fps 
    /// groups need a separate warning.
    pub fps: Option<f64>,
    /// Whether the user has this clip enabled in the merge. Lets you 
    /// keep a clip in the list but skip it without removing it.
    pub enabled: bool,
}

impl GroupClip {
    pub fn new(path: PathBuf, bytes: u64) -> Self {
        Self { path, bytes, width: None, height: None, fps: None, enabled: true }
    }
}

/// Per-group upscaling configuration. Off by default. When on, the
/// block-processing layer runs the configured processor over the
/// group's frames with dedup, optionally guided by reference clips.
///
/// NOTE: the actual high-precision model is a swappable component. Today
/// the only processor is the basic cross-reference upscaler; this enum
/// carries the toggle + factor + reference choice so the data model and
/// UI are ready when better processors drop in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpscaleConfig {
    /// Master toggle for this group.
    pub enabled: bool,
    /// Integer upscale factor (1, 2, ...).
    pub factor: u32,
    /// Use reference clips (clean footage of the same content) to guide
    /// enhancement. When false, blind upscale only.
    pub use_references: bool,
    /// Indices (into the group's clips) to treat as reference sources
    /// rather than footage-to-upscale. These provide clean detail; they
    /// are NOT included in the merged output unless also listed as
    /// regular clips.
    pub reference_clip_indices: Vec<u32>,
}

impl Default for UpscaleConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            factor: 2,
            use_references: true,
            reference_clip_indices: Vec::new(),
        }
    }
}

/// A merge group = one tab. Ordered clips → one merged output.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct MergeGroup {
    /// Display name + default {groupname} token value.
    pub name: String,
    /// Ordered source clips (merge order = this order).
    pub clips: Vec<GroupClip>,
    /// Per-group encoder override. None = use global settings.
    pub settings_override: Option<EncoderSettings>,
    /// Resolution handling for mismatched clips.
    pub resolution_policy: ResolutionPolicy,
    /// Output filename template. Empty = use the global template.
    pub output_template: String,
    /// Per-group upscaling config. Off by default.
    pub upscale: UpscaleConfig,
}

impl Default for MergeGroup {
    fn default() -> Self {
        Self {
            name: "Group 1".to_string(),
            clips: Vec::new(),
            settings_override: None,
            resolution_policy: ResolutionPolicy::default(),
            output_template: String::new(),
            upscale: UpscaleConfig::default(),
        }
    }
}

impl MergeGroup {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), ..Default::default() }
    }
    
    /// Clips that are enabled (will be in the merge).
    pub fn active_clips(&self) -> impl Iterator<Item = &GroupClip> {
        self.clips.iter().filter(|c| c.enabled)
    }
    
    pub fn active_count(&self) -> usize {
        self.active_clips().count()
    }
    
    /// Total bytes of active clips (for savings projection).
    pub fn active_bytes(&self) -> u64 {
        self.active_clips().map(|c| c.bytes).sum()
    }
    
    /// Whether the group's active clips have mismatched resolutions. 
    /// Returns the set of distinct (w,h) among clips that have been 
    /// probed. The UI uses this to decide whether to show the mismatch 
    /// flag. Clips not yet probed (None dims) are ignored.
    pub fn distinct_resolutions(&self) -> Vec<(u32, u32)> {
        let mut seen: Vec<(u32, u32)> = Vec::new();
        for c in self.active_clips() {
            if let (Some(w), Some(h)) = (c.width, c.height) {
                if !seen.contains(&(w, h)) {
                    seen.push((w, h));
                }
            }
        }
        seen
    }
    
    pub fn has_resolution_mismatch(&self) -> bool {
        self.distinct_resolutions().len() > 1
    }
    
    /// Distinct framerates among probed active clips, rounded to 2 
    /// decimals to avoid float-equality noise (29.97 vs 29.970029).
    pub fn distinct_framerates(&self) -> Vec<f64> {
        let mut seen: Vec<f64> = Vec::new();
        for c in self.active_clips() {
            if let Some(fps) = c.fps {
                let rounded = (fps * 100.0).round() / 100.0;
                if !seen.iter().any(|x| (x - rounded).abs() < 0.005) {
                    seen.push(rounded);
                }
            }
        }
        seen
    }
    
    /// True if active clips have differing framerates. The resolution 
    /// policy does NOT fix this — stitching mixed-fps clips with stream 
    /// copy produces inconsistent timing. The UI flags it; the user 
    /// must decide (re-encode to common fps is a future option).
    pub fn has_framerate_mismatch(&self) -> bool {
        self.distinct_framerates().len() > 1
    }
    
    /// The target resolution implied by the policy, given the clips' 
    /// resolutions. None if no clips are probed yet or KeepNative.
    pub fn target_resolution(&self) -> Option<(u32, u32)> {
        let res = self.distinct_resolutions();
        if res.is_empty() {
            return None;
        }
        match self.resolution_policy {
            ResolutionPolicy::KeepNative => None,
            ResolutionPolicy::ScaleToSmallest => {
                // Smallest by pixel area.
                res.into_iter().min_by_key(|(w, h)| (*w as u64) * (*h as u64))
            }
            ResolutionPolicy::ScaleToLargest => {
                res.into_iter().max_by_key(|(w, h)| (*w as u64) * (*h as u64))
            }
        }
    }
}

/// Tokens available in output templates. Resolved against a context.
///
/// Note: `{savings}` is only meaningful AFTER encoding (we don't know 
/// the output size until then). When resolving a template pre-encode, 
/// `{savings}` resolves to a placeholder; a post-encode rename pass 
/// substitutes the real value if the template used it.
pub struct TemplateContext<'a> {
    pub groupname: &'a str,
    pub sourcename: &'a str,
    pub codec: &'a str,
    pub crf: u32,
    /// Local "default" directory (e.g. the first source's parent, or a 
    /// user-chosen base). Resolves {localdir}.
    pub localdir: &'a str,
    /// Savings percentage; None pre-encode → placeholder.
    pub savings_pct: Option<f64>,
}

/// Resolve a template string against a context. Unknown tokens are left 
/// as-is (so a typo'd token is visible rather than silently dropped).
pub fn resolve_template(template: &str, ctx: &TemplateContext) -> String {
    let now = current_datetime_parts();
    let mut out = template.to_string();
    out = out.replace("{groupname}", ctx.groupname);
    out = out.replace("{sourcename}", ctx.sourcename);
    out = out.replace("{codec}", ctx.codec);
    out = out.replace("{crf}", &ctx.crf.to_string());
    out = out.replace("{localdir}", ctx.localdir);
    out = out.replace("{date}", &now.0);
    out = out.replace("{time}", &now.1);
    let savings = match ctx.savings_pct {
        Some(p) => format!("{:.0}pct", p),
        None => "PENDING".to_string(),
    };
    out = out.replace("{savings}", &savings);
    out
}

/// Whether a template references {savings} (so the batch runner knows 
/// to do a post-encode rename).
pub fn template_uses_savings(template: &str) -> bool {
    template.contains("{savings}")
}

/// Default template if a group/global doesn't set one.
pub const DEFAULT_TEMPLATE: &str = "{localdir}/av1tuner/{groupname}.{codec}.mkv";

/// (date, time) as filename-safe strings: YYYY-MM-DD, HHMMSS. Uses 
/// only std — no chrono dependency. Derives from SystemTime via a 
/// minimal civil-date conversion.
fn current_datetime_parts() -> (String, String) {
    let now = core_kit::time::now_secs();
    let (y, mo, d, h, mi, s) = civil_from_unix(now as i64);
    (
        format!("{:04}-{:02}-{:02}", y, mo, d),
        format!("{:02}{:02}{:02}", h, mi, s),
    )
}

/// Convert unix seconds (UTC) to (year, month, day, hour, min, sec). 
/// Based on Howard Hinnant's days_from_civil algorithm, inverted. 
/// Avoids a chrono dependency for one timestamp string.
fn civil_from_unix(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let hour = (rem / 3600) as u32;
    let min = ((rem % 3600) / 60) as u32;
    let sec = (rem % 60) as u32;
    
    // days since 1970-01-01 → civil date
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;                       // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153;                       // [0, 11]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;    // [1, 31]
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    let year = if month <= 2 { y + 1 } else { y };
    
    (year, month, day, hour, min, sec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoder::EncoderKind;
    
    #[test]
    fn template_resolves_tokens() {
        let ctx = TemplateContext {
            groupname: "Gameplay",
            sourcename: "clip1",
            codec: "av1",
            crf: 30,
            localdir: "/home/me/videos",
            savings_pct: None,
        };
        let out = resolve_template("{localdir}/{groupname}.{codec}.mkv", &ctx);
        assert_eq!(out, "/home/me/videos/Gameplay.av1.mkv");
        
        let with_crf = resolve_template("{groupname}_crf{crf}", &ctx);
        assert_eq!(with_crf, "Gameplay_crf30");
    }
    
    #[test]
    fn savings_placeholder_pre_encode() {
        let ctx = TemplateContext {
            groupname: "G", sourcename: "s", codec: "av1", crf: 28,
            localdir: ".", savings_pct: None,
        };
        let out = resolve_template("{groupname}_{savings}", &ctx);
        assert_eq!(out, "G_PENDING");
        assert!(template_uses_savings("{groupname}_{savings}"));
        assert!(!template_uses_savings("{groupname}.mkv"));
    }
    
    #[test]
    fn savings_resolves_post_encode() {
        let ctx = TemplateContext {
            groupname: "G", sourcename: "s", codec: "av1", crf: 28,
            localdir: ".", savings_pct: Some(42.7),
        };
        let out = resolve_template("{groupname}_{savings}", &ctx);
        assert_eq!(out, "G_43pct");
    }
    
    #[test]
    fn unknown_tokens_preserved() {
        let ctx = TemplateContext {
            groupname: "G", sourcename: "s", codec: "av1", crf: 28,
            localdir: ".", savings_pct: None,
        };
        // A typo'd token stays visible rather than vanishing.
        let out = resolve_template("{groupname}_{bogus}", &ctx);
        assert_eq!(out, "G_{bogus}");
    }
    
    #[test]
    fn resolution_policy_picks_target() {
        let mut g = MergeGroup::new("test");
        let mut c1 = GroupClip::new(PathBuf::from("a.mp4"), 100);
        c1.width = Some(1920); c1.height = Some(1080);
        let mut c2 = GroupClip::new(PathBuf::from("b.mp4"), 200);
        c2.width = Some(1280); c2.height = Some(720);
        g.clips = vec![c1, c2];
        
        assert!(g.has_resolution_mismatch());
        
        g.resolution_policy = ResolutionPolicy::ScaleToSmallest;
        assert_eq!(g.target_resolution(), Some((1280, 720)));
        
        g.resolution_policy = ResolutionPolicy::ScaleToLargest;
        assert_eq!(g.target_resolution(), Some((1920, 1080)));
        
        g.resolution_policy = ResolutionPolicy::KeepNative;
        assert_eq!(g.target_resolution(), None);
    }
    
    #[test]
    fn framerate_mismatch_detection() {
        let mut g = MergeGroup::new("test");
        let mut c1 = GroupClip::new(PathBuf::from("a.mp4"), 100);
        c1.fps = Some(30.0);
        let mut c2 = GroupClip::new(PathBuf::from("b.mp4"), 200);
        c2.fps = Some(60.0);
        g.clips = vec![c1, c2];
        assert!(g.has_framerate_mismatch());
        
        // Same fps with float noise → no mismatch.
        g.clips[1].fps = Some(30.000029);
        assert!(!g.has_framerate_mismatch());
        
        // Unprobed clips (None fps) ignored.
        g.clips[1].fps = None;
        assert!(!g.has_framerate_mismatch());
    }
    
    #[test]
    fn civil_date_epoch() {
        // Unix epoch = 1970-01-01 00:00:00
        assert_eq!(civil_from_unix(0), (1970, 1, 1, 0, 0, 0));
        // 2021-01-01 00:00:00 UTC = 1609459200
        assert_eq!(civil_from_unix(1609459200), (2021, 1, 1, 0, 0, 0));
    }
    
    #[test]
    fn upscale_config_roundtrip() {
        let mut g = MergeGroup::new("test");
        g.upscale.enabled = true;
        g.upscale.factor = 2;
        g.upscale.reference_clip_indices = vec![3, 7];
        let json = serde_json::to_string(&g).unwrap();
        let back: MergeGroup = serde_json::from_str(&json).unwrap();
        assert!(back.upscale.enabled);
        assert_eq!(back.upscale.factor, 2);
        assert_eq!(back.upscale.reference_clip_indices, vec![3, 7]);
    }
    
    #[test]
    fn upscale_off_by_default() {
        let g = MergeGroup::new("test");
        assert!(!g.upscale.enabled);
    }
    
    #[test]
    fn override_settings_roundtrip() {
        let mut g = MergeGroup::new("test");
        let mut s = EncoderSettings::default();
        s.kind = EncoderKind::Vvenc;
        s.vvenc_qp = 40;
        g.settings_override = Some(s);
        let json = serde_json::to_string(&g).unwrap();
        let back: MergeGroup = serde_json::from_str(&json).unwrap();
        assert_eq!(back.settings_override.unwrap().vvenc_qp, 40);
    }
}
