//! The shell's current colours, for apps that want to match it.
//!
//! tideshell dresses to its surroundings: an accent from the place and the
//! time of day, surfaces tinted warm or cool, contrast that rises with the
//! ambient light, a light look in sunlight. Until now that palette stayed
//! inside the shell, and an app sat on its own fixed theme next to it.
//!
//! The contract, in the same style as the place hint (see tideshell's
//! `deploy/AMBIENCE-API.md`):
//!
//! `$XDG_RUNTIME_DIR/tideshell.palette`, one line, replaced atomically:
//! ```text
//! v1 light=0 seq=41 bg_base=#101418ff bg_card=#181d23ff ... accent=#d9905aff
//! ```
//! Unknown keys are ignored, so the shell can add roles without breaking
//! readers; a missing role keeps the app's own colour.
//!
//! Costs, because a phone pays them: the shell writes at most twice a second
//! and only when a colour moved by a visible amount; a reader `stat`s the file
//! once a second and reads it only when its mtime changed.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub type Rgba = [u8; 4];

/// Every role is optional on the wire; `None` means "keep yours".
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AmbientPalette {
    pub light: bool,
    /// Increments per write, so a reader can tell a rewrite from a replay.
    pub seq: u64,
    pub bg_base: Option<Rgba>,
    pub bg_card: Option<Rgba>,
    pub bg_raised: Option<Rgba>,
    pub bg_hover: Option<Rgba>,
    pub text: Option<Rgba>,
    pub text_weak: Option<Rgba>,
    pub accent: Option<Rgba>,
    pub accent_soft: Option<Rgba>,
    pub border: Option<Rgba>,
}

const ROLES: [&str; 9] = ["bg_base", "bg_card", "bg_raised", "bg_hover", "text", "text_weak", "accent", "accent_soft", "border"];

impl AmbientPalette {
    fn role(&self, name: &str) -> Option<Rgba> {
        match name {
            "bg_base" => self.bg_base,
            "bg_card" => self.bg_card,
            "bg_raised" => self.bg_raised,
            "bg_hover" => self.bg_hover,
            "text" => self.text,
            "text_weak" => self.text_weak,
            "accent" => self.accent,
            "accent_soft" => self.accent_soft,
            "border" => self.border,
            _ => None,
        }
    }

    fn role_mut(&mut self, name: &str) -> Option<&mut Option<Rgba>> {
        Some(match name {
            "bg_base" => &mut self.bg_base,
            "bg_card" => &mut self.bg_card,
            "bg_raised" => &mut self.bg_raised,
            "bg_hover" => &mut self.bg_hover,
            "text" => &mut self.text,
            "text_weak" => &mut self.text_weak,
            "accent" => &mut self.accent,
            "accent_soft" => &mut self.accent_soft,
            "border" => &mut self.border,
            _ => return None,
        })
    }

    pub fn to_line(&self) -> String {
        let mut s = format!("v1 light={} seq={}", self.light as u8, self.seq);
        for r in ROLES {
            if let Some(c) = self.role(r) {
                s.push_str(&format!(" {r}=#{:02x}{:02x}{:02x}{:02x}", c[0], c[1], c[2], c[3]));
            }
        }
        s
    }

    /// Parse a line. `None` for anything that is not a v1 line; a malformed
    /// colour is skipped (that role keeps the app's own), not fatal.
    pub fn from_line(line: &str) -> Option<Self> {
        let mut it = line.split_whitespace();
        if it.next()? != "v1" {
            return None;
        }
        let mut p = AmbientPalette::default();
        for kv in it {
            let Some((k, v)) = kv.split_once('=') else { continue };
            match k {
                "light" => p.light = v == "1",
                "seq" => p.seq = v.parse().unwrap_or(0),
                _ => {
                    if let (Some(slot), Some(c)) = (p.role_mut(k), parse_hex(v)) {
                        *slot = Some(c);
                    }
                }
            }
        }
        Some(p)
    }

