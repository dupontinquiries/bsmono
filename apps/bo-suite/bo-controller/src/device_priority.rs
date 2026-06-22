// src/device_priority.rs
//
// Decide which devices on the LAN are best suited to act as the system's
// control hub. The B&O fleet is heterogeneous: some devices are wall-
// powered and always-online (Stage, A9), some are portable with battery
// (Level, A5, Emerge, H100/HX), some are Bluetooth-only (the earbuds).
// A premium multi-room experience picks the right hub automatically and
// shifts gracefully as conditions change.
//
// The priority calculation combines two layers:
//
//   1. INTRINSIC TRAITS — known from the product type alone. These never
//      change for a given device: a Stage will always be wall-powered
//      and always have Ethernet capability; an H100 will never have a
//      network interface. Encoded in DeviceTraits below, derived from
//      the product_id.
//
//   2. LIVE STATE — queried over the air via the device's ASE/BeoZone
//      API: battery percentage and charging state, whether the device
//      is currently on Wi-Fi or Ethernet, whether Bluetooth is enabled.
//      Encoded in DeviceState.
//
// Combined into DeviceContext, scored, and bucketed into a PriorityGroup
// that the UI surfaces as a badge ("PRIMARY HUB", "SECONDARY", etc).
//
// The scoring is deliberately simple and explainable: each factor adds a
// fixed weight to the total. Tests below codify the orderings we promise
// (wall-power > battery; Ethernet > Wi-Fi > Bluetooth; high battery >
// low battery; stationary > portable). When the user reports surprises,
// the test failures will point straight at the broken invariant.

/// Intrinsic traits derived from product type. Constant per model —
/// doesn't change with state. The controller maintains a table by product_id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceTraits {
    /// True for devices that are typically wall-powered and stationary
    /// (Stage, A9, TVs). False for handhelds / portables / headphones.
    pub stationary: bool,
    /// Wired Ethernet jack on the device.
    pub has_ethernet: bool,
    /// Wi-Fi radio.
    pub has_wifi: bool,
    /// Bluetooth radio (almost everything has this).
    pub has_bluetooth: bool,
    /// Can act as a Beolink / network-audio source for other devices.
    /// Most full-size B&O products can; earbuds and small portables can't.
    pub can_lead_network_audio: bool,
}

impl DeviceTraits {
    /// Look up traits for a known product_id. Returns sensible defaults
    /// for unknown products (treated as a generic Wi-Fi speaker).
    pub fn for_product(product_id: &str) -> Self {
        match product_id {
            // Big stationary speakers
            "BeosoundStage" | "BeovisionEclipse" | "BeovisionContour" => Self {
                stationary: true, has_ethernet: true, has_wifi: true,
                has_bluetooth: true, can_lead_network_audio: true,
            },
            "BeosoundA9" | "BeoplayA9" => Self {
                stationary: true, has_ethernet: true, has_wifi: true,
                has_bluetooth: true, can_lead_network_audio: true,
            },
            "BeosoundBalance" => Self {
                stationary: true, has_ethernet: true, has_wifi: true,
                has_bluetooth: true, can_lead_network_audio: true,
            },
            // Portable hi-fi
            "BeosoundLevel" => Self {
                stationary: false, has_ethernet: false, has_wifi: true,
                has_bluetooth: true, can_lead_network_audio: true,
            },
            "BeosoundA5" => Self {
                stationary: false, has_ethernet: false, has_wifi: true,
                has_bluetooth: true, can_lead_network_audio: true,
            },
            "BeosoundEmerge" => Self {
                stationary: false, has_ethernet: false, has_wifi: true,
                has_bluetooth: true, can_lead_network_audio: true,
            },
            // Headphones — Bluetooth only, can't lead.
            "BeoplayH100" | "BeoplayHX" => Self {
                stationary: false, has_ethernet: false, has_wifi: false,
                has_bluetooth: true, can_lead_network_audio: false,
            },
            // Unknown — assume a generic wired-or-wifi speaker.
            _ => Self {
                stationary: true, has_ethernet: false, has_wifi: true,
                has_bluetooth: true, can_lead_network_audio: false,
            },
        }
    }
}

/// Live state queried over the air. Fields are Option so that a device we
/// haven't probed yet (or that doesn't expose a particular endpoint)
/// doesn't get penalized — we just don't add that factor to the score.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviceState {
    /// True if the device responded recently (≤ ~5 s).
    pub reachable: bool,
    /// True if device reports running on internal battery (not AC).
    /// None when the device has no battery or didn't tell us.
    pub on_battery: Option<bool>,
    /// 0..100, when known.
    pub battery_percent: Option<u8>,
    /// True if currently charging (on battery AND plugged in).
    pub charging: Option<bool>,
    /// True if currently using Ethernet for network audio.
    pub on_ethernet: Option<bool>,
    /// True if currently using Wi-Fi.
    pub on_wifi: Option<bool>,
    /// True if Bluetooth is enabled and accepting connections.
    pub bluetooth_active: Option<bool>,
}

