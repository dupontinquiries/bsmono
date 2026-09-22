//! The clock: digital or analog, exact or fuzzy (nearest quarter hour, half
//! hour, hour, or just the weekday). Pure: the caller supplies the local
//! time (`LocalTime`) and draws what this returns.
//!
//! Fuzzy rounding goes to the NEAREST step, the fuzzy-clock convention:
//! 3:07 is "three o'clock", 3:08 is "quarter past three". Rounding can roll
//! into the next hour (and past midnight). The analog hour hand follows the
//! snapped time, so the two hands always agree with each other and with the
//! words.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockStyle {
    Digital,
    Analog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fuzzy {
    Exact,
    Quarter,
    Half,
    Hour,
    /// Only the day of the week.
    Weekday,
}

impl Fuzzy {
    pub const ALL: [Fuzzy; 5] = [Fuzzy::Exact, Fuzzy::Quarter, Fuzzy::Half, Fuzzy::Hour, Fuzzy::Weekday];

    pub fn token(self) -> &'static str {
        match self {
            Fuzzy::Exact => "exact",
            Fuzzy::Quarter => "15",
            Fuzzy::Half => "30",
            Fuzzy::Hour => "hour",
            Fuzzy::Weekday => "weekday",
        }
    }

    pub fn from_token(s: &str) -> Option<Fuzzy> {
        Fuzzy::ALL.into_iter().find(|f| f.token() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            Fuzzy::Exact => "Exact",
            Fuzzy::Quarter => "15 min",
            Fuzzy::Half => "30 min",
            Fuzzy::Hour => "Hour",
            Fuzzy::Weekday => "Weekday",
        }
    }

    fn step(self) -> Option<u32> {
        match self {
            Fuzzy::Exact => Some(1),
            Fuzzy::Quarter => Some(15),
            Fuzzy::Half => Some(30),
            Fuzzy::Hour => Some(60),
            Fuzzy::Weekday => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    pub hour: u32,
    pub minute: u32,
    /// 0 = Sunday, as libc's tm_wday.
    pub weekday: u32,
}

const WEEKDAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const HOURS: [&str; 12] = ["twelve", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven"];

pub fn weekday_name(w: u32) -> &'static str {
    WEEKDAYS[(w % 7) as usize]
}

/// (hour, minute) rounded to the nearest step (Weekday: unchanged).
pub fn snap(t: LocalTime, f: Fuzzy) -> (u32, u32) {
    let Some(step) = f.step() else { return (t.hour, t.minute) };
    let total = t.hour * 60 + t.minute;
    let snapped = (total + step / 2) / step * step; // nearest; ties (7.5) round down
    ((snapped / 60) % 24, snapped % 60)
}

fn hour_word(h: u32) -> &'static str {
    HOURS[(h % 12) as usize]
}

/// The time in words (or the weekday).
pub fn words(t: LocalTime, f: Fuzzy) -> String {
    if f == Fuzzy::Weekday {
        return weekday_name(t.weekday).to_string();
    }
    let (h, m) = snap(t, f);
    let prefix = if f == Fuzzy::Hour { "about " } else { "" };
    match m {
        0 => format!("{prefix}{} o'clock", hour_word(h)),
        15 => format!("quarter past {}", hour_word(h)),
        30 => format!("half past {}", hour_word(h)),
        45 => format!("quarter to {}", hour_word(h + 1)),
        _ => format!("{h:02}:{m:02}"),
    }
}

/// Short text for narrow places (the bar): exact "15:07", fuzzy "~15:15",
/// weekday "Mon".
pub fn short_text(t: LocalTime, f: Fuzzy) -> String {
    match f {
        Fuzzy::Exact => format!("{:02}:{:02}", t.hour, t.minute),
        Fuzzy::Weekday => weekday_name(t.weekday)[..3].to_string(),
        _ => {
            let (h, m) = snap(t, f);
            format!("~{h:02}:{m:02}")
        }
    }
}

