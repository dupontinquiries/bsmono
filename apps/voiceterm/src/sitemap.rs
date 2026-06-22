// src/sitemap.rs
// Site discovery: build a map of a domain from browsing history,
// sitemap.xml, robots.txt, and internal link extraction.

use std::collections::{HashMap, HashSet};

/// A discovered page on a site
#[derive(Clone, Debug)]
pub struct SitePage {
    pub path: String,
    pub title: String,
    pub source: &'static str, // "visited", "sitemap", "robots", "link", "probe"
    pub status: PageStatus,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PageStatus {
    Known,    // Found in sitemap/robots/links but not visited
    Visited,  // Actually loaded by user
    Probed,   // Checked via HEAD/GET, got 200
    NotFound, // Probed, got 404
    Error,    // Probed, got other error
}

/// Site map for a single domain
#[derive(Clone, Debug)]
pub struct SiteMap {
    pub domain: String,
    pub pages: HashMap<String, SitePage>, // path → page
    pub sitemap_fetched: bool,
    pub robots_fetched: bool,
    /// v0.1.227: full Sitemap: URLs discovered in robots.txt.
    /// May point to non-default sitemap paths
    /// (e.g. `/wp-sitemap.xml`, `/sitemap_index.xml`, sitemap
    /// indexes). The caller is expected to fetch each and pass
    /// the contents to `parse_sitemap_xml`. Cross-domain URLs
    /// are stored but the caller should filter to same-domain
    /// before fetching (security: don't follow arbitrary URLs
    /// from a domain we don't trust).
    pub discovered_sitemaps: Vec<String>,
}

impl SiteMap {
    pub fn new(domain: &str) -> Self {
        SiteMap {
            domain: domain.to_string(),
            pages: HashMap::new(),
            sitemap_fetched: false,
            robots_fetched: false,
            discovered_sitemaps: Vec::new(),
        }
    }

    pub fn add_visited(&mut self, path: &str, title: &str) {
        let entry = self.pages.entry(path.to_string()).or_insert_with(|| SitePage {
            path: path.to_string(), title: String::new(), source: "visited", status: PageStatus::Visited,
        });
        entry.status = PageStatus::Visited;
        if !title.is_empty() { entry.title = title.to_string(); }
    }

    pub fn add_link(&mut self, path: &str) {
        self.pages.entry(path.to_string()).or_insert_with(|| SitePage {
            path: path.to_string(), title: String::new(), source: "link", status: PageStatus::Known,
        });
    }

    pub fn add_from_sitemap(&mut self, path: &str) {
        // v0.1.182: validate path looks reasonable before storing.
        // Reject paths that contain "://" (full URLs accidentally
        // landing here), empty paths, paths with doubled slashes
        // within them (which produce malformed final URLs in
        // format_for_qml). The check is loose — we just want to
        // avoid obvious garbage from upstream parse bugs or weird
        // site configurations.
        if !is_safe_path(path) { return; }
        let entry = self.pages.entry(path.to_string()).or_insert_with(|| SitePage {
            path: path.to_string(), title: String::new(), source: "sitemap", status: PageStatus::Known,
        });
        if entry.source == "link" { entry.source = "sitemap"; }
    }

    pub fn add_from_robots(&mut self, path: &str) {
        if !is_safe_path(path) { return; }
        self.pages.entry(path.to_string()).or_insert_with(|| SitePage {
            path: path.to_string(), title: String::new(), source: "robots", status: PageStatus::Known,
        });
    }

    /// Get sorted path tree
    pub fn path_tree(&self) -> Vec<&SitePage> {
        let mut sorted: Vec<&SitePage> = self.pages.values().collect();
        sorted.sort_by(|a, b| a.path.cmp(&b.path));
        sorted
    }

