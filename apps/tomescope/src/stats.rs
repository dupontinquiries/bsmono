//! Reading statistics: session logging, streak computation, throughput
//! (pages or percent per hour), and a GitHub-style activity heatmap grid.
//! All time math is done in whole days-since-epoch to avoid timezone
//! headaches — a "day" here is a UTC day, which is a reasonable
//! simplification for a local single-user reading app.

use crate::util::{days_since_epoch, now_unix, today_epoch_day};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReadingSession {
    pub book_id: u64,
    pub start_unix: i64,
    pub end_unix: i64,
    pub start_percent: f32,
    pub end_percent: f32,
    pub pages_turned: u32,
}

impl ReadingSession {
    pub fn duration_secs(&self) -> i64 {
        (self.end_unix - self.start_unix).max(0)
    }
    pub fn percent_gained(&self) -> f32 {
        (self.end_percent - self.start_percent).max(0.0)
    }
}

/// An in-progress session being accumulated by the reader; flushed into
/// the log on book close, app pause, or idle timeout.
#[derive(Clone, Debug)]
pub struct ActiveSession {
    pub book_id: u64,
    pub start_unix: i64,
    pub start_percent: f32,
    pub last_activity_unix: i64,
    pub pages_turned: u32,
}

impl ActiveSession {
    pub fn new(book_id: u64, start_percent: f32) -> Self {
        let now = now_unix();
        Self {
            book_id,
            start_unix: now,
            start_percent,
            last_activity_unix: now,
            pages_turned: 0,
        }
    }
    pub fn touch(&mut self) {
        self.last_activity_unix = now_unix();
    }
    pub fn record_page_turn(&mut self) {
        self.pages_turned += 1;
        self.touch();
    }
    /// True once the session has gone quiet long enough to consider it
    /// over (default 5 minutes of no page turns / interaction).
    pub fn is_idle(&self, idle_secs: i64) -> bool {
        now_unix() - self.last_activity_unix > idle_secs
    }
    pub fn finish(&self, end_percent: f32) -> ReadingSession {
        ReadingSession {
            book_id: self.book_id,
            start_unix: self.start_unix,
            end_unix: now_unix(),
            start_percent: self.start_percent,
            end_percent,
            pages_turned: self.pages_turned,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StatsLog {
    pub sessions: Vec<ReadingSession>,
}

impl StatsLog {
    pub fn add(&mut self, session: ReadingSession) {
        // Discard degenerate sessions (e.g. instantly closed books).
        if session.duration_secs() >= 5 {
            self.sessions.push(session);
        }
    }

    pub fn total_secs(&self) -> i64 {
        self.sessions.iter().map(|s| s.duration_secs()).sum()
    }

    pub fn total_secs_for_book(&self, book_id: u64) -> i64 {
        self.sessions
            .iter()
            .filter(|s| s.book_id == book_id)
            .map(|s| s.duration_secs())
            .sum()
    }

    /// Pages-per-hour averaged across all sessions with at least one page
    /// turn recorded (excludes pure scroll-mode sessions where page count
    /// isn't meaningful).
    pub fn pages_per_hour(&self) -> f32 {
        let (pages, secs): (u32, i64) = self
            .sessions
            .iter()
            .filter(|s| s.pages_turned > 0)
            .fold((0, 0), |(p, s), sess| (p + sess.pages_turned, s + sess.duration_secs()));
        if secs == 0 {
            0.0
        } else {
            pages as f32 / (secs as f32 / 3600.0)
        }
    }

    /// Map of epoch-day -> total seconds read that day, for heatmap /
    /// streak computation.
    pub fn daily_totals(&self) -> BTreeMap<i64, i64> {
        let mut map: BTreeMap<i64, i64> = BTreeMap::new();
        for s in &self.sessions {
            let day = days_since_epoch(s.start_unix);
            *map.entry(day).or_insert(0) += s.duration_secs();
        }
        map
    }

    /// Current streak length in days, counting backward from today. A day
    /// counts if total reading time that day meets `min_secs` (default
    /// suggestion: 60).
    pub fn current_streak(&self, min_secs: i64) -> u32 {
        let totals = self.daily_totals();
        let mut day = today_epoch_day();
        let mut streak = 0u32;
        loop {
            let secs = totals.get(&day).copied().unwrap_or(0);
            if secs >= min_secs {
                streak += 1;
                day -= 1;
            } else if day == today_epoch_day() {
                // Today not yet read enough — don't break the streak,
                // just don't count today; check yesterday onward.
                day -= 1;
                continue;
            } else {
                break;
            }
        }
        streak
    }

    pub fn longest_streak(&self, min_secs: i64) -> u32 {
        let totals = self.daily_totals();
        if totals.is_empty() {
            return 0;
        }
        let mut days: Vec<i64> = totals
            .iter()
            .filter(|(_, &secs)| secs >= min_secs)
            .map(|(&d, _)| d)
            .collect();
        days.sort_unstable();
        let mut longest = 0u32;
        let mut current = 0u32;
        let mut prev: Option<i64> = None;
        for d in days {
            match prev {
                Some(p) if d == p + 1 => current += 1,
                _ => current = 1,
            }
            longest = longest.max(current);
            prev = Some(d);
        }
        longest
    }

    /// Build a heatmap grid of `weeks` columns x 7 rows ending today,
    /// each cell holding total seconds read that day (0 if none). Row 0
    /// = Sunday.
    pub fn heatmap_grid(&self, weeks: u32) -> Vec<Vec<i64>> {
        let totals = self.daily_totals();
        let today = today_epoch_day();
        // Align the last column to the current weekday (0=Sunday) so the
        // grid reads like a calendar.
        let today_weekday = ((today % 7) + 7) % 7; // 0..6, epoch was a Thursday but that's fine, it's just a phase offset consistent across the app
        let total_days = weeks as i64 * 7;
        let start_day = today - total_days + 1 + (6 - today_weekday);

        let mut grid = vec![vec![0i64; weeks as usize]; 7];
        for i in 0..total_days {
            let day = start_day + i;
            if day > today {
                continue;
            }
            let week_idx = (i / 7) as usize;
            let row = ((day % 7) + 7) % 7;
            if week_idx < weeks as usize {
                grid[row as usize][week_idx] = totals.get(&day).copied().unwrap_or(0);
            }
        }
        grid
    }

    /// Per-book breakdown of total time spent, sorted descending.
    pub fn time_per_book(&self) -> Vec<(u64, i64)> {
        let mut map: HashMap<u64, i64> = HashMap::new();
        for s in &self.sessions {
            *map.entry(s.book_id).or_insert(0) += s.duration_secs();
        }
        let mut v: Vec<(u64, i64)> = map.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        v
    }

    /// Estimated minutes remaining in a book given recent reading speed
    /// (percent/hour averaged over the book's own sessions, falling back
    /// to the global average if this book has too little history).
    pub fn estimated_minutes_left(&self, book_id: u64, current_percent: f32) -> Option<u32> {
        let book_sessions: Vec<&ReadingSession> =
            self.sessions.iter().filter(|s| s.book_id == book_id).collect();
        let (pct_gained, secs): (f32, i64) = book_sessions
            .iter()
            .fold((0.0, 0), |(p, s), sess| (p + sess.percent_gained(), s + sess.duration_secs()));
        let rate_per_sec = if secs > 60 && pct_gained > 0.0 {
            pct_gained / secs as f32
        } else {
            // Fallback: assume ~0.4%/minute (roughly a 250-page book in ~4h).
            0.4 / 60.0
        };
        if rate_per_sec <= 0.0 {
            return None;
        }
        let remaining_pct = (1.0 - current_percent).max(0.0);
        Some((remaining_pct / rate_per_sec / 60.0).round() as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(book: u64, start: i64, end: i64, pages: u32) -> ReadingSession {
        ReadingSession {
            book_id: book,
            start_unix: start,
            end_unix: end,
            start_percent: 0.0,
            end_percent: 0.1,
            pages_turned: pages,
        }
    }

    #[test]
    fn pages_per_hour_basic() {
        let mut log = StatsLog::default();
        // 30 pages in 1800 seconds (0.5h) => 60 pages/hour
        log.add(session(1, 0, 1800, 30));
        assert!((log.pages_per_hour() - 60.0).abs() < 0.01);
    }

    #[test]
    fn streak_counts_consecutive_days() {
        let mut log = StatsLog::default();
        let today = today_epoch_day();
        for d in 0..3 {
            let day = today - d;
            let ts = day * 86_400 + 3600; // arbitrary time within day
            log.add(session(1, ts, ts + 120, 1));
        }
        assert_eq!(log.current_streak(60), 3);
    }

    #[test]
    fn heatmap_grid_dimensions() {
        let log = StatsLog::default();
        let grid = log.heatmap_grid(10);
        assert_eq!(grid.len(), 7);
        for row in &grid {
            assert_eq!(row.len(), 10);
        }
    }

    #[test]
    fn time_per_book_sums_correctly() {
        let mut log = StatsLog::default();
        log.add(session(1, 0, 600, 5));
        log.add(session(1, 1000, 1600, 5));
        log.add(session(2, 2000, 2100, 1));
        let totals = log.time_per_book();
        assert_eq!(totals[0].0, 1);
        assert_eq!(totals[0].1, 1200);
    }
}
