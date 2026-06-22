// src/web_islands.rs
//
// "Web islands" are regions of HTML that don't render usefully through
// our static HTML→QML pipeline because they depend on JavaScript to
// produce their visual output. Examples:
//
//   - <canvas> elements drawn at runtime by Chart.js / d3 / threejs
//   - <video> embeds with custom JS players
//   - <svg> trees built/animated by JS
//   - <iframe> embeds (YouTube, Twitter, Codepen)
//   - <div class="chart-..."> containers populated by frontend JS
//
// The current pipeline drops these silently or shows empty boxes.
// This module's job is to RECOGNIZE them up-front so the QML emitter
// can:
//
//   (1) Render a Qt-native placeholder card explaining what was here
//       ("[Chart.js sales data — tap to render]")
//   (2) Optionally launch external rendering on demand (chromium
//       headless screenshot of just that fragment) — deferred for
//       future session
//   (3) Use pattern recognition for COMMON cases (Chart.js with a
//       JSON data attribute → re-render as native Qt chart) —
//       multi-session work
//
// This module ships only step (1): the detection + placeholder QML
// generation. (2) and (3) are concrete plans documented in TODO.
//
// Design goal aligned with project philosophy: "leverage Qt as the
// champion of UI over web UIs." Where we CAN detect intent
// statically, we do; where we can't, we surface a clear card the
// user can act on rather than a broken-looking blank space.

use scraper::{Html, Selector};
use std::sync::OnceLock;

/// Kind of JS-rendered island. Drives the placeholder emitted by
/// the QML generator and informs which external renderer (if any)
/// should be invoked when the user taps "render externally".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IslandKind {
    /// `<canvas>` — usually Chart.js or similar 2D rendering. We
    /// can't reproduce without running the JS, but we DO try to
    /// extract the data attribute (in detect_data_signals).
    Canvas,
    /// `<svg>` with significant child structure that's likely
    /// JS-driven (no inline path data, but classes suggesting a
    /// chart library).
    JsDrivenSvg,
    /// `<video>` element. We surface controls + the poster image
    /// rather than playing it (no Qt-native HTML5 video pipeline).
    Video,
    /// `<iframe>` embedding external content. Most ad iframes are
    /// already filtered upstream by ad_filter; what reaches us
    /// here is content embeds (YouTube, Twitter, Codepen).
    Iframe,
    /// `<div>` whose class names match a known chart-library pattern
    /// (chart-, viz-, plot-, dashboard-) — likely JS-populated even
    /// though it's a plain div in the parsed HTML.
    ChartContainer,
}

/// One detected island.
#[derive(Debug, Clone)]
pub struct Island {
    pub kind: IslandKind,
    /// Tag name (canvas / video / iframe / svg / div).
    pub tag: String,
    /// Title / aria-label / alt — best-effort label for the placeholder.
    pub label: Option<String>,
    /// Width and height attributes if specified, in CSS pixels.
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// For iframes: the src URL (so we can link out / launch the embed
    /// in an external browser). For other kinds: None.
    pub src: Option<String>,
    /// Hint about rendering complexity: was there a `data-*` attribute
    /// or a class name indicating a known chart library? Helps future
    /// (C) work — when we add Chart.js native re-rendering, these
    /// hints tell us which library to mimic.
    pub data_hints: Vec<String>,
    /// A stable identifier for this island within the page — used by
    /// the QML side to refer back when the user taps "render externally"
    /// or "launch in browser". Computed from (tag, position-in-document).
    pub id: String,
}

