//! v0.1.641: note clipping — the data model + markdown serialization for
//! the notes/clippings feature. You select content from ANYWHERE (a web
//! page, a markdown file, terminal output, a code segment, an image crop),
//! clip it into a note, and each clip REMEMBERS WHERE IT CAME FROM so you
//! can reference back. The note serializes to markdown (viewable in the
//! web view, since we already render markdown) with the provenance
//! embedded as a citation line per clip.
//!
//! This is the PURE foundation the rest attaches to: selection→clip
//! (highlight/voice), voice-dictation entry, and clip suggestions all
//! produce Clips that go into a Note; the Note renders to markdown. No FFI
//! here — the caller supplies the captured text + the source it came from.
//!
//! Why provenance matters (the user's "reference back"): a plain notes app
//! loses where a snippet came from. Here every clip carries its origin —
//! the URL + quote, the file + line range, the command that produced the
//! output, etc. — rendered as a markdown citation so you (or a future
//! "jump to source") can return to it.

/// Where a clip came from. Covers every surface the app exposes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A web page: url + page title (for a readable citation).
    Web { url: String, title: String },
    /// A local file: path + an optional anchor (heading or line range
    /// like "L12-L30") to point back precisely.
    File { path: String, anchor: Option<String> },
    /// Terminal output: the command that produced it + a unix timestamp
    /// to find the block again.
    Terminal { command: String, at: u64 },
    /// Hand-typed / voice-dictated with no external source.
    Manual,
}

impl Source {
    /// A one-line markdown citation for this source, e.g.
    ///   "[Example Page](https://…)"  /  "`src/main.rs` (L12-L30)"  /
    ///   "`cargo test`"  /  "(typed)".
    pub fn citation(&self) -> String {
        match self {
            Source::Web { url, title } => {
                let t = if title.trim().is_empty() { url.as_str() } else { title.as_str() };
                format!("[{}]({})", t, url)
            }
            Source::File { path, anchor } => match anchor {
                Some(a) if !a.is_empty() => format!("`{}` ({})", path, a),
                _ => format!("`{}`", path),
            },
            Source::Terminal { command, .. } => format!("`{}`", command),
            Source::Manual => "(typed)".to_string(),
        }
    }
}

/// What KIND of content a clip holds — drives how it renders in markdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipKind {
    /// Prose / plain text → rendered as a blockquote.
    Text,
    /// A code segment → rendered as a fenced code block. Carries the
    /// language hint (may be empty).
    Code { lang: String },
    /// An image crop → rendered as a markdown image ref. Carries the
    /// stored image path (the caller saves the crop) + alt text.
    Image { path: String, alt: String },
}

/// One clipped snippet with its provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    pub kind: ClipKind,
    /// The clipped content (the quoted text or code; empty for Image,
    /// whose payload is the path in ClipKind::Image).
    pub content: String,
    pub source: Source,
    /// unix seconds clipped (for ordering / "clipped just now").
    pub captured_at: u64,
}

impl Clip {
    /// Render this clip to a markdown fragment: the content in its
    /// kind-appropriate form, followed by an indented source citation.
    pub fn to_markdown(&self) -> String {
        let body = match &self.kind {
            ClipKind::Text => self
                .content
                .lines()
                .map(|l| format!("> {}", l))
                .collect::<Vec<_>>()
                .join("\n"),
            ClipKind::Code { lang } => {
                format!("```{}\n{}\n```", lang, self.content.trim_end())
            }
            ClipKind::Image { path, alt } => format!("![{}]({})", alt, path),
        };
        // citation as a sub-line so it reads as attribution.
        format!("{}\n>\n> — {}", body, self.source.citation())
    }
}

/// A note: a titled, ordered collection of clips plus free-text blocks
/// the user dictates/types. Stored as / rendered to markdown.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Note {
    pub title: String,
    /// Ordered blocks: a note interleaves dictated prose with clips.
    pub blocks: Vec<Block>,
}

/// A note is a sequence of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// Free-text the user typed or dictated (voice entry mode).
    Prose(String),
    /// A clipped snippet with provenance.
    Clip(Clip),
}

impl Note {
    pub fn new(title: &str) -> Self {
        Note { title: title.to_string(), blocks: Vec::new() }
    }
    pub fn push_prose(&mut self, text: &str) {
        self.blocks.push(Block::Prose(text.to_string()));
    }
    pub fn push_clip(&mut self, clip: Clip) {
        self.blocks.push(Block::Clip(clip));
    }

