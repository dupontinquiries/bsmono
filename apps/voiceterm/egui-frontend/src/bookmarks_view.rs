//! bookmarks_view — saved URLs for the browser, which had a URL bar and no way
//! to keep anything.
//!
//! `src/bookmarks.rs` owns the file format and the merge rules. It is **not**
//! mounted: it resolves `HOME/.config/web2qml/bookmarks.toml` itself and does
//! its own I/O, and a widget crate that reads a hardcoded path cannot be tested
//! and cannot be pointed anywhere else. The port re-expresses the **rules**,
//! which is the part that has to match, and leaves the file to the host.
//!
//! ## The merge rule is the interesting part
//!
//! Saving a URL that is already bookmarked does not add a second entry and does
//! not replace the first. It:
//!
//! * takes the **union** of the tags,
//! * updates `saved_at_ms` to now,
//! * and updates the title **only if the existing one was empty**.
//!
//! That last clause is the one worth reading twice. A title is often derived
//! from the page — `<title>`, or a URL fallback — while an existing title may
//! have been typed by the user. Overwriting it means re-saving a page silently
//! discards the name someone chose, and they will not notice until they go
//! looking for it.

/// One saved URL. Field names match `bookmarks::Bookmark` so the mapping is
/// checkable by eye.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    pub url: String,
    pub title: String,
    pub saved_at_ms: u64,
    pub tags: Vec<String>,
}

impl Bookmark {
    /// What a row shows. A bookmark with no title falls back to its URL, since
    /// an untitled entry is common — the page may not have had a `<title>` —
    /// and a blank row is unusable.
    pub fn label(&self) -> &str {
        if self.title.trim().is_empty() { &self.url } else { &self.title }
    }
}

/// What a save did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveResult {
    /// The URL was not saved before.
    Fresh,
    /// It was. Tags merged; `new_tags` counts only the ones that were not
    /// already there, so the UI can say "added 2 tags" rather than "added 5"
    /// when three were already present.
    Merged { new_tags: usize },
}

#[derive(Debug, Clone, Default)]
pub struct Bookmarks {
    items: Vec<Bookmark>,
    pub query: String,
    /// Restrict to one tag; empty means all.
    pub tag: String,
}

impl Bookmarks {
    pub fn load(&mut self, items: Vec<Bookmark>) { self.items = items; }
    pub fn all(&self) -> &[Bookmark] { &self.items }

    /// Save or merge.
    pub fn save(&mut self, url: &str, title: &str, tags: &[&str], now_ms: u64) -> SaveResult {
        let url = url.trim();
        if let Some(existing) = self.items.iter_mut().find(|b| b.url == url) {
            let mut added = 0;
            for t in tags {
                let t = t.trim().to_lowercase();
                if t.is_empty() { continue; }
                if !existing.tags.iter().any(|e| e == &t) {
                    existing.tags.push(t);
                    added += 1;
                }
            }
            existing.saved_at_ms = now_ms;
            // ONLY when empty. A derived title must never overwrite one the
            // user typed — they will not notice until they go looking for it.
            if existing.title.trim().is_empty() && !title.trim().is_empty() {
                existing.title = title.trim().to_string();
            }
            return SaveResult::Merged { new_tags: added };
        }
        let mut clean: Vec<String> = Vec::new();
        for t in tags {
            let t = t.trim().to_lowercase();
            if !t.is_empty() && !clean.contains(&t) { clean.push(t); }
        }
        self.items.push(Bookmark {
            url: url.to_string(), title: title.trim().to_string(),
            saved_at_ms: now_ms, tags: clean,
        });
        SaveResult::Fresh
    }

    pub fn remove(&mut self, url: &str) -> bool {
        let before = self.items.len();
        self.items.retain(|b| b.url != url);
        self.items.len() != before
    }

    pub fn is_saved(&self, url: &str) -> bool {
        self.items.iter().any(|b| b.url == url.trim())
    }

    /// Rows, newest first. Recency is the useful order for saved URLs — you
    /// bookmark something to come back to it soon — and ties break on url so
    /// the list cannot shuffle.
    pub fn rows(&self) -> Vec<&Bookmark> {
        let q = self.query.trim().to_lowercase();
        let tag = self.tag.trim().to_lowercase();
        let mut v: Vec<&Bookmark> = self.items.iter()
            .filter(|b| tag.is_empty() || b.tags.iter().any(|t| t == &tag))
            .filter(|b| q.is_empty()
                    || b.label().to_lowercase().contains(&q)
                    || b.url.to_lowercase().contains(&q))
            .collect();
        v.sort_by(|a, b| b.saved_at_ms.cmp(&a.saved_at_ms).then(a.url.cmp(&b.url)));
        v
    }

