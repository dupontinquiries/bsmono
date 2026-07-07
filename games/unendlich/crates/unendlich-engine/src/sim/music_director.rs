//! `sim::music_director` — maps the app/menu state to what music should be playing
//! and at what volume, with fades. Pure: the bin owns the audio backend (decode +
//! output of `assets/music/title_koyopper.opus`); this decides *what cue* and drives
//! a fader so transitions (menu → loading → gameplay) cross-fade instead of cutting.

use crate::sim::game_flow::AppState;

/// Logical tracks (extend as more music lands). `None` = silence/gameplay ambience.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MusicTrack {
    None,
    TitleTheme, // assets/music/title_koyopper.opus
}

/// What the director wants playing right now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MusicCue {
    pub track: MusicTrack,
    pub target_volume01: f32,
    pub fade_ms: u32,
}

/// The cue for a given app state: full title theme in the menu, ducked while
/// loading/paused, faded out into gameplay.
pub fn cue_for(state: AppState) -> MusicCue {
    match state {
        AppState::MainMenu => MusicCue { track: MusicTrack::TitleTheme, target_volume01: 1.0, fade_ms: 800 },
        AppState::Loading => MusicCue { track: MusicTrack::TitleTheme, target_volume01: 0.5, fade_ms: 600 },
        AppState::Paused => MusicCue { track: MusicTrack::TitleTheme, target_volume01: 0.35, fade_ms: 400 },
        AppState::InGame => MusicCue { track: MusicTrack::None, target_volume01: 0.0, fade_ms: 1500 },
        AppState::Exiting => MusicCue { track: MusicTrack::None, target_volume01: 0.0, fade_ms: 300 },
    }
}

/// The carried playback state the bin mirrors onto the audio backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MusicState {
    pub track: MusicTrack,
    pub volume01: f32,
}

impl Default for MusicState {
    fn default() -> Self {
        MusicState { track: MusicTrack::None, volume01: 0.0 }
    }
}

impl MusicState {
    pub fn new() -> MusicState {
        MusicState::default()
    }

    /// Advance toward the cue over `dt_ms`. A track change fades the old out first
    /// (volume to 0) then swaps and fades the new in — so the bin only ever needs to
    /// play `self.track` at `self.volume01`.
    pub fn step(&mut self, cue: &MusicCue, dt_ms: u32) -> MusicState {
        let dt = dt_ms.max(0) as f32;
        if cue.track != self.track {
            // fade the current track out before switching
            let rate = if cue.fade_ms > 0 { 1.0 / cue.fade_ms as f32 } else { 1.0 };
            self.volume01 = (self.volume01 - rate * dt).max(0.0);
            if self.volume01 <= 0.0 {
                self.track = cue.track;
                self.volume01 = 0.0;
            }
            return *self;
        }
        // same track: approach the target volume
        let rate = if cue.fade_ms > 0 { 1.0 / cue.fade_ms as f32 } else { 1.0 };
        let delta = rate * dt;
        if self.volume01 < cue.target_volume01 {
            self.volume01 = (self.volume01 + delta).min(cue.target_volume01);
        } else {
            self.volume01 = (self.volume01 - delta).max(cue.target_volume01);
        }
        *self
    }

    /// What asset the bin should have loaded for the current track.
    pub fn asset(&self) -> Option<&'static str> {
        match self.track {
            MusicTrack::TitleTheme => Some("assets/music/title_koyopper.opus"),
            MusicTrack::None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_plays_title_theme_at_full() {
        let c = cue_for(AppState::MainMenu);
        assert_eq!(c.track, MusicTrack::TitleTheme);
        assert_eq!(c.target_volume01, 1.0);
    }

    #[test]
    fn fades_in_on_the_menu() {
        let mut m = MusicState::new();
        let c = cue_for(AppState::MainMenu);
        // first step switches to the track (from None), subsequent steps fade in
        for _ in 0..20 {
            m.step(&c, 200);
        }
        assert_eq!(m.track, MusicTrack::TitleTheme);
        assert!(m.volume01 > 0.5, "ramps up toward full over time, got {}", m.volume01);
        assert!(m.volume01 <= 1.0);
    }

    #[test]
    fn ducks_while_loading_then_fades_out_in_game() {
        let mut m = MusicState { track: MusicTrack::TitleTheme, volume01: 1.0 };
        // loading ducks toward 0.5
        for _ in 0..20 {
            m.step(&cue_for(AppState::Loading), 200);
        }
        assert!((m.volume01 - 0.5).abs() < 0.06, "ducked near 0.5, got {}", m.volume01);
        // entering gameplay fades the theme out and switches to None
        for _ in 0..40 {
            m.step(&cue_for(AppState::InGame), 200);
        }
        assert_eq!(m.track, MusicTrack::None);
        assert_eq!(m.volume01, 0.0);
    }

    #[test]
    fn asset_path_for_title() {
        let m = MusicState { track: MusicTrack::TitleTheme, volume01: 1.0 };
        assert_eq!(m.asset(), Some("assets/music/title_koyopper.opus"));
        assert_eq!(MusicState::new().asset(), None);
    }

    #[test]
    fn deterministic_and_bounded() {
        let mut a = MusicState::new();
        let mut b = MusicState::new();
        for _ in 0..50 {
            a.step(&cue_for(AppState::MainMenu), 100);
            b.step(&cue_for(AppState::MainMenu), 100);
            assert_eq!(a, b);
            assert!((0.0..=1.0).contains(&a.volume01));
        }
    }
}