/// Traits + live state for one device. Score is a function of this whole
/// struct; the UI presents the bucketed PriorityGroup and details.
#[derive(Debug, Clone)]
pub struct DeviceContext {
    pub product_id: String,
    pub name: String,
    pub url: String,
    pub traits: DeviceTraits,
    pub state: DeviceState,
}

/// Bucketed priority. The UI badges devices with these labels and sorts
/// the picker top-to-bottom in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PriorityGroup {
    /// Best-available hub — wall-powered, wired, can lead. Top of the picker.
    PrimaryHub = 0,
    /// Capable hub but at a disadvantage (Wi-Fi instead of Ethernet, or
    /// portable on AC). Used when no PrimaryHub is online.
    SecondaryHub = 1,
    /// Not a hub candidate — should follow a chosen hub. Portables on
    /// battery, devices that can't lead.
    Satellite = 2,
    /// Listener-only — Bluetooth headphones / earbuds. Can play, can't
    /// orchestrate.
    Listener = 3,
}

impl PriorityGroup {
    pub fn label(&self) -> &'static str {
        match self {
            PriorityGroup::PrimaryHub   => "PRIMARY HUB",
            PriorityGroup::SecondaryHub => "SECONDARY",
            PriorityGroup::Satellite    => "SATELLITE",
            PriorityGroup::Listener     => "LISTENER",
        }
    }

    /// Hex color used for the badge in the device picker. Pulled from the
    /// product accent family so the badges feel native rather than
    /// pasted-on.
    pub fn badge_color(&self) -> &'static str {
        match self {
            PriorityGroup::PrimaryHub   => "#C8A04B",  // warm bronze
            PriorityGroup::SecondaryHub => "#8A7A55",  // muted bronze
            PriorityGroup::Satellite    => "#5A5448",  // silver-shadow
            PriorityGroup::Listener     => "#3F3B33",  // graphite-warm
        }
    }
}

/// Aggregate scoring weights. Kept as constants so the test suite can
/// reason about boundaries (e.g. "what tier does an unplugged Stage land in?").
const W_STATIONARY:        f32 = 25.0;
const W_AC_POWER:          f32 = 100.0;
const W_CHARGING:          f32 = 50.0;
const W_BATTERY_HIGH:      f32 = 30.0;   // ≥70%
const W_BATTERY_MID:       f32 = 15.0;   // 40..70
const W_BATTERY_LOW:       f32 = 5.0;    // 20..40
const W_ETHERNET:          f32 = 40.0;
const W_WIFI:              f32 = 20.0;
const W_CAN_LEAD_NETWORK:  f32 = 30.0;
const W_BLUETOOTH:         f32 = 10.0;

/// Compute a numeric priority score. Higher = better hub candidate.
/// Returns 0.0 for unreachable devices — they can't be the hub.
pub fn priority_score(ctx: &DeviceContext) -> f32 {
    if !ctx.state.reachable { return 0.0; }
    let mut score = 0.0;

    // Stationary devices are inherently better hub candidates.
    if ctx.traits.stationary { score += W_STATIONARY; }

    // Power: AC > charging > high battery > mid > low.
    match (ctx.state.on_battery, ctx.state.charging, ctx.state.battery_percent) {
        (Some(false), _, _) => { score += W_AC_POWER; }
        (Some(true), Some(true), _) => { score += W_CHARGING; }
        (Some(true), _, Some(pct)) if pct >= 70 => { score += W_BATTERY_HIGH; }
        (Some(true), _, Some(pct)) if pct >= 40 => { score += W_BATTERY_MID; }
        (Some(true), _, Some(pct)) if pct >= 20 => { score += W_BATTERY_LOW; }
        (Some(true), _, _) => { /* <20% or unknown — 0 */ }
        // No battery info — for stationary devices assume AC, for portables
        // assume some battery (mid-range).
        (None, _, _) => {
            if ctx.traits.stationary { score += W_AC_POWER; }
            else                     { score += W_BATTERY_MID; }
        }
    }

    // Network: Ethernet > Wi-Fi > nothing. If we don't know, fall back
    // to whatever's intrinsic (Stage assumes Ethernet, Level assumes Wi-Fi).
    let on_eth = ctx.state.on_ethernet.unwrap_or(ctx.traits.has_ethernet);
    let on_wifi = ctx.state.on_wifi.unwrap_or(ctx.traits.has_wifi);
    if on_eth { score += W_ETHERNET; }
    else if on_wifi { score += W_WIFI; }

    if ctx.traits.can_lead_network_audio { score += W_CAN_LEAD_NETWORK; }
    if ctx.state.bluetooth_active.unwrap_or(ctx.traits.has_bluetooth) {
        score += W_BLUETOOTH;
    }

    score
}