/// Hand angles in radians, clockwise from 12: (hour hand, minute hand).
/// Weekday: no hands. Hour: the minute hand is hidden (the hour hand alone
/// points at the hour).
pub fn hands(t: LocalTime, f: Fuzzy) -> Option<(f32, Option<f32>)> {
    if f == Fuzzy::Weekday {
        return None;
    }
    let (h, m) = snap(t, f);
    let tau = std::f32::consts::TAU;
    let hour = ((h % 12) as f32 + m as f32 / 60.0) / 12.0 * tau;
    let minute = (f != Fuzzy::Hour).then_some(m as f32 / 60.0 * tau);
    Some((hour, minute))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(hour: u32, minute: u32) -> LocalTime {
        LocalTime { hour, minute, weekday: 1 }
    }

    #[test]
    fn rounds_to_the_nearest_step_and_rolls_over() {
        assert_eq!(snap(at(15, 7), Fuzzy::Quarter), (15, 0));
        assert_eq!(snap(at(15, 8), Fuzzy::Quarter), (15, 15));
        assert_eq!(snap(at(15, 52), Fuzzy::Quarter), (15, 45));
        assert_eq!(snap(at(15, 53), Fuzzy::Quarter), (16, 0), "rolls into the next hour");
        assert_eq!(snap(at(23, 53), Fuzzy::Quarter), (0, 0), "and past midnight");
        assert_eq!(snap(at(15, 14), Fuzzy::Half), (15, 0));
        assert_eq!(snap(at(15, 15), Fuzzy::Half), (15, 30));
        assert_eq!(snap(at(15, 29), Fuzzy::Hour), (15, 0));
        assert_eq!(snap(at(15, 30), Fuzzy::Hour), (16, 0));
        assert_eq!(snap(at(15, 7), Fuzzy::Exact), (15, 7));
    }

    #[test]
    fn words_read_naturally() {
        assert_eq!(words(at(15, 2), Fuzzy::Quarter), "three o'clock");
        assert_eq!(words(at(15, 13), Fuzzy::Quarter), "quarter past three");
        assert_eq!(words(at(15, 31), Fuzzy::Quarter), "half past three");
        assert_eq!(words(at(15, 44), Fuzzy::Quarter), "quarter to four");
        assert_eq!(words(at(11, 46), Fuzzy::Quarter), "quarter to twelve");
        assert_eq!(words(at(0, 5), Fuzzy::Quarter), "twelve o'clock", "midnight");
        assert_eq!(words(at(15, 20), Fuzzy::Half), "half past three");
        assert_eq!(words(at(15, 40), Fuzzy::Hour), "about four o'clock");
        assert_eq!(words(at(15, 40), Fuzzy::Weekday), "Monday");
        assert_eq!(words(at(15, 7), Fuzzy::Exact), "15:07");
    }

    #[test]
    fn short_text_for_the_bar() {
        assert_eq!(short_text(at(9, 5), Fuzzy::Exact), "09:05");
        assert_eq!(short_text(at(9, 8), Fuzzy::Quarter), "~09:15");
        assert_eq!(short_text(LocalTime { hour: 9, minute: 8, weekday: 5 }, Fuzzy::Weekday), "Fri");
    }

    #[test]
    fn hands_follow_the_snapped_time_and_agree() {
        let deg = |r: f32| (r.to_degrees() * 10.0).round() / 10.0;
        let (h, m) = hands(at(3, 0), Fuzzy::Exact).unwrap();
        assert_eq!((deg(h), deg(m.unwrap())), (90.0, 0.0));
        let (h, m) = hands(at(3, 26), Fuzzy::Half).unwrap(); // snaps to 3:30
        assert_eq!((deg(h), deg(m.unwrap())), (105.0, 180.0), "hour hand half-way to four");
        let (h, m) = hands(at(15, 40), Fuzzy::Hour).unwrap(); // about four
        assert_eq!((deg(h), m), (120.0, None), "hour mode: minute hand hidden");
        assert_eq!(hands(at(3, 0), Fuzzy::Weekday), None);
    }

    #[test]
    fn tokens_round_trip_and_weekday_names() {
        for f in Fuzzy::ALL {
            assert_eq!(Fuzzy::from_token(f.token()), Some(f));
        }
        assert_eq!(weekday_name(0), "Sunday");
        assert_eq!(weekday_name(13), "Saturday");
    }
}
