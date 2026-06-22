// qml_lint.rs
//
// Standalone QML property-existence pre-flight check, invoked from
// build.rs. Catches the specific class of regression that bites only
// at app launch:
//
//     Cannot assign to non-existent property "accent"
//
// Qt5's `qmllint` is syntax-only — it accepts arbitrary property
// assignments on user-defined component types without verifying that
// the property is declared. This module fills that gap.
//
// The module's API is consumed by build.rs (via `include!`) and by
// `cargo test`, but not by the production binary. The `mod qml_lint`
// declaration in main.rs carries the dead_code allow as an outer
// attribute — we can't use an inner attribute here because `include!`
// from build.rs inlines this content into a non-module context.
//
// Approach
// --------
//
// For every `*.qml` file under `qml/` we extract two things:
//
//   1. The set of properties THIS file declares as its own interface:
//      `property color accent: "#C8A04B"`, `property string foo: ""`,
//      `property var bar`, `property alias x: someId.x`, etc.
//      Also signal declarations (`signal moved(real v)`) which permit
//      a matching `onMoved:` handler.
//
//   2. The component instantiations elsewhere in the tree that target
//      THIS file's basename. For each, the top-level property
//      assignments inside the braces. We then check that every
//      assigned name is either declared on this component or in the
//      hardcoded inherited-property set (Item / Component baseline).
//
// The parser is regex-based. QML is not regular but the structure we
// care about — declaration vs. assignment of a simple identifier
// followed by `:` — has a stable shape that regex handles. We
// deliberately avoid trying to build a real QML AST. The check is a
// safety net, not a type system; false positives are caught by the
// `BO_SKIP_QML_LINT=1` escape hatch.
//
// Scope
// -----
//
// What this catches:
//   ✓ MixerPage { accent: ... }              when accent isn't declared
//   ✓ ChannelColumn { tinted_at: ... }       likely typo
//   ✓ <UserComponent> { <unknown-prop>: ... }
//
// What this does NOT catch:
//   ✗ Connections target's signal mismatch (qmllint partially does)
//   ✗ Type-system errors (e.g. assigning int to a color property)
//   ✗ Property names on built-in QtQuick types — we whitelist Item's
//     inheritable set but don't enumerate every type's interface.
//
// The "user-defined" detection is conservative: we only check
// instantiations where the type name matches a basename in the qml/
// directory. Built-in types (Rectangle, Text, ListView, Connections,
// Behavior, ...) are skipped entirely — qmllint already handles them
// where it can.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

/// Properties every QML object inherits from QtQuick's Item base
/// class (or QtObject for non-visual roots). Conservative: includes
/// the common-case attributes and Component-level attached
/// properties. If a check false-positives on something on this list
/// being missing, add it here.
const ITEM_INHERITED: &[&str] = &[
    // Geometry
    "x", "y", "z", "width", "height", "implicitWidth", "implicitHeight",
    "anchors", "scale", "rotation", "transform", "transformOrigin",
    // Visibility / state
    "opacity", "visible", "enabled", "focus", "clip", "smooth",
    "antialiasing", "activeFocus", "activeFocusOnTab",
    // Hierarchy
    "parent", "children", "data", "childrenRect", "resources",
    // State machine
    "state", "states", "transitions",
    // Layer + effects
    "layer",
    // Component attached
    "Component.onCompleted", "Component.onDestruction",
    // Attached properties commonly used
    "Keys", "ListView", "TableView", "GridView", "PathView",
    // Loader-specific (only flagged when target is a Loader, but we
    // include here since user components often expose them via passthrough)
    "active", "source", "sourceComponent", "asynchronous", "item",
    // ListView / Repeater common
    "model", "delegate", "currentIndex", "currentItem",
    // Item-level signal handlers — anything matching `on<Capital>` is
    // separately treated as a signal handler.
    "objectName",
];

/// Things that look like properties but are actually special syntax
/// we should not flag.
const SPECIAL_KEYS: &[&str] = &[
    "id",            // every QML object's identity, not a property
    "default",       // default property override
];

#[derive(Debug, Default)]
struct ComponentDef {
    /// User-declared property names: `property X name: ...` and
    /// `property alias name: ...`.
    properties: BTreeSet<String>,
    /// User-declared signal names (without the `on` prefix): `signal
    /// nameChanged()` → "nameChanged" → permits "onNameChanged".
    signals: BTreeSet<String>,
}

