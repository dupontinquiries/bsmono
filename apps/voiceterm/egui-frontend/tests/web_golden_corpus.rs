//! Conformance against the QML engine's own golden corpus.
//!
//! `tests/golden/fixtures/*.html` are the inputs web2qml was built against, and
//! `expected/*.html.qml` are its outputs. The QML is NOT the target here — this
//! renderer emits a node tree, not QML source, so a byte comparison would be
//! comparing against the very representation this replaces. What the corpus is
//! good for is the inputs: whatever the Qt engine had to survive, this one does
//! too.
use uikit_web::doc::{parse_page, Kind, FormState, submit};

fn fixture(name: &str) -> String {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/golden/fixtures/");
    std::fs::read_to_string(format!("{p}{name}")).expect("fixture on disk")
}

#[test]
fn every_fixture_parses_without_panicking_and_yields_something() {
    // The corpus deliberately includes malformed.html and edge_cases.html.
    for name in ["article.html", "bg_image.html", "edge_cases.html", "forms.html",
                 "inline_styles.html", "malformed.html", "media.html",
                 "nested_deep.html", "sidebar_nav.html", "tracking.html"] {
        let page = parse_page(&fixture(name));
        let mut nodes = 0;
        page.root.walk(&mut |_| nodes += 1);
        assert!(nodes > 1, "{name} produced an empty tree");
    }
}

#[test]
fn the_forms_fixture_round_trips_a_search_the_way_qml_did() {
    // QML gave this field objectName "_field_0" and read it back at submit
    // time. Here the identity is the node id and the value lives in FormState.
    let page = parse_page(&fixture("forms.html"));
    let mut form = None;
    page.root.walk(&mut |n| if matches!(n.kind, Kind::Form { .. }) { form = Some(n.id) });
    let form = form.expect("forms.html has a form");

    let mut q = None;
    page.root.walk(&mut |n| if let Kind::Field { name, .. } = &n.kind {
        if name == "q" { q = Some(n.id); }
    });
    let mut st = FormState::default();
    st.set(q.expect("the q field"), "hello world");
    let sub = submit(&page, &st, form).expect("submittable");
    assert_eq!(sub.action, "/submit");
    assert_eq!(sub.query, "q=hello+world");
}

#[test]
fn the_malformed_fixture_does_not_truncate() {
    // The whole reason a tolerant parser is not optional.
    let page = parse_page(&fixture("malformed.html"));
    assert!(page.root.text().len() > 20, "content was swallowed by bad markup");
}

#[test]
fn the_media_fixture_keeps_its_media_as_islands_or_images() {
    let page = parse_page(&fixture("media.html"));
    let mut found = 0;
    page.root.walk(&mut |n| if matches!(n.kind, Kind::Image { .. } | Kind::Island { .. }) {
        found += 1;
    });
    assert!(found > 0, "media.html rendered with no media in it");
}

#[test]
fn no_fixture_leaks_script_source_into_the_text() {
    for name in ["article.html", "tracking.html", "edge_cases.html", "nested_deep.html"] {
        let t = parse_page(&fixture(name)).root.text();
        assert!(!t.contains("function "), "{name} rendered JavaScript as prose");
        assert!(!t.contains("var "), "{name} rendered JavaScript as prose");
    }
}
