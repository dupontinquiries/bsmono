// src/seasons.rs
//
// Compute a season + time-of-day "mood" from the current date.
// Pure function in / pure data out — no I/O — so it's trivially testable.
//
// The QML side picks up these values (via QtBackend properties) and uses
// them to bias background gradients and accent colors. Nothing here is
// authoritative; it's all aesthetic.

use chrono::{Datelike, Timelike, NaiveDateTime};

/// Approximate season for a Northern-Hemisphere user.
///
/// We don't have the user's hemisphere, so we just pick the convention
/// most apps assume. Southern-hemisphere users get inverted seasons —
/// future improvement: detect via geolocation / system locale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Season {
    Winter,
    Spring,
    Summer,
    Autumn,
}

impl Season {
    pub fn as_str(&self) -> &'static str {
        match self {
            Season::Winter => "winter",
            Season::Spring => "spring",
            Season::Summer => "summer",
            Season::Autumn => "autumn",
        }
    }

    /// Northern-hemisphere meteorological seasons:
    ///   winter = Dec, Jan, Feb
    ///   spring = Mar, Apr, May
    ///   summer = Jun, Jul, Aug
    ///   autumn = Sep, Oct, Nov
    pub fn for_month(month: u32) -> Self {
        match month {
            12 | 1 | 2 => Season::Winter,
            3..=5      => Season::Spring,
            6..=8      => Season::Summer,
            _          => Season::Autumn,
        }
    }
}

/// Coarse part of the day. Boundaries are deliberately fuzzy to avoid
/// jarring transitions exactly on the hour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DayPhase {
    Night,    // 22 – 5
    Dawn,     // 5  – 8
    Morning,  // 8  – 12
    Afternoon,// 12 – 17
    Dusk,     // 17 – 20
    Evening,  // 20 – 22
}

impl DayPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            DayPhase::Night     => "night",
            DayPhase::Dawn      => "dawn",
            DayPhase::Morning   => "morning",
            DayPhase::Afternoon => "afternoon",
            DayPhase::Dusk      => "dusk",
            DayPhase::Evening   => "evening",
        }
    }

    pub fn for_hour(hour: u32) -> Self {
        match hour {
            22..=23 | 0..=4 => DayPhase::Night,
            5..=7           => DayPhase::Dawn,
            8..=11          => DayPhase::Morning,
            12..=16         => DayPhase::Afternoon,
            17..=19         => DayPhase::Dusk,
            20..=21         => DayPhase::Evening,
            _               => DayPhase::Afternoon, // unreachable but explicit
        }
    }
}

/// Combined snapshot of "what does the world look like right now?"
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mood {
    pub season: Season,
    pub phase: DayPhase,
}

impl Mood {
    /// Mood for a specific naive datetime (mostly for testing).
    pub fn at(dt: NaiveDateTime) -> Self {
        Mood {
            season: Season::for_month(dt.month()),
            phase:  DayPhase::for_hour(dt.hour()),
        }
    }

    /// Mood at the current local time.
    pub fn now_local() -> Self {
        let now = chrono::Local::now().naive_local();
        Mood::at(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn dt(month: u32, day: u32, hour: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2025, month, day)
            .unwrap()
            .and_hms_opt(hour, 0, 0)
            .unwrap()
    }

    #[test]
    fn seasons_cover_year() {
        assert_eq!(Season::for_month(1),  Season::Winter);
        assert_eq!(Season::for_month(2),  Season::Winter);
        assert_eq!(Season::for_month(3),  Season::Spring);
        assert_eq!(Season::for_month(5),  Season::Spring);
        assert_eq!(Season::for_month(6),  Season::Summer);
        assert_eq!(Season::for_month(8),  Season::Summer);
        assert_eq!(Season::for_month(9),  Season::Autumn);
        assert_eq!(Season::for_month(11), Season::Autumn);
        assert_eq!(Season::for_month(12), Season::Winter);
    }

    #[test]
    fn day_phases_cover_24h() {
        assert_eq!(DayPhase::for_hour(0),  DayPhase::Night);
        assert_eq!(DayPhase::for_hour(3),  DayPhase::Night);
        assert_eq!(DayPhase::for_hour(6),  DayPhase::Dawn);
        assert_eq!(DayPhase::for_hour(10), DayPhase::Morning);
        assert_eq!(DayPhase::for_hour(14), DayPhase::Afternoon);
        assert_eq!(DayPhase::for_hour(18), DayPhase::Dusk);
        assert_eq!(DayPhase::for_hour(21), DayPhase::Evening);
        assert_eq!(DayPhase::for_hour(23), DayPhase::Night);
    }

    #[test]
    fn mood_combines() {
        let summer_morning = Mood::at(dt(7, 15, 9));
        assert_eq!(summer_morning.season, Season::Summer);
        assert_eq!(summer_morning.phase,  DayPhase::Morning);
    }
}