/// Detect all JS-rendered islands in the parsed HTML.
///
/// Returns them in document order. The QML emitter inserts placeholder
/// blocks at each position where we find one.
pub fn detect_islands(html: &str) -> Vec<Island> {
    let doc = Html::parse_document(html);
    let mut out = Vec::new();
    let mut next_id: u32 = 0;
    let mut new_id = |kind: &str| -> String {
        next_id += 1;
        format!("isl-{}-{}", kind, next_id)
    };

    // ─── canvas ────────────────────────────────────────────────────
    let sel = canvas_sel();
    for el in doc.select(sel) {
        let v = el.value();
        out.push(Island {
            kind: IslandKind::Canvas,
            tag: "canvas".into(),
            label: pick_label(&el),
            width: parse_dim(v.attr("width")),
            height: parse_dim(v.attr("height")),
            src: None,
            data_hints: collect_data_hints(&el),
            id: new_id("canvas"),
        });
    }

    // ─── video ─────────────────────────────────────────────────────
    let sel = video_sel();
    for el in doc.select(sel) {
        let v = el.value();
        // Pull poster src for the placeholder image; otherwise label.
        let src = v.attr("poster")
            .or_else(|| v.attr("src"))
            .map(|s| s.to_string());
        out.push(Island {
            kind: IslandKind::Video,
            tag: "video".into(),
            label: pick_label(&el),
            width: parse_dim(v.attr("width")),
            height: parse_dim(v.attr("height")),
            src,
            data_hints: collect_data_hints(&el),
            id: new_id("video"),
        });
    }

    // ─── iframe ────────────────────────────────────────────────────
    let sel = iframe_sel();
    for el in doc.select(sel) {
        let v = el.value();
        let src = v.attr("src").map(|s| s.to_string());
        // Skip iframes whose src looks like an ad / tracker — these
        // are usually already filtered by ad_filter, but we double-
        // check here. The provider gate prevents tag soup ad
        // networks from getting "render externally" affordances.
        if let Some(ref s) = src {
            if looks_like_ad_iframe(s) { continue; }
        }
        out.push(Island {
            kind: IslandKind::Iframe,
            tag: "iframe".into(),
            label: pick_label(&el),
            width: parse_dim(v.attr("width")),
            height: parse_dim(v.attr("height")),
            src,
            data_hints: collect_data_hints(&el),
            id: new_id("iframe"),
        });
    }

    // ─── svg with JS-library classes ──────────────────────────────
    // Only flag as island if the SVG looks JS-driven. Static SVGs
    // (logos, icons) render fine via the existing pipeline. JS-driven
    // ones have classes like `chartjs-`, `d3-`, `apexcharts-`, etc.
    //
    // We DON'T strip these — the QML emitter will still try to render
    // the static SVG content. We just ALSO surface a card that says
    // "this is interactive in the original page; tap to view live."
    let sel = svg_sel();
    for el in doc.select(sel) {
        let class_attr = el.value().attr("class").unwrap_or("");
        if !looks_js_driven(class_attr) { continue; }
        out.push(Island {
            kind: IslandKind::JsDrivenSvg,
            tag: "svg".into(),
            label: pick_label(&el),
            width: parse_dim(el.value().attr("width")),
            height: parse_dim(el.value().attr("height")),
            src: None,
            data_hints: collect_data_hints(&el),
            id: new_id("svg"),
        });
    }

    // ─── chart-container divs ─────────────────────────────────────
    // Catch divs that LOOK like chart containers via class name.
    // These are what JS frameworks write into; without their JS
    // running, they appear in the parsed DOM as plain empty divs.
    let sel = div_sel();
    for el in doc.select(sel) {
        let class_attr = el.value().attr("class").unwrap_or("");
        let id_attr = el.value().attr("id").unwrap_or("");
        // Tight criteria: class OR id contains a known chart-library
        // marker AND the div has minimal text content (otherwise it's
        // likely a regular content section, not a chart slot).
        if !is_chart_container(class_attr, id_attr) { continue; }
        let text_len: usize = el.text().map(|t| t.len()).sum();
        // Allow up to ~120 chars of text — chart titles + axis labels
        // sometimes get rendered as fallback content. More than that
        // and it's probably actual content.
        if text_len > 120 { continue; }
        out.push(Island {
            kind: IslandKind::ChartContainer,
            tag: "div".into(),
            label: pick_label(&el),
            width: parse_dim(el.value().attr("width")),
            height: parse_dim(el.value().attr("height")),
            src: None,
            data_hints: collect_data_hints(&el),
            id: new_id("chartdiv"),
        });
    }

    out
}

