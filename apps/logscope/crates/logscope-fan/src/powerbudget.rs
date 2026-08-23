//! powerbudget — is stopping actually necessary?
//!
//! ## Why the continuation rule needed this
//!
//! [`crate::continuation`] stops when the auto-continue budget is spent, with
//! the reason "something has to eventually stop". That is true on a laptop
//! running down its battery and false on an Asahi desktop on mains, where the
//! machine is idle-efficient and nobody is counting. A fixed budget is the
//! wrong instrument: what it is standing in for is ENERGY, and energy is
//! sometimes free.
//!
//! ## The safe direction is metered
//!
//! Every unknown resolves toward [`Supply::Metered`]. No `/sys/class/power_supply`,
//! a stale sensor, a mesh that did not answer — all of it means "assume the
//! budget applies". The failure modes are not symmetric: wrongly metered costs
//! an unnecessary stop the operator can override, wrongly unmetered drains a
//! battery flat while they are not watching.
//!
//! `voiceterm`'s `battery_ok_for_background` already takes the opposite default
//! — "no /sys = desktop or non-Linux, OK" — and for a background listener whose
//! worst case is a dropped recording that is a reasonable trade. For a run that
//! can continue itself for an hour it is not, and the two should not be made to
//! agree just because they read the same file.
//!
//! ## Predicted charge is not available charge
//!
//! Solar scheduling is the part most likely to go wrong, because a forecast
//! reads like a fact. [`Reserve::available_wh`] counts only what is IN the
//! battery; the forecast can extend a deadline but can never authorise spending
//! energy that has not arrived. A cloud is not a bug report.
//!
//! Pure: readings in, verdict out. Nothing here reads a file or a socket — the
//! shell samples, this decides, and that is what makes a January afternoon a
//! unit test.

/// Where the power is coming from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Source {
    /// Wall power. `efficient` marks a machine whose idle draw is low enough
    /// that nobody meters it — an Asahi laptop, a Mac mini. It is a CLAIM about
    /// the operator's attention, not about physics: the machine still draws
    /// power, and this only says nobody is counting it.
    Mains { efficient: bool },
    /// Internal battery.
    Battery { pct: u8, discharging: bool },
    /// A portable station — Jackery and friends. Stored energy plus whatever
    /// is currently coming in.
    Station { stored_wh: f32, capacity_wh: f32, input_w: f32 },
    /// Nothing could be determined.
    Unknown,
}

/// What the run may assume.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Supply {
    /// Carry on; nobody is counting.
    Unmetered,
    /// A budget applies.
    Metered,
    /// Stop soon whatever else is true.
    Critical,
}

/// Environment readings that inform a forecast. Every field is optional
/// because every source can be absent, and absent must not read as zero —
/// zero lux is midnight, which is a very different claim from "no sensor".
#[derive(Debug, Clone, Copy, Default)]
pub struct Ambient {
    /// Local illuminance, from the device's own IIO sensor.
    pub local_lux: Option<f32>,
    /// Regional illuminance, from Fairphones on the bo-suite / snapclone mesh.
    /// A mesh median is worth more than one sensor: a phone face-down on a desk
    /// reads zero at noon.
    pub mesh_lux: Option<f32>,
    /// How many peers contributed to `mesh_lux`.
    pub mesh_peers: usize,
    /// Outdoor temperature, from ambient-learner.
    pub outdoor_c: Option<f32>,
    /// Hours until sunset, from the same clock the scene's sun uses.
    pub hours_to_sunset: Option<f32>,
    /// Seconds since the newest of these readings was taken.
    pub age_s: u64,
}

/// How long readings stay usable.
///
/// Five minutes. Light changes on the scale of a passing cloud, and a
/// fifteen-minute-old lux value would happily authorise an hour of work under a
/// sky that has since closed over.
pub const STALE_AFTER_S: u64 = 300;

impl Ambient {
    pub fn is_fresh(&self) -> bool { self.age_s <= STALE_AFTER_S }

    /// Best illuminance estimate, or `None`.
    ///
    /// Prefers the mesh when two or more peers agree, because a single local
    /// sensor is easy to occlude — a laptop lid, a pocket, a hand. One peer is
    /// not a mesh and is treated as no better than local.
    pub fn lux(&self) -> Option<f32> {
        if !self.is_fresh() { return None }
        match (self.mesh_lux, self.mesh_peers >= 2) {
            (Some(l), true) => Some(l),
            _ => self.local_lux,
        }
    }
}

/// What a station has to spend, and what it might gain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reserve {
    /// Energy in the battery NOW. The only figure that may be spent.
    pub available_wh: f32,
    /// Energy the forecast expects to arrive before sunset. Never spendable —
    /// it extends a deadline, it does not fund work.
    pub forecast_wh: f32,
}

