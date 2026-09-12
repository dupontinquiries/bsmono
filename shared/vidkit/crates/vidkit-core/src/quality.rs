//! Quality as an intent, not a stream.
//!
//! "1080p60 VP9" is not a choice anyone wants to make. It depends on the
//! panel, the decoders present, the battery and what the site happens to
//! offer, and every one of those is already modelled — so asking the user is
//! asking them to do arithmetic the app has the inputs for.
//!
//! So the setting is an intent, and it works by adjusting the
//! [`PlaybackBudget`] the selector already reads. Nothing here picks a stream
//! or knows a codec: it moves the ceiling and lets `select` do what it does
//! everywhere else. That is the difference between a quality setting and a
//! second, competing selector.
//!
//! # Why a ceiling and not a target
//!
//! A preset that demanded 1080p would fail on a video that only has 720p, and
//! "Best" on a phone would pick 4K that the decoder cannot keep up with.
//! Everything here is an upper bound: the selector already prefers the largest
//! stream that fits, so lowering the bound is sufficient and cannot make a
//! video unplayable.

use crate::caps::PlaybackBudget;

/// What the viewer asked for, in their terms.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Quality {
    /// Let the device profile decide. The panel size, the decoders and the
    /// battery state are already known and are better inputs than a guess.
    #[default]
    Auto,
    /// Smallest watchable. For a metered connection.
    DataSaver,
    /// 720p ceiling.
    Low,
    /// 1080p ceiling.
    Medium,
    /// Whatever the panel and decoders allow.
    High,
}

impl Quality {
    pub const ALL: [Quality; 5] = [
        Quality::Auto,
        Quality::DataSaver,
        Quality::Low,
        Quality::Medium,
        Quality::High,
    ];

    /// The label, which names the effect rather than a number.
    ///
    /// "480p" means nothing on a phone held in portrait, and a menu of
    /// resolutions invites people to pick one the decoder cannot manage.
    pub fn label(self) -> &'static str {
        match self {
            Quality::Auto => "Automatic",
            Quality::DataSaver => "Data saver",
            Quality::Low => "Lower",
            Quality::Medium => "Standard",
            Quality::High => "Highest",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Quality::Auto => "match the screen and the battery",
            Quality::DataSaver => "smallest that is still watchable",
            Quality::Low => "up to 720p",
            Quality::Medium => "up to 1080p",
            Quality::High => "as much as this device can decode",
        }
    }

    /// The long-edge ceiling, or `None` to leave the device's own limit alone.
    pub fn max_long_edge(self) -> Option<u32> {
        match self {
            Quality::Auto | Quality::High => None,
            Quality::DataSaver => Some(640),
            Quality::Low => Some(1280),
            Quality::Medium => Some(1920),
        }
    }

    /// A bitrate ceiling for the presets where data is the point.
    ///
    /// Only Data saver sets one. A resolution ceiling already bounds the others
    /// well enough, and a bitrate cap that fights the resolution cap produces
    /// "no playable format" on videos that are simply well encoded.
    pub fn max_bitrate(self) -> Option<u64> {
        match self {
            Quality::DataSaver => Some(800_000),
            _ => None,
        }
    }
}

/// A frame-rate ceiling, separate from quality because the trade is different.
///
/// Halving the frame rate roughly halves decode work at the same sharpness,
/// which is a better deal on a phone than dropping resolution — text stays
/// readable. Anyone who has watched a 30 fps stream on a 120 Hz panel knows the
/// reverse trade is also real, so it is a separate control.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum FpsCap {
    #[default]
    Auto,
    Cap24,
    Cap30,
    Cap60,
}

impl FpsCap {
    pub const ALL: [FpsCap; 4] = [FpsCap::Auto, FpsCap::Cap24, FpsCap::Cap30, FpsCap::Cap60];

    pub fn label(self) -> &'static str {
        match self {
            FpsCap::Auto => "Automatic",
            FpsCap::Cap24 => "24 fps",
            FpsCap::Cap30 => "30 fps",
            FpsCap::Cap60 => "60 fps",
        }
    }

    pub fn hz(self) -> Option<u32> {
        match self {
            FpsCap::Auto => None,
            FpsCap::Cap24 => Some(24),
            FpsCap::Cap30 => Some(30),
            FpsCap::Cap60 => Some(60),
        }
    }
}

