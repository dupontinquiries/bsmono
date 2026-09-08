//! Recognising what a pasted string is, and keeping what yt-dlp already told
//! us about the rest of the channel.
//!
//! Two jobs, both pure so they need no network to test:
//!
//! 1. Decide whether text on the clipboard is something playable, and what
//!    kind. A command palette that offers "play from clipboard" for every
//!    string is noise; one that only recognises `youtube.com/watch?v=` misses
//!    most of what people actually copy.
//! 2. Keep the related/channel information `yt-dlp -J` already returns.
//!    Resolving a video downloads a description of the uploader and, for a
//!    playlist or a channel URL, of its entries — all of which was being parsed
//!    and thrown away. A sidebar of "more from this channel" is then free,
//!    where fetching it separately would be another yt-dlp process per video.

use serde::Deserialize;

/// What a piece of clipboard text appears to be.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Paste {
    /// A single video on a site yt-dlp handles.
    Video { url: String, id: Option<String> },
    /// A channel or user page; its entries are the videos.
    Channel { url: String, id: Option<String> },
    /// A playlist.
    Playlist { url: String, id: Option<String> },
    /// A local file path that exists.
    LocalFile(std::path::PathBuf),
    /// Nothing recognisable.
    Nothing,
}

impl Paste {
    pub fn is_playable(&self) -> bool {
        !matches!(self, Paste::Nothing)
    }

    /// One line for the palette entry.
    pub fn label(&self) -> String {
        match self {
            Paste::Video { id: Some(id), .. } => format!("play video {id}"),
            Paste::Video { .. } => "play video from clipboard".into(),
            Paste::Channel { id: Some(id), .. } => format!("browse channel {id}"),
            Paste::Channel { .. } => "browse channel from clipboard".into(),
            Paste::Playlist { id: Some(id), .. } => format!("open playlist {id}"),
            Paste::Playlist { .. } => "open playlist from clipboard".into(),
            Paste::LocalFile(p) => format!("open {}", p.display()),
            Paste::Nothing => "nothing playable on the clipboard".into(),
        }
    }
}

fn query_param<'a>(url: &'a str, key: &str) -> Option<&'a str> {
    let q = url.split_once('?')?.1;
    q.split('&')
        .find_map(|pair| pair.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| v))
}

/// Classify clipboard text.
///
/// Deliberately lenient about the host: yt-dlp handles well over a thousand
/// sites, so refusing anything that is not youtube.com would throw away most of
/// what it can actually play. Anything that parses as an http(s) URL is offered
/// as a video and yt-dlp gets to decide — it fails with a clear message, which
/// is a better outcome than the palette silently not offering the entry.
pub fn classify(text: &str) -> Paste {
    let t = text.trim();
    if t.is_empty() {
        return Paste::Nothing;
    }

    // A local path first: it needs no network and an existing file is
    // unambiguous.
    if t.starts_with('/') || t.starts_with("~/") || t.starts_with("file://") {
        let p = std::path::PathBuf::from(t.trim_start_matches("file://"));
        if p.is_file() {
            return Paste::LocalFile(p);
        }
    }

    if !(t.starts_with("http://") || t.starts_with("https://")) {
        // A bare 11-character YouTube id is a common thing to copy.
        if t.len() == 11 && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Paste::Video {
                url: format!("https://www.youtube.com/watch?v={t}"),
                id: Some(t.to_string()),
            };
        }
        return Paste::Nothing;
    }

    // Whitespace or a newline means someone copied a sentence containing a
    // link, not a link. Taking the first token would guess at what they meant.
    if t.split_whitespace().count() > 1 {
        return Paste::Nothing;
    }

    let lower = t.to_ascii_lowercase();
    let is_youtube = lower.contains("youtube.com") || lower.contains("youtu.be");

    if is_youtube {
        // Order matters: a watch URL can carry a `list=`, and the video is what
        // was asked for. Checking playlist first would open the whole list from
        // a link to one video in it.
        if let Some(v) = query_param(t, "v") {
            return Paste::Video {
                url: t.to_string(),
                id: Some(v.split('&').next().unwrap_or(v).to_string()),
            };
        }
        if lower.contains("youtu.be/") {
            let id = t.rsplit('/').next().and_then(|s| s.split('?').next());
            return Paste::Video {
                url: t.to_string(),
                id: id.map(str::to_string).filter(|s| !s.is_empty()),
            };
        }
        if let Some(l) = query_param(t, "list") {
            return Paste::Playlist {
                url: t.to_string(),
                id: Some(l.to_string()),
            };
        }
        for marker in ["/channel/", "/user/", "/c/", "/@"] {
            if let Some(i) = t.find(marker) {
                let rest = &t[i + marker.len()..];
                let id = rest.split(['/', '?']).next().unwrap_or("");
                return Paste::Channel {
                    url: t.to_string(),
                    id: (!id.is_empty()).then(|| id.to_string()),
                };
            }
        }
        // A bare youtube.com with nothing else on it.
        return Paste::Nothing;
    }

    Paste::Video {
        url: t.to_string(),
        id: None,
    }
}