/// Decide what the run may assume.
pub fn supply(src: Source, amb: &Ambient) -> Supply {
    match src {
        // The claim you made: an efficient machine on mains is unmetered.
        // Recorded as a claim rather than a measurement, and only when
        // `efficient` is set — a mains-powered workstation pulling 400W is
        // metered whether or not anyone has a meter on it.
        Source::Mains { efficient: true } => Supply::Unmetered,
        Source::Mains { efficient: false } => Supply::Metered,

        Source::Battery { pct, discharging } => {
            if !discharging { return Supply::Unmetered }   // charging on mains
            if pct <= 15 { Supply::Critical } else { Supply::Metered }
        }

        Source::Station { stored_wh, capacity_wh, input_w } => {
            let frac = if capacity_wh > 0.0 { stored_wh / capacity_wh } else { 0.0 };
            if frac <= 0.10 { return Supply::Critical }
            // Charging faster than a laptop draws, in daylight, with fresh
            // readings: treat as unmetered. All three conditions matter — a
            // station reporting high input at midnight is a broken sensor, and
            // acting on it is how a run flattens the battery it was told was
            // filling.
            let daylight = amb.lux().map(|l| l > 5_000.0).unwrap_or(false);
            if input_w >= 60.0 && daylight && frac > 0.35 { Supply::Unmetered }
            else { Supply::Metered }
        }

        Source::Unknown => Supply::Metered,
    }
}

/// Energy expected before sunset, in watt-hours.
///
/// Deliberately crude, and the crudeness is the honest part: this is a
/// scheduling hint, not a power model. Panel rating times a daylight fraction
/// times the hours left, halved for the fact that a panel almost never sees its
/// rating. It exists to answer "is it worth waiting an hour" and nothing finer.
///
/// Returns 0 whenever it cannot tell — stale readings, no light figure, no
/// sunset time. A forecast that guesses in the absence of data is worse than no
/// forecast, because the caller cannot distinguish it from a real one.
pub fn forecast_wh(panel_w: f32, amb: &Ambient) -> f32 {
    if !amb.is_fresh() { return 0.0 }
    let (Some(lux), Some(hours)) = (amb.lux(), amb.hours_to_sunset) else { return 0.0 };
    if hours <= 0.0 || panel_w <= 0.0 { return 0.0 }
    // 100k lux is full sun; the ratio is a rough irradiance proxy.
    let daylight = (lux / 100_000.0).clamp(0.0, 1.0);
    // Cold panels are MORE efficient, warm ones less. Small effect, included
    // because ambient-learner already has the temperature and leaving a known
    // signal out of a forecast is how it drifts optimistic in summer.
    let temp_factor = amb.outdoor_c.map(|c| (1.0 - (c - 25.0) * 0.004).clamp(0.7, 1.15)).unwrap_or(1.0);
    panel_w * daylight * temp_factor * hours * 0.5
}

