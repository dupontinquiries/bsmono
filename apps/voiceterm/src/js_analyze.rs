// src/js_analyze.rs
// Static analysis of <script> blocks to extract:
// - Navigation menus from JSON/array structures
// - Dynamic element creation patterns
// - Data-driven content (product lists, prices)
// - Config objects with URLs and labels


/// Extracted navigation item from JS analysis
#[derive(Debug, Clone)]
pub struct NavItem {
    pub label: String,
    pub url: String,
    pub children: Vec<NavItem>,
}

/// Extracted data from JS analysis
#[derive(Debug)]
pub struct JsExtract {
    pub nav_items: Vec<NavItem>,
    pub json_urls: Vec<(String, String)>, // (label, url)
    pub config_values: Vec<(String, String)>, // (key, value)
}

/// Analyze all <script> blocks in the HTML and extract useful structures
pub fn analyze_scripts(html: &str) -> JsExtract {
    let doc = scraper::Html::parse_document(html);
    static SEL: std::sync::OnceLock<scraper::Selector> = std::sync::OnceLock::new();
    let sel = SEL.get_or_init(|| scraper::Selector::parse("script").unwrap());

    let mut extract = JsExtract { nav_items: Vec::new(), json_urls: Vec::new(), config_values: Vec::new() };

    for script in doc.select(sel) {
        let text: String = script.text().collect();
        if text.len() < 20 || text.len() > 200_000 { continue; }

        // Extract JSON-LD structured data
        if script.value().attr("type") == Some("application/ld+json") {
            extract_json_ld(&text, &mut extract);
            continue;
        }

        // Look for navigation arrays: [{label:"...", url:"..."}, ...]
        extract_nav_arrays(&text, &mut extract);

        // Look for menu/config objects
        extract_config_objects(&text, &mut extract);

        // Look for document.createElement patterns
        extract_create_elements(&text, &mut extract);
    }

    // Also check data attributes on nav elements for JS-driven menus
    static SEL_NAV: std::sync::OnceLock<scraper::Selector> = std::sync::OnceLock::new();
    let sel_nav = SEL_NAV.get_or_init(|| scraper::Selector::parse("[data-nav], [data-menu], [data-items]").unwrap());
    for el in doc.select(sel_nav) {
        for attr in &["data-nav", "data-menu", "data-items"] {
            if let Some(val) = el.value().attr(attr) {
                extract_json_nav(val, &mut extract);
            }
        }
    }

    extract
}

/// Extract navigation items from JSON-LD
fn extract_json_ld(json: &str, extract: &mut JsExtract) {
    // Look for SiteNavigationElement or similar
    if !json.contains("NavigationElement") && !json.contains("\"url\"") { return; }

    // Extract "name":"..." "url":"..." pairs
    let mut i = 0;
    let bytes = json.as_bytes();
    while i < bytes.len().saturating_sub(10) {
        if let Some(pos) = json[i..].find("\"name\"") {
            let val_start = i + pos;
            if let Some(name) = extract_json_string(json, val_start + 6) {
                // Look for nearby "url".
                //
                // v0.1.396: char-boundary-safe slicing.
                // Arithmetic-derived byte index can land
                // mid-codepoint when JSON contains multibyte
                // chars (very common: site names with `é`,
                // emoji, CJK). Same class of bug as the
                // archive.rs date-window panic from the
                // hardware test. Snap back to char boundary.
                let want_end = val_start + 200;
                let mut search_end = want_end.min(json.len());
                while search_end > val_start && !json.is_char_boundary(search_end) {
                    search_end -= 1;
                }
                if let Some(url_pos) = json[val_start..search_end].find("\"url\"") {
                    if let Some(url) = extract_json_string(json, val_start + url_pos + 4) {
                        if !name.is_empty() && !url.is_empty() {
                            extract.nav_items.push(NavItem { label: name, url, children: Vec::new() });
                        }
                    }
                }
            }
            i = val_start + 6;
        } else { break; }
    }
}

/// Extract nav items from JS arrays like: [{text:"Home", href:"/"}, ...]
fn extract_nav_arrays(js: &str, extract: &mut JsExtract) {
    // Pattern: array of objects with text/label/name + href/url/link
    let label_keys = ["text", "label", "name", "title"];
    let url_keys = ["href", "url", "link", "path"];

    // Find array patterns
    for start_pat in &["menuItems", "navItems", "navigation", "menuLinks", "navLinks", "links"] {
        if let Some(pos) = js.find(start_pat) {
            let rest = &js[pos..js.len().min(pos + 5000)];
            // Find the array start
            if let Some(arr_start) = rest.find('[') {
                let arr = &rest[arr_start..];
                if let Some(arr_end) = find_matching_bracket(arr) {
                    let arr_content = &arr[1..arr_end];
                    extract_items_from_array(arr_content, &label_keys, &url_keys, extract);
                }
            }
        }
    }
}