/// Render a list of detected islands as a QML fragment that the
/// generated-page emitter can splice into the document. The fragment
/// is a self-contained Column — no external dependencies.
///
/// Each placeholder card shows:
///   - An icon based on the IslandKind
///   - A short label ("Chart" / "Video" / "Embedded page")
///   - The detected library hint if any
///   - A "Open in browser" / "Render externally" button that the
///     user can tap. The action signal is emitted via a
///     `_islandAction(id, action)` JS hook on the QML root.
pub fn render_islands_qml(islands: &[Island]) -> String {
    if islands.is_empty() { return String::new(); }
    let mut out = String::with_capacity(islands.len() * 256);
    out.push_str("ColumnLayout { Layout.fillWidth: true; spacing: 6\n");
    for is in islands {
        let icon = match is.kind {
            IslandKind::Canvas         => "\u{1F4CA}",  // bar chart
            IslandKind::JsDrivenSvg    => "\u{1F4C8}",  // chart line
            IslandKind::Video          => "\u{25B6}",   // play
            IslandKind::Iframe         => "\u{1F310}",  // globe
            IslandKind::ChartContainer => "\u{1F4CA}",
        };
        let kind_name = match is.kind {
            IslandKind::Canvas         => "Chart (canvas)",
            IslandKind::JsDrivenSvg    => "Interactive graphic",
            IslandKind::Video          => "Video",
            IslandKind::Iframe         => "Embedded page",
            IslandKind::ChartContainer => "Chart placeholder",
        };
        let label = is.label.as_deref().unwrap_or(kind_name);
        // Library hint — first non-empty data_hint.
        let library = is.data_hints.iter().next().cloned().unwrap_or_default();
        // Approximate height: half the original (or a min of 80) so
        // the placeholder doesn't dominate the page when the original
        // was tall.
        let h = is.height.unwrap_or(120).max(80).min(240) / 2 + 60;
        let src_attr = is.src.as_deref().unwrap_or("");
        out.push_str(&format!(
            "Rectangle {{ Layout.fillWidth: true; Layout.preferredHeight: {h}; \
             radius: 8; color: Qt.rgba(0.2, 0.4, 0.6, 0.12); \
             border.color: Qt.rgba(0.4, 0.6, 0.8, 0.35); border.width: 1\n\
             RowLayout {{ anchors.fill: parent; anchors.margins: 10; spacing: 10\n\
             Text {{ text: \"{icon}\"; font.pixelSize: 22; \
             Layout.alignment: Qt.AlignVCenter }}\n\
             ColumnLayout {{ Layout.fillWidth: true; spacing: 2\n\
             Text {{ text: \"{label_esc}\"; color: th.tc; font.pixelSize: 12; \
             font.bold: true; elide: Text.ElideRight; Layout.fillWidth: true }}\n\
             Text {{ text: \"{kind_name}{lib_suffix}\"; color: th.tm; \
             font.pixelSize: 10; elide: Text.ElideRight; Layout.fillWidth: true }}\n\
             }}\n\
             Rectangle {{ Layout.preferredWidth: 88; Layout.preferredHeight: 28; \
             radius: 14; color: _act{idx}Ma.containsMouse ? Qt.lighter(th.ac, 1.2) : th.ac\n\
             Text {{ anchors.centerIn: parent; text: \"{btn_label}\"; \
             color: \"#fff\"; font.pixelSize: 10 }}\n\
             MouseArea {{ id: _act{idx}Ma; anchors.fill: parent; hoverEnabled: true; \
             cursorShape: Qt.PointingHandCursor; \
             onClicked: {{ if (typeof _islandAction === \"function\") \
             _islandAction(\"{id}\", \"{action_kind}\", \"{src_attr}\") }} }} }}\n\
             }} }}\n",
            h = h,
            icon = icon,
            label_esc = qml_escape(label),
            kind_name = kml_escape_static(kind_name),
            lib_suffix = if library.is_empty() {
                String::new()
            } else {
                format!(" \u{00B7} {}", qml_escape(&library))
            },
            idx = sanitize_id(&is.id),
            btn_label = match is.kind {
                IslandKind::Iframe         => "Open",
                IslandKind::Video          => "Play",
                _                          => "View",
            },
            id = qml_escape(&is.id),
            action_kind = match is.kind {
                IslandKind::Iframe         => "open_external",
                IslandKind::Video          => "play_video",
                _                          => "render_external",
            },
            src_attr = qml_escape(src_attr),
        ));
    }
    out.push_str("}\n");
    out
}