impl ComponentDef {
    /// Set of permitted assignment keys on this component: declared
    /// properties + standard inherited + signal handlers derived
    /// from declared signals.
    fn allowed_assignments(&self) -> BTreeSet<String> {
        let mut s: BTreeSet<String> = self.properties.iter().cloned().collect();
        for inh in ITEM_INHERITED {
            s.insert((*inh).to_string());
        }
        for sp in SPECIAL_KEYS {
            s.insert((*sp).to_string());
        }
        // Auto-derived signal handlers: `onName` for a declared `name`
        // signal, `onPropertyChanged` for a declared `property`.
        for sig in &self.signals {
            s.insert(format!("on{}", capitalize_first(sig)));
        }
        for prop in &self.properties {
            s.insert(format!("on{}Changed", capitalize_first(prop)));
        }
        s
    }
}

fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[derive(Debug)]
pub struct LintIssue {
    pub source_file: String,
    pub component_type: String,
    pub bad_property: String,
    pub line_number: usize,
}

/// Lint the qml/ tree. Returns the list of issues found (empty if
/// clean). Each issue is one bad property assignment.
pub fn check_qml_properties(qml_dir: &Path) -> Result<Vec<LintIssue>, String> {
    // Pass 1: enumerate every component definition.
    let mut components: BTreeMap<String, ComponentDef> = BTreeMap::new();
    let entries = fs::read_dir(qml_dir)
        .map_err(|e| format!("read qml/: {}", e))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("qml") {
            continue;
        }
        let basename = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        // Skip files starting with lowercase — by QML convention,
        // they're entry-point scenes (volume_knobs.qml), not
        // instantiable components.
        let first = basename.chars().next();
        if !first.map(|c| c.is_uppercase()).unwrap_or(false) {
            // Still parse so volume_knobs.qml's instantiations get
            // scanned in pass 2, but don't index it as a component.
            continue;
        }
        let body = fs::read_to_string(&path)
            .map_err(|e| format!("read {}: {}", basename, e))?;
        components.insert(basename, extract_component_def(&body));
    }

    // Pass 2: scan every qml file for instantiations of those components.
    let mut issues: Vec<LintIssue> = Vec::new();
    let entries = fs::read_dir(qml_dir)
        .map_err(|e| format!("read qml/ pass 2: {}", e))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("qml") {
            continue;
        }
        let source_file = path.file_name()
            .and_then(|s| s.to_str()).unwrap_or("?").to_string();
        let body = fs::read_to_string(&path)
            .map_err(|e| format!("read {}: {}", source_file, e))?;
        scan_instantiations(&body, &source_file, &components, &mut issues);
    }
    Ok(issues)
}

/// Extract a component definition from a QML source body. Parses
/// `property X name` declarations and `signal name` declarations.
fn extract_component_def(body: &str) -> ComponentDef {
    let mut def = ComponentDef::default();
    for line in body.lines() {
        let line = strip_inline_comment(line.trim());
        // property declarations: `property color accent: ...` or
        // `property color accent` or `property alias x: foo.x`
        if let Some(rest) = line.strip_prefix("property ") {
            // Two cases: `property TYPE name [: value]` or
            //            `property alias name: target`
            // Skip `readonly property` and `default property` too.
            let (rest, _) = strip_modifier(rest);
            // First token after stripping = type or "alias"
            let mut parts = rest.split_whitespace();
            let _ty = parts.next();
            if let Some(name) = parts.next() {
                let name = name.trim_end_matches(':');
                // Strip any trailing chars (rare: `property var x={}`).
                let name = name.split(':').next().unwrap_or(name);
                if !name.is_empty() {
                    def.properties.insert(name.to_string());
                }
            }
        } else if let Some(rest) = line.strip_prefix("readonly property ") {
            let (rest, _) = strip_modifier(rest);
            let mut parts = rest.split_whitespace();
            let _ty = parts.next();
            if let Some(name) = parts.next() {
                let name = name.trim_end_matches(':')
                    .split(':').next().unwrap_or("");
                if !name.is_empty() {
                    def.properties.insert(name.to_string());
                }
            }
        } else if let Some(rest) = line.strip_prefix("signal ") {
            // `signal moved(real v)` → name = "moved"
            let name: String = rest.chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                def.signals.insert(name);
            }
        }
    }
    def
}

