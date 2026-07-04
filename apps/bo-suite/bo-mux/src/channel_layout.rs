// src/channel_layout.rs
//
// Speaker positions, channel layouts, and routing strategies.
//
// A soundscape's `Layer::channel` field used to be a free-form string
// (e.g. "FL", "vocals"). With multi-channel support we standardise on
// well-known channel labels AND add a router that maps logical layer
// channels to physical device channels under any target layout.
//
// Layouts supported:
//   * Stereo    — FL, FR
//   * Quad      — FL, FR, RL, RR        (4-channel surround, no center/LFE)
//   * FivePointOne  — FL, FR, C, LFE, RL, RR
//   * SevenPointOne — FL, FR, C, LFE, RL, RR, SL, SR
//
// The router knows how to:
//   * Pin a layer to a specific channel ("vocals → C").
//   * Spread a layer across pairs ("ambient pad → FL+FR equal-gain").
//   * Diffuse with phase variation to fill the rear field
//     ("wind → RL/RR independent with slight pan jitter").
//   * Downmix when a soundscape targets more channels than the
//     hardware exposes (5.1 scene → stereo output via standard ITU
//     downmix coefficients).
//
// All logic is pure (no audio I/O). The transport layer uses the
// resolved routing to assign physical speakers in the mesh.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Standard channel positions. The enum is exhaustive across all
/// layouts we support; per-layout layouts use a subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Channel {
    /// Front-left
    FL,
    /// Front-right
    FR,
    /// Front-center (dialog, lead vocal)
    C,
    /// Low-frequency effects (subwoofer)
    LFE,
    /// Rear-left (back surround)
    RL,
    /// Rear-right
    RR,
    /// Side-left (7.1 only)
    SL,
    /// Side-right (7.1 only)
    SR,
}

impl Channel {
    /// String key matching `Layer.channel` and `forest.soundscape.json`
    /// usage. Returns the canonical name (uppercase, hyphen-free).
    pub fn as_str(&self) -> &'static str {
        match self {
            Channel::FL => "FL",
            Channel::FR => "FR",
            Channel::C  => "C",
            Channel::LFE => "LFE",
            Channel::RL => "RL",
            Channel::RR => "RR",
            Channel::SL => "SL",
            Channel::SR => "SR",
        }
    }

    /// Parse a channel from a string. Accepts canonical names plus
    /// common variants ("center" → C, "sub" → LFE, "rear-left" → RL).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().replace('-', "").replace('_', "").as_str() {
            "FL" | "FRONTLEFT" | "L" | "LEFT" => Some(Channel::FL),
            "FR" | "FRONTRIGHT" | "R" | "RIGHT" => Some(Channel::FR),
            "C" | "CENTER" | "CENTRE" => Some(Channel::C),
            "LFE" | "SUB" | "SUBWOOFER" => Some(Channel::LFE),
            "RL" | "REARLEFT" | "BL" | "BACKLEFT" | "SURROUNDLEFT" => Some(Channel::RL),
            "RR" | "REARRIGHT" | "BR" | "BACKRIGHT" | "SURROUNDRIGHT" => Some(Channel::RR),
            "SL" | "SIDELEFT" => Some(Channel::SL),
            "SR" | "SIDERIGHT" => Some(Channel::SR),
            _ => None,
        }
    }
}

/// Speaker layout — the physical configuration the soundscape targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Layout {
    Stereo,
    Quad,
    FivePointOne,
    SevenPointOne,
}

impl Layout {
    /// Channels in this layout in canonical order.
    pub fn channels(&self) -> Vec<Channel> {
        match self {
            Layout::Stereo => vec![Channel::FL, Channel::FR],
            Layout::Quad   => vec![Channel::FL, Channel::FR, Channel::RL, Channel::RR],
            Layout::FivePointOne => vec![
                Channel::FL, Channel::FR, Channel::C, Channel::LFE,
                Channel::RL, Channel::RR,
            ],
            Layout::SevenPointOne => vec![
                Channel::FL, Channel::FR, Channel::C, Channel::LFE,
                Channel::RL, Channel::RR, Channel::SL, Channel::SR,
            ],
        }
    }

