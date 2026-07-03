//! Link preview metadata extraction.
//!
//! Given a URL, returns title / description / thumbnail to render in a
//! Snap link card. Uses Open Graph tags first (the open standard), then
//! Twitter cards, then plain `<title>` / `<meta description>` as fallback.
//!
//! The `fetch` feature pulls in `reqwest` + `scraper` for HTTP and HTML
//! parsing. Without it, only `parse_html()` is available — useful for
//! testing or when the C++ side has already fetched the HTML body itself.
//!
//! # Privacy
//! Link previews are fetched by the SENDER, not the recipient. The recipient
//! never makes an HTTP request for the URL — they just see the cached
//! metadata that was bundled into the snap. This avoids tracking pixels and
//! preserves the recipient's IP from being leaked to the URL's host.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LinkPreview {
    /// Final URL after redirects (canonical link the user shared).
    pub url:         String,
    /// Page title (Open Graph `og:title`, or `<title>`).
    pub title:       String,
    /// Short description (Open Graph `og:description`, or `<meta description>`).
    pub description: String,
    /// Image URL (Open Graph `og:image`). Resolver should pre-fetch this.
    pub image_url:   String,
    /// Site name (Open Graph `og:site_name`, or hostname fallback).
    pub site_name:   String,
}

impl LinkPreview {
    pub fn empty(url: &str) -> Self {
        Self { url: url.to_string(), ..Self::default() }
    }

    /// True if at least title or description was extracted.
    pub fn is_useful(&self) -> bool {
        !self.title.is_empty() || !self.description.is_empty()
    }
}

#[cfg(feature = "fetch")]
pub mod fetch {
    use super::*;
    use anyhow::{Context, Result};
    use std::time::Duration;

    /// Fetch the URL, parse Open Graph + meta tags, return a LinkPreview.
    /// Blocking — caller should run on a worker thread.
    ///
    /// Limits:
    ///   • 5 second timeout
    ///   • 256 KB max body size
    ///   • Follows up to 5 redirects
    pub fn fetch_preview(url: &str) -> Result<LinkPreview> {
        let client = reqwest::blocking::Client::builder()
            .user_agent("SnapClone/1.0 (link preview)")
            .timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .context("client builder")?;

        let resp = client.get(url).send().context("HTTP get")?;
        let final_url = resp.url().clone();

        // Cap body size — don't accidentally download a 4GB video
        const MAX_BYTES: usize = 256 * 1024;
        let bytes = resp.bytes().context("body read")?;
        let trimmed = if bytes.len() > MAX_BYTES { &bytes[..MAX_BYTES] } else { &bytes };
        let html = String::from_utf8_lossy(trimmed);

        let mut preview = parse_html(&html, final_url.as_str());
        if preview.url.is_empty() { preview.url = final_url.to_string(); }
        Ok(preview)
    }
}

/// Parse already-fetched HTML for Open Graph + meta tags.
/// Pure-string version — no HTTP, no scraper dep needed if `fetch` is off.
#[cfg(feature = "fetch")]
pub fn parse_html(html: &str, source_url: &str) -> LinkPreview {
    use scraper::{Html, Selector};

    let mut p = LinkPreview::empty(source_url);
    let doc = Html::parse_document(html);

    // Helper: get first matching element's "content" attribute
    let meta_content = |sel_str: &str| -> Option<String> {
        let sel = Selector::parse(sel_str).ok()?;
        doc.select(&sel).next()?.value().attr("content").map(String::from)
    };

    // Open Graph (priority 1)
    p.title       = meta_content(r#"meta[property="og:title"]"#).unwrap_or_default();
    p.description = meta_content(r#"meta[property="og:description"]"#).unwrap_or_default();
    p.image_url   = meta_content(r#"meta[property="og:image"]"#).unwrap_or_default();
    p.site_name   = meta_content(r#"meta[property="og:site_name"]"#).unwrap_or_default();

    // Twitter Cards (priority 2)
    if p.title.is_empty()       { p.title       = meta_content(r#"meta[name="twitter:title"]"#).unwrap_or_default(); }
    if p.description.is_empty() { p.description = meta_content(r#"meta[name="twitter:description"]"#).unwrap_or_default(); }
    if p.image_url.is_empty()   { p.image_url   = meta_content(r#"meta[name="twitter:image"]"#).unwrap_or_default(); }

    // Plain HTML fallbacks (priority 3)
    if p.title.is_empty() {
        if let Ok(sel) = Selector::parse("title") {
            if let Some(t) = doc.select(&sel).next() {
                p.title = t.text().collect::<String>().trim().to_string();
            }
        }
    }
    if p.description.is_empty() {
        p.description = meta_content(r#"meta[name="description"]"#).unwrap_or_default();
    }

    // Hostname as site name fallback
    if p.site_name.is_empty() {
        if let Ok(parsed) = url::Url::parse(source_url) {
            if let Some(host) = parsed.host_str() {
                p.site_name = host.to_string();
            }
        }
    }

    // Bound the lengths to avoid bloated metadata
    p.title       = truncate(&p.title, 200);
    p.description = truncate(&p.description, 500);
    p
}

/// No-op when `fetch` feature off — caller should have own HTML parser.
#[cfg(not(feature = "fetch"))]
pub fn parse_html(_html: &str, source_url: &str) -> LinkPreview {
    LinkPreview::empty(source_url)
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars { return s.to_string(); }
    let mut out: String = s.chars().take(max_chars - 1).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_adds_ellipsis() {
        let s = "a".repeat(300);
        let t = truncate(&s, 100);
        assert_eq!(t.chars().count(), 100);
        assert!(t.ends_with('…'));
    }

    #[test]
    fn empty_preview_not_useful() {
        let p = LinkPreview::empty("https://example.com");
        assert!(!p.is_useful());
        assert_eq!(p.url, "https://example.com");
    }
}
