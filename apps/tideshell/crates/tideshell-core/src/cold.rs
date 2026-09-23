//! Cold mode. Below freezing a lithium battery sags under load (the phone
//! dies at 30 %), gloves come on, and fingers are clumsy: the shell grows
//! its targets and slows its timings, and the governor caps performance.
//! Driven by the battery temperature (the one thermometer that is inside
//! the phone's own thermal mass), with hysteresis so it does not flicker
//! around the threshold.

pub const COLD_ON_C: f32 = 8.0;
pub const COLD_OFF_C: f32 = 12.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cold {
    pub on: bool,
    /// Manual override: Some(true/false) wins over the temperature.
    pub force: Option<bool>,
    last_c: Option<f32>,
}

impl Default for Cold {
    fn default() -> Self {
        Cold { on: false, force: None, last_c: None }
    }
}

/// What cold mode changes, as multipliers on the normal values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColdTuning {
    /// Edge strips, tiles, peeks, hit targets.
    pub target_scale: f32,
    /// Tap / hold time windows.
    pub time_scale: f32,
    /// Touch slop (a gloved finger wobbles).
    pub slop_scale: f32,
}

impl Cold {
    pub fn on_battery_temp(&mut self, c: f32) -> bool {
        self.last_c = Some(c);
        let was = self.on;
        self.on = match self.force {
            Some(f) => f,
            None => {
                if c <= COLD_ON_C {
                    true
                } else if c >= COLD_OFF_C {
                    false
                } else {
                    self.on
                }
            }
        };
        self.on != was
    }

    /// Set the override and apply it now (a pref change must not wait for
    /// the next temperature reading). Returns whether `on` changed.
    pub fn set_force(&mut self, force: Option<bool>) -> bool {
        self.force = force;
        let was = self.on;
        match force {
            Some(f) => self.on = f,
            None => {
                if let Some(c) = self.last_c {
                    self.on_battery_temp(c);
                }
            }
        }
        self.on != was
    }

    pub fn tuning(&self) -> ColdTuning {
        if self.on {
            ColdTuning { target_scale: 1.35, time_scale: 1.4, slop_scale: 2.0 }
        } else {
            ColdTuning { target_scale: 1.0, time_scale: 1.0, slop_scale: 1.0 }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_engages_below_8_and_releases_above_12() {
        let mut c = Cold::default();
        assert!(!c.on_battery_temp(20.0) && !c.on);
        assert!(!c.on_battery_temp(9.0), "not yet");
        assert!(c.on_battery_temp(7.5) && c.on);
        assert!(!c.on_battery_temp(10.0) && c.on, "hysteresis: stays cold at 10");
        assert!(c.on_battery_temp(12.5) && !c.on);
        assert!(c.tuning().target_scale == 1.0);
        assert!(c.set_force(Some(true)) && c.on && c.tuning().slop_scale == 2.0, "forced on, at once");
        assert!(c.set_force(None) && !c.on, "auto again at 12.5 C: off");
    }
}
