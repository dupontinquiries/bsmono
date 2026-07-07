//! profile.rs — a tiny persistent player "metaprofile". It accumulates lifetime kills, deaths, matches,
//! best killstreak and best wave across runs, and derives a K/D ratio and a coarse skill rating that a
//! future matchmaker can bucket players by. It serialises to a small `key=value` text file in the user's
//! config/home directory (no external crates), loaded once at launch and saved after each life/match.
//!
//! This module does file I/O only through `load`/`save`, which the app calls — the headless tests never
//! touch the disk.

use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub name: String,
    pub kills: u64,
    pub deaths: u64,
    pub matches: u64,
    pub best_streak: u32,
    pub best_wave: u32,
}

impl Default for Profile {
    fn default() -> Self {
        Self { name: "PLAYER".to_string(), kills: 0, deaths: 0, matches: 0, best_streak: 0, best_wave: 0 }
    }
}

impl Profile {
    /// Kills per death (deaths floored at 1 so a flawless profile still reads sensibly).
    pub fn kd(&self) -> f32 {
        self.kills as f32 / self.deaths.max(1) as f32
    }

    /// A coarse 0–100 skill score derived from K/D and best wave — the seed of future matchmaking buckets.
    pub fn skill(&self) -> u32 {
        let kd = self.kd().min(5.0);
        let wave = (self.best_wave as f32).min(30.0);
        ((kd / 5.0) * 70.0 + (wave / 30.0) * 30.0).round() as u32
    }

    /// Matchmaking tier label.
    pub fn tier(&self) -> &'static str {
        match self.skill() {
            0..=19 => "Bronze",
            20..=39 => "Silver",
            40..=59 => "Gold",
            60..=79 => "Platinum",
            _ => "Diamond",
        }
    }

    /// Fold one life's result into the lifetime totals.
    pub fn record_life(&mut self, kills: u64, wave: u32, best_streak: u32) {
        self.kills += kills;
        self.deaths += 1;
        self.best_streak = self.best_streak.max(best_streak);
        self.best_wave = self.best_wave.max(wave);
    }

    /// Count a completed match (one run from menu to menu).
    pub fn record_match(&mut self) {
        self.matches += 1;
    }

    /// Where the profile file lives: `$XDG_CONFIG_HOME` or `$HOME/.config`, falling back to the cwd.
    pub fn path() -> PathBuf {
        if let Ok(x) = std::env::var("XDG_CONFIG_HOME") {
            return PathBuf::from(x).join("early-fps").join("profile.txt");
        }
        if let Ok(h) = std::env::var("HOME") {
            return PathBuf::from(h).join(".config").join("early-fps").join("profile.txt");
        }
        if let Ok(h) = std::env::var("USERPROFILE") {
            return PathBuf::from(h).join("early-fps").join("profile.txt");
        }
        PathBuf::from("early-fps-profile.txt")
    }

    /// Load the profile from disk, or a fresh default if absent/unparseable.
    pub fn load() -> Self {
        let mut p = Profile::default();
        if let Ok(text) = std::fs::read_to_string(Self::path()) {
            p.merge_from_text(&text);
        }
        p
    }

    fn merge_from_text(&mut self, text: &str) {
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "name" => self.name = v.to_string(),
                "kills" => self.kills = v.parse().unwrap_or(self.kills),
                "deaths" => self.deaths = v.parse().unwrap_or(self.deaths),
                "matches" => self.matches = v.parse().unwrap_or(self.matches),
                "best_streak" => self.best_streak = v.parse().unwrap_or(self.best_streak),
                "best_wave" => self.best_wave = v.parse().unwrap_or(self.best_wave),
                _ => {}
            }
        }
    }

    fn to_text(&self) -> String {
        format!(
            "name={}\nkills={}\ndeaths={}\nmatches={}\nbest_streak={}\nbest_wave={}\n",
            self.name, self.kills, self.deaths, self.matches, self.best_streak, self.best_wave
        )
    }

    /// Persist to disk (best-effort; a failure to write is silently ignored).
    pub fn save(&self) {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, self.to_text());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kd_and_skill_are_sane() {
        let mut p = Profile::default();
        assert_eq!(p.kd(), 0.0);
        p.record_life(10, 4, 6);
        assert!((p.kd() - 10.0).abs() < 1e-6, "10 kills / 1 death");
        p.record_life(2, 2, 1);
        assert_eq!(p.deaths, 2);
        assert!((p.kd() - 6.0).abs() < 1e-6, "12 kills / 2 deaths");
        assert_eq!(p.best_wave, 4);
        assert!(p.skill() <= 100);
        assert!(!p.tier().is_empty());
    }

    #[test]
    fn round_trips_through_text() {
        let mut p = Profile::default();
        p.record_life(7, 5, 9);
        p.record_match();
        p.name = "ACE".to_string();
        let text = p.to_text();
        let mut q = Profile::default();
        q.merge_from_text(&text);
        assert_eq!(p, q);
    }
}
