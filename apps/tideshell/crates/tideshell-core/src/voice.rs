//! The voice route: which recognizer tiers to run for a listen, chosen
//! where the cost pays off. voiceterm can dispatch Tiny, Base and
//! HighAccuracy whisper models in parallel and take the best that answers
//! in time; loading the big one costs seconds of CPU and heat. So:
//!
//! * the palette or the cover listening for a command: Tiny only (short
//!   words, latency matters, accuracy barely does);
//! * dictation into a text app: Tiny + Base, plus HighAccuracy when the
//!   governor says there is headroom (charging or cool and not low);
//! * a hot or low phone, or the governor already capping: Tiny only;
//! * everything else: Tiny + Base.
//!
//! The decision is published for voiceterm; the reason travels with it.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Tiny,
    Base,
    HighAccuracy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// The shell's palette or cover is listening for a command.
    Command,
    /// A text-entry app is in front: dictation.
    Dictation,
    /// Some other app.
    Other,
}

/// What the governor reports (its state file), reduced to what matters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Headroom {
    /// comfortgov's ceiling factor (thermal x battery), 1.0 = free.
    pub ceiling: f32,
    pub on_battery: bool,
    pub battery_pct: u32,
    pub sustained_load: bool,
}

impl Default for Headroom {
    fn default() -> Self {
        Headroom { ceiling: 1.0, on_battery: true, battery_pct: 100, sustained_load: false }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub tiers: Vec<Tier>,
    pub reason: &'static str,
}

pub fn route(focus: Focus, h: &Headroom) -> Route {
    let capped = h.ceiling < 0.85 || (h.on_battery && h.battery_pct <= 20) || h.sustained_load;
    match (focus, capped) {
        (Focus::Command, _) => Route { tiers: vec![Tier::Tiny], reason: "command: latency over accuracy" },
        (_, true) => Route { tiers: vec![Tier::Tiny], reason: "phone is hot, low or busy: cheapest tier" },
        (Focus::Dictation, false) => {
            let plenty = !h.on_battery || (h.ceiling >= 0.98 && h.battery_pct >= 50);
            if plenty {
                Route { tiers: vec![Tier::Tiny, Tier::Base, Tier::HighAccuracy], reason: "dictation with headroom: full cascade" }
            } else {
                Route { tiers: vec![Tier::Tiny, Tier::Base], reason: "dictation on battery: tiny + base" }
            }
        }
        (Focus::Other, false) => Route { tiers: vec![Tier::Tiny, Tier::Base], reason: "default: tiny + base" },
    }
}

impl Route {
    /// The line voiceterm reads: `tiers=tiny,base reason=...`.
    pub fn line(&self) -> String {
        let names: Vec<&str> = self
            .tiers
            .iter()
            .map(|t| match t {
                Tier::Tiny => "tiny",
                Tier::Base => "base",
                Tier::HighAccuracy => "high",
            })
            .collect();
        format!("tiers={} reason={}", names.join(","), self.reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_cheap_dictation_scales_with_headroom_and_a_stressed_phone_stays_cheap() {
        let free = Headroom { ceiling: 1.0, on_battery: false, ..Default::default() };
        assert_eq!(route(Focus::Command, &free).tiers, vec![Tier::Tiny]);
        assert_eq!(route(Focus::Dictation, &free).tiers, vec![Tier::Tiny, Tier::Base, Tier::HighAccuracy]);
        let battery = Headroom { on_battery: true, battery_pct: 40, ..Default::default() };
        assert_eq!(route(Focus::Dictation, &battery).tiers, vec![Tier::Tiny, Tier::Base]);
        let hot = Headroom { ceiling: 0.6, ..Default::default() };
        assert_eq!(route(Focus::Dictation, &hot).tiers, vec![Tier::Tiny]);
        let low = Headroom { battery_pct: 15, ..Default::default() };
        assert_eq!(route(Focus::Other, &low).tiers, vec![Tier::Tiny]);
        assert_eq!(route(Focus::Other, &Headroom::default()).tiers, vec![Tier::Tiny, Tier::Base]);
        assert_eq!(route(Focus::Dictation, &free).line(), "tiers=tiny,base,high reason=dictation with headroom: full cascade");
    }
}