    /// Does this layout contain `ch`?
    pub fn contains(&self, ch: Channel) -> bool {
        self.channels().iter().any(|&c| c == ch)
    }

    /// Friendly name (e.g. for UI display).
    pub fn label(&self) -> &'static str {
        match self {
            Layout::Stereo => "Stereo",
            Layout::Quad   => "Quadraphonic (4)",
            Layout::FivePointOne => "5.1 Surround",
            Layout::SevenPointOne => "7.1 Surround",
        }
    }

    /// Parse from a config string (case-insensitive).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "stereo" | "2.0" | "2" => Some(Layout::Stereo),
            "quad" | "4.0" | "4"   => Some(Layout::Quad),
            "5.1" | "51" | "5_1"   => Some(Layout::FivePointOne),
            "7.1" | "71" | "7_1"   => Some(Layout::SevenPointOne),
            _ => None,
        }
    }
}

/// How a single source signal lands on physical channels. The
/// "strategy" determines how the (potentially mono or stereo) source
/// gets spread across the target layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RouteStrategy {
    /// Pin to a single physical channel at the given gain.
    Pin { channel: Channel, gain: f32 },
    /// Spread across a pair, e.g. front (FL+FR) for ambient sources.
    /// `gain_per_channel` is applied to each.
    Pair { a: Channel, b: Channel, gain_per_channel: f32 },
    /// Diffuse: spread across MANY channels, each with optional gain.
    /// Useful for "rain everywhere", "wind in all surrounds".
    Diffuse { channels: Vec<(Channel, f32)> },
    /// Downmix-aware: prefer the listed channels in priority order, fall
    /// back to standard downmix coefficients when none are available.
    /// Allows a 5.1-authored scene to play on stereo speakers.
    Auto { prefer: Vec<Channel> },
}

/// A resolved channel assignment: which physical channels receive
/// what gain. The transport layer consumes this directly.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedRouting {
    /// Per-channel gain. Channels not in the layout are dropped.
    pub channel_gains: HashMap<Channel, f32>,
}

impl ResolvedRouting {
    pub fn empty() -> Self { Self { channel_gains: HashMap::new() } }
    pub fn has(&self, ch: Channel) -> bool { self.channel_gains.contains_key(&ch) }
    pub fn gain(&self, ch: Channel) -> f32 {
        *self.channel_gains.get(&ch).unwrap_or(&0.0)
    }
    /// Total energy across all assigned channels (sum of gains squared).
    /// Useful for normalising power across layers with different fanout.
    pub fn power(&self) -> f32 {
        self.channel_gains.values().map(|g| g * g).sum()
    }
}

