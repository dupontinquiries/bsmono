// src/layout_cache.rs
// Compile-once layout cache: hash page structure → cached QML template with content slots.
//
// Flow:
//   1. structural_hash(element) → u64 hash of tag names + classes (ignoring text/URLs)
//   2. Cache lookup: hash → Option<CachedLayout>
//   3. MISS: generate QML normally, extract template with {SLOT_N} placeholders
//   4. HIT: fill template slots with current content
//
// This means revisiting a page (or a page with the same structure) skips the
// recursive QML generation entirely — just fill slots.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;

/// A cached page layout template
#[derive(Clone)]
pub struct CachedLayout {
    /// QML template with {SLOT_0}, {SLOT_1}, etc. for dynamic content
    pub template: String,
    /// Number of slots
    pub slot_count: usize,
    /// Structural hash that produced this template
    pub hash: u64,
    /// Hit count for stats
    pub hits: u32,
}

/// Content slots extracted from a page
pub struct PageSlots {
    pub texts: Vec<String>,
    pub urls: Vec<String>,
    pub images: Vec<String>,
}

/// The layout cache
pub struct LayoutCache {
    /// hash → cached layout
    cache: HashMap<u64, CachedLayout>,
    /// domain → set of hashes (for domain-specific cache management)
    domain_hashes: HashMap<String, Vec<u64>>,
    /// Max cache entries
    max_entries: usize,
}

impl LayoutCache {
    pub fn new(max_entries: usize) -> Self {
        LayoutCache {
            cache: HashMap::with_capacity(64),
            domain_hashes: HashMap::new(),
            max_entries,
        }
    }

    /// Look up a cached layout by structural hash
    pub fn get(&mut self, hash: u64) -> Option<&CachedLayout> {
        if let Some(entry) = self.cache.get_mut(&hash) {
            entry.hits += 1;
            Some(entry)
        } else {
            None
        }
    }

    /// Store a compiled layout
    pub fn put(&mut self, hash: u64, template: String, slot_count: usize, domain: &str) {
        // Evict if full — remove least-hit entries
        if self.cache.len() >= self.max_entries {
            let min_hash = self.cache.iter()
                .min_by_key(|(_, v)| v.hits)
                .map(|(k, _)| *k);
            if let Some(h) = min_hash { self.cache.remove(&h); }
        }

        self.cache.insert(hash, CachedLayout {
            template, slot_count, hash, hits: 0,
        });
        self.domain_hashes.entry(domain.into()).or_default().push(hash);
    }

    /// Clear cache for a specific domain
    pub fn clear_domain(&mut self, domain: &str) {
        if let Some(hashes) = self.domain_hashes.remove(domain) {
            for h in hashes { self.cache.remove(&h); }
        }
    }

    /// Clear entire cache
    pub fn clear(&mut self) {
        self.cache.clear();
        self.domain_hashes.clear();
    }

    /// Stats
    pub fn stats(&self) -> (usize, u32) {
        let total_hits: u32 = self.cache.values().map(|v| v.hits).sum();
        (self.cache.len(), total_hits)
    }
}

/// Compute a structural hash of an HTML element tree.
/// Hashes tag names and class attributes, ignoring text content, URLs, and IDs.
/// Two pages with the same DOM structure but different content will hash the same.
pub fn structural_hash(elem: &scraper::ElementRef, max_depth: usize) -> u64 {
    let mut hasher = DefaultHasher::new();
    hash_element(&mut hasher, elem, 0, max_depth);
    hasher.finish()
}

