// src/ad_filter.rs
// Native ad/tracker filter. Zero-alloc case-insensitive matching.

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum FilterMode { Off, Standard, Aggressive }
impl Default for FilterMode { fn default() -> Self { FilterMode::Standard } }

// Zero-allocation case-insensitive ASCII contains
#[inline]
fn contains_ci(haystack: &str, needle: &str) -> bool {
    let nb = needle.as_bytes();
    let hb = haystack.as_bytes();
    if nb.len() > hb.len() { return false; }
    hb.windows(nb.len()).any(|w| w.iter().zip(nb).all(|(a, b)| a.to_ascii_lowercase() == *b))
}

pub fn should_filter(tag: &str, id: Option<&str>, classes: Option<&str>, src: Option<&str>, mode: FilterMode) -> bool {
    if mode == FilterMode::Off { return false; }

    if tag == "iframe" {
        if let Some(s) = src {
            if ALLOWED_IFRAMES.iter().any(|d| contains_ci(s, d)) { return false; }
            if AD_DOMAINS.iter().any(|d| contains_ci(s, d)) { return true; }
            if mode == FilterMode::Aggressive { return true; }
        } else { return true; }
    }

    if let Some(id) = id {
        if AD_IDS.iter().any(|p| contains_ci(id, p)) { return true; }
        if mode == FilterMode::Aggressive && AGG_IDS.iter().any(|p| contains_ci(id, p)) { return true; }
    }

    if let Some(classes) = classes {
        for cls in classes.split_ascii_whitespace() {
            if AD_CLASSES.iter().any(|p| contains_ci(cls, p)) { return true; }
            if mode == FilterMode::Aggressive && AGG_CLASSES.iter().any(|p| contains_ci(cls, p)) { return true; }
        }
    }

    if let Some(s) = src {
        if AD_DOMAINS.iter().any(|d| contains_ci(s, d)) { return true; }
    }

    false
}

/// v0.1.379 (anti-modern-web identity): is this element
/// a tracking-pixel container — a wrapper element whose
/// purpose is telemetry, not user-visible content?
///
/// Returns true ONLY when the element has the unmistakable
/// shape of a tracking wrapper:
/// - Tag is structurally-neutral (`div`, `span`, `p`)
/// - HAS at least one tracking-shaped data attribute
///   (`data-impression-id`, `data-analytics-event`,
///   `data-tracking-id`, `data-ga-event`, `data-pixel-url`)
/// - AND either has NO children that would render
///   anything (empty), OR all attributes look telemetry-
///   shaped
///
/// We're conservative — false-negatives (we keep a tracker)
/// are reversible (perf cost, not correctness). False-
/// positives (we drop real content) are silent and
/// frustrating. So this fires only on the unmistakable
/// case; pattern variants get caller-side handling.
///
/// Returns `(should_remove, should_unwrap)`:
/// - (true, _) — drop the element and its subtree
/// - (false, true) — keep children but unwrap the
///   tracking-only wrapper itself (render its contents
///   directly into the parent's flow)
/// - (false, false) — not a tracker, render normally
///
/// Caller pattern: if `should_unwrap`, iterate the element's
/// children directly instead of emitting a Column/Row
/// wrapper around them. This is the "aggressive skipping"
/// from the JS-rendering design conversation.
pub fn classify_tracking_wrapper(
    tag: &str,
    attrs: &[(&str, &str)],
    has_child_elements: bool,
) -> (bool, bool) {
    // Only div/span/p are candidates — semantic tags
    // (article, section, header, etc.) carry meaning we
    // shouldn't override.
    if !matches!(tag, "div" | "span" | "p") {
        return (false, false);
    }

    // Look for at least one definitively-tracking attribute.
    let mut has_tracking_attr = false;
    for (name, _value) in attrs {
        let n_lower_ok = name.starts_with("data-analytics")
            || name.starts_with("data-tracking")
            || name.starts_with("data-impression")
            || name.starts_with("data-ga-")
            || name.starts_with("data-gtm-")
            || name.starts_with("data-pixel")
            || name.starts_with("data-fb-")
            || name.starts_with("data-segment")
            || name.starts_with("data-amplitude")
            || name.starts_with("data-event-")
            || *name == "data-component-list";  // Apple's pattern
        if n_lower_ok {
            has_tracking_attr = true;
            break;
        }
    }
    if !has_tracking_attr {
        return (false, false);
    }

    // If the element has no child elements, it's a pure
    // tracking pixel — drop it entirely.
    if !has_child_elements {
        return (true, false);
    }

    // Has children + tracking attrs: it's a tracking wrapper
    // AROUND real content. Unwrap — render the children
    // directly without the wrapper.
    (false, true)
}