    /// Format for QML display
    pub fn format_for_qml(&self) -> String {
        let mut out = String::with_capacity(self.pages.len() * 80);
        let tree = self.path_tree();
        for page in &tree {
            let icon = match page.status {
                PageStatus::Visited => "\u{2705}",  // ✅
                PageStatus::Probed => "\u{1F7E2}",   // 🟢
                PageStatus::Known => "\u{1F535}",     // 🔵
                PageStatus::NotFound => "\u{1F534}",  // 🔴
                PageStatus::Error => "\u{1F7E1}",     // 🟡
            };
            // Indent based on path depth
            let depth = page.path.matches('/').count().saturating_sub(1);
            let indent = "  ".repeat(depth.min(4));
            out.push_str("P\x1F");
            out.push_str(icon); out.push(' '); out.push_str(&indent);
            out.push_str(&page.path);
            if !page.title.is_empty() { out.push_str(" — "); out.push_str(&page.title[..page.title.len().min(40)]); }
            out.push('\x1F');
            // Full URL for navigation
            out.push_str("https://"); out.push_str(&self.domain); out.push_str(&page.path);
            out.push('\x1F');
            out.push_str(page.source);
            out.push('\n');
        }
        out
    }
}

/// Global site map registry
pub struct SiteMapRegistry {
    pub sites: HashMap<String, SiteMap>,
}

impl SiteMapRegistry {
    pub fn new() -> Self { SiteMapRegistry { sites: HashMap::new() } }

    pub fn get_or_create(&mut self, domain: &str) -> &mut SiteMap {
        self.sites.entry(domain.to_string()).or_insert_with(|| SiteMap::new(domain))
    }

    /// Record a page visit
    pub fn record_visit(&mut self, url: &str, title: &str) {
        let (domain, path) = split_url(url);
        if domain.is_empty() { return; }
        self.get_or_create(&domain).add_visited(&path, title);
    }

    /// Record internal links found on a page
    pub fn record_links(&mut self, page_url: &str, links: &[String]) {
        let (domain, _) = split_url(page_url);
        if domain.is_empty() { return; }
        let site = self.get_or_create(&domain);
        for link in links {
            let (ld, lp) = split_url(link);
            if ld == domain { site.add_link(&lp); }
        }
    }

    /// Parse sitemap.xml content.
    ///
    /// Extracts `<loc>...</loc>` URLs. Two flavors:
    ///   - `<urlset>`: standard sitemap with page URLs. Each
    ///     `<loc>` is a page; its path gets added via
    ///     `add_from_sitemap`. Returns empty Vec.
    ///   - `<sitemapindex>`: nested-sitemap index. Each `<loc>`
    ///     points to another sitemap.xml file, not a page. We
    ///     return the list of nested-sitemap URLs (full URLs)
    ///     for the caller to fetch and recursively parse.
    ///
    /// The flavor is detected by the presence of `<sitemapindex`
    /// in the XML (case-sensitive — the spec mandates lowercase).
    ///
    /// v0.1.228: previously this function returned `()` and
    /// emitted a warning for index-flavored input without
    /// recursing. Now returns the nested URLs so the caller
    /// (`site_discover` in main.rs) can fetch each. Same-domain
    /// + recursion-depth limits are the caller's responsibility,
    /// not this parser's.
    pub fn parse_sitemap_xml(&mut self, domain: &str, xml: &str) -> Vec<String> {
        let site = self.get_or_create(domain);
        site.sitemap_fetched = true;
        let is_index = xml.contains("<sitemapindex");
        let mut nested_urls: Vec<String> = Vec::new();
        // Simple XML extraction — find all <loc>...</loc> tags
        let mut pos = 0;
        while let Some(start) = xml[pos..].find("<loc>") {
            let s = pos + start + 5;
            if let Some(end) = xml[s..].find("</loc>") {
                let url = &xml[s..s + end];
                if is_index {
                    // v0.1.228: collect for caller to fetch.
                    // Dedup against already-collected.
                    let url_trimmed = url.trim();
                    if !url_trimmed.is_empty()
                        && (url_trimmed.starts_with("http://")
                            || url_trimmed.starts_with("https://"))
                        && !nested_urls.iter().any(|u| u == url_trimmed)
                    {
                        nested_urls.push(url_trimmed.to_string());
                    }
                    pos = s + end;
                    continue;
                }
                let (_, path) = split_url(url);
                if !path.is_empty() { site.add_from_sitemap(&path); }
                pos = s + end;
            } else { break; }
        }
        nested_urls
    }