/// Resolve a strategy into actual per-channel gains under `layout`.
/// Channels not in the layout are dropped (Pin to LFE under Stereo
/// becomes silent). For `Auto`, we walk `prefer` in order and use
/// the first channel that exists; if none do, we standard-downmix
/// the prefer list onto front L/R.
pub fn resolve(strategy: &RouteStrategy, layout: Layout) -> ResolvedRouting {
    let mut gains: HashMap<Channel, f32> = HashMap::new();
    match strategy {
        RouteStrategy::Pin { channel, gain } => {
            if layout.contains(*channel) {
                gains.insert(*channel, *gain);
            } else {
                // The pinned channel doesn't exist in this layout — the
                // classic case is a layer pinned to C (center) or LFE on
                // a plain Stereo system, which is the most common setup.
                // Without a fallback the layer would be SILENT (the
                // "synth pad / vocals plays nothing on stereo" bug).
                // Fold the pinned channel into the front pair using the
                // standard downmix coefficient so it's still audible and
                // centered.
                //
                //   C   → FL+FR at -3 dB (phantom center)
                //   LFE → FL+FR at -3 dB (no sub: better than nothing)
                //   any other absent channel → same front fold
                if layout.contains(Channel::FL) && layout.contains(Channel::FR) {
                    let g = gain * 0.707_106_77; // -3 dB per side
                    gains.insert(Channel::FL, g);
                    gains.insert(Channel::FR, g);
                } else if let Some(first) = layout.channels().first() {
                    // Degenerate layout (e.g. a single physical channel):
                    // put it wherever we can rather than dropping it.
                    gains.insert(*first, *gain);
                }
            }
        }
        RouteStrategy::Pair { a, b, gain_per_channel } => {
            if layout.contains(*a) { gains.insert(*a, *gain_per_channel); }
            if layout.contains(*b) { gains.insert(*b, *gain_per_channel); }
            // If neither was present (e.g. RL/RR under Stereo) fall
            // back to standard downmix onto FL/FR at -3dB.
            if gains.is_empty() && layout.contains(Channel::FL) && layout.contains(Channel::FR) {
                let g = gain_per_channel * 0.707_106_77;  // -3dB
                gains.insert(Channel::FL, g);
                gains.insert(Channel::FR, g);
            }
        }
        RouteStrategy::Diffuse { channels } => {
            for (ch, g) in channels {
                if layout.contains(*ch) { gains.insert(*ch, *g); }
            }
            // If none of the listed channels are present, downmix
            // onto FL+FR with a power-preserving sum.
            if gains.is_empty() && layout.contains(Channel::FL) && layout.contains(Channel::FR) {
                let total_power: f32 = channels.iter().map(|(_, g)| g * g).sum();
                let downmix_gain = total_power.sqrt() * 0.707_106_77;
                gains.insert(Channel::FL, downmix_gain);
                gains.insert(Channel::FR, downmix_gain);
            }
        }
        RouteStrategy::Auto { prefer } => {
            // Use the first preferred channel that exists in the layout.
            // If multiple preferred channels exist, use them all with
            // equal gain (this is the "best fit" behaviour for an
            // ambient layer that doesn't care which surrounds it gets).
            let present: Vec<Channel> = prefer.iter()
                .copied()
                .filter(|c| layout.contains(*c))
                .collect();
            if !present.is_empty() {
                let g = 1.0 / (present.len() as f32).sqrt();  // power-preserving
                for ch in present {
                    gains.insert(ch, g);
                }
            } else if layout.contains(Channel::FL) && layout.contains(Channel::FR) {
                // None of the preferred channels exist. Standard ITU
                // downmix: average all preferences onto front L+R.
                gains.insert(Channel::FL, 0.707_106_77);
                gains.insert(Channel::FR, 0.707_106_77);
            }
        }
    }
    ResolvedRouting { channel_gains: gains }
}

