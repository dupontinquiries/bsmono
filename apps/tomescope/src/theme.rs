//! Reading themes: the colour scheme used for the *page content itself*
//! (background + text + accent while reading), which is deliberately
//! kept separate from the app chrome palette (`snapclone_theme::PaletteId`
//! used for menus/toolbars). A reader wants "sepia page, dark toolbar" as
//! a very common combination, so these are two independent choices that
//! both get applied when rendering the reader screen.

use eframe::egui::Color32;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReadingTheme {
    Paper,
    Sepia,
    Slate,
    Night,
    EInk,
    Forest,
    HighContrast,
}

impl ReadingTheme {
    pub fn all() -> &'static [ReadingTheme] {
        &[
            ReadingTheme::Paper,
            ReadingTheme::Sepia,
            ReadingTheme::Slate,
            ReadingTheme::Night,
            ReadingTheme::EInk,
            ReadingTheme::Forest,
            ReadingTheme::HighContrast,
        ]
    }

    pub fn label(&self) -> &'static str {
        match self {
            ReadingTheme::Paper => "Paper",
            ReadingTheme::Sepia => "Sepia",
            ReadingTheme::Slate => "Slate",
            ReadingTheme::Night => "Night",
            ReadingTheme::EInk => "E-Ink",
            ReadingTheme::Forest => "Forest",
            ReadingTheme::HighContrast => "High contrast",
        }
    }

    /// (background, text, accent/link) colours for the reading surface.
    pub fn colors(&self) -> (Color32, Color32, Color32) {
        match self {
            ReadingTheme::Paper => (
                Color32::from_rgb(250, 248, 242),
                Color32::from_rgb(30, 28, 24),
                Color32::from_rgb(120, 84, 40),
            ),
            ReadingTheme::Sepia => (
                Color32::from_rgb(240, 224, 191),
                Color32::from_rgb(59, 44, 30),
                Color32::from_rgb(139, 90, 43),
            ),
            ReadingTheme::Slate => (
                Color32::from_rgb(45, 50, 58),
                Color32::from_rgb(214, 219, 224),
                Color32::from_rgb(120, 170, 220),
            ),
            ReadingTheme::Night => (
                Color32::from_rgb(14, 14, 18),
                Color32::from_rgb(196, 196, 204),
                Color32::from_rgb(150, 140, 220),
            ),
            ReadingTheme::EInk => (
                Color32::from_rgb(255, 255, 255),
                Color32::from_rgb(0, 0, 0),
                Color32::from_rgb(60, 60, 60),
            ),
            ReadingTheme::Forest => (
                Color32::from_rgb(232, 238, 226),
                Color32::from_rgb(35, 46, 33),
                Color32::from_rgb(74, 122, 71),
            ),
            ReadingTheme::HighContrast => (
                Color32::from_rgb(0, 0, 0),
                Color32::from_rgb(255, 255, 0),
                Color32::from_rgb(0, 220, 255),
            ),
        }
    }

    /// Whether this theme should suppress animation/gradient chrome (for
    /// e-ink displays where ghosting makes smooth motion counterproductive).
    pub fn prefers_static_chrome(&self) -> bool {
        matches!(self, ReadingTheme::EInk)
    }

    /// A recommended snapclone-theme app-chrome palette id to pair with
    /// this reading theme by default, so picking a reading theme also
    /// nudges the surrounding UI to something coherent (the user can
    /// still override the app palette independently).
    pub fn suggested_app_palette(&self) -> &'static str {
        match self {
            ReadingTheme::Paper | ReadingTheme::Sepia | ReadingTheme::Forest | ReadingTheme::EInk => "light",
            ReadingTheme::Slate | ReadingTheme::Night | ReadingTheme::HighContrast => "dark",
        }
    }
}

impl Default for ReadingTheme {
    fn default() -> Self {
        ReadingTheme::Paper
    }
}

/// Reader-local preferences that live alongside (not inside) the shared
/// `settings_kit::Prefs`, since these are specific to book reading rather
/// than general app/accessibility settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReaderPrefs {
    pub reading_theme: ReadingTheme,
    pub sync_app_palette_to_theme: bool,
    pub keep_screen_awake: bool,
    pub show_status_bar: bool,
    pub show_progress_percent: bool,
    pub tap_zones_enabled: bool,
    pub volume_key_page_turn: bool,
    pub auto_night_by_clock: bool,
    pub night_start_hour: u8,
    pub night_end_hour: u8,
}

impl Default for ReaderPrefs {
    fn default() -> Self {
        Self {
            reading_theme: ReadingTheme::default(),
            sync_app_palette_to_theme: true,
            keep_screen_awake: true,
            show_status_bar: true,
            show_progress_percent: true,
            tap_zones_enabled: true,
            volume_key_page_turn: false,
            auto_night_by_clock: false,
            night_start_hour: 21,
            night_end_hour: 7,
        }
    }
}

impl ReaderPrefs {
    /// If auto-night is enabled, returns the theme that should currently
    /// be in effect regardless of the stored preference, based on the
    /// given local hour (0..=23).
    pub fn effective_theme(&self, local_hour: u8) -> ReadingTheme {
        if !self.auto_night_by_clock {
            return self.reading_theme;
        }
        let in_night = if self.night_start_hour <= self.night_end_hour {
            local_hour >= self.night_start_hour && local_hour < self.night_end_hour
        } else {
            // wraps past midnight, e.g. 21 -> 7
            local_hour >= self.night_start_hour || local_hour < self.night_end_hour
        };
        if in_night {
            ReadingTheme::Night
        } else {
            self.reading_theme
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn night_window_wraps_midnight() {
        let prefs = ReaderPrefs {
            auto_night_by_clock: true,
            night_start_hour: 21,
            night_end_hour: 7,
            ..ReaderPrefs::default()
        };
        assert_eq!(prefs.effective_theme(23), ReadingTheme::Night);
        assert_eq!(prefs.effective_theme(3), ReadingTheme::Night);
        assert_eq!(prefs.effective_theme(12), ReadingTheme::Paper);
    }

    #[test]
    fn all_themes_have_distinct_colors() {
        let mut seen = std::collections::HashSet::new();
        for theme in ReadingTheme::all() {
            let (bg, _, _) = theme.colors();
            seen.insert((bg.r(), bg.g(), bg.b()));
        }
        assert_eq!(seen.len(), ReadingTheme::all().len());
    }
}