    /// Parse robots.txt content
    ///
    /// Recognized directives:
    ///   - `Disallow:` / `Allow:` paths added as `add_from_robots`
    ///     (these are paths the site author told crawlers about,
    ///     useful for site discovery even if we're not a crawler)
    ///   - `Sitemap:` URLs collected in `discovered_sitemaps` for
    ///     the caller to fetch (v0.1.227)
    ///
    /// Wildcards in Disallow/Allow paths are skipped — we keep
    /// the simple match-by-exact-path model and don't try to
    /// expand glob patterns.
    pub fn parse_robots_txt(&mut self, domain: &str, txt: &str) {
        let site = self.get_or_create(domain);
        site.robots_fetched = true;
        for line in txt.lines() {
            let line = line.trim();
            if let Some(path) = line.strip_prefix("Disallow:").or_else(|| line.strip_prefix("Allow:")) {
                let path = path.trim();
                if !path.is_empty() && path.starts_with('/') && !path.contains('*') {
                    site.add_from_robots(path);
                }
            }
            if let Some(sm) = line.strip_prefix("Sitemap:") {
                let sm = sm.trim();
                // v0.1.227: collect Sitemap: URLs for the caller
                // to fetch separately. Skipping non-absolute URLs
                // (the spec requires absolute URLs for Sitemap:
                // directives — some malformed robots.txt files
                // have relative paths, but the loss from skipping
                // them is small and the alternative is fragile
                // resolution against the robots.txt base URL).
                //
                // Deduplicate against already-discovered URLs so
                // repeated directives (some robots.txt files
                // list the same sitemap twice) don't cause
                // duplicate fetches.
                if !sm.is_empty()
                    && (sm.starts_with("http://") || sm.starts_with("https://"))
                    && !site.discovered_sitemaps.iter().any(|u| u == sm)
                {
                    site.discovered_sitemaps.push(sm.to_string());
                }
            }
        }
    }

    /// Get common paths to probe for a domain
    pub fn common_probe_paths() -> &'static [&'static str] {
        &[
            "/", "/about", "/contact", "/blog", "/docs", "/api",
            "/login", "/signup", "/register", "/status", "/health",
            "/help", "/faq", "/terms", "/privacy", "/sitemap.xml",
            "/robots.txt", "/feed", "/rss", "/atom.xml",
            "/.well-known/security.txt", "/humans.txt",
        ]
    }
}

/// Extract internal links from HTML
pub fn extract_internal_links(html: &str, base_url: &str) -> Vec<String> {
    let doc = scraper::Html::parse_document(html);
    static SEL: std::sync::OnceLock<scraper::Selector> = std::sync::OnceLock::new();
    let sel = SEL.get_or_init(|| scraper::Selector::parse("a[href]").unwrap());
    let (base_domain, _) = split_url(base_url);
    let mut links = Vec::new();
    let mut seen = HashSet::new();

    for el in doc.select(sel) {
        if let Some(href) = el.value().attr("href") {
            let resolved = crate::qml_gen::resolve_url(href, base_url);
            let (ld, _) = split_url(&resolved);
            if ld == base_domain && !seen.contains(&resolved) {
                seen.insert(resolved.clone());
                links.push(resolved);
            }
        }
    }
    links
}