// ─── Helpers ──────────────────────────────────────────────────────

fn canvas_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    S.get_or_init(|| Selector::parse("canvas").unwrap())
}
fn video_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    S.get_or_init(|| Selector::parse("video").unwrap())
}
fn iframe_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    S.get_or_init(|| Selector::parse("iframe").unwrap())
}
fn svg_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    S.get_or_init(|| Selector::parse("svg").unwrap())
}
fn div_sel() -> &'static Selector {
    static S: OnceLock<Selector> = OnceLock::new();
    S.get_or_init(|| Selector::parse("div").unwrap())
}

/// Best-effort label: aria-label, then title, then alt, then truncated text.
fn pick_label(el: &scraper::ElementRef) -> Option<String> {
    let v = el.value();
    if let Some(s) = v.attr("aria-label") {
        let t = s.trim();
        if !t.is_empty() { return Some(t.to_string()); }
    }
    if let Some(s) = v.attr("title") {
        let t = s.trim();
        if !t.is_empty() { return Some(t.to_string()); }
    }
    if let Some(s) = v.attr("alt") {
        let t = s.trim();
        if !t.is_empty() { return Some(t.to_string()); }
    }
    // Fallback: text content, capped at 80 chars.
    let text: String = el.text().collect::<Vec<_>>().join(" ");
    let trimmed = text.trim();
    if trimmed.is_empty() { return None; }
    let mut out: String = trimmed.chars().take(80).collect();
    if trimmed.chars().count() > 80 { out.push('…'); }
    Some(out)
}

fn parse_dim(s: Option<&str>) -> Option<u32> {
    let s = s?;
    // Handle "300", "300px", "300.5", strip non-digit suffixes.
    let mut end = 0usize;
    let bytes = s.as_bytes();
    while end < bytes.len() && bytes[end].is_ascii_digit() { end += 1; }
    if end == 0 { return None; }
    s[..end].parse::<u32>().ok()
}

/// Pick out attributes that hint at which JS library was meant to
/// render this island. Useful for future native-mimic work.
fn collect_data_hints(el: &scraper::ElementRef) -> Vec<String> {
    let mut out = Vec::new();
    let v = el.value();
    let class_attr = v.attr("class").unwrap_or("");
    for marker in &[
        "chartjs", "chart-js", "apexcharts", "highcharts",
        "plotly", "d3", "amcharts", "chartist", "echarts",
        "datatables", "leaflet", "mapbox", "openlayers",
        "monaco", "codemirror", "ace_editor",
    ] {
        if class_attr.contains(marker) {
            out.push((*marker).to_string());
        }
    }
    // data- attributes that look semantic ("data-chart-type", "data-src",
    // "data-config") get dumped as hints. These are how chart libraries
    // typically wire up declarative configs.
    for (name, _value) in v.attrs() {
        if name.starts_with("data-") && name.len() > 5 {
            // Just record the key — values can be huge JSON blobs we
            // don't want in our hint list.
            let stripped = &name[5..];
            if matches!(stripped, "chart" | "chart-type" | "config" | "src"
                | "type" | "library" | "format")
            {
                out.push(format!("data-{}", stripped));
            }
        }
    }
    out
}

fn looks_js_driven(class_attr: &str) -> bool {
    let lower = class_attr.to_lowercase();
    for marker in &[
        "chartjs", "chart-js", "apexcharts", "highcharts",
        "plotly", "d3-", "amcharts", "echarts",
    ] {
        if lower.contains(marker) { return true; }
    }
    false
}

fn is_chart_container(class_attr: &str, id_attr: &str) -> bool {
    let combined = format!("{} {}", class_attr, id_attr).to_lowercase();
    // Need MORE than just "chart" alone — plenty of legitimate divs
    // contain that word ("uncharted territories" article class).
    // Look for chart-library specific markers OR explicit chart-N
    // / viz-N / plot-N patterns.
    for marker in &[
        "chartjs", "chart-js", "apexcharts", "highcharts",
        "plotly", "d3-", "amcharts", "chartist", "echarts",
        "chart-container", "chart-wrapper", "chart-canvas",
        "viz-container", "plot-container", "graph-container",
        "dashboard-widget",
    ] {
        if combined.contains(marker) { return true; }
    }
    false
}