/// The uploader and sibling videos `yt-dlp -J` already returned.
///
/// Parsed from the same JSON the format selection reads, so a channel sidebar
/// costs nothing extra. Fetching it separately would be another yt-dlp process
/// per video, which on a phone is seconds and a spun-up Python interpreter.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Related {
    pub channel: Option<String>,
    pub channel_id: Option<String>,
    pub channel_url: Option<String>,
    pub uploader: Option<String>,
    /// Present when the URL was a playlist or channel: each is a video.
    #[serde(default)]
    pub entries: Vec<RelatedEntry>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RelatedEntry {
    pub id: Option<String>,
    pub title: Option<String>,
    pub url: Option<String>,
    pub duration: Option<f64>,
}

impl RelatedEntry {
    /// A playable URL, reconstructed from the id when the extractor gave only
    /// that — which is what a flat playlist listing does.
    pub fn watch_url(&self) -> Option<String> {
        if let Some(u) = &self.url {
            if u.starts_with("http") {
                return Some(u.clone());
            }
        }
        self.id
            .as_ref()
            .map(|id| format!("https://www.youtube.com/watch?v={id}"))
    }

    pub fn label(&self) -> String {
        match (&self.title, &self.duration) {
            (Some(t), Some(d)) => format!("{t}  ({})", crate::resolve::fmt_secs(*d)),
            (Some(t), None) => t.clone(),
            (None, _) => self.id.clone().unwrap_or_else(|| "untitled".into()),
        }
    }
}

