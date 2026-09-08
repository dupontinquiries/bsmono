//! clip_source — where a clipped selection came from.
//!
//! `note_clip::Source` has had four variants since v0.1.641: `Web`, `File`, `Terminal`,
//! `Manual`. The app produced exactly one of them. Every clip — from a web page, from an
//! open file, from anywhere — was filed as `Terminal { command, at }` with whatever command
//! happened to have run last, so a paragraph clipped from a documentation page cited itself
//! as `cargo build`.
//!
//! A citation that names the wrong source is worse than none: the note LOOKS sourced, and the
//! provenance is the reason to clip rather than retype.
//!
//! Pure: what the focused pane is, in and a `Source` out. The app supplies the facts.

use crate::note_clip::Source;

/// What the focused pane is showing, as far as provenance is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Focus {
    /// A web pane, with its url and page title.
    Web { url: String, title: String },
    /// A file being viewed or edited, and the line the selection starts on.
    File { path: String, line: Option<u32> },
    /// The terminal, with the command that produced the output and when it ran.
    Terminal { command: String, at: u64 },
    /// Something with no provenance to offer.
    Other,
}

/// The source to file a clip under.
pub fn of(focus: &Focus) -> Source {
    match focus {
        // A url with no title still cites better than a command that had nothing to do with
        // it: the url IS the citation, and the title is the readable part.
        Focus::Web { url, title } if !url.trim().is_empty() => Source::Web {
            url: url.clone(),
            title: if title.trim().is_empty() { url.clone() } else { title.clone() },
        },
        Focus::File { path, line } if !path.trim().is_empty() => Source::File {
            path: path.clone(),
            // `L12`, the anchor form `Source::citation` already renders. A line of 0 is not
            // a line — editors count from 1, and an anchor pointing at L0 sends someone to
            // the wrong place with confidence.
            anchor: line.filter(|n| *n > 0).map(|n| format!("L{n}")),
        },
        Focus::Terminal { command, at } if !command.trim().is_empty() => {
            Source::Terminal { command: command.clone(), at: *at }
        }
        // A clip whose origin cannot be named is MANUAL, not a terminal clip with an empty
        // command. `Manual` cites as "(typed)", which is true; an empty command cites as an
        // empty backtick pair, which reads as a bug.
        _ => Source::Manual,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug this exists for: a paragraph clipped from a documentation page cited itself
    /// as `cargo build`, because the app only ever produced `Source::Terminal`.
    #[test]
    fn a_web_clip_cites_the_page_not_the_last_command() {
        let s = of(&Focus::Web {
            url: "https://doc.rust-lang.org/std".into(),
            title: "std - Rust".into(),
        });
        assert_eq!(
            s,
            Source::Web {
                url: "https://doc.rust-lang.org/std".into(),
                title: "std - Rust".into()
            }
        );
        let cite = s.citation();
        assert!(cite.contains("doc.rust-lang.org"), "{cite}");
        assert!(!cite.contains("cargo"), "{cite}");
    }

    /// A url with no title still cites better than a command that had nothing to do with it.
    #[test]
    fn a_titleless_page_falls_back_to_its_url() {
        let s = of(&Focus::Web { url: "https://example.com/x".into(), title: "  ".into() });
        match s {
            Source::Web { title, url } => assert_eq!(title, url, "an empty title was kept"),
            other => panic!("{other:?}"),
        }
    }

    /// Editors count from 1. An anchor pointing at L0 sends someone to the wrong place with
    /// confidence.
    #[test]
    fn a_file_clip_anchors_on_a_real_line_or_none() {
        assert_eq!(
            of(&Focus::File { path: "src/main.rs".into(), line: Some(12) }),
            Source::File { path: "src/main.rs".into(), anchor: Some("L12".into()) }
        );
        assert_eq!(
            of(&Focus::File { path: "src/main.rs".into(), line: Some(0) }),
            Source::File { path: "src/main.rs".into(), anchor: None },
            "L0 is not a line"
        );
        assert_eq!(
            of(&Focus::File { path: "src/main.rs".into(), line: None }),
            Source::File { path: "src/main.rs".into(), anchor: None }
        );
    }

    /// A clip whose origin cannot be named is MANUAL. An empty command cites as an empty
    /// backtick pair, which reads as a bug; `Manual` cites as "(typed)", which is true.
    #[test]
    fn an_unnameable_origin_is_manual_not_an_empty_terminal_clip() {
        assert_eq!(of(&Focus::Other), Source::Manual);
        assert_eq!(of(&Focus::Terminal { command: "   ".into(), at: 5 }), Source::Manual);
        assert_eq!(of(&Focus::Web { url: "".into(), title: "x".into() }), Source::Manual);
        assert_eq!(of(&Focus::File { path: "".into(), line: Some(3) }), Source::Manual);
        assert!(Source::Manual.citation().contains("typed"));
    }

    /// The terminal case still works — it is the one that shipped, and this must not trade
    /// one wrong answer for another.
    #[test]
    fn a_terminal_clip_still_cites_its_command() {
        let s = of(&Focus::Terminal { command: "cargo test".into(), at: 99 });
        assert_eq!(s, Source::Terminal { command: "cargo test".into(), at: 99 });
        assert!(s.citation().contains("cargo test"));
    }
}