    /// Render the whole note to a markdown document (this is what gets
    /// saved as a .md and shown in the web view).
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        let title = if self.title.trim().is_empty() { "Note" } else { self.title.trim() };
        out.push_str(&format!("# {}\n", title));
        for b in &self.blocks {
            out.push('\n');
            match b {
                Block::Prose(t) => out.push_str(t.trim_end()),
                Block::Clip(c) => out.push_str(&c.to_markdown()),
            }
            out.push('\n');
        }
        out
    }

    /// Distinct sources referenced (for a "Sources" footer or a
    /// jump-back menu). De-duplicated, in first-seen order.
    pub fn sources(&self) -> Vec<&Source> {
        let mut seen: Vec<&Source> = Vec::new();
        for b in &self.blocks {
            if let Block::Clip(c) = b {
                if !seen.iter().any(|s| **s == c.source) {
                    seen.push(&c.source);
                }
            }
        }
        seen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn web() -> Source { Source::Web { url: "https://ex.com/a".into(), title: "Ex A".into() } }

    #[test]
    fn web_citation() {
        assert_eq!(web().citation(), "[Ex A](https://ex.com/a)");
        // empty title falls back to the url
        let s = Source::Web { url: "https://x".into(), title: "".into() };
        assert_eq!(s.citation(), "[https://x](https://x)");
    }

    #[test]
    fn file_citation_with_and_without_anchor() {
        assert_eq!(
            Source::File { path: "src/main.rs".into(), anchor: Some("L12-L30".into()) }.citation(),
            "`src/main.rs` (L12-L30)"
        );
        assert_eq!(
            Source::File { path: "notes.md".into(), anchor: None }.citation(),
            "`notes.md`"
        );
    }

    #[test]
    fn terminal_and_manual_citation() {
        assert_eq!(Source::Terminal { command: "cargo test".into(), at: 0 }.citation(), "`cargo test`");
        assert_eq!(Source::Manual.citation(), "(typed)");
    }

    #[test]
    fn text_clip_is_blockquote_with_citation() {
        let c = Clip {
            kind: ClipKind::Text,
            content: "first line\nsecond line".into(),
            source: web(),
            captured_at: 0,
        };
        let md = c.to_markdown();
        assert!(md.contains("> first line"));
        assert!(md.contains("> second line"));
        assert!(md.contains("— [Ex A](https://ex.com/a)"));
    }

    #[test]
    fn code_clip_is_fenced_with_lang() {
        let c = Clip {
            kind: ClipKind::Code { lang: "rust".into() },
            content: "fn main() {}".into(),
            source: Source::File { path: "m.rs".into(), anchor: Some("L1".into()) },
            captured_at: 0,
        };
        let md = c.to_markdown();
        assert!(md.starts_with("```rust\nfn main() {}\n```"));
        assert!(md.contains("— `m.rs` (L1)"));
    }

    #[test]
    fn image_clip_is_markdown_image() {
        let c = Clip {
            kind: ClipKind::Image { path: "clips/crop1.png".into(), alt: "a chart".into() },
            content: "".into(),
            source: web(),
            captured_at: 0,
        };
        assert!(c.to_markdown().starts_with("![a chart](clips/crop1.png)"));
    }

    #[test]
    fn note_renders_title_and_interleaved_blocks() {
        let mut n = Note::new("My research");
        n.push_prose("Some thoughts on the topic.");
        n.push_clip(Clip {
            kind: ClipKind::Text,
            content: "an important quote".into(),
            source: web(),
            captured_at: 0,
        });
        n.push_prose("Follow-up idea.");
        let md = n.to_markdown();
        assert!(md.starts_with("# My research\n"));
        assert!(md.contains("Some thoughts on the topic."));
        assert!(md.contains("> an important quote"));
        assert!(md.contains("Follow-up idea."));
        // order preserved: prose, then clip, then prose
        let p1 = md.find("Some thoughts").unwrap();
        let q = md.find("an important quote").unwrap();
        let p2 = md.find("Follow-up idea").unwrap();
        assert!(p1 < q && q < p2);
    }

    #[test]
    fn empty_title_defaults() {
        let n = Note::new("");
        assert!(n.to_markdown().starts_with("# Note\n"));
    }

    #[test]
    fn sources_dedup_first_seen_order() {
        let mut n = Note::new("n");
        let a = web();
        let b = Source::Terminal { command: "ls".into(), at: 0 };
        n.push_clip(Clip { kind: ClipKind::Text, content: "x".into(), source: a.clone(), captured_at: 0 });
        n.push_clip(Clip { kind: ClipKind::Text, content: "y".into(), source: b.clone(), captured_at: 0 });
        n.push_clip(Clip { kind: ClipKind::Text, content: "z".into(), source: a.clone(), captured_at: 0 });
        let srcs = n.sources();
        assert_eq!(srcs.len(), 2, "deduped");
        assert_eq!(*srcs[0], a, "first-seen order");
        assert_eq!(*srcs[1], b);
    }
}