/// Pull the related information out of a `yt-dlp -J` document.
///
/// Missing fields are normal — extractors differ wildly in what they populate —
/// so this never fails, it just returns less.
pub fn related_from_json(json: &str) -> Related {
    serde_json::from_str(json).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_watch_url_is_a_video() {
        match classify("https://www.youtube.com/watch?v=dQw4w9WgXcQ") {
            Paste::Video { id, .. } => assert_eq!(id.as_deref(), Some("dQw4w9WgXcQ")),
            other => panic!("got {other:?}"),
        }
    }

    /// A watch URL carrying a `list=` is still a VIDEO. Checking playlist first
    /// opens the whole list from a link to one video in it, which is not what
    /// was copied.
    #[test]
    fn a_watch_url_inside_a_playlist_is_still_the_video() {
        match classify("https://www.youtube.com/watch?v=abc12345678&list=PL9999") {
            Paste::Video { id, .. } => assert_eq!(id.as_deref(), Some("abc12345678")),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn short_links_and_bare_ids_are_videos() {
        match classify("https://youtu.be/dQw4w9WgXcQ") {
            Paste::Video { id, .. } => assert_eq!(id.as_deref(), Some("dQw4w9WgXcQ")),
            other => panic!("got {other:?}"),
        }
        match classify("dQw4w9WgXcQ") {
            Paste::Video { id, url } => {
                assert_eq!(id.as_deref(), Some("dQw4w9WgXcQ"));
                assert!(url.contains("watch?v="), "no playable URL built");
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn short_links_with_a_query_keep_only_the_id() {
        match classify("https://youtu.be/dQw4w9WgXcQ?t=42") {
            Paste::Video { id, .. } => assert_eq!(id.as_deref(), Some("dQw4w9WgXcQ")),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn channels_are_recognised_in_all_four_spellings() {
        for (url, want) in [
            ("https://www.youtube.com/channel/UC123", "UC123"),
            ("https://www.youtube.com/user/somebody", "somebody"),
            ("https://www.youtube.com/c/somebody", "somebody"),
            ("https://www.youtube.com/@somebody", "somebody"),
        ] {
            match classify(url) {
                Paste::Channel { id, .. } => assert_eq!(id.as_deref(), Some(want), "{url}"),
                other => panic!("{url} -> {other:?}"),
            }
        }
    }

    #[test]
    fn a_bare_playlist_link_is_a_playlist() {
        match classify("https://www.youtube.com/playlist?list=PL1234") {
            Paste::Playlist { id, .. } => assert_eq!(id.as_deref(), Some("PL1234")),
            other => panic!("got {other:?}"),
        }
    }

    /// yt-dlp handles over a thousand sites. Refusing anything that is not
    /// YouTube would throw away most of what it can play, and yt-dlp fails with
    /// a clear message where the palette would just silently offer nothing.
    #[test]
    fn other_sites_are_offered_rather_than_refused() {
        assert!(classify("https://vimeo.com/12345").is_playable());
        assert!(classify("https://example.org/clip.mp4").is_playable());
    }

    #[test]
    fn prose_and_rubbish_are_not_offered() {
        for t in [
            "",
            "   ",
            "hello",
            "look at https://youtube.com/watch?v=abc it is good",
            // The URL FIRST, which is the only case that reaches the
            // whitespace guard -- the one above fails the http:// check long
            // before it, so on its own it tested nothing.
            "https://youtu.be/dQw4w9WgXcQ great video",
            "https://youtu.be/dQw4w9WgXcQ\nhttps://youtu.be/aaaaaaaaaaa",
            "ftp://example.org/x.mp4",
            "not-11-chars",
        ] {
            assert_eq!(classify(t), Paste::Nothing, "offered {t:?}");
        }
    }

    #[test]
    fn whitespace_around_a_url_is_tolerated() {
        assert!(classify("  https://youtu.be/dQw4w9WgXcQ\n").is_playable());
    }

    /// A palette entry has to say what it will do.
    #[test]
    fn every_variant_has_a_label() {
        for p in [
            classify("https://youtu.be/dQw4w9WgXcQ"),
            classify("https://www.youtube.com/@somebody"),
            classify("https://www.youtube.com/playlist?list=PL1"),
            Paste::Nothing,
        ] {
            assert!(!p.label().is_empty());
        }
    }

    // ---- related ----------------------------------------------------------

    const CHANNEL_JSON: &str = r#"{
      "channel": "Some Channel",
      "channel_id": "UC123",
      "channel_url": "https://www.youtube.com/channel/UC123",
      "uploader": "Some Channel",
      "entries": [
        {"id": "aaa11111111", "title": "First", "duration": 212.0},
        {"id": "bbb22222222", "title": "Second", "url": "https://youtu.be/bbb22222222"},
        {"id": "ccc33333333"}
      ]
    }"#;

    #[test]
    fn channel_details_are_kept_from_the_probe_we_already_ran() {
        let r = related_from_json(CHANNEL_JSON);
        assert_eq!(r.channel_id.as_deref(), Some("UC123"));
        assert_eq!(r.entries.len(), 3);
    }

    /// A flat listing gives ids and no URLs; a sidebar entry that cannot be
    /// played is worse than no sidebar.
    #[test]
    fn every_entry_yields_a_playable_url() {
        let r = related_from_json(CHANNEL_JSON);
        for e in &r.entries {
            let u = e.watch_url().unwrap_or_else(|| panic!("{e:?} has no URL"));
            assert!(u.starts_with("http"), "{u}");
        }
        // The one that came with a real URL keeps it rather than being rebuilt.
        assert_eq!(
            r.entries[1].watch_url().as_deref(),
            Some("https://youtu.be/bbb22222222")
        );
    }

    #[test]
    fn entries_without_a_title_still_label() {
        let r = related_from_json(CHANNEL_JSON);
        assert_eq!(r.entries[2].label(), "ccc33333333");
        assert!(r.entries[0].label().contains("First"));
    }

    /// Extractors differ wildly in what they populate; missing fields must
    /// degrade rather than fail, or one odd site breaks the sidebar.
    #[test]
    fn a_document_with_none_of_this_is_not_an_error() {
        let r = related_from_json(r#"{"formats":[]}"#);
        assert!(r.channel.is_none());
        assert!(r.entries.is_empty());
        let r = related_from_json("not json at all");
        assert!(r.entries.is_empty());
    }
}