/// Extract config objects with URL patterns
fn extract_config_objects(js: &str, extract: &mut JsExtract) {
    // Look for objects like: { routes: [{path: "/about", component: "About"}] }
    // or: window.__NEXT_DATA__ = {...}
    for config_pat in &["__NEXT_DATA__", "__NUXT__", "window.config", "APP_CONFIG", "pageConfig"] {
        if let Some(pos) = js.find(config_pat) {
            let rest = &js[pos..js.len().min(pos + 10000)];
            // Extract URL-like values
            extract_url_values(rest, extract);
        }
    }
}

/// Look for document.createElement and innerHTML patterns
fn extract_create_elements(js: &str, extract: &mut JsExtract) {
    // Pattern: document.createElement('a') or innerHTML with links
    let mut pos = 0;
    while pos < js.len() {
        if let Some(ce_pos) = js[pos..].find("createElement") {
            let ctx_start = pos + ce_pos;
            let ctx = &js[ctx_start..js.len().min(ctx_start + 500)];

            // Check for link creation: createElement('a')
            if ctx.contains("'a'") || ctx.contains("\"a\"") {
                // Look for href assignment nearby
                if let Some(href_val) = extract_assigned_string(ctx, "href") {
                    let label = extract_assigned_string(ctx, "textContent")
                        .or_else(|| extract_assigned_string(ctx, "innerText"))
                        .or_else(|| extract_assigned_string(ctx, "innerHTML"))
                        .unwrap_or_else(|| href_val.clone());
                    if !href_val.is_empty() {
                        extract.json_urls.push((label, href_val));
                    }
                }
            }
            pos = ctx_start + 13;
        } else { break; }
    }

    // Also check innerHTML assignments with link patterns
    pos = 0;
    while pos < js.len() {
        if let Some(ih_pos) = js[pos..].find("innerHTML") {
            let ctx_start = pos + ih_pos;
            let ctx = &js[ctx_start..js.len().min(ctx_start + 2000)];
            // Look for <a href="..."> patterns in innerHTML strings
            extract_href_from_html_string(ctx, extract);
            pos = ctx_start + 9;
        } else { break; }
    }
}

/// Extract from data-nav/data-menu JSON attributes
fn extract_json_nav(json: &str, extract: &mut JsExtract) {
    // Try to parse as JSON array of {label, url} objects
    let label_keys = ["text", "label", "name", "title"];
    let url_keys = ["href", "url", "link", "path"];
    if json.starts_with('[') {
        if let Some(end) = find_matching_bracket(json) {
            extract_items_from_array(&json[1..end], &label_keys, &url_keys, extract);
        }
    }
}

// ─── Helpers ───

fn extract_json_string(json: &str, after: usize) -> Option<String> {
    let rest = &json[after..];
    // Skip to the string value
    let q_start = rest.find('"')? + 1;
    let inner = &rest[q_start..];
    let q_end = inner.find('"')?;
    Some(inner[..q_end].to_string())
}

fn extract_assigned_string(js: &str, key: &str) -> Option<String> {
    let pos = js.find(key)?;
    let rest = &js[pos + key.len()..];
    let eq = rest.find('=')?;
    let after_eq = rest[eq + 1..].trim_start();
    for q in &['\'', '"', '`'] {
        if after_eq.starts_with(*q) {
            let inner = &after_eq[1..];
            let end = inner.find(*q)?;
            return Some(inner[..end].to_string());
        }
    }
    None
}

fn extract_url_values(js: &str, extract: &mut JsExtract) {
    // Find all quoted strings that look like URLs — cap at 30
    let mut pos = 0;
    let mut count = 0;
    while pos < js.len() && count < 30 {
        for q in &['"', '\''] {
            if js.as_bytes().get(pos) == Some(&(*q as u8)) {
                let inner = &js[pos + 1..];
                if let Some(end) = inner.find(*q) {
                    let val = &inner[..end];
                    if val.starts_with('/') && val.len() > 1 && val.len() < 100 && !val.contains(' ') && !val.contains("//") {
                        extract.json_urls.push((val.to_string(), val.to_string()));
                        count += 1;
                    }
                    pos += end + 2;
                    continue;
                }
            }
        }
        pos += 1;
    }
}

fn extract_href_from_html_string(js: &str, extract: &mut JsExtract) {
    let mut pos = 0;
    while pos < js.len() {
        if let Some(href_pos) = js[pos..].find("href=") {
            let start = pos + href_pos + 5;
            let rest = &js[start..];
            // Skip possible escaped quote
            let rest = rest.trim_start_matches('\\');
            for q in &['"', '\''] {
                if rest.starts_with(*q) {
                    let inner = &rest[1..];
                    if let Some(end) = inner.find(*q) {
                        let url = inner[..end].replace("\\/", "/");
                        if !url.is_empty() && (url.starts_with('/') || url.starts_with("http")) {
                            extract.json_urls.push((url.clone(), url));
                        }
                    }
                    break;
                }
            }
            pos = start + 5;
        } else { break; }
    }
}

fn find_matching_bracket(s: &str) -> Option<usize> {
    let open = s.as_bytes().first()?;
    let close = match open { b'[' => b']', b'{' => b'}', _ => return None };
    let mut depth = 0;
    let mut in_str = false;
    let mut esc = false;
    for (i, &b) in s.as_bytes().iter().enumerate() {
        if esc { esc = false; continue; }
        if b == b'\\' && in_str { esc = true; continue; }
        if b == b'"' { in_str = !in_str; continue; }
        if in_str { continue; }
        if b == *open { depth += 1; }
        if b == close { depth -= 1; if depth == 0 { return Some(i); } }
    }
    None
}