const AD_DOMAINS: &[&str] = &[
    "doubleclick.net","googlesyndication.com","googleadservices.com","google-analytics.com",
    "googletagmanager.com","adservice.google","pagead2.googlesyndication","amazon-adsystem.com",
    "facebook.com/tr","facebook.net/en_us/fbevents","connect.facebook.net",
    "ads.yahoo.com","advertising.com","adnxs.com","adsrvr.org",
    "criteo.com","criteo.net","taboola.com","outbrain.com","media.net",
    "moatads.com","quantserve.com","scorecardresearch.com","chartbeat.com","hotjar.com",
    "optimizely.com","segment.com","segment.io","mixpanel.com","amplitude.com",
    "fullstory.com","mouseflow.com","onetrust.com","cookiebot.com",
    "ad.doubleclick","pubads.g.doubleclick","securepubads",
    "analytics.tiktok.com","bat.bing.com",
];

const ALLOWED_IFRAMES: &[&str] = &[
    "youtube.com","youtube-nocookie.com","player.vimeo.com","maps.google.com",
    "google.com/maps","openstreetmap.org","codepen.io","bandcamp.com",
    "soundcloud.com","spotify.com",
];

const AD_IDS: &[&str] = &[
    "ad-","ad_","ads-","ads_","adsense","advert","banner-ad","google_ads",
    "dfp-ad","gpt-ad","taboola","outbrain","sponsored","promo-",
];

const AD_CLASSES: &[&str] = &[
    "ad-container","ad-wrapper","ad-slot","ad-unit","ad-banner","ad-block",
    "adsbygoogle","advertisement","advertorial","sponsored-content","sponsored-post",
    "native-ad","promoted-content","dfp-ad","gpt-ad","taboola","outbrain",
    // v0.1.466: bare "sponsored"/"promoted" as standalone class
    // tokens. A multi-class attribute like `class="layout sponsored
    // content"` splits to ["layout","sponsored","content"]; the
    // pattern list only had the HYPHENATED forms ("sponsored-content")
    // which aren't substrings of the bare token, so the bare marker
    // slipped through (the bug behind ad_class_filters_multi). The
    // id list (AD_IDS) already trusts bare "sponsored" as an ad
    // signal, so this just makes class-matching consistent. Matching
    // is `contains_ci(token, pattern)`, so this still catches
    // "sponsored", "sponsored-x", etc. without over-matching unrelated
    // words.
    "sponsored","promoted",
];

const AGG_IDS: &[&str] = &[
    "cookie-consent","cookie-banner","cookie-notice","gdpr","consent-banner",
    "newsletter-popup","newsletter-modal","email-signup","subscribe-modal",
    "paywall","social-share","share-buttons",
];

const AGG_CLASSES: &[&str] = &[
    "cookie-consent","cookie-banner","cookie-notice","cookie-overlay","gdpr",
    "consent-manager","consent-modal","newsletter-popup","email-capture",
    "subscribe-overlay","paywall","overlay-modal","social-share","share-bar",
    "share-buttons","popup-overlay","interstitial","nag-bar","app-banner",
];