/// Apply an intent to a budget.
///
/// Only ever LOWERS a ceiling. Raising one would let a preset promise something
/// the panel or the decoders cannot deliver, and "Highest" would then mean
/// "pick a 4K stream and drop half its frames".
pub fn apply(budget: &PlaybackBudget, quality: Quality, fps: FpsCap) -> PlaybackBudget {
    let mut b = budget.clone();

    if let Some(edge) = quality.max_long_edge() {
        let long = b.display_width.max(b.display_height);
        let short = b.display_width.min(b.display_height);
        if edge < long {
            // Scaled, not clamped on one axis: setting the long edge alone
            // leaves the short one at panel size and the budget then describes
            // a shape no video has.
            let scaled_short = ((short as u64 * edge as u64) / long.max(1) as u64) as u32;
            if b.display_width >= b.display_height {
                b.display_width = edge;
                b.display_height = scaled_short.max(1);
            } else {
                b.display_height = edge;
                b.display_width = scaled_short.max(1);
            }
        }
        // The battery ceiling must come down with it, or switching to Data
        // saver on battery changes nothing because the battery cap was already
        // the binding one.
        b.battery_max_long_edge = b.battery_max_long_edge.min(edge);
    }

    if let Some(hz) = fps.hz() {
        b.display_max_hz = b.display_max_hz.min(hz);
        b.battery_max_hz = b.battery_max_hz.min(hz);
    }

    if let Some(bps) = quality.max_bitrate() {
        b.max_bitrate = Some(match b.max_bitrate {
            Some(existing) => existing.min(bps),
            None => bps,
        });
    }

    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::{desktop_full_hw, fairphone_venus};

    fn budget() -> PlaybackBudget {
        desktop_full_hw().budget
    }

    #[test]
    fn auto_and_high_leave_the_device_alone() {
        let b = budget();
        for q in [Quality::Auto, Quality::High] {
            assert_eq!(apply(&b, q, FpsCap::Auto), b, "{q:?} changed the budget");
        }
    }

    #[test]
    fn each_preset_lowers_the_ceiling_further_than_the_last() {
        let b = budget();
        let edge = |q: Quality| {
            let r = apply(&b, q, FpsCap::Auto);
            r.display_width.max(r.display_height)
        };
        assert!(edge(Quality::DataSaver) < edge(Quality::Low));
        assert!(edge(Quality::Low) < edge(Quality::Medium));
        assert!(edge(Quality::Medium) <= edge(Quality::High));
    }

    /// A ceiling is never a floor. Raising one lets a preset promise what the
    /// panel cannot deliver.
    #[test]
    fn a_preset_never_raises_a_ceiling() {
        // A small panel: "Highest" and "Standard" must not enlarge it.
        let small = PlaybackBudget {
            display_width: 800,
            display_height: 480,
            ..budget()
        };
        for q in Quality::ALL {
            let r = apply(&small, q, FpsCap::Auto);
            assert!(
                r.display_width <= 800 && r.display_height <= 480,
                "{q:?} grew an 800x480 panel to {}x{}",
                r.display_width,
                r.display_height
            );
        }
    }

    /// Clamping one axis leaves the budget describing a shape no video has.
    #[test]
    fn the_aspect_ratio_survives_a_lowered_ceiling() {
        let b = PlaybackBudget {
            display_width: 3840,
            display_height: 2160,
            ..budget()
        };
        let r = apply(&b, Quality::Low, FpsCap::Auto);
        assert_eq!(r.display_width, 1280);
        assert_eq!(r.display_height, 720, "aspect ratio was not preserved");
    }

    /// A portrait panel is the case that catches an implementation that
    /// assumed width is the long edge.
    #[test]
    fn a_portrait_panel_is_scaled_on_its_own_long_edge() {
        let b = PlaybackBudget {
            display_width: 1080,
            display_height: 2400,
            ..budget()
        };
        let r = apply(&b, Quality::Low, FpsCap::Auto);
        assert_eq!(r.display_height, 1280, "the long edge was not the height");
        assert!(r.display_width < r.display_height, "orientation flipped");
    }

    /// Switching to Data saver on battery must actually do something. If only
    /// the display ceiling moved, the battery cap would still be the binding
    /// one and the setting would appear to have no effect.
    #[test]
    fn data_saver_lowers_the_battery_ceiling_too() {
        let b = fairphone_venus().budget;
        let before = b.battery_max_long_edge;
        let after = apply(&b, Quality::DataSaver, FpsCap::Auto).battery_max_long_edge;
        assert!(after < before, "battery ceiling unchanged: {before} -> {after}");
        assert_eq!(after, 640);
    }

    #[test]
    fn an_fps_cap_lowers_both_the_display_and_the_battery_rate() {
        let b = budget();
        let r = apply(&b, Quality::Auto, FpsCap::Cap30);
        assert_eq!(r.display_max_hz, 30);
        assert!(r.battery_max_hz <= 30);
    }

    /// A cap above what the panel does is not an upgrade.
    #[test]
    fn an_fps_cap_above_the_panel_changes_nothing() {
        let b = PlaybackBudget {
            display_max_hz: 30,
            ..budget()
        };
        assert_eq!(apply(&b, Quality::Auto, FpsCap::Cap60).display_max_hz, 30);
    }

    /// Only Data saver sets a bitrate cap: one that fights the resolution cap
    /// produces "no playable format" on videos that are merely well encoded.
    #[test]
    fn only_data_saver_caps_the_bitrate() {
        let b = budget();
        for q in Quality::ALL {
            let r = apply(&b, q, FpsCap::Auto);
            if q == Quality::DataSaver {
                assert!(r.max_bitrate.is_some());
            } else {
                assert_eq!(r.max_bitrate, b.max_bitrate, "{q:?} capped the bitrate");
            }
        }
    }

    /// An existing device cap must win over the preset's, or a preset can
    /// raise a limit the device set for a reason.
    #[test]
    fn a_tighter_existing_bitrate_cap_is_kept() {
        let b = PlaybackBudget {
            max_bitrate: Some(300_000),
            ..budget()
        };
        assert_eq!(
            apply(&b, Quality::DataSaver, FpsCap::Auto).max_bitrate,
            Some(300_000)
        );
    }

    #[test]
    fn every_option_has_a_label_and_a_hint() {
        for q in Quality::ALL {
            assert!(!q.label().is_empty() && !q.hint().is_empty());
        }
        for f in FpsCap::ALL {
            assert!(!f.label().is_empty());
        }
    }
}