fn extract_items_from_array(arr: &str, label_keys: &[&str], url_keys: &[&str], extract: &mut JsExtract) {
    // Split on },{ to get individual objects
    for obj in arr.split("},{") {
        let obj = obj.trim().trim_matches(|c| c == '{' || c == '}');
        let mut label = None;
        let mut url = None;
        for lk in label_keys {
            if let Some(v) = extract_json_string_by_key(obj, lk) { label = Some(v); break; }
        }
        for uk in url_keys {
            if let Some(v) = extract_json_string_by_key(obj, uk) { url = Some(v); break; }
        }
        if let (Some(l), Some(u)) = (label, url) {
            if !l.is_empty() && !u.is_empty() {
                extract.nav_items.push(NavItem { label: l, url: u, children: Vec::new() });
            }
        }
    }
}

fn extract_json_string_by_key(obj: &str, key: &str) -> Option<String> {
    // Match "key":"value" or 'key':'value' or key:"value"
    for pat in &[
        format!("\"{}\":", key), format!("'{}':", key), format!("{}:", key),
    ] {
        if let Some(pos) = obj.find(pat.as_str()) {
            return extract_json_string(obj, pos + pat.len() - 1); // -1 because extract_json_string skips to next "
        }
    }
    None
}

/// Generate QML for extracted nav items
pub fn nav_to_qml(items: &[NavItem], base_url: &str, link_color: &str, font_size: i32, font_family: &str) -> String {
    if items.is_empty() { return String::new(); }
    let w = "parent ? parent.width : 800";
    let mut o = String::with_capacity(items.len() * 120);
    o.push_str("Column {\nwidth: "); o.push_str(w);
    o.push_str("\nspacing: 2\n");
    for item in items {
        let url = crate::qml_gen::resolve_url(&item.url, base_url);
        o.push_str("Text {\nwidth: parent.width\ntext: \"");
        crate::qml_gen::write_esc_pub(&mut o, &item.label);
        o.push_str("\"\ncolor: \""); o.push_str(link_color);
        o.push_str("\"\nfont.pixelSize: "); o.push_str(&font_size.to_string());
        o.push_str("\nfont.family: \""); o.push_str(font_family);
        o.push_str("\"\nfont.underline: true\nwrapMode: Text.WordWrap\nMouseArea {\nanchors.fill: parent\ncursorShape: Qt.PointingHandCursor\nonClicked: { if (backend) backend.load_page(\"");
        crate::qml_gen::write_esc_pub(&mut o, &url);
        o.push_str("\", backend.recursion_depth) }\n}\n}\n");
    }
    o.push_str("}");
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    /// extract_json_string: pulls the quoted value AFTER a position.
    #[test]
    fn json_string_basic() {
        let s = r#"{"key":"value"}"#;
        // Position right after the colon (pointing at the open quote)
        let after = s.find(':').unwrap() + 1;
        assert_eq!(extract_json_string(s, after), Some("value".to_string()));
    }

    /// extract_json_string: returns None when no quotes follow.
    #[test]
    fn json_string_no_quotes() {
        let s = "{key: nostring}";
        assert_eq!(extract_json_string(s, 0), None);
    }

    /// extract_assigned_string: handles double quotes.
    #[test]
    fn assigned_string_double_quote() {
        let js = "var url = \"https://example.com\"";
        assert_eq!(extract_assigned_string(js, "url"),
            Some("https://example.com".to_string()));
    }

    /// extract_assigned_string: handles single quotes.
    #[test]
    fn assigned_string_single_quote() {
        let js = "var url = 'https://example.com'";
        assert_eq!(extract_assigned_string(js, "url"),
            Some("https://example.com".to_string()));
    }

    /// extract_assigned_string: handles template literals.
    #[test]
    fn assigned_string_template_literal() {
        let js = "var url = `https://example.com`";
        assert_eq!(extract_assigned_string(js, "url"),
            Some("https://example.com".to_string()));
    }

    /// extract_assigned_string: returns None for missing key.
    #[test]
    fn assigned_string_missing_key() {
        let js = "var url = \"foo\"";
        assert_eq!(extract_assigned_string(js, "nonexistent"), None);
    }

    /// analyze_scripts on empty html returns empty extract.
    #[test]
    fn analyze_empty_html() {
        let r = analyze_scripts("<html></html>");
        assert!(r.nav_items.is_empty());
        assert!(r.json_urls.is_empty());
        assert!(r.config_values.is_empty());
    }

    /// Tiny scripts (<20 chars) are skipped (sanity check the
    /// length filter).
    #[test]
    fn analyze_tiny_script_skipped() {
        let html = r#"<html><body><script>var x=1;</script></body></html>"#;
        let r = analyze_scripts(html);
        // Tiny: 'var x=1;' is 8 chars — skipped. No extraction.
        assert!(r.config_values.is_empty());
    }
}