fn split_url(url: &str) -> (String, String) {
    if let Some(start) = url.find("://") {
        let rest = &url[start + 3..];
        let host_end = rest.find('/').unwrap_or(rest.len());
        let domain = rest[..host_end].to_string();
        let path = if host_end < rest.len() { rest[host_end..].to_string() } else { "/".to_string() };
        // Strip query and fragment
        let path = if let Some(q) = path.find('?') { path[..q].to_string() } else { path };
        let path = if let Some(h) = path.find('#') { path[..h].to_string() } else { path };
        (domain, path)
    } else {
        (String::new(), url.to_string())
    }
}

/// v0.1.400: public wrapper over the internal `split_url` so
/// callers outside this module (main.rs sitemap filter) can
/// extract (host, path) without duplicating the logic.
pub fn split_url_pub(url: &str) -> (String, String) {
    split_url(url)
}

/// v0.1.400 (cross-subdomain sitemap acceptance): decide
/// whether two hosts belong to the same site for the purpose
/// of accepting a `Sitemap:` directive from robots.txt.
///
/// Before this, the filter required an EXACT host match, so
/// browsing `apple.com` and seeing
/// `Sitemap: https://www.apple.com/sitemap.xml` in robots.txt
/// rejected the sitemap because `www.apple.com != apple.com`.
///
/// The relaxation: accept when one host is a subdomain of the
/// other — i.e., the longer host ends with `.` + the shorter
/// host. So:
///   apple.com         ~ www.apple.com        → accept
///   www.apple.com     ~ apple.com            → accept
///   apple.com         ~ shop.apple.com       → accept (shop is
///                                              a child of apple.com)
///   shop.apple.com    ~ store.apple.com      → REJECT (siblings;
///                                              neither is a parent
///                                              of the other)
///   apple.com         ~ evil.com             → reject
///   apple.com         ~ notapple.com         → reject (suffix
///                                              match without a
///                                              dot boundary)
///   apple.com         ~ apple.com.evil.com   → reject (the dot-
///                                              boundary check
///                                              prevents this
///                                              classic attack —
///                                              `apple.com.evil.com`
///                                              ends with
///                                              `.com.evil.com`,
///                                              not `.apple.com`)
///
/// SECURITY NOTE: robots.txt is unauthenticated. The original
/// comment correctly flagged cross-domain Sitemap URLs as a
/// security smell. This relaxation stays conservative: it only
/// admits a strict parent/child subdomain relationship, gated
/// on a `.` boundary so `notapple.com` can't masquerade as a
/// subdomain of `apple.com`. It does NOT admit sibling
/// subdomains or arbitrary same-suffix hosts. The most common
/// legitimate case (apex ↔ www) is covered; exotic
/// multi-subdomain setups still reject (and fall back to the
/// default /sitemap.xml probe).
pub fn same_registrable_domain(base_host: &str, candidate_host: &str) -> bool {
    let base = base_host.trim().trim_end_matches('.').to_lowercase();
    let cand = candidate_host.trim().trim_end_matches('.').to_lowercase();
    if base.is_empty() || cand.is_empty() {
        return false;
    }
    if base == cand {
        return true;
    }
    // Strip a leading "www." from each for the comparison so
    // www.apple.com and apple.com normalize together, AND
    // www.apple.com ~ apple.com works symmetrically.
    let base_core = base.strip_prefix("www.").unwrap_or(&base);
    let cand_core = cand.strip_prefix("www.").unwrap_or(&cand);
    if base_core == cand_core {
        return true;
    }
    // Parent/child: the longer host must end with "." + the
    // shorter host. The dot boundary prevents notapple.com
    // from matching apple.com.
    let (shorter, longer) = if base_core.len() <= cand_core.len() {
        (base_core, cand_core)
    } else {
        (cand_core, base_core)
    };
    let suffix = format!(".{}", shorter);
    longer.ends_with(&suffix)
}