    /// Tags with counts, most-used first, for the filter row.
    pub fn tags(&self) -> Vec<(String, usize)> {
        let mut m: std::collections::BTreeMap<String, usize> = Default::default();
        for b in &self.items {
            for t in &b.tags { *m.entry(t.clone()).or_default() += 1; }
        }
        let mut v: Vec<(String, usize)> = m.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved() -> Bookmarks {
        let mut b = Bookmarks::default();
        b.save("https://a.test/one", "One", &["news"], 100);
        b.save("https://b.test/two", "Two", &["tech", "news"], 200);
        b
    }

    #[test]
    fn re_saving_never_overwrites_a_title_someone_typed() {
        // THE rule from bookmarks.rs. A derived title — from <title>, or a URL
        // fallback — silently discarding a chosen name is not noticed until the
        // user goes looking for it.
        let mut b = saved();
        b.save("https://a.test/one", "Some Derived Page Title", &[], 300);
        assert_eq!(b.all().iter().find(|x| x.url.contains("a.test")).unwrap().title, "One");
    }

    #[test]
    fn an_empty_title_is_filled_in_on_re_save() {
        // The other half of the same rule: nothing is being clobbered.
        let mut b = Bookmarks::default();
        b.save("https://c.test", "", &[], 100);
        b.save("https://c.test", "Now It Has One", &[], 200);
        assert_eq!(b.all()[0].title, "Now It Has One");
    }

    #[test]
    fn saving_twice_merges_rather_than_duplicating() {
        let mut b = saved();
        let r = b.save("https://a.test/one", "One", &["tech"], 300);
        assert_eq!(r, SaveResult::Merged { new_tags: 1 });
        assert_eq!(b.all().len(), 2, "no second entry");
        let one = b.all().iter().find(|x| x.url.contains("a.test")).unwrap();
        assert_eq!(one.tags, vec!["news", "tech"], "union, in order");
        assert_eq!(one.saved_at_ms, 300, "recency updated");
    }

    #[test]
    fn only_genuinely_new_tags_are_counted() {
        // So the UI can say "added 2 tags" rather than "added 5" when three
        // were already there.
        let mut b = saved();
        let r = b.save("https://b.test/two", "Two", &["news", "tech", "rust"], 400);
        assert_eq!(r, SaveResult::Merged { new_tags: 1 });
    }

    #[test]
    fn tags_are_normalised_so_the_same_tag_is_the_same_tag() {
        // "News" and "news" filtering to different sets is the kind of thing
        // that makes a tag system quietly useless.
        let mut b = Bookmarks::default();
        b.save("https://d.test", "D", &["News", "  news  ", ""], 100);
        assert_eq!(b.all()[0].tags, vec!["news"], "lowercased, trimmed, deduped, no blanks");
    }

    #[test]
    fn the_list_is_newest_first_and_stable() {
        // You bookmark something to come back to it soon.
        let b = saved();
        let urls: Vec<&str> = b.rows().iter().map(|x| x.url.as_str()).collect();
        assert_eq!(urls[0], "https://b.test/two");
        let again: Vec<&str> = b.rows().iter().map(|x| x.url.as_str()).collect();
        assert_eq!(urls, again);
    }

    #[test]
    fn a_tag_filter_narrows_and_clears() {
        let mut b = saved();
        b.tag = "tech".into();
        assert_eq!(b.rows().len(), 1);
        b.tag = "  ".into();
        assert_eq!(b.rows().len(), 2, "a blank tag is no filter");
    }

    #[test]
    fn the_tag_row_is_most_used_first() {
        let b = saved();
        let tags = b.tags();
        assert_eq!(tags[0], ("news".to_string(), 2));
    }

    #[test]
    fn is_saved_drives_the_star_and_ignores_stray_whitespace() {
        // The browser asks this every frame to decide whether the star is
        // filled; a URL bar routinely carries a trailing space.
        let b = saved();
        assert!(b.is_saved("https://a.test/one"));
        assert!(b.is_saved("  https://a.test/one  "));
        assert!(!b.is_saved("https://nope.test"));
    }

    #[test]
    fn removing_something_that_is_not_there_reports_it() {
        let mut b = saved();
        assert!(b.remove("https://a.test/one"));
        assert!(!b.remove("https://a.test/one"), "already gone");
        assert_eq!(b.all().len(), 1);
    }
}