/// Decide which priority group a device sits in. Thresholds chosen so
/// the canonical cases land in the obvious group:
///   - Stage on AC + Ethernet: 25+100+40+30+10 = 205 → PrimaryHub
///   - Level on AC + Wi-Fi:     0+100+20+30+10 = 160 → PrimaryHub (deserved)
///   - Level on battery 80%:    0+30 +20+30+10 = 90  → SecondaryHub
///   - A5 on battery 30%:       0+5  +20+30+10 = 65  → Satellite
///   - H100 (BT only):          0+0  + 0+0 +10 = 10  → Listener
///   - Unreachable Stage:                          = 0   → Listener
pub fn priority_group(ctx: &DeviceContext) -> PriorityGroup {
    if !ctx.traits.can_lead_network_audio
       && !(ctx.traits.has_wifi || ctx.traits.has_ethernet) {
        // BT-only headphones / earbuds. Never a hub, regardless of state.
        return PriorityGroup::Listener;
    }
    let score = priority_score(ctx);
    if score >= 150.0      { PriorityGroup::PrimaryHub }
    else if score >= 80.0  { PriorityGroup::SecondaryHub }
    else if score >= 30.0  { PriorityGroup::Satellite }
    else                   { PriorityGroup::Listener }
}

/// Pick the recommended hub from a list. Returns None if no devices are
/// reachable. Ties broken by product_id (deterministic).
pub fn recommended_hub<'a>(devices: &'a [DeviceContext]) -> Option<&'a DeviceContext> {
    devices.iter()
        .filter(|d| d.state.reachable && d.traits.can_lead_network_audio)
        .max_by(|a, b| {
            let sa = priority_score(a);
            let sb = priority_score(b);
            sa.partial_cmp(&sb)
              .unwrap_or(std::cmp::Ordering::Equal)
              .then_with(|| b.product_id.cmp(&a.product_id))  // tie-break
        })
}

