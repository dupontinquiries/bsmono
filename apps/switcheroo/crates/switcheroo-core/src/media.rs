//! Media control: the same seam shape as the radio, for the player.
//!
//! On Linux the player is whatever speaks MPRIS over D-Bus, and `playerctl`
//! is the one-line way to drive it without a D-Bus crate in this tree. The
//! argv mapping lives here so the shell's job is `Command::new`; a fabric
//! backend that forwards to another node submits the same
//! [`MediaCommand`]s.

pub use crate::intent::MediaCommand;

/// The player, behind the same sync/poll seam as `Backend`.
pub trait MediaBackend {
    fn submit(&mut self, cmd: MediaCommand);
    /// Status lines since last poll, for the journal.
    fn poll(&mut self) -> Vec<String>;
    fn describe(&self) -> String;
}

/// `playerctl` arguments for a command. Volume steps are relative (`0.1+`),
/// mute is `volume 0` — playerctl has no toggle-mute, and a remembered
/// pre-mute level is UI state, not the media seam's.
pub fn playerctl_argv(cmd: MediaCommand) -> Vec<&'static str> {
    match cmd {
        MediaCommand::PlayPause => vec!["play-pause"],
        MediaCommand::Play => vec!["play"],
        MediaCommand::Pause => vec!["pause"],
        MediaCommand::Stop => vec!["stop"],
        MediaCommand::Next => vec!["next"],
        MediaCommand::Previous => vec!["previous"],
        MediaCommand::VolumeUp => vec!["volume", "0.1+"],
        MediaCommand::VolumeDown => vec!["volume", "0.1-"],
        MediaCommand::Mute => vec!["volume", "0"],
    }
}

/// Parse `playerctl status` output.
pub fn parse_status(out: &str) -> Option<bool> {
    match out.trim() {
        "Playing" => Some(true),
        "Paused" | "Stopped" => Some(false),
        _ => None,
    }
}

/// A media backend that records what it was asked, for tests and the demo.
#[derive(Debug, Default)]
pub struct DemoMedia {
    pub playing: bool,
    pub journal: Vec<String>,
    pending: Vec<String>,
}

impl MediaBackend for DemoMedia {
    fn submit(&mut self, cmd: MediaCommand) {
        match cmd {
            MediaCommand::Play => self.playing = true,
            MediaCommand::Pause | MediaCommand::Stop => self.playing = false,
            MediaCommand::PlayPause => self.playing = !self.playing,
            _ => {}
        }
        let line = format!("media: playerctl {}", playerctl_argv(cmd).join(" "));
        self.journal.push(line.clone());
        self.pending.push(line);
    }
    fn poll(&mut self) -> Vec<String> { std::mem::take(&mut self.pending) }
    fn describe(&self) -> String { "demo media".into() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_maps_to_a_playerctl_line() {
        assert_eq!(playerctl_argv(MediaCommand::Next), vec!["next"]);
        assert_eq!(playerctl_argv(MediaCommand::VolumeUp), vec!["volume", "0.1+"]);
        assert_eq!(playerctl_argv(MediaCommand::Mute), vec!["volume", "0"]);
    }

    #[test]
    fn status_parses_the_three_words() {
        assert_eq!(parse_status("Playing\n"), Some(true));
        assert_eq!(parse_status("Paused"), Some(false));
        assert_eq!(parse_status("No players found"), None);
    }

    #[test]
    fn the_demo_tracks_play_state_and_journals() {
        let mut m = DemoMedia::default();
        m.submit(MediaCommand::Play);
        assert!(m.playing);
        m.submit(MediaCommand::PlayPause);
        assert!(!m.playing);
        assert_eq!(m.poll().len(), 2);
        assert!(m.poll().is_empty());
    }
}