/// v0.1.182: validate a path string before storing in the sitemap.
/// We reject obvious garbage that would produce malformed URLs in
/// format_for_qml's `https://{domain}{path}` reconstruction:
///   - empty paths
///   - paths containing "://" (full URLs)
///   - paths with consecutive duplicate path segments (likely a
///     resolver bug somewhere or a real malformed entry — either
///     way the resulting URL won't work)
///   - paths not starting with "/"
///
/// Logs to stderr when rejecting so we can diagnose what's hitting
/// the filter (and adjust if a legit pattern is being rejected).
fn is_safe_path(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    if !path.starts_with('/') {
        eprintln!("  sitemap: rejecting non-absolute path: {}", path);
        return false;
    }
    if path.contains("://") {
        eprintln!("  sitemap: rejecting path with embedded scheme: {}", path);
        return false;
    }
    // Reject duplicate consecutive path segments like /ca/ca/ —
    // these were seen in user's report and produce 404s on Apple.
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    for window in segments.windows(2) {
        if window[0] == window[1] {
            eprintln!("  sitemap: rejecting path with duplicate segments: {}", path);
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    // v0.1.400: same_registrable_domain tests. These guard
    // both the legitimate-acceptance cases and the security-
    // relevant rejections.

    #[test]
    fn srd_exact_match() {
        assert!(same_registrable_domain("apple.com", "apple.com"));
    }

    #[test]
    fn srd_apex_and_www() {
        // The headline case from the todo.
        assert!(same_registrable_domain("apple.com", "www.apple.com"));
        assert!(same_registrable_domain("www.apple.com", "apple.com"));
    }

    #[test]
    fn srd_parent_child_subdomain() {
        // shop.apple.com is a child of apple.com → accept.
        assert!(same_registrable_domain("apple.com", "shop.apple.com"));
        assert!(same_registrable_domain("shop.apple.com", "apple.com"));
    }

    #[test]
    fn srd_deep_subdomain() {
        assert!(same_registrable_domain("apple.com", "cdn.assets.apple.com"));
    }

    #[test]
    fn srd_sibling_subdomains_rejected() {
        // Neither is a parent of the other → reject. We don't
        // try to compute the registrable domain via a public-
        // suffix list; siblings simply aren't admitted.
        assert!(!same_registrable_domain("shop.apple.com", "store.apple.com"));
    }

    #[test]
    fn srd_different_domain_rejected() {
        assert!(!same_registrable_domain("apple.com", "evil.com"));
    }

    #[test]
    fn srd_lookalike_suffix_rejected() {
        // notapple.com ends with "apple.com" but NOT with
        // ".apple.com" — the dot boundary saves us.
        assert!(!same_registrable_domain("apple.com", "notapple.com"));
        assert!(!same_registrable_domain("notapple.com", "apple.com"));
    }

    #[test]
    fn srd_hyphen_lookalike_rejected() {
        // evil-apple.com ends with "-apple.com", not ".apple.com".
        assert!(!same_registrable_domain("apple.com", "evil-apple.com"));
    }

    #[test]
    fn srd_classic_suffix_attack_rejected() {
        // The classic "append your domain as a subdomain of
        // mine" attack. apple.com.evil.com ends with
        // ".com.evil.com", NOT ".apple.com".
        assert!(!same_registrable_domain("apple.com", "apple.com.evil.com"));
    }

    #[test]
    fn srd_case_insensitive() {
        assert!(same_registrable_domain("Apple.COM", "www.apple.com"));
    }

    #[test]
    fn srd_trailing_dot_normalized() {
        // FQDN trailing dot shouldn't break the match.
        assert!(same_registrable_domain("apple.com.", "www.apple.com"));
    }

    #[test]
    fn srd_empty_rejected() {
        assert!(!same_registrable_domain("", "apple.com"));
        assert!(!same_registrable_domain("apple.com", ""));
        assert!(!same_registrable_domain("", ""));
    }

    #[test]
    fn srd_www_on_both_sides() {
        assert!(same_registrable_domain("www.apple.com", "www.apple.com"));
    }

    #[test]
    fn split_url_pub_extracts_host() {
        let (host, path) = split_url_pub("https://www.apple.com/sitemap.xml");
        assert_eq!(host, "www.apple.com");
        assert_eq!(path, "/sitemap.xml");
    }

    #[test]
    fn safe_path_accepts_normal() {
        assert!(is_safe_path("/"));
        assert!(is_safe_path("/about"));
        assert!(is_safe_path("/ca/apple-watch-ultra-3/"));
        assert!(is_safe_path("/blog/2026/01/post-name"));
    }

    #[test]
    fn safe_path_rejects_doubled_segments() {
        // The case from the v0.1.181 user report.
        assert!(!is_safe_path("/ca/ca/apple-watch-ultra-3/"));
        assert!(!is_safe_path("/foo/foo"));
        assert!(!is_safe_path("/a/b/b/c"));
    }

    #[test]
    fn safe_path_rejects_full_url() {
        // The robots.txt Sitemap: bug.
        assert!(!is_safe_path("https://example.com/sitemap.xml"));
    }

    #[test]
    fn safe_path_rejects_non_absolute() {
        assert!(!is_safe_path("relative/path"));
        assert!(!is_safe_path(""));
    }

    #[test]
    fn add_from_sitemap_rejects_garbage() {
        let mut s = SiteMap::new("apple.com");
        s.add_from_sitemap("/ca/ca/x");        // doubled — rejected
        s.add_from_sitemap("https://x.com/y");  // full URL — rejected
        s.add_from_sitemap("/ca/x");           // ok
        assert_eq!(s.pages.len(), 1);
        assert!(s.pages.contains_key("/ca/x"));
    }

    // v0.1.227: Sitemap: directive collection from robots.txt.

    #[test]
    fn parse_robots_collects_sitemap_directive() {
        let mut reg = SiteMapRegistry::new();
        let robots = "\
User-agent: *
Disallow: /admin
Sitemap: https://example.com/sitemap.xml
Sitemap: https://example.com/wp-sitemap.xml
";
        reg.parse_robots_txt("example.com", robots);
        let site = reg.get_or_create("example.com");
        assert_eq!(site.discovered_sitemaps.len(), 2);
        assert!(site.discovered_sitemaps.iter().any(|u| u == "https://example.com/sitemap.xml"));
        assert!(site.discovered_sitemaps.iter().any(|u| u == "https://example.com/wp-sitemap.xml"));
    }

    #[test]
    fn parse_robots_dedupes_sitemap_directives() {
        // Some robots.txt files list the same Sitemap: twice.
        let mut reg = SiteMapRegistry::new();
        let robots = "\
Sitemap: https://example.com/sitemap.xml
Sitemap: https://example.com/sitemap.xml
Sitemap: https://example.com/other.xml
";
        reg.parse_robots_txt("example.com", robots);
        let site = reg.get_or_create("example.com");
        // Only 2 distinct, despite 3 directives.
        assert_eq!(site.discovered_sitemaps.len(), 2);
    }

    #[test]
    fn parse_robots_skips_relative_sitemap_url() {
        // Spec requires absolute URLs; we don't try to resolve
        // relative paths.
        let mut reg = SiteMapRegistry::new();
        let robots = "Sitemap: /sitemap.xml\n";
        reg.parse_robots_txt("example.com", robots);
        let site = reg.get_or_create("example.com");
        assert_eq!(site.discovered_sitemaps.len(), 0);
    }

    #[test]
    fn parse_robots_accepts_http_and_https() {
        let mut reg = SiteMapRegistry::new();
        let robots = "\
Sitemap: http://example.com/a.xml
Sitemap: https://example.com/b.xml
";
        reg.parse_robots_txt("example.com", robots);
        let site = reg.get_or_create("example.com");
        assert_eq!(site.discovered_sitemaps.len(), 2);
    }

    #[test]
    fn parse_robots_cross_domain_sitemap_preserved_but_caller_filters() {
        // The parser stores cross-domain Sitemap: URLs — the
        // caller is expected to filter to same-domain before
        // fetching. This keeps the parser's responsibility narrow.
        let mut reg = SiteMapRegistry::new();
        let robots = "Sitemap: https://cdn.example.com/sitemap.xml\n";
        reg.parse_robots_txt("example.com", robots);
        let site = reg.get_or_create("example.com");
        assert_eq!(site.discovered_sitemaps.len(), 1);
        assert_eq!(site.discovered_sitemaps[0], "https://cdn.example.com/sitemap.xml");
    }

    #[test]
    fn parse_sitemap_xml_index_skips_pages() {
        // A sitemapindex file has <loc> URLs that point to nested
        // sitemaps, not pages. We detect the case and skip adding
        // entries (rather than corrupting the registry with
        // sub-sitemap URLs as if they were page paths).
        let mut reg = SiteMapRegistry::new();
        let index_xml = "\
<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<sitemapindex xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">
  <sitemap><loc>https://example.com/sitemap-posts.xml</loc></sitemap>
  <sitemap><loc>https://example.com/sitemap-pages.xml</loc></sitemap>
</sitemapindex>
";
        let nested = reg.parse_sitemap_xml("example.com", index_xml);
        let site = reg.get_or_create("example.com");
        // No pages added.
        assert_eq!(site.pages.len(), 0);
        // But sitemap_fetched flag is set (we did process the file).
        assert!(site.sitemap_fetched);
        // v0.1.228: nested URLs returned for caller to fetch.
        assert_eq!(nested.len(), 2);
        assert!(nested.iter().any(|u| u == "https://example.com/sitemap-posts.xml"));
        assert!(nested.iter().any(|u| u == "https://example.com/sitemap-pages.xml"));
    }

    #[test]
    fn parse_sitemap_xml_urlset_works() {
        // The normal case — urlset with page URLs.
        let mut reg = SiteMapRegistry::new();
        let urlset_xml = "\
<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">
  <url><loc>https://example.com/about</loc></url>
  <url><loc>https://example.com/contact</loc></url>
</urlset>
";
        let nested = reg.parse_sitemap_xml("example.com", urlset_xml);
        let site = reg.get_or_create("example.com");
        assert_eq!(site.pages.len(), 2);
        assert!(site.pages.contains_key("/about"));
        assert!(site.pages.contains_key("/contact"));
        // v0.1.228: urlset has no nested URLs.
        assert!(nested.is_empty());
    }

    #[test]
    fn parse_sitemap_xml_index_dedupes_nested() {
        // A pathological index that lists the same nested URL
        // multiple times should only report it once.
        let mut reg = SiteMapRegistry::new();
        let index_xml = "\
<sitemapindex>
  <sitemap><loc>https://example.com/a.xml</loc></sitemap>
  <sitemap><loc>https://example.com/a.xml</loc></sitemap>
  <sitemap><loc>https://example.com/b.xml</loc></sitemap>
</sitemapindex>
";
        let nested = reg.parse_sitemap_xml("example.com", index_xml);
        assert_eq!(nested.len(), 2);
    }

    #[test]
    fn parse_sitemap_xml_index_skips_non_absolute_loc() {
        // Defensive: nested URLs that aren't absolute http(s)://
        // get dropped. Sitemaps shouldn't contain these but real-
        // world XML is messy.
        let mut reg = SiteMapRegistry::new();
        let index_xml = "\
<sitemapindex>
  <sitemap><loc>/relative-sitemap.xml</loc></sitemap>
  <sitemap><loc>https://example.com/ok.xml</loc></sitemap>
  <sitemap><loc></loc></sitemap>
</sitemapindex>
";
        let nested = reg.parse_sitemap_xml("example.com", index_xml);
        assert_eq!(nested.len(), 1);
        assert_eq!(nested[0], "https://example.com/ok.xml");
    }
}