/// Strip readonly / default modifiers if present (already consumed
/// by the caller via prefix matching, but normalize whitespace).
fn strip_modifier(s: &str) -> (&str, bool) {
    (s.trim_start(), false)
}

/// Drop `// ...` from end of line (naive — doesn't handle `//` inside
/// strings, which is fine for property declarations).
fn strip_inline_comment(line: &str) -> &str {
    if let Some(idx) = line.find("//") {
        &line[..idx]
    } else {
        line
    }
}

/// Walk a QML source body looking for instantiations of known
/// user-defined components, then check each property assignment
/// inside.
///
/// A component "instantiation" is any `<UppercaseName>` followed by
/// `{` — possibly with whitespace between. The name can appear:
///   • at the start of a line       (e.g. `Rectangle {`)
///   • after a property assignment  (e.g. `delegate: Widget {`)
///   • after `sourceComponent:`     (the case that bit us)
///   • inline with content          (`MixerPage { accent: x }` all one line)
///
/// Detection works on the body as a flat byte stream, not on a list
/// of lines — the inline case had no later lines to scan, which was
/// our initial false-negative.
fn scan_instantiations(
    body: &str,
    source_file: &str,
    components: &BTreeMap<String, ComponentDef>,
    issues: &mut Vec<LintIssue>,
) {
    let bytes = body.as_bytes();
    let mut i = 0usize;
    let mut in_str = false;
    let mut in_comment = false;
    while i < bytes.len() {
        let c = bytes[i] as char;
        // Track line-comments (// ...).
        if in_comment {
            if c == '\n' { in_comment = false; }
            i += 1;
            continue;
        }
        // Track string literals (best-effort, no escape handling).
        if c == '"' && !in_comment {
            in_str = !in_str;
            i += 1;
            continue;
        }
        if in_str { i += 1; continue; }
        // Line-comment open.
        if c == '/' && i + 1 < bytes.len() && bytes[i + 1] as char == '/' {
            in_comment = true;
            i += 2;
            continue;
        }
        // Look for `[A-Z]\w*\s*\{`.
        if c.is_ascii_uppercase() {
            // Must be at a token boundary — previous char shouldn't be
            // an identifier character (else we'd match the middle of
            // `someIdentifier`).
            let at_boundary = i == 0 || {
                let prev = bytes[i - 1] as char;
                !prev.is_alphanumeric() && prev != '_'
            };
            if at_boundary {
                let start = i;
                while i < bytes.len() {
                    let ch = bytes[i] as char;
                    if ch.is_alphanumeric() || ch == '_' { i += 1; }
                    else { break; }
                }
                let name = &body[start..i];
                // Skip whitespace.
                while i < bytes.len() && (bytes[i] as char).is_whitespace() {
                    i += 1;
                }
                // If followed by `{`, we have an instantiation.
                if i < bytes.len() && bytes[i] as char == '{' {
                    if components.contains_key(name) {
                        // Find matching `}`.
                        let open = i;
                        let close = find_matching_close(body, open);
                        // Compute line number for diagnostics.
                        let line_no = body[..start].lines().count();
                        let inner = &body[open + 1..close];
                        check_assignments_in_span(
                            inner,
                            line_no,
                            source_file,
                            name,
                            components.get(name).unwrap(),
                            issues,
                        );
                        i = close + 1;
                        continue;
                    }
                }
                // Not an instantiation — keep scanning from current i.
                continue;
            }
        }
        i += 1;
    }
}

/// Given the byte offset of an opening `{`, walk forward in `body`
/// counting brace nesting until the matching close. Returns the
/// byte offset of the close `}`. If unmatched, returns body.len().
fn find_matching_close(body: &str, open: usize) -> usize {
    let bytes = body.as_bytes();
    let mut depth: i32 = 0;
    let mut i = open;
    let mut in_str = false;
    let mut in_comment = false;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if in_comment {
            if c == '\n' { in_comment = false; }
            i += 1;
            continue;
        }
        if c == '"' {
            in_str = !in_str;
            i += 1;
            continue;
        }
        if in_str { i += 1; continue; }
        if c == '/' && i + 1 < bytes.len() && bytes[i + 1] as char == '/' {
            in_comment = true;
            i += 2;
            continue;
        }
        if c == '{' { depth += 1; }
        else if c == '}' {
            depth -= 1;
            if depth == 0 { return i; }
        }
        i += 1;
    }
    body.len().saturating_sub(1)
}

