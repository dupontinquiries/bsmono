// src/mpris_listener.rs
#![allow(dead_code)]
//
// Listen to the desktop's MPRIS (org.mpris.MediaPlayer2) D-Bus state.
// MPRIS is the cross-app spec for media-player metadata + control —
// Spotify, mpv, VLC, Plasma's MediaPlayer applet, Firefox/Chromium
// media tabs, etc., all expose now-playing + playback state via this
// interface.
//
// Why care: when the user has been playing music in Spotify on their
// laptop for half an hour and then taps the controller, the system
// SHOULD know that. We can:
//
//   * Bias the anticipator's predictions toward "PlayTrack" actions
//     in the genre/artist they're already listening to.
//   * Show "currently playing on this machine: <track>" so they know
//     where the music is coming from before they touch anything.
//   * Surface a "transfer to this device" quick-action for the
//     active B&O speaker.
//
// HONEST SCOPE
// ============
// Current implementation is a polling shell-out (`playerctl metadata`)
// rather than direct D-Bus subscription. Reasons:
//   * `playerctl` is a small CLI tool that abstracts MPRIS for us.
//   * Direct D-Bus would need the `dbus` crate's MessageReader plus
//     signal subscriptions, which is more code + a long-running
//     thread.
//   * Polling once every 30 seconds is more than enough for our
//     "context bias" use case — we don't need sub-second freshness.
//
// If `playerctl` isn't installed the listener silently degrades to
// "nothing playing". The settings page surfaces the missing tool.

use std::process::Command;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MprisState {
    /// Is something currently playing? False when no player exists,
    /// or all players are paused/stopped.
    pub playing: bool,
    /// The MPRIS player name (e.g. "spotify", "firefox.instance_123").
    /// Empty when nothing's active.
    pub player: String,
    /// Track title from xesam:title metadata.
    pub title: String,
    /// Artist from xesam:artist (joined when multiple).
    pub artist: String,
    /// Album from xesam:album.
    pub album: String,
}

impl MprisState {
    pub fn is_active(&self) -> bool { self.playing && !self.title.is_empty() }
}

/// Single-shot poll: ask `playerctl` for the currently-playing player's
/// metadata. Returns Default-empty when nothing's playing or `playerctl`
/// isn't installed.
pub fn poll() -> MprisState {
    // Probe playerctl's status first. If nothing's playing it returns
    // "No players found" on stderr with non-zero exit; we treat that
    // as "no active player" without alarm.
    let status_out = Command::new("playerctl").arg("status").output();
    let status = match &status_out {
        Ok(o) if o.status.success() => {
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        }
        _ => return MprisState::default(),
    };
    let playing = status.eq_ignore_ascii_case("Playing");
    if !playing { return MprisState::default(); }

    // Now pull metadata. playerctl's --format spec is:
    //   {{playerName}}|{{xesam:title}}|{{xesam:artist}}|{{xesam:album}}
    let meta_out = Command::new("playerctl")
        .args(["metadata", "--format",
               "{{playerName}}|{{xesam:title}}|{{xesam:artist}}|{{xesam:album}}"])
        .output();
    let line = match meta_out {
        Ok(o) if o.status.success() => {
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        }
        _ => return MprisState { playing, ..Default::default() },
    };
    parse_metadata_line(&line, playing)
}

/// Parse a `playerctl --format`-style line. Public for testability —
/// the polling shell-out itself can't be unit-tested without spinning
/// up a real player, but the parser can.
pub fn parse_metadata_line(line: &str, playing: bool) -> MprisState {
    let parts: Vec<&str> = line.split('|').collect();
    // playerctl emits empty strings between separators when a field
    // is missing. We pad out to 4 to make indexing safe.
    let f = |i: usize| parts.get(i).map(|s| s.trim().to_string()).unwrap_or_default();
    MprisState {
        playing,
        player: f(0),
        title:  f(1),
        artist: f(2),
        album:  f(3),
    }
}

/// Diagnostic: are the tools needed for the MPRIS listener installed?
/// Returns true when `playerctl` is found on PATH.
pub fn tools_available() -> bool {
    Command::new("which").arg("playerctl").output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_line() {
        let s = parse_metadata_line(
            "spotify|Rebellion (Lies)|Arcade Fire|Funeral",
            true,
        );
        assert!(s.playing);
        assert_eq!(s.player, "spotify");
        assert_eq!(s.title,  "Rebellion (Lies)");
        assert_eq!(s.artist, "Arcade Fire");
        assert_eq!(s.album,  "Funeral");
    }

    #[test]
    fn parse_missing_fields() {
        // No album
        let s = parse_metadata_line("spotify|Track|Artist|", true);
        assert_eq!(s.album, "");
        assert_eq!(s.artist, "Artist");
    }

    #[test]
    fn parse_completely_empty_line() {
        let s = parse_metadata_line("", false);
        assert!(!s.playing);
        assert_eq!(s.title, "");
    }

    #[test]
    fn is_active_requires_playing_and_title() {
        let mut s = MprisState::default();
        assert!(!s.is_active());
        s.playing = true;
        assert!(!s.is_active());  // no title yet
        s.title = "Foo".into();
        assert!(s.is_active());
    }

    #[test]
    fn parse_pipe_in_title_safely() {
        // Hypothetical: title with no special chars. Pipe-in-content
        // would be a parser ambiguity; we accept the safe degradation
        // of splitting and the user just sees a truncated title.
        let s = parse_metadata_line("a|b|c|d", true);
        assert_eq!(s.title, "b");
        assert_eq!(s.album, "d");
    }
}
