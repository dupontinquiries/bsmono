//! OPDS (Open Publication Distribution System) support: tomescope can
//! browse a local file or pasted XML blob as an OPDS 1.2 Atom catalog —
//! useful for library/school ebook servers (Calibre-Web, COPS, etc).
//! Network fetch is stubbed behind a trait so the parser itself is fully
//! unit-testable without a live server.

use quick_xml::events::Event;
use quick_xml::reader::Reader;

#[derive(Clone, Debug, Default)]
pub struct OpdsEntry {
    pub title: String,
    pub id: String,
    pub summary: String,
    pub author: Option<String>,
    pub updated: Option<String>,
    /// (rel, href, mime type) for every <link>, so the UI can pick the
    /// right acquisition link (epub/pdf download) vs navigation link
    /// (subsection of the catalog).
    pub links: Vec<OpdsLink>,
}

#[derive(Clone, Debug)]
pub struct OpdsLink {
    pub rel: String,
    pub href: String,
    pub mime_type: String,
}

impl OpdsEntry {
    /// The best acquisition link for direct download, preferring epub
    /// over pdf over anything else.
    pub fn acquisition_link(&self) -> Option<&OpdsLink> {
        let is_acq = |l: &&OpdsLink| l.rel.contains("acquisition");
        self.links
            .iter()
            .filter(is_acq)
            .find(|l| l.mime_type.contains("epub"))
            .or_else(|| self.links.iter().filter(is_acq).find(|l| l.mime_type.contains("pdf")))
            .or_else(|| self.links.iter().find(is_acq))
    }

    /// True if this entry is itself a navigable sub-catalog rather than a
    /// downloadable book (OPDS "navigation" link relation).
    pub fn is_navigation(&self) -> bool {
        self.links.iter().any(|l| l.rel == "subsection" || l.mime_type.contains("opds-catalog"))
    }
}

#[derive(Clone, Debug, Default)]
pub struct OpdsFeed {
    pub title: String,
    pub entries: Vec<OpdsEntry>,
    /// Pagination links (rel="next"/"prev") if the server paginates large
    /// catalogs.
    pub next_href: Option<String>,
    pub prev_href: Option<String>,
}

/// Parse an OPDS/Atom XML string into a feed. Tolerant of missing
/// namespaces prefixes (matches on local tag name).
pub fn parse_opds_xml(xml: &str) -> anyhow::Result<OpdsFeed> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut feed = OpdsFeed::default();
    let mut buf = Vec::new();

    let mut in_entry = false;
    let mut current = OpdsEntry::default();
    let mut current_tag = String::new();
    let mut text_buf = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = local_name(e.name().as_ref());
                match name.as_str() {
                    "entry" => {
                        in_entry = true;
                        current = OpdsEntry::default();
                    }
                    "link" => {
                        let mut rel = String::new();
                        let mut href = String::new();
                        let mut mime_type = String::new();
                        for attr in e.attributes().flatten() {
                            let key = local_name(attr.key.as_ref());
                            let val = String::from_utf8_lossy(&attr.value).to_string();
                            match key.as_str() {
                                "rel" => rel = val,
                                "href" => href = val,
                                "type" => mime_type = val,
                                _ => {}
                            }
                        }
                        let link = OpdsLink { rel: rel.clone(), href: href.clone(), mime_type };
                        if in_entry {
                            current.links.push(link);
                        } else {
                            if rel == "next" {
                                feed.next_href = Some(href);
                            } else if rel == "prev" || rel == "previous" {
                                feed.prev_href = Some(href);
                            }
                        }
                    }
                    _ => {
                        current_tag = name;
                        text_buf.clear();
                    }
                }
            }
            Ok(Event::Text(t)) => {
                if let Ok(s) = t.unescape() {
                    text_buf.push_str(&s);
                }
            }
            Ok(Event::End(e)) => {
                let name = local_name(e.name().as_ref());
                match name.as_str() {
                    "entry" => {
                        in_entry = false;
                        feed.entries.push(std::mem::take(&mut current));
                    }
                    "title" => {
                        if in_entry {
                            current.title = text_buf.trim().to_string();
                        } else {
                            feed.title = text_buf.trim().to_string();
                        }
                    }
                    "id" if in_entry => current.id = text_buf.trim().to_string(),
                    "summary" | "content" if in_entry => current.summary = text_buf.trim().to_string(),
                    "name" if in_entry && current_tag == "name" => {
                        current.author = Some(text_buf.trim().to_string())
                    }
                    "updated" if in_entry => current.updated = Some(text_buf.trim().to_string()),
                    _ => {}
                }
                text_buf.clear();
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(feed)
}

fn local_name(full: &[u8]) -> String {
    let s = String::from_utf8_lossy(full);
    s.rsplit(':').next().unwrap_or(&s).to_string()
}

/// Abstraction over fetching a catalog by URL, so the app doesn't need a
/// hard network dependency baked into this module — the real
/// implementation (using ureq/reqwest, gated behind a feature) can be
/// swapped in without touching the parser or UI code above.
pub trait OpdsFetcher {
    fn fetch(&self, url: &str) -> anyhow::Result<String>;
}

/// Placeholder fetcher that always errors, used when no network backend
/// is compiled in. Keeps the OPDS browser UI functional in
/// "paste XML" / "open local file" mode even without network access.
pub struct NoNetworkFetcher;

impl OpdsFetcher for NoNetworkFetcher {
    fn fetch(&self, _url: &str) -> anyhow::Result<String> {
        anyhow::bail!("Network fetch is not enabled in this build. Paste the catalog XML or open a local .xml file instead.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Demo Catalog</title>
  <link rel="next" href="/catalog?page=2" type="application/atom+xml"/>
  <entry>
    <title>Sample Book</title>
    <id>urn:uuid:1234</id>
    <summary>A test book.</summary>
    <link rel="http://opds-spec.org/acquisition" href="/download/1234.epub" type="application/epub+zip"/>
  </entry>
</feed>"#;

    #[test]
    fn parses_feed_title_and_entries() {
        let feed = parse_opds_xml(SAMPLE).unwrap();
        assert_eq!(feed.title, "Demo Catalog");
        assert_eq!(feed.entries.len(), 1);
        assert_eq!(feed.entries[0].title, "Sample Book");
        assert_eq!(feed.next_href.as_deref(), Some("/catalog?page=2"));
    }

    #[test]
    fn acquisition_link_prefers_epub() {
        let feed = parse_opds_xml(SAMPLE).unwrap();
        let link = feed.entries[0].acquisition_link().unwrap();
        assert!(link.mime_type.contains("epub"));
    }

    #[test]
    fn no_network_fetcher_errors() {
        let fetcher = NoNetworkFetcher;
        assert!(fetcher.fetch("http://example.com").is_err());
    }
}