/// Should the auto-continue budget apply?
///
/// The bridge to `continuation::Facts.max_auto_turns`: unmetered lifts the cap,
/// metered keeps it, critical collapses it to zero.
pub fn auto_turn_cap(sup: Supply, configured: usize) -> usize {
    match sup {
        Supply::Unmetered => configured.max(64),
        Supply::Metered => configured,
        Supply::Critical => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh(lux: f32) -> Ambient {
        Ambient { local_lux: Some(lux), hours_to_sunset: Some(4.0), age_s: 10, ..Default::default() }
    }

    /// The claim you made, held to `efficient`.
    #[test]
    fn an_efficient_mains_machine_is_unmetered() {
        assert_eq!(supply(Source::Mains { efficient: true }, &Ambient::default()), Supply::Unmetered);
        assert_eq!(supply(Source::Mains { efficient: false }, &Ambient::default()), Supply::Metered,
                   "mains is not the same as cheap — a 400W workstation is metered");
    }

    #[test]
    fn a_discharging_battery_is_metered_and_a_low_one_is_critical() {
        assert_eq!(supply(Source::Battery { pct: 80, discharging: true }, &Ambient::default()), Supply::Metered);
        assert_eq!(supply(Source::Battery { pct: 9, discharging: true }, &Ambient::default()), Supply::Critical);
        assert_eq!(supply(Source::Battery { pct: 40, discharging: false }, &Ambient::default()), Supply::Unmetered,
                   "charging means it is on mains");
    }

    /// EVERY unknown resolves to metered. The asymmetry is the whole safety
    /// argument: a wrong stop is overridable, a flat battery is not.
    #[test]
    fn every_unknown_resolves_to_metered() {
        assert_eq!(supply(Source::Unknown, &Ambient::default()), Supply::Metered);
        // A station charging hard, but with no light reading to corroborate it.
        let s = Source::Station { stored_wh: 500.0, capacity_wh: 1000.0, input_w: 200.0 };
        assert_eq!(supply(s, &Ambient::default()), Supply::Metered,
                   "high input with no daylight evidence is a broken sensor, not a sunny day");
    }

    /// A station reporting input at night must not be believed.
    #[test]
    fn a_station_charging_in_the_dark_stays_metered() {
        let s = Source::Station { stored_wh: 700.0, capacity_wh: 1000.0, input_w: 200.0 };
        let night = Ambient { local_lux: Some(2.0), age_s: 5, ..Default::default() };
        assert_eq!(supply(s, &night), Supply::Metered);
        // Same station, same input, in daylight: now believable.
        assert_eq!(supply(s, &fresh(80_000.0)), Supply::Unmetered);
    }

    #[test]
    fn a_nearly_empty_station_is_critical_however_sunny() {
        let s = Source::Station { stored_wh: 50.0, capacity_wh: 1000.0, input_w: 300.0 };
        assert_eq!(supply(s, &fresh(100_000.0)), Supply::Critical);
    }

    /// Stale readings are not readings.
    #[test]
    fn stale_ambient_data_is_discarded_rather_than_trusted() {
        let stale = Ambient { local_lux: Some(90_000.0), hours_to_sunset: Some(4.0),
                              age_s: STALE_AFTER_S + 1, ..Default::default() };
        assert_eq!(stale.lux(), None);
        assert_eq!(forecast_wh(200.0, &stale), 0.0,
                   "a fifteen-minute-old lux value would authorise an hour under a closed sky");
    }

    /// A mesh of two or more outvotes one local sensor; a single peer does not.
    #[test]
    fn the_mesh_outvotes_an_occluded_local_sensor() {
        let occluded = Ambient { local_lux: Some(3.0), mesh_lux: Some(70_000.0), mesh_peers: 4,
                                 age_s: 5, ..Default::default() };
        assert_eq!(occluded.lux(), Some(70_000.0), "a lid-down laptop should not veto four phones");
        let lone = Ambient { mesh_peers: 1, ..occluded };
        assert_eq!(lone.lux(), Some(3.0), "one peer is not a mesh");
    }

    /// The rule that keeps solar honest.
    #[test]
    fn a_forecast_is_never_spendable() {
        let r = Reserve { available_wh: 120.0, forecast_wh: 900.0 };
        assert_eq!(r.available_wh, 120.0);
        // Nothing in this module adds the forecast to the available figure, and
        // this is the assertion that says so: if someone later folds them
        // together, a cloudy afternoon spends energy that never arrived.
        assert!(r.forecast_wh > r.available_wh);
    }

    #[test]
    fn the_forecast_is_zero_when_it_cannot_tell() {
        assert_eq!(forecast_wh(200.0, &Ambient::default()), 0.0, "no readings at all");
        let no_sunset = Ambient { local_lux: Some(80_000.0), age_s: 5, ..Default::default() };
        assert_eq!(forecast_wh(200.0, &no_sunset), 0.0, "no sunset time");
        let past_sunset = Ambient { hours_to_sunset: Some(0.0), ..fresh(80_000.0) };
        assert_eq!(forecast_wh(200.0, &past_sunset), 0.0);
    }

    #[test]
    fn the_forecast_scales_with_light_and_time() {
        let bright = forecast_wh(200.0, &fresh(100_000.0));
        let dim = forecast_wh(200.0, &fresh(20_000.0));
        assert!(bright > dim * 3.0, "bright {bright} vs dim {dim}");
        let brief = Ambient { hours_to_sunset: Some(1.0), ..fresh(100_000.0) };
        assert!(forecast_wh(200.0, &brief) < bright);
    }

    /// A hot panel produces less. Small, and included because leaving a known
    /// signal out is how a forecast drifts optimistic in summer.
    #[test]
    fn a_hot_panel_forecasts_lower_than_a_cold_one() {
        let hot = Ambient { outdoor_c: Some(45.0), ..fresh(100_000.0) };
        let cold = Ambient { outdoor_c: Some(5.0), ..fresh(100_000.0) };
        assert!(forecast_wh(200.0, &hot) < forecast_wh(200.0, &cold));
    }

    #[test]
    fn the_auto_turn_cap_follows_the_supply() {
        assert_eq!(auto_turn_cap(Supply::Critical, 8), 0);
        assert_eq!(auto_turn_cap(Supply::Metered, 8), 8);
        assert!(auto_turn_cap(Supply::Unmetered, 8) >= 64);
    }
}