/// Human-readable "why this priority" — for tooltips/diagnostics.
pub fn explain(ctx: &DeviceContext) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !ctx.state.reachable {
        return "offline".to_string();
    }
    if ctx.traits.stationary { parts.push("stationary".into()); }
    match (ctx.state.on_battery, ctx.state.battery_percent, ctx.state.charging) {
        (Some(false), _, _) => parts.push("AC power".into()),
        (Some(true), Some(p), Some(true))  => parts.push(format!("battery {}% (charging)", p)),
        (Some(true), Some(p), _)           => parts.push(format!("battery {}%", p)),
        (Some(true), None, _)              => parts.push("on battery".into()),
        (None, _, _) if ctx.traits.stationary => parts.push("AC (assumed)".into()),
        (None, _, _)                       => parts.push("portable".into()),
    }
    if ctx.state.on_ethernet.unwrap_or(false) { parts.push("Ethernet".into()); }
    else if ctx.state.on_wifi.unwrap_or(ctx.traits.has_wifi) { parts.push("Wi-Fi".into()); }
    if ctx.traits.can_lead_network_audio { parts.push("can lead".into()); }
    if ctx.state.bluetooth_active.unwrap_or(false) { parts.push("BT ready".into()); }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(product_id: &str, reachable: bool) -> DeviceContext {
        DeviceContext {
            product_id: product_id.to_string(),
            name: product_id.to_string(),
            url: "http://x".to_string(),
            traits: DeviceTraits::for_product(product_id),
            state: DeviceState { reachable, ..Default::default() },
        }
    }

    #[test]
    fn stage_on_ac_ethernet_is_primary_hub() {
        let mut c = ctx("BeosoundStage", true);
        c.state.on_battery = Some(false);
        c.state.on_ethernet = Some(true);
        assert_eq!(priority_group(&c), PriorityGroup::PrimaryHub);
    }

    #[test]
    fn level_on_ac_wifi_is_primary_hub() {
        let mut c = ctx("BeosoundLevel", true);
        c.state.on_battery = Some(false);
        c.state.on_wifi = Some(true);
        // 0+100+20+30+10 = 160 → PrimaryHub
        assert_eq!(priority_group(&c), PriorityGroup::PrimaryHub);
    }

    #[test]
    fn level_on_battery_high_is_secondary() {
        let mut c = ctx("BeosoundLevel", true);
        c.state.on_battery = Some(true);
        c.state.battery_percent = Some(80);
        c.state.on_wifi = Some(true);
        assert_eq!(priority_group(&c), PriorityGroup::SecondaryHub);
    }

    #[test]
    fn portable_low_battery_drops_to_satellite() {
        let mut c = ctx("BeosoundA5", true);
        c.state.on_battery = Some(true);
        c.state.battery_percent = Some(25);
        c.state.on_wifi = Some(true);
        assert_eq!(priority_group(&c), PriorityGroup::Satellite);
    }

    #[test]
    fn bt_only_headphones_are_always_listener() {
        let mut c = ctx("BeoplayH100", true);
        // Even with full battery, they can't be a hub.
        c.state.on_battery = Some(true);
        c.state.battery_percent = Some(100);
        c.state.bluetooth_active = Some(true);
        assert_eq!(priority_group(&c), PriorityGroup::Listener);
    }

    #[test]
    fn unreachable_devices_are_listener() {
        let c = ctx("BeosoundStage", false);
        assert_eq!(priority_score(&c), 0.0);
        assert_eq!(priority_group(&c), PriorityGroup::Listener);
    }

    #[test]
    fn ethernet_beats_wifi_at_same_power() {
        let mut wired = ctx("BeosoundStage", true);
        wired.state.on_battery = Some(false);
        wired.state.on_ethernet = Some(true);
        wired.state.on_wifi = Some(false);

        let mut wireless = ctx("BeosoundStage", true);
        wireless.state.on_battery = Some(false);
        wireless.state.on_ethernet = Some(false);
        wireless.state.on_wifi = Some(true);

        assert!(priority_score(&wired) > priority_score(&wireless),
                "wired {} should beat wireless {}",
                priority_score(&wired), priority_score(&wireless));
    }

    #[test]
    fn ac_beats_battery_for_same_device() {
        let mut acd = ctx("BeosoundLevel", true);
        acd.state.on_battery = Some(false);
        acd.state.on_wifi = Some(true);

        let mut batt = ctx("BeosoundLevel", true);
        batt.state.on_battery = Some(true);
        batt.state.battery_percent = Some(90);
        batt.state.on_wifi = Some(true);

        assert!(priority_score(&acd) > priority_score(&batt));
    }

    #[test]
    fn battery_charging_beats_battery_discharging() {
        let mut ch = ctx("BeosoundLevel", true);
        ch.state.on_battery = Some(true);
        ch.state.charging = Some(true);
        ch.state.battery_percent = Some(40);  // mid
        ch.state.on_wifi = Some(true);

        let mut dis = ctx("BeosoundLevel", true);
        dis.state.on_battery = Some(true);
        dis.state.charging = Some(false);
        dis.state.battery_percent = Some(95);  // high
        dis.state.on_wifi = Some(true);

        // Charging adds 50, high battery adds 30. Charging should win.
        assert!(priority_score(&ch) > priority_score(&dis));
    }

    #[test]
    fn recommended_hub_picks_highest_score() {
        let mut stage = ctx("BeosoundStage", true);
        stage.state.on_battery = Some(false);
        stage.state.on_ethernet = Some(true);

        let mut level = ctx("BeosoundLevel", true);
        level.state.on_battery = Some(true);
        level.state.battery_percent = Some(80);
        level.state.on_wifi = Some(true);

        let h100 = ctx("BeoplayH100", true);

        let devices = vec![level, h100, stage.clone()];
        let pick = recommended_hub(&devices).expect("should pick something");
        assert_eq!(pick.product_id, "BeosoundStage");
    }

    #[test]
    fn recommended_hub_returns_none_when_all_offline() {
        let devices = vec![ctx("BeosoundStage", false), ctx("BeosoundLevel", false)];
        assert!(recommended_hub(&devices).is_none());
    }

    #[test]
    fn explain_describes_state_in_human_terms() {
        let mut c = ctx("BeosoundLevel", true);
        c.state.on_battery = Some(true);
        c.state.battery_percent = Some(67);
        c.state.charging = Some(false);
        c.state.on_wifi = Some(true);
        c.state.bluetooth_active = Some(true);
        let s = explain(&c);
        assert!(s.contains("battery 67%"), "got: {}", s);
        assert!(s.contains("Wi-Fi") || s.contains("WiFi"));
    }

    #[test]
    fn group_labels_are_distinct() {
        let groups = [
            PriorityGroup::PrimaryHub,
            PriorityGroup::SecondaryHub,
            PriorityGroup::Satellite,
            PriorityGroup::Listener,
        ];
        // Labels should be unique
        let mut seen: Vec<&str> = Vec::new();
        for g in groups {
            assert!(!seen.contains(&g.label()), "duplicate label {}", g.label());
            seen.push(g.label());
        }
    }

    #[test]
    fn group_ordering_is_priority_descending() {
        // PrimaryHub < SecondaryHub < Satellite < Listener (lower = better)
        assert!(PriorityGroup::PrimaryHub < PriorityGroup::SecondaryHub);
        assert!(PriorityGroup::SecondaryHub < PriorityGroup::Satellite);
        assert!(PriorityGroup::Satellite < PriorityGroup::Listener);
    }
}