    /// The largest per-channel difference to `other`, over roles both have;
    /// a role present in one and not the other counts as maximal.
    pub fn distance(&self, other: &Self) -> u8 {
        let mut d = 0u8;
        for r in ROLES {
            match (self.role(r), other.role(r)) {
                (Some(a), Some(b)) => {
                    for i in 0..4 {
                        d = d.max(a[i].abs_diff(b[i]));
                    }
                }
                (None, None) => {}
                _ => return u8::MAX,
            }
        }
        d
    }
}

/// `#rrggbb` or `#rrggbbaa`.
pub fn parse_hex(s: &str) -> Option<Rgba> {
    let h = s.strip_prefix('#')?;
    let byte = |i: usize| h.get(i..i + 2).and_then(|x| u8::from_str_radix(x, 16).ok());
    match h.len() {
        6 => Some([byte(0)?, byte(2)?, byte(4)?, 255]),
        8 => Some([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
        _ => None,
    }
}

/// `$XDG_RUNTIME_DIR/tideshell.palette`.
pub fn runtime_path() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(|r| PathBuf::from(r).join("tideshell.palette"))
}

/// Decides when the shell writes. Pure.
///
/// The shell's palette is recomputed every frame and springs through
/// intermediate values during a change; writing each would be 60 writes a
/// second to a file every app is watching. A write happens when a colour has
/// moved by at least `min_step` (2/255 is below what anyone sees) and
/// `min_interval_ms` has passed -- except a light/dark flip, which is written
/// at once, because half a second of dark text on a sunlit screen is the
/// failure this whole thing exists to avoid.
pub struct Publisher {
    last: Option<AmbientPalette>,
    last_ms: u64,
    seq: u64,
    pub min_step: u8,
    pub min_interval_ms: u64,
}

impl Default for Publisher {
    fn default() -> Self {
        Self { last: None, last_ms: 0, seq: 0, min_step: 3, min_interval_ms: 500 }
    }
}

impl Publisher {
    /// Offer the current palette; returns the line to write, if any.
    pub fn offer(&mut self, mut p: AmbientPalette, now_ms: u64) -> Option<String> {
        let due = match &self.last {
            None => true,
            Some(last) if last.light != p.light => true,
            Some(last) => {
                now_ms.saturating_sub(self.last_ms) >= self.min_interval_ms && p.distance(last) >= self.min_step
            }
        };
        if !due {
            return None;
        }
        self.seq += 1;
        p.seq = self.seq;
        let line = p.to_line();
        self.last = Some(p);
        self.last_ms = now_ms;
        Some(line)
    }
}

/// Replace the file atomically: a reader never sees half a line.
pub fn write_atomic(path: &Path, line: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("palette.tmp");
    std::fs::write(&tmp, format!("{line}\n"))?;
    std::fs::rename(&tmp, path)
}

/// Follows the file from an app. `poll` is cheap to call every frame: it
/// stats at most once per `interval_ms` and reads only when the mtime (or
/// size) changed.
pub struct Watcher {
    path: PathBuf,
    seen: Option<(SystemTime, u64)>,
    last_check_ms: Option<u64>,
    pub interval_ms: u64,
    /// Whether the file existed at the last check: "the shell is not
    /// publishing" is worth showing, not an error.
    pub present: bool,
}

impl Watcher {
    pub fn new(path: PathBuf) -> Self {
        Self { path, seen: None, last_check_ms: None, interval_ms: 1000, present: false }
    }

    pub fn at_runtime_dir() -> Option<Self> {
        runtime_path().map(Self::new)
    }

    /// A new palette, if the file changed since the last call.
    pub fn poll(&mut self, now_ms: u64) -> Option<AmbientPalette> {
        if self.last_check_ms.is_some_and(|t| now_ms.saturating_sub(t) < self.interval_ms) {
            return None;
        }
        self.last_check_ms = Some(now_ms);
        let meta = match std::fs::metadata(&self.path) {
            Ok(m) => m,
            Err(_) => {
                self.present = false;
                self.seen = None;
                return None;
            }
        };
        self.present = true;
        let stamp = (meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len());
        if self.seen == Some(stamp) {
            return None;
        }
        self.seen = Some(stamp);
        let text = std::fs::read_to_string(&self.path).ok()?;
        AmbientPalette::from_line(text.lines().next()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pal(accent: Rgba, light: bool) -> AmbientPalette {
        AmbientPalette { light, accent: Some(accent), bg_base: Some([16, 20, 24, 255]), text: Some([230, 230, 230, 255]), ..Default::default() }
    }

    #[test]
    fn a_line_round_trips_and_missing_roles_stay_missing() {
        let p = AmbientPalette { seq: 7, ..pal([217, 144, 90, 255], true) };
        let line = p.to_line();
        assert!(line.starts_with("v1 light=1 seq=7 "), "{line}");
        assert!(line.contains("accent=#d9905aff"));
        assert!(!line.contains("border="), "absent roles are not written");
        assert_eq!(AmbientPalette::from_line(&line), Some(p));
    }

    #[test]
    fn readers_tolerate_the_future_and_reject_the_foreign() {
        let p = AmbientPalette::from_line("v1 light=0 accent=#112233 glow2=#ffffff text=#zz0000 bogus").unwrap();
        assert_eq!(p.accent, Some([0x11, 0x22, 0x33, 255]), "6-digit hex is opaque");
        assert_eq!(p.text, None, "a malformed colour keeps the app's own");
        assert_eq!(AmbientPalette::from_line("v2 accent=#112233"), None);
        assert_eq!(AmbientPalette::from_line("landmark=x name=y"), None);
    }

    #[test]
    fn the_publisher_skips_invisible_steps_and_throttles() {
        let mut pb = Publisher::default();
        assert!(pb.offer(pal([200, 100, 50, 255], false), 0).is_some(), "the first is always written");
        assert!(pb.offer(pal([201, 100, 50, 255], false), 1000).is_none(), "1/255 is invisible");
        assert!(pb.offer(pal([220, 100, 50, 255], false), 100).is_none(), "too soon");
        let l = pb.offer(pal([220, 100, 50, 255], false), 600).expect("moved and due");
        assert!(l.contains("seq=2"), "{l}");
    }

    /// Sixty frames a second of a springing accent: a handful of writes.
    #[test]
    fn an_animation_costs_at_most_two_writes_a_second() {
        let mut pb = Publisher::default();
        let mut writes = 0;
        for f in 0..180u64 {
            let t = f as f32 / 180.0;
            let r = (100.0 + 150.0 * t) as u8;
            if pb.offer(pal([r, 80, 60, 255], false), f * 16).is_some() {
                writes += 1;
            }
        }
        assert!((3..=7).contains(&writes), "{writes} writes over 3 s");
    }

    #[test]
    fn a_light_flip_is_written_at_once() {
        let mut pb = Publisher::default();
        pb.offer(pal([200, 100, 50, 255], false), 0);
        assert!(pb.offer(pal([200, 100, 50, 255], true), 10).is_some());
    }

    #[test]
    fn the_watcher_reads_only_on_change_and_at_most_once_a_second() {
        let dir = std::env::temp_dir().join(format!("ambient-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tideshell.palette");
        let mut w = Watcher::new(path.clone());
        assert_eq!(w.poll(0), None);
        assert!(!w.present);
        write_atomic(&path, &pal([1, 2, 3, 255], false).to_line()).unwrap();
        assert_eq!(w.poll(500), None, "within the interval: not even a stat");
        assert_eq!(w.poll(1000).and_then(|p| p.accent), Some([1, 2, 3, 255]));
        assert!(w.present);
        assert_eq!(w.poll(2000), None, "unchanged file: no palette");
        // A rewrite of a different length is seen even within one mtime tick.
        write_atomic(&path, &AmbientPalette { border: Some([9, 9, 9, 255]), ..pal([4, 5, 6, 255], true) }.to_line()).unwrap();
        let p = w.poll(3000).expect("changed");
        assert!(p.light && p.accent == Some([4, 5, 6, 255]));
        std::fs::remove_file(&path).unwrap();
        assert_eq!(w.poll(4000), None);
        assert!(!w.present, "the shell stopped publishing");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