/// Convenience: parse the legacy free-form `Layer.channel` string into
/// a RouteStrategy. Returns Pin when a single channel is named, Auto
/// otherwise. This keeps existing scenes loadable while encouraging
/// new ones to use the structured form.
pub fn legacy_channel_to_strategy(s: &str) -> RouteStrategy {
    if let Some(ch) = Channel::parse(s) {
        return RouteStrategy::Pin { channel: ch, gain: 1.0 };
    }
    // Recognised semantic names:
    let lower = s.trim().to_ascii_lowercase();
    match lower.as_str() {
        "vocals" | "voice" | "lead"   => RouteStrategy::Pin { channel: Channel::C,   gain: 1.0 },
        "sub"    | "bass"  | "lfe"    => RouteStrategy::Pin { channel: Channel::LFE, gain: 1.0 },
        "rear"   | "surround" => RouteStrategy::Pair {
            a: Channel::RL, b: Channel::RR, gain_per_channel: 0.707
        },
        "front" => RouteStrategy::Pair {
            a: Channel::FL, b: Channel::FR, gain_per_channel: 0.707
        },
        // Truly ambient — diffuse across the rear surround pair when
        // available, falling back to front pair via Auto.
        _ => RouteStrategy::Auto { prefer: vec![Channel::RL, Channel::RR, Channel::FL, Channel::FR] },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_parse_roundtrip() {
        for ch in [Channel::FL, Channel::FR, Channel::C, Channel::LFE,
                   Channel::RL, Channel::RR, Channel::SL, Channel::SR] {
            assert_eq!(Channel::parse(ch.as_str()), Some(ch));
        }
    }

    #[test]
    fn channel_parse_aliases() {
        assert_eq!(Channel::parse("center"), Some(Channel::C));
        assert_eq!(Channel::parse("CENTRE"), Some(Channel::C));
        assert_eq!(Channel::parse("rear-left"), Some(Channel::RL));
        assert_eq!(Channel::parse("Sub"), Some(Channel::LFE));
    }

    #[test]
    fn channel_parse_rejects_garbage() {
        assert_eq!(Channel::parse("vocals"), None);
        assert_eq!(Channel::parse(""), None);
        assert_eq!(Channel::parse("XYZ"), None);
    }

    #[test]
    fn layout_channels_counts() {
        assert_eq!(Layout::Stereo.channels().len(), 2);
        assert_eq!(Layout::Quad.channels().len(), 4);
        assert_eq!(Layout::FivePointOne.channels().len(), 6);
        assert_eq!(Layout::SevenPointOne.channels().len(), 8);
    }

    #[test]
    fn layout_contains() {
        assert!(!Layout::Stereo.contains(Channel::LFE));
        assert!(!Layout::Stereo.contains(Channel::RL));
        assert!( Layout::Quad.contains(Channel::RL));
        assert!(!Layout::Quad.contains(Channel::C));
        assert!( Layout::FivePointOne.contains(Channel::LFE));
        assert!( Layout::SevenPointOne.contains(Channel::SL));
    }

    #[test]
    fn layout_parse() {
        assert_eq!(Layout::parse("stereo"), Some(Layout::Stereo));
        assert_eq!(Layout::parse("4.0"),    Some(Layout::Quad));
        assert_eq!(Layout::parse("5.1"),    Some(Layout::FivePointOne));
        assert_eq!(Layout::parse("7_1"),    Some(Layout::SevenPointOne));
    }

    #[test]
    fn resolve_pin_present_channel() {
        let r = resolve(&RouteStrategy::Pin { channel: Channel::C, gain: 0.8 },
                        Layout::FivePointOne);
        assert_eq!(r.gain(Channel::C), 0.8);
        assert_eq!(r.gain(Channel::FL), 0.0);
    }

    #[test]
    fn resolve_pin_absent_downmixes_to_front() {
        // Center has no dedicated speaker in stereo, so a layer pinned
        // to C must fold into the front pair (phantom center) rather
        // than going silent. This is the fix for "synth pad / vocals
        // plays nothing on a stereo system".
        let r = resolve(&RouteStrategy::Pin { channel: Channel::C, gain: 1.0 },
                        Layout::Stereo);
        assert!(!r.channel_gains.is_empty(), "C must not be dropped on stereo");
        assert!(r.has(Channel::FL) && r.has(Channel::FR));
        // -3 dB per side preserves perceived loudness for a centered source.
        let g = r.gain(Channel::FL);
        assert!((g - 0.707_106_77).abs() < 1e-5, "expected -3dB fold, got {}", g);
        assert!((r.gain(Channel::FR) - g).abs() < 1e-6, "front pair symmetric");
    }

    #[test]
    fn resolve_pin_lfe_absent_downmixes_to_front() {
        // An LFE-pinned layer on stereo should also fold forward rather
        // than vanish (no sub is better than no sound).
        let r = resolve(&RouteStrategy::Pin { channel: Channel::LFE, gain: 0.8 },
                        Layout::Stereo);
        assert!(r.has(Channel::FL) && r.has(Channel::FR));
        assert!((r.gain(Channel::FL) - 0.8 * 0.707_106_77).abs() < 1e-5);
    }

    #[test]
    fn resolve_pair_present() {
        let r = resolve(&RouteStrategy::Pair {
            a: Channel::RL, b: Channel::RR, gain_per_channel: 0.6
        }, Layout::Quad);
        assert_eq!(r.gain(Channel::RL), 0.6);
        assert_eq!(r.gain(Channel::RR), 0.6);
    }

    #[test]
    fn resolve_pair_downmixes_when_absent() {
        // RL/RR don't exist in stereo → downmix to front pair at -3dB.
        let r = resolve(&RouteStrategy::Pair {
            a: Channel::RL, b: Channel::RR, gain_per_channel: 1.0
        }, Layout::Stereo);
        let g = r.gain(Channel::FL);
        assert!(g > 0.65 && g < 0.75, "expected ~0.707, got {}", g);
        assert_eq!(r.gain(Channel::FR), r.gain(Channel::FL));
    }

    #[test]
    fn resolve_diffuse_uses_present_channels() {
        let r = resolve(&RouteStrategy::Diffuse {
            channels: vec![(Channel::FL, 0.5), (Channel::FR, 0.5),
                           (Channel::RL, 0.3), (Channel::RR, 0.3)]
        }, Layout::Quad);
        assert_eq!(r.gain(Channel::FL), 0.5);
        assert_eq!(r.gain(Channel::RL), 0.3);
        assert_eq!(r.gain(Channel::C), 0.0);
    }

    #[test]
    fn resolve_diffuse_downmixes_when_layout_too_small() {
        let r = resolve(&RouteStrategy::Diffuse {
            channels: vec![(Channel::SL, 0.5), (Channel::SR, 0.5)]
        }, Layout::Stereo);
        // SL/SR don't exist → both should downmix to FL+FR.
        assert!(r.gain(Channel::FL) > 0.0);
        assert!(r.gain(Channel::FR) > 0.0);
    }

    #[test]
    fn resolve_auto_uses_first_preferred() {
        // Auto with preference [C, FL] under stereo → only FL available.
        let r = resolve(&RouteStrategy::Auto {
            prefer: vec![Channel::C, Channel::FL]
        }, Layout::Stereo);
        assert!(r.gain(Channel::FL) > 0.0);
        assert_eq!(r.gain(Channel::C), 0.0);
    }

    #[test]
    fn resolve_auto_uses_multiple_present() {
        let r = resolve(&RouteStrategy::Auto {
            prefer: vec![Channel::RL, Channel::RR]
        }, Layout::Quad);
        assert!(r.gain(Channel::RL) > 0.0);
        assert!(r.gain(Channel::RR) > 0.0);
    }

    #[test]
    fn resolve_auto_falls_back_to_downmix() {
        // None of the preferred channels exist (LFE+C+SL+SR in stereo)
        // → standard downmix onto FL+FR.
        let r = resolve(&RouteStrategy::Auto {
            prefer: vec![Channel::C, Channel::LFE, Channel::SL]
        }, Layout::Stereo);
        assert!(r.gain(Channel::FL) > 0.0);
        assert!(r.gain(Channel::FR) > 0.0);
    }

    #[test]
    fn legacy_channel_pin() {
        match legacy_channel_to_strategy("FL") {
            RouteStrategy::Pin { channel: Channel::FL, .. } => {},
            other => panic!("expected Pin(FL), got {:?}", other),
        }
    }

    #[test]
    fn legacy_channel_vocals_to_center() {
        match legacy_channel_to_strategy("vocals") {
            RouteStrategy::Pin { channel: Channel::C, .. } => {},
            other => panic!("expected Pin(C), got {:?}", other),
        }
    }

    #[test]
    fn legacy_channel_unknown_falls_to_auto() {
        match legacy_channel_to_strategy("ambient_birds") {
            RouteStrategy::Auto { .. } => {},
            other => panic!("expected Auto, got {:?}", other),
        }
    }

    #[test]
    fn legacy_channel_rear_pair() {
        match legacy_channel_to_strategy("rear") {
            RouteStrategy::Pair { a: Channel::RL, b: Channel::RR, .. } => {},
            other => panic!("expected Pair(RL,RR), got {:?}", other),
        }
    }

    #[test]
    fn resolved_routing_power_calculation() {
        let mut r = ResolvedRouting::empty();
        r.channel_gains.insert(Channel::FL, 0.5);
        r.channel_gains.insert(Channel::FR, 0.5);
        let p = r.power();
        assert!((p - 0.5).abs() < 1e-6, "power should be 0.5, got {}", p);
    }
}

// ── audiomix-kit bridge: delegate channel semantics + spatial panning ────────
// bo-suite's routing was the origin of audiomix-kit; now it delegates to the shared kit so the
// same panning drives voiceterm/snapclone/ambient-learner — and so a logscope "mind palace"
// (spatial item positions) can place sources in a soundscape via audiomix_kit::soundscape.

impl Channel {
    /// Map to the shared-kit channel.
    pub fn to_mix(self) -> audiomix_kit::Channel {
        use audiomix_kit::Channel as M;
        match self {
            Channel::FL => M::FL,
            Channel::FR => M::FR,
            Channel::C => M::C,
            Channel::LFE => M::LFE,
            Channel::RL => M::RL,
            Channel::RR => M::RR,
            Channel::SL => M::SL,
            Channel::SR => M::SR,
        }
    }
    /// Map from the shared-kit channel.
    pub fn from_mix(c: audiomix_kit::Channel) -> Channel {
        use audiomix_kit::Channel as M;
        match c {
            M::FL => Channel::FL,
            M::FR => Channel::FR,
            M::C => Channel::C,
            M::LFE => Channel::LFE,
            M::RL => Channel::RL,
            M::RR => Channel::RR,
            M::SL => Channel::SL,
            M::SR => Channel::SR,
        }
    }
}

impl Layout {
    /// Map to the shared-kit layout.
    pub fn to_mix(self) -> audiomix_kit::Layout {
        match self {
            Layout::Stereo => audiomix_kit::Layout::Stereo,
            Layout::Quad => audiomix_kit::Layout::Quad,
            Layout::FivePointOne => audiomix_kit::Layout::FivePointOne,
            Layout::SevenPointOne => audiomix_kit::Layout::SevenPointOne,
        }
    }
}

impl ResolvedRouting {
    /// Spatialize a mono source at `(x, y)` (x: 0=left…1=right, y: 0=front…1=rear) into
    /// `layout`, delegating the equal-power pan to audiomix-kit. This is how a spatial layout in
    /// another app (e.g. a logscope mind-palace item's position) becomes a positioned source in
    /// the soundscape.
    pub fn from_pan(x: f32, y: f32, layout: Layout) -> ResolvedRouting {
        let channel_gains = audiomix_kit::pan_gains(x, y, layout.to_mix())
            .into_iter()
            .map(|(c, g)| (Channel::from_mix(c), g))
            .collect();
        ResolvedRouting { channel_gains }
    }
}

#[cfg(test)]
mod audiomix_bridge_tests {
    use super::*;

    #[test]
    fn from_pan_delegates_to_audiomix() {
        // hard-left/front source → FL dominant, FR silent
        let r = ResolvedRouting::from_pan(0.0, 0.0, Layout::Quad);
        assert!(r.gain(Channel::FL) > 0.9);
        assert!(r.gain(Channel::FR) < 1e-3);
        // channel conversion round-trips
        assert_eq!(Channel::from_mix(Channel::RR.to_mix()), Channel::RR);
    }
}