fn hash_element(hasher: &mut DefaultHasher, elem: &scraper::ElementRef, depth: usize, max_depth: usize) {
    if depth > max_depth { return; }

    let tag = elem.value().name();
    // Hash tag name
    tag.hash(hasher);

    // Hash significant class names (layout-related, not utility)
    if let Some(classes) = elem.value().attr("class") {
        for cls in classes.split_ascii_whitespace() {
            let cl = cls.to_ascii_lowercase();
            // Only hash classes that affect layout structure
            if is_structural_class(&cl) {
                cl.hash(hasher);
            }
        }
    }

    // Hash child count (structural indicator)
    let child_count = elem.children().filter(|c| c.value().is_element()).count();
    child_count.hash(hasher);

    // Recurse into children
    for child in elem.children() {
        if let Some(el) = scraper::ElementRef::wrap(child) {
            hash_element(hasher, &el, depth + 1, max_depth);
        }
    }
}

/// Is this class name structurally significant (affects layout)?
fn is_structural_class(cls: &str) -> bool {
    // Layout classes
    if cls.starts_with("flex") || cls.starts_with("grid") || cls.starts_with("col-")
        || cls.starts_with("row") || cls.starts_with("container")
        || cls.starts_with("card") || cls.starts_with("hero") || cls.starts_with("nav")
        || cls.starts_with("header") || cls.starts_with("footer") || cls.starts_with("sidebar")
        || cls.starts_with("section") || cls.starts_with("article") || cls.starts_with("main")
        || cls.starts_with("wrapper") || cls.starts_with("layout") || cls.starts_with("d-")
    { return true; }

    // Size classes
    if cls.starts_with("sm-") || cls.starts_with("md-") || cls.starts_with("lg-")
        || cls.starts_with("xl-") || cls.starts_with("w-") || cls.starts_with("h-")
    { return true; }

    false
}

/// Section type classification
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SectionType {
    Hero,       // Full-width banner with heading + CTA
    CardGrid,   // Grid of cards (products, articles)
    NavBar,     // Navigation bar
    Article,    // Long-form text content
    Sidebar,    // Side panel
    Footer,     // Page footer
    Form,       // Input form
    Gallery,    // Image gallery
    List,       // Ordered/unordered list
    Generic,    // Default
}

/// Classify an element's likely section type from its tag, classes, and children
pub fn classify_section(elem: &scraper::ElementRef) -> SectionType {
    let tag = elem.value().name();
    let cls = elem.value().attr("class").unwrap_or("").to_ascii_lowercase();

    // Tag-based
    match tag {
        "nav" | "menu" => return SectionType::NavBar,
        "footer" => return SectionType::Footer,
        "article" => return SectionType::Article,
        "aside" => return SectionType::Sidebar,
        "form" => return SectionType::Form,
        "ul" | "ol" => return SectionType::List,
        _ => {}
    }

    // Class-based
    if cls.contains("hero") || cls.contains("banner") || cls.contains("jumbotron") { return SectionType::Hero; }
    if cls.contains("card") && (cls.contains("grid") || cls.contains("deck") || cls.contains("container")) { return SectionType::CardGrid; }
    if cls.contains("nav") || cls.contains("menu") || cls.contains("toolbar") { return SectionType::NavBar; }
    if cls.contains("article") || cls.contains("post") || cls.contains("content") || cls.contains("entry") { return SectionType::Article; }
    if cls.contains("sidebar") || cls.contains("aside") { return SectionType::Sidebar; }
    if cls.contains("footer") { return SectionType::Footer; }
    if cls.contains("gallery") || cls.contains("carousel") || cls.contains("slider") { return SectionType::Gallery; }
    if cls.contains("form") { return SectionType::Form; }

    // Children-based heuristic
    let child_tags: Vec<&str> = elem.children()
        .filter_map(|c| scraper::ElementRef::wrap(c))
        .map(|e| e.value().name())
        .collect();

    if child_tags.iter().filter(|t| **t == "img").count() > 2 { return SectionType::Gallery; }
    if child_tags.iter().all(|t| *t == "li") { return SectionType::List; }

    SectionType::Generic
}