/// Walk the inner span of a component instantiation and check every
/// top-level (depth-0 within the span) `name:` assignment.
fn check_assignments_in_span(
    inner: &str,
    component_line_no: usize,
    source_file: &str,
    component_type: &str,
    def: &ComponentDef,
    issues: &mut Vec<LintIssue>,
) {
    let allowed = def.allowed_assignments();
    let bytes = inner.as_bytes();
    let mut i = 0usize;
    let mut depth: i32 = 0;
    let mut in_str = false;
    let mut in_comment = false;
    let mut at_line_start = true;
    let mut line_offset = 0usize;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if in_comment {
            if c == '\n' { in_comment = false; at_line_start = true; line_offset += 1; }
            i += 1;
            continue;
        }
        if c == '\n' {
            at_line_start = true;
            line_offset += 1;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '"' {
            in_str = !in_str;
            i += 1;
            at_line_start = false;
            continue;
        }
        if in_str { i += 1; continue; }
        if c == '/' && i + 1 < bytes.len() && bytes[i + 1] as char == '/' {
            in_comment = true;
            i += 2;
            continue;
        }
        if c == '{' { depth += 1; i += 1; at_line_start = false; continue; }
        if c == '}' { depth -= 1; i += 1; at_line_start = false; continue; }

        // At depth 0 + at a position where a new identifier would
        // start: scan an identifier and check for `:` follower.
        if depth == 0 && at_line_start
           && (c.is_ascii_alphabetic() || c == '_')
        {
            let start = i;
            while i < bytes.len() {
                let ch = bytes[i] as char;
                if ch.is_alphanumeric() || ch == '_' || ch == '.' { i += 1; }
                else { break; }
            }
            let name = &inner[start..i];
            // Skip whitespace.
            let mut j = i;
            while j < bytes.len() && (bytes[j] as char).is_whitespace() {
                if bytes[j] as char == '\n' {
                    // Newline before colon means this wasn't a
                    // property assignment.
                    break;
                }
                j += 1;
            }
            if j < bytes.len() && bytes[j] as char == ':' {
                // It's `name:` — verify against allowed.
                let is_decl = matches!(name,
                    "property" | "readonly" | "default" | "signal" | "function"
                );
                if !is_decl
                   && !allowed.contains(name)
                   && !name.contains('.')
                   && !name.starts_with("on")
                {
                    issues.push(LintIssue {
                        source_file: source_file.to_string(),
                        component_type: component_type.to_string(),
                        bad_property: name.to_string(),
                        line_number: component_line_no + line_offset,
                    });
                }
                // Advance past the `:` so we don't re-scan.
                i = j + 1;
                at_line_start = false;
                continue;
            }
            // Not a property assignment — skip this token.
            at_line_start = false;
            continue;
        }
        at_line_start = false;
        i += 1;
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_declared_properties() {
        let body = r##"
import QtQuick 2.15

Item {
    property color accent: "#C8A04B"
    property string foo: ""
    readonly property int gap: 12
    property alias label: txt.text
    signal sliderMoved(real v)
}
"##;
        let def = extract_component_def(body);
        assert!(def.properties.contains("accent"));
        assert!(def.properties.contains("foo"));
        assert!(def.properties.contains("gap"));
        assert!(def.properties.contains("label"));
        assert!(def.signals.contains("sliderMoved"));
    }

    #[test]
    fn instantiation_with_undeclared_prop_flagged() {
        // Set up a synthetic two-file scenario in memory.
        let mut comps = BTreeMap::new();
        let mut def = ComponentDef::default();
        def.properties.insert("good".to_string());
        comps.insert("Widget".to_string(), def);

        let body = r#"
Item {
    Widget {
        good: 1
        bogus: 2
        anchors.fill: parent
    }
}
"#;
        let mut issues = Vec::new();
        scan_instantiations(body, "test.qml", &comps, &mut issues);
        let bogus_issues: Vec<_> = issues.iter()
            .filter(|i| i.bad_property == "bogus")
            .collect();
        assert_eq!(bogus_issues.len(), 1,
            "expected one bogus issue, got {:?}", issues);
        let good_issues: Vec<_> = issues.iter()
            .filter(|i| i.bad_property == "good")
            .collect();
        assert!(good_issues.is_empty());
    }

    #[test]
    fn anchors_fill_is_allowed() {
        let mut comps = BTreeMap::new();
        let def = ComponentDef::default();
        comps.insert("Widget".to_string(), def);
        let body = "Widget { anchors.fill: parent }";
        let mut issues = Vec::new();
        scan_instantiations(body, "t.qml", &comps, &mut issues);
        assert!(issues.is_empty(), "{:?}", issues);
    }

    #[test]
    fn signal_handlers_resolved_via_declared_signals() {
        let mut comps = BTreeMap::new();
        let mut def = ComponentDef::default();
        def.signals.insert("clicked".to_string());
        comps.insert("Button".to_string(), def);
        let body = "Button { onClicked: console.log(1) }";
        let mut issues = Vec::new();
        scan_instantiations(body, "t.qml", &comps, &mut issues);
        assert!(issues.is_empty(),
            "onClicked must resolve via clicked signal: {:?}", issues);
    }

    #[test]
    fn the_regression_case_caught() {
        // Reproduce the exact bug that brought us here:
        //   sourceComponent: MixerPage { accent: window.accent }
        // when MixerPage no longer declares an `accent` property.
        let mut comps = BTreeMap::new();
        let def = ComponentDef::default();   // empty — no `accent`
        comps.insert("MixerPage".to_string(), def);
        let body = r#"
Item {
    sourceComponent: MixerPage {
        accent: window.accent
    }
}
"#;
        let mut issues = Vec::new();
        scan_instantiations(body, "volume_knobs.qml", &comps, &mut issues);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].bad_property, "accent");
        assert_eq!(issues[0].component_type, "MixerPage");
    }

    #[test]
    fn nested_objects_dont_falsely_flag_outer_props() {
        // Property assignments at INNER depth shouldn't trip the
        // outer's allowlist check.
        let mut comps = BTreeMap::new();
        let mut def = ComponentDef::default();
        def.properties.insert("title".to_string());
        comps.insert("Card".to_string(), def);
        let body = r#"
Item {
    Card {
        title: "ok"
        Rectangle {
            // `color` isn't on Card, but it's on Rectangle — must
            // not be flagged.
            color: "red"
            width: 10
        }
    }
}
"#;
        let mut issues = Vec::new();
        scan_instantiations(body, "t.qml", &comps, &mut issues);
        assert!(issues.is_empty(),
            "inner Rectangle's color must not be flagged: {:?}", issues);
    }

    #[test]
    fn property_changed_handlers_auto_derive() {
        // `onAccentChanged` should be allowed when `accent` is
        // declared (every property gets a synthetic Changed signal).
        let mut comps = BTreeMap::new();
        let mut def = ComponentDef::default();
        def.properties.insert("accent".to_string());
        comps.insert("Card".to_string(), def);
        let body = r#"
Item {
    Card {
        accent: "red"
        onAccentChanged: console.log("changed")
    }
}
"#;
        let mut issues = Vec::new();
        scan_instantiations(body, "t.qml", &comps, &mut issues);
        assert!(issues.is_empty(),
            "onAccentChanged should be auto-derived: {:?}", issues);
    }

    #[test]
    fn builtin_types_skipped() {
        // Rectangle isn't a user-defined component — assignments on
        // it (color, border.width, etc.) shouldn't be flagged here.
        // (Those are qmllint's job where it can.)
        let comps = BTreeMap::new();   // empty — no user components
        let body = r#"
Item {
    Rectangle {
        color: "red"
        bogus_built_in: 1     // we don't catch this — Rectangle is built-in
    }
}
"#;
        let mut issues = Vec::new();
        scan_instantiations(body, "t.qml", &comps, &mut issues);
        assert!(issues.is_empty(),
            "built-in type assignments are out of scope: {:?}", issues);
    }
}