#[derive(Default, Debug, Clone)]
pub struct FilterStats {
    pub total_elements: u32,
    pub filtered_elements: u32,
    pub filtered_iframes: u32,
    pub filtered_by_class: u32,
    pub filtered_by_id: u32,
    pub filtered_by_domain: u32,
    /// v0.1.199: count of elements that the renderer wrapped for
    /// paragraph selection. Originally `<p>` only (10-50 word
    /// range); as of v0.1.236-244 ALSO ticks for headings (h1-h3),
    /// unordered lists, ordered lists, tables, blockquotes, and
    /// block-level code. Each call site passes its own ContentType
    /// so the reading overlay can apply per-content-type WPM
    /// multipliers. Useful for understanding how often the
    /// read-aloud feature is available on random pages. Populated
    /// by qml_gen during emission.
    pub selectable_paragraphs: u32,
}

impl FilterStats {
    pub fn summary(&self) -> String {
        if self.filtered_elements == 0 { "No ads detected".into() }
        else { format!("Blocked {} elements", self.filtered_elements) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FilterMode::Off should NEVER filter, regardless of content.
    #[test]
    fn off_mode_filters_nothing() {
        assert!(!should_filter("div", Some("google_ads"), None, None, FilterMode::Off));
        assert!(!should_filter("iframe", None, None, Some("doubleclick.net"), FilterMode::Off));
        assert!(!should_filter("script", None, Some("ads-banner"), None, FilterMode::Off));
    }

    /// Known ad domain in script src should filter.
    #[test]
    fn ad_domain_in_src_filters() {
        assert!(should_filter("script", None, None, Some("https://doubleclick.net/foo.js"),
            FilterMode::Standard));
        assert!(should_filter("img", None, None, Some("https://googlesyndication.com/pixel.gif"),
            FilterMode::Standard));
        // Case-insensitive
        assert!(should_filter("script", None, None, Some("https://DOUBLECLICK.NET/foo.js"),
            FilterMode::Standard));
    }

    /// Non-ad domain in src should NOT filter.
    #[test]
    fn non_ad_domain_in_src_passes() {
        assert!(!should_filter("script", None, None, Some("https://example.com/foo.js"),
            FilterMode::Standard));
        assert!(!should_filter("img", None, None, Some("https://anthropic.com/logo.png"),
            FilterMode::Standard));
    }

    /// AD_IDS list matches against id attribute.
    #[test]
    fn ad_id_filters() {
        assert!(should_filter("div", Some("google_ads"), None, None, FilterMode::Standard));
        assert!(should_filter("div", Some("banner-ad-top"), None, None, FilterMode::Standard));
        assert!(should_filter("div", Some("sponsored-content"), None, None, FilterMode::Standard));
    }

    /// Innocent ids should pass.
    #[test]
    fn non_ad_id_passes() {
        assert!(!should_filter("div", Some("main-content"), None, None, FilterMode::Standard));
        assert!(!should_filter("div", Some("header"), None, None, FilterMode::Standard));
        assert!(!should_filter("div", Some("article"), None, None, FilterMode::Standard));
    }

    /// AD_CLASSES list matches against class attribute. Multi-class
    /// supported via whitespace split.
    #[test]
    fn ad_class_filters_multi() {
        assert!(should_filter("div", None, Some("ad-banner"), None, FilterMode::Standard));
        assert!(should_filter("div", None, Some("layout sponsored content"), None, FilterMode::Standard));
    }

    /// Allowed iframe domains (youtube, vimeo) pass even with
    /// other ad markers.
    #[test]
    fn allowed_iframes_pass() {
        assert!(!should_filter("iframe", None, None, Some("https://www.youtube.com/embed/abc"),
            FilterMode::Standard));
        assert!(!should_filter("iframe", None, None, Some("https://player.vimeo.com/video/123"),
            FilterMode::Standard));
    }

    /// Unknown iframe src in Aggressive mode filters; in Standard
    /// it doesn't.
    #[test]
    fn unknown_iframe_aggressive_only() {
        let src = Some("https://example.com/embed");
        assert!(!should_filter("iframe", None, None, src, FilterMode::Standard));
        assert!(should_filter("iframe", None, None, src, FilterMode::Aggressive));
    }

    /// Iframe with no src — assume worst, always filter.
    #[test]
    fn srcless_iframe_filters() {
        assert!(should_filter("iframe", None, None, None, FilterMode::Standard));
    }

    // v0.1.379: tracking-wrapper classification tests.

    #[test]
    fn tracking_wrapper_empty_div_with_analytics_attr_dropped() {
        // Empty wrapper with telemetry attr — drop it.
        let attrs = vec![
            ("data-analytics-event", "hero-impression"),
            ("data-impression-id", "abc123"),
        ];
        let (remove, unwrap) = classify_tracking_wrapper("div", &attrs, false);
        assert!(remove, "empty tracking wrapper should be removed");
        assert!(!unwrap);
    }

    #[test]
    fn tracking_wrapper_with_content_unwrapped() {
        // Wrapper around real content — keep children, drop wrapper.
        let attrs = vec![
            ("data-analytics-section", "switch-to-iphone"),
            ("class", "section"),
        ];
        let (remove, unwrap) = classify_tracking_wrapper("div", &attrs, true);
        assert!(!remove, "wrapper with children should not be removed");
        assert!(unwrap, "wrapper with children should be unwrapped");
    }

    #[test]
    fn tracking_wrapper_no_tracking_attr_keeps() {
        // Normal div with normal attrs — no special handling.
        let attrs = vec![
            ("class", "container"),
            ("id", "main"),
        ];
        let (remove, unwrap) = classify_tracking_wrapper("div", &attrs, true);
        assert!(!remove);
        assert!(!unwrap);
    }

    #[test]
    fn tracking_wrapper_semantic_tags_not_classified() {
        // Even with tracking attrs, semantic tags like <article>
        // carry meaning we shouldn't override.
        let attrs = vec![
            ("data-analytics-event", "article-view"),
            ("data-impression-id", "xyz"),
        ];
        let (remove, unwrap) = classify_tracking_wrapper("article", &attrs, true);
        assert!(!remove);
        assert!(!unwrap, "semantic article tag should never be unwrapped");
    }

    #[test]
    fn tracking_wrapper_recognizes_data_component_list() {
        // Apple's data-component-list pattern.
        let attrs = vec![
            ("data-component-list", "Card Gallery"),
        ];
        let (remove, unwrap) = classify_tracking_wrapper("div", &attrs, true);
        assert!(unwrap, "data-component-list should be recognized as tracking wrapper");
        assert!(!remove,
            "a wrapper WITH child elements must be unwrapped, not removed \
             (removing would delete the legitimate content inside)");
    }

    #[test]
    fn tracking_wrapper_recognizes_gtm_attrs() {
        // Google Tag Manager pattern.
        let attrs = vec![
            ("data-gtm-event", "click"),
            ("data-gtm-category", "engagement"),
        ];
        let (remove, _unwrap) = classify_tracking_wrapper("div", &attrs, false);
        assert!(remove, "empty GTM wrapper should be removed");
    }

    #[test]
    fn tracking_wrapper_recognizes_fb_attrs() {
        // Facebook Pixel companion attrs.
        let attrs = vec![
            ("data-fb-pixel", "track"),
        ];
        let (remove, _) = classify_tracking_wrapper("span", &attrs, false);
        assert!(remove);
    }

    #[test]
    fn tracking_wrapper_recognizes_segment_attrs() {
        let attrs = vec![
            ("data-segment-event", "page-view"),
        ];
        let (remove, _) = classify_tracking_wrapper("div", &attrs, false);
        assert!(remove);
    }

    #[test]
    fn tracking_wrapper_paragraph_with_tracking_attr() {
        // <p data-analytics-event="quote-impression">Inspirational quote</p>
        // — keep the quote, drop the wrapper. Edge case where p
        // has tracking attrs (rare but seen on marketing pages).
        let attrs = vec![
            ("data-analytics-event", "quote-impression"),
        ];
        let (remove, unwrap) = classify_tracking_wrapper("p", &attrs, true);
        assert!(!remove);
        assert!(unwrap);
    }

    #[test]
    fn tracking_wrapper_no_attrs_no_action() {
        let attrs: Vec<(&str, &str)> = vec![];
        let (remove, unwrap) = classify_tracking_wrapper("div", &attrs, false);
        assert!(!remove);
        assert!(!unwrap);
    }
}