/// Extract domain from URL
pub fn url_domain(url: &str) -> String {
    if let Some(start) = url.find("://") {
        let rest = &url[start + 3..];
        let end = rest.find('/').unwrap_or(rest.len());
        return rest[..end].to_string();
    }
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── is_structural_class tests ───

    #[test]
    fn structural_class_layout_prefixes() {
        assert!(is_structural_class("flex-row"));
        assert!(is_structural_class("grid-cols-3"));
        assert!(is_structural_class("col-md-6"));
        assert!(is_structural_class("row"));
        assert!(is_structural_class("container"));
        assert!(is_structural_class("card-body"));
        assert!(is_structural_class("hero"));
        assert!(is_structural_class("nav-link"));
        assert!(is_structural_class("header"));
        assert!(is_structural_class("footer-section"));
        assert!(is_structural_class("sidebar"));
        assert!(is_structural_class("section-light"));
        assert!(is_structural_class("article-body"));
        assert!(is_structural_class("main-content"));
        assert!(is_structural_class("wrapper"));
        assert!(is_structural_class("layout-grid"));
        assert!(is_structural_class("d-flex"));
    }

    #[test]
    fn structural_class_size_prefixes() {
        assert!(is_structural_class("sm-12"));
        assert!(is_structural_class("md-6"));
        assert!(is_structural_class("lg-4"));
        assert!(is_structural_class("xl-3"));
        assert!(is_structural_class("w-full"));
        assert!(is_structural_class("h-screen"));
    }

    #[test]
    fn non_structural_classes_pass_through() {
        // Content classes that should NOT be treated as structural.
        assert!(!is_structural_class("highlight"));
        assert!(!is_structural_class("button-primary"));
        assert!(!is_structural_class("text-bold"));
        assert!(!is_structural_class("active"));
        assert!(!is_structural_class("selected"));
    }

    // ─── url_domain tests ───

    #[test]
    fn url_domain_https() {
        assert_eq!(url_domain("https://example.com/path"), "example.com");
    }

    #[test]
    fn url_domain_http() {
        assert_eq!(url_domain("http://example.com"), "example.com");
    }

    #[test]
    fn url_domain_with_port() {
        // The implementation captures everything between :// and the
        // first /, so port stays in the result.
        assert_eq!(url_domain("https://example.com:8080/foo"), "example.com:8080");
    }

    #[test]
    fn url_domain_no_path() {
        assert_eq!(url_domain("https://example.com"), "example.com");
    }

    #[test]
    fn url_domain_no_scheme_passes_through() {
        // No `://` means we can't extract — return whole input.
        assert_eq!(url_domain("example.com/foo"), "example.com/foo");
    }

    #[test]
    fn url_domain_empty_string() {
        assert_eq!(url_domain(""), "");
    }

    // ─── LayoutCache LRU tests ───

    #[test]
    fn cache_new_empty() {
        let c = LayoutCache::new(5);
        assert_eq!(c.stats().0, 0); // 0 entries
    }

    #[test]
    fn cache_put_get_roundtrip() {
        let mut c = LayoutCache::new(5);
        c.put(42, "template1".to_string(), 3, "example.com");
        let entry = c.get(42).expect("should retrieve");
        assert_eq!(entry.template, "template1");
        assert_eq!(entry.slot_count, 3);
    }

    #[test]
    fn cache_miss_returns_none() {
        let mut c = LayoutCache::new(5);
        c.put(1, "t1".to_string(), 1, "site.com");
        assert!(c.get(9999).is_none());
    }

    #[test]
    fn cache_clear_empties_all() {
        let mut c = LayoutCache::new(5);
        c.put(1, "t1".to_string(), 1, "site.com");
        c.put(2, "t2".to_string(), 2, "site.com");
        c.clear();
        assert_eq!(c.stats().0, 0);
        assert!(c.get(1).is_none());
    }

    #[test]
    fn cache_clear_domain_selective() {
        let mut c = LayoutCache::new(5);
        c.put(1, "t1".to_string(), 1, "a.com");
        c.put(2, "t2".to_string(), 2, "b.com");
        c.clear_domain("a.com");
        assert!(c.get(1).is_none(), "a.com entry should be cleared");
        assert!(c.get(2).is_some(), "b.com entry should remain");
    }
}