fn looks_like_ad_iframe(src: &str) -> bool {
    let lower = src.to_lowercase();
    for marker in &[
        "doubleclick.net", "googlesyndication", "googletagmanager",
        "ad.doubleclick", "adsystem.amazon", "adservice.google",
        "/ads/", "/ad-", "scorecardresearch", "outbrain.com",
    ] {
        if lower.contains(marker) { return true; }
    }
    false
}

/// Escape a string for embedding in a QML literal between double quotes.
/// We escape backslash, double-quote, and newline — that's all that
/// breaks valid QML string literals.
fn qml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"'  => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out
}

/// For static literals we control — same as qml_escape but accepts &str
/// directly (avoids allocating a copy when the input is known clean).
fn kml_escape_static(s: &'static str) -> &'static str {
    // Can't actually mutate a static at runtime, but our static labels
    // never contain unsafe characters, so this is just a marker.
    s
}

/// Strip non-alphanumeric chars from an island id so it can be used
/// as part of a QML id (id-N-canvas → idNcanvas). QML ids must be
/// valid JS identifiers.
fn sanitize_id(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_canvas() {
        let html = r#"<html><body><canvas id="c1" width="300" height="200"></canvas></body></html>"#;
        let islands = detect_islands(html);
        assert_eq!(islands.len(), 1);
        assert_eq!(islands[0].kind, IslandKind::Canvas);
        assert_eq!(islands[0].width, Some(300));
        assert_eq!(islands[0].height, Some(200));
    }

    #[test]
    fn detects_video_with_poster() {
        let html = r#"<html><body><video poster="/p.jpg" src="/v.mp4" width="640"></video></body></html>"#;
        let islands = detect_islands(html);
        assert_eq!(islands.len(), 1);
        assert_eq!(islands[0].kind, IslandKind::Video);
        assert_eq!(islands[0].src.as_deref(), Some("/p.jpg"));
    }

    #[test]
    fn detects_iframe_skipping_ads() {
        let html = r#"<html><body>
            <iframe src="https://www.youtube.com/embed/abc"></iframe>
            <iframe src="https://googletagmanager.com/foo"></iframe>
        </body></html>"#;
        let islands = detect_islands(html);
        // Only YouTube should remain.
        assert_eq!(islands.len(), 1);
        assert_eq!(islands[0].kind, IslandKind::Iframe);
    }

    #[test]
    fn skips_static_svg() {
        // Plain SVG without JS-library classes — should NOT be flagged.
        let html = r#"<html><body>
            <svg viewBox="0 0 100 100"><circle cx="50" cy="50" r="40"/></svg>
        </body></html>"#;
        let islands = detect_islands(html);
        assert_eq!(islands.iter().filter(|i| i.kind == IslandKind::JsDrivenSvg).count(), 0);
    }

    #[test]
    fn flags_chartjs_class_div() {
        let html = r#"<html><body>
            <div class="chartjs-render-monitor"></div>
        </body></html>"#;
        let islands = detect_islands(html);
        assert_eq!(islands.len(), 1);
        assert_eq!(islands[0].kind, IslandKind::ChartContainer);
        assert!(islands[0].data_hints.contains(&"chartjs".to_string()));
    }

    #[test]
    fn skips_chart_word_in_article_class() {
        // "uncharted" contains "chart" but should NOT be detected.
        let html = r#"<html><body>
            <div class="uncharted-region">Long article text about exploration</div>
        </body></html>"#;
        let islands = detect_islands(html);
        assert_eq!(islands.len(), 0);
    }

    #[test]
    fn render_qml_smoke() {
        let islands = vec![
            Island {
                kind: IslandKind::Canvas,
                tag: "canvas".into(),
                label: Some("Sales chart".into()),
                width: Some(400), height: Some(300),
                src: None,
                data_hints: vec!["chartjs".into()],
                id: "isl-canvas-1".into(),
            },
        ];
        let qml = render_islands_qml(&islands);
        assert!(qml.contains("ColumnLayout"));
        assert!(qml.contains("Sales chart"));
        assert!(qml.contains("chartjs"));
    }
}
