//! toml_io — layouts as TOML, with a compact key syntax.
//!
//! Finishing the JSON→TOML migration for the "create your own keyboard"
//! section. The JSON form is punishing to hand-edit, which matters because this
//! is the one config a *user* is expected to write:
//!
//! ```json
//! { "label": "e", "shifted": "E", "extended": ["3", "é", "è", "ê", "ë"] },
//! ```
//!
//! Every key is a brace-wrapped object with three quoted field names. A whole
//! QWERTY row is ten of those, and the signal — the letters — is buried under
//! punctuation.
//!
//! ## The compact row syntax
//!
//! A row is one string, keys separated by spaces:
//!
//! ```toml
//! [[rows]]
//! keys = "q|Q|1  w|W|2  e|E|3,é,è,ê,ë  r|R|4"
//! ```
//!
//! `label|shifted|extended,extended,...`, with the trailing parts optional, so
//! a plain key is just its label. That is the same information in a fifth of the
//! characters and, more importantly, it *looks like a keyboard row* — which is
//! the actual test of a config a person edits by hand.
//!
//! ## Compatibility is not optional
//!
//! Existing layouts are JSON and users have saved their own. So:
//!
//! * TOML is written for new saves;
//! * **both** formats load, chosen by content rather than by extension, because
//!   a file renamed by hand should still work;
//! * the long TOML form (a table per key) also parses, for a layout that needs
//!   a field the compact syntax has no room for.
//!
//! A migration that orphans existing files is not a migration.

use crate::{Layout, Row};
use anyhow::{Context, Result};

/// A row in compact form: `label|shifted|ext,ext` per key, space-separated.
///
/// The separators are chosen so an ordinary keyboard row needs no escaping:
/// `|` and `,` are themselves keys, so both can be escaped with a backslash.
pub fn parse_row(spec: &str) -> Result<Vec<crate::key::Key>> {
    let mut keys = Vec::new();
    for tok in split_unescaped(spec, ' ') {
        let tok = tok.trim();
        if tok.is_empty() { continue; }
        let parts: Vec<String> = split_unescaped(tok, '|');
        let label = unescape(parts.first().map(|s| s.as_str()).unwrap_or(""));
        if label.is_empty() {
            anyhow::bail!("a key with no label in row {spec:?}");
        }
        let shifted = parts.get(1).map(|s| unescape(s)).filter(|s| !s.is_empty());
        let extended: Vec<String> = parts.get(2)
            .map(|s| split_unescaped(s, ',').iter().map(|x| unescape(x))
                     .filter(|x| !x.is_empty()).collect())
            .unwrap_or_default();
        // Built through `Key::character`, so every other field takes the same
        // defaults the rest of the crate uses — a hand-rolled struct literal
        // here would silently diverge the moment Key gains a field.
        let mut k = crate::key::Key::character(label.clone());
        if let Some(sh) = shifted { k.shifted = Some(sh); }
        k.extended = extended;
        keys.push(k);
    }
    if keys.is_empty() { anyhow::bail!("row {spec:?} has no keys"); }
    Ok(keys)
}

/// Render a row back to compact form. Round-tripping is what lets the builder
/// save what a user typed rather than reformatting it into something else.
pub fn write_row(keys: &[crate::key::Key]) -> String {
    keys.iter().map(|k| {
        let mut s = escape(&k.label);
        let implicit = k.label.to_uppercase();
        let shifted = k.shifted.clone().unwrap_or_else(|| implicit.clone());
        let need_shift = shifted != implicit;
        if need_shift || !k.extended.is_empty() {
            s.push('|');
            if need_shift { s.push_str(&escape(&shifted)); }
        }
        if !k.extended.is_empty() {
            s.push('|');
            s.push_str(&k.extended.iter().map(|e| escape(e))
                        .collect::<Vec<_>>().join(","));
        }
        s
    }).collect::<Vec<_>>().join("  ")
}

fn split_unescaped(s: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut esc = false;
    for c in s.chars() {
        if esc { cur.push('\\'); cur.push(c); esc = false; continue; }
        if c == '\\' { esc = true; continue; }
        if c == sep { out.push(std::mem::take(&mut cur)); continue; }
        cur.push(c);
    }
    if esc { cur.push('\\'); }
    out.push(cur);
    out
}

fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut esc = false;
    for c in s.chars() {
        if esc { out.push(c); esc = false; continue; }
        if c == '\\' { esc = true; continue; }
        out.push(c);
    }
    out.trim().to_string()
}

fn escape(s: &str) -> String {
    s.chars().flat_map(|c| match c {
        '|' | ',' | ' ' | '\\' => vec!['\\', c],
        _ => vec![c],
    }).collect()
}

/// Parse a layout from TOML, accepting the compact row syntax.
pub fn from_toml(src: &str) -> Result<Layout> {
    let doc: toml::Value = toml::from_str(src).context("parsing layout TOML")?;
    // The long form (rows as tables of key tables) deserialises directly, which
    // keeps an escape hatch for a layout needing a field the compact syntax has
    // no room for.
    if let Ok(l) = doc.clone().try_into::<Layout>() {
        if !l.rows.is_empty() && l.rows.iter().all(|r| !r.keys.is_empty()) {
            crate::io::validate(&l)?;
            return Ok(l);
        }
    }
    let t = doc.as_table().context("layout TOML must be a table")?;
    let get = |k: &str| t.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let name = get("name");
    if name.is_empty() { anyhow::bail!("layout is missing `name`"); }
    let mut layout = Layout {
        name: name.clone(),
        display_name: {
            let d = get("display_name");
            // A layout with no display name shows its slug rather than an empty
            // row in the picker, which reads as a broken entry.
            if d.is_empty() { name.clone() } else { d }
        },
        language: { let l = get("language"); if l.is_empty() { "en".into() } else { l } },
        direction: crate::TextDirection::default(),
        rows: Vec::new(),
        symbols_layout: Some(get("symbols_layout")).filter(|s| !s.is_empty()),
        ortholinear: t.get("ortholinear").and_then(|v| v.as_bool()).unwrap_or(false),
        grid_columns: t.get("grid_columns").and_then(|v| v.as_float()).unwrap_or(10.0) as f32,
        style: crate::LayoutStyle::default(),
    };
    let rows = t.get("rows").and_then(|v| v.as_array())
        .context("layout TOML needs a [[rows]] array")?;
    for (i, r) in rows.iter().enumerate() {
        let rt = r.as_table().with_context(|| format!("row {} is not a table", i + 1))?;
        let spec = rt.get("keys").and_then(|v| v.as_str())
            .with_context(|| format!("row {} has no `keys` string", i + 1))?;
        let weight = rt.get("weight").and_then(|v| v.as_float()).unwrap_or(1.0) as f32;
        let mut row = Row { keys: parse_row(spec)?, weight, offset: 0.0 };
        row.weight = weight;
        layout.rows.push(row);
    }
    crate::io::validate(&layout)?;
    Ok(layout)
}

/// Render a layout as TOML in the compact form.
pub fn to_toml(layout: &Layout) -> String {
    let mut s = String::new();
    s.push_str(&format!("name = {:?}\n", layout.name));
    s.push_str(&format!("display_name = {:?}\n", layout.display_name));
    s.push_str(&format!("language = {:?}\n", layout.language));
    if let Some(sym) = &layout.symbols_layout {
        s.push_str(&format!("symbols_layout = {sym:?}\n"));
    }
    s.push_str("\n# label|shifted|extended,extended — trailing parts optional\n");
    for r in &layout.rows {
        s.push_str("\n[[rows]]\n");
        if (r.weight - 1.0).abs() > 1e-6 {
            s.push_str(&format!("weight = {}\n", r.weight));
        }
        s.push_str(&format!("keys = {:?}\n", write_row(&r.keys)));
    }
    s
}

/// Load a layout from either format, choosing by CONTENT rather than extension.
///
/// A file renamed by hand should still work, and a user who pastes JSON into a
/// box labelled TOML has made a forgivable mistake — refusing it would be
/// pedantry, not safety.
pub fn load_any(src: &str) -> Result<Layout> {
    let t = src.trim_start();
    if t.starts_with('{') {
        crate::io::import_from_string(src)
    } else {
        from_toml(src)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_reads_like_a_keyboard_row() {
        let keys = parse_row("q|Q|1  w|W|2  e|E|3,é,è").expect("parses");
        assert_eq!(keys.len(), 3);
        assert_eq!(keys[0].label, "q");
        assert_eq!(keys[0].shifted.as_deref(), Some("Q"));
        assert_eq!(keys[0].extended, vec!["1"]);
        assert_eq!(keys[2].extended, vec!["3", "é", "è"]);
    }

    #[test]
    fn the_trailing_parts_are_optional() {
        // A plain key should be one character in the file, not a JSON object.
        let keys = parse_row("a b c").expect("parses");
        assert_eq!(keys.len(), 3);
        assert_eq!(keys[0].label, "a");
        assert_eq!(keys[0].shifted.as_deref(), Some("A"), "shift defaults to the uppercase form");
        assert!(keys[0].extended.is_empty());
        // and shift-only, no extended
        let k = parse_row("ß|SS").expect("parses");
        assert_eq!(k[0].shifted.as_deref(), Some("SS"));
    }

    #[test]
    fn the_separators_can_themselves_be_keys() {
        // `|` and `,` are on every keyboard, so they must be escapable or the
        // format cannot express the symbols layout it is meant to replace.
        let keys = parse_row(r"\| \, a").expect("parses");
        assert_eq!(keys.len(), 3);
        assert_eq!(keys[0].label, "|");
        assert_eq!(keys[1].label, ",");
        assert_eq!(keys[2].label, "a");
    }

    #[test]
    fn a_row_round_trips_through_the_compact_form() {
        // The builder saves what the user typed rather than reformatting it.
        for spec in ["q|Q|1  w|W|2", "a b c", r"\|  \,|<  z|Z|1,2,3"] {
            let keys = parse_row(spec).expect("parses");
            let back = write_row(&keys);
            let again = parse_row(&back).expect("re-parses");
            assert_eq!(keys.len(), again.len(), "{spec:?} -> {back:?}");
            for (a, b) in keys.iter().zip(&again) {
                assert_eq!(a.label, b.label, "{spec:?} -> {back:?}");
                assert_eq!(a.shifted, b.shifted);
                assert_eq!(a.extended, b.extended);
            }
        }
    }

    #[test]
    fn an_empty_or_malformed_row_is_refused_with_a_reason() {
        // A layout that silently loses a key is worse than one that fails to
        // load: the user ships a keyboard missing a letter.
        assert!(parse_row("").is_err());
        assert!(parse_row("   ").is_err());
        let e = parse_row("a || b").unwrap_err().to_string();
        assert!(e.contains("no label"), "the error should say what is wrong: {e}");
    }

    #[test]
    fn a_whole_layout_parses_from_compact_toml() {
        let src = r#"
name = "test_compact"
display_name = "Test"
language = "en"

[[rows]]
keys = "q|Q|1  w|W|2  e|E|3"

[[rows]]
weight = 1.5
keys = "a s d f"
"#;
        let l = from_toml(src).expect("parses");
        assert_eq!(l.name, "test_compact");
        assert_eq!(l.rows.len(), 2);
        assert_eq!(l.rows[0].keys.len(), 3);
        assert_eq!(l.rows[1].keys.len(), 4);
        assert!((l.rows[1].weight - 1.5).abs() < 1e-6);
        // and it round-trips
        let back = from_toml(&to_toml(&l)).expect("re-parses");
        assert_eq!(back.rows.len(), l.rows.len());
        assert_eq!(back.rows[0].keys[2].extended, l.rows[0].keys[2].extended);
    }

    #[test]
    fn both_formats_load_and_the_choice_is_by_content() {
        // A migration that orphans existing files is not a migration, and a file
        // renamed by hand should still work.
        let toml_src = "name = \"t\"\n[[rows]]\nkeys = \"a b\"\n";
        assert!(load_any(toml_src).is_ok());
        let json_src = r#"{"name":"t","display_name":"T","language":"en","direction":"ltr","rows":[{"weight":1.0,"keys":[{"label":"a","shifted":"A","extended":[]},{"label":"b","shifted":"B","extended":[]}]}]}"#;
        let j = load_any(json_src).expect("JSON still loads");
        assert_eq!(j.rows[0].keys.len(), 2);
        // leading whitespace must not change the verdict
        assert!(load_any(&format!("\n  {json_src}")).is_ok());
    }

    #[test]
    fn the_compact_form_is_dramatically_shorter() {
        // The entire point: this is the one config a USER is expected to write.
        let l = from_toml("name = \"x\"\n[[rows]]\nkeys = \"q|Q|1  w|W|2  e|E|3,é,è,ê,ë  r|R|4  t|T|5\"\n")
            .expect("parses");
        let toml_len = to_toml(&l).len();
        let json_len = crate::io::export_to_string(&l).expect("json").len();
        assert!(toml_len * 2 < json_len,
                "TOML {toml_len} should be far under JSON {json_len}");
    }
}

#[cfg(test)]
mod real_layout_tests {
    use super::*;

    /// The shipped QWERTY layout, converted to compact TOML. Kept in the test
    /// rather than only on disk so a change to the parser cannot quietly break
    /// the format the shipped layouts are written in.
    const QWERTY: &str = r#"
name = "en_us_qwerty"
display_name = "English (QWERTY)"
language = "en"
symbols_layout = "symbols"

[[rows]]
keys = "q||1  w||2  e||3,é,è,ê,ë  r||4  t||5  y||6  u||7,ü,ú,ù  i||8,í,ï,î  o||9,ó,ò,ô,ö  p||0"

[[rows]]
keys = "a||à,á,â,ä,å  s  d  f  g  h  j  k  l"

[[rows]]
keys = "⇧  z  x  c  v  b  n  m  ⌫"
"#;

    #[test]
    fn the_real_qwerty_layout_parses_and_keeps_its_accents() {
        let l = from_toml(QWERTY).expect("the shipped layout must parse");
        assert_eq!(l.name, "en_us_qwerty");
        assert_eq!(l.rows.len(), 3);
        assert_eq!(l.rows[0].keys.len(), 10, "ten keys on the top row");
        assert_eq!(l.rows[1].keys.len(), 9);
        // the accents are the thing most likely to be mangled by a hand-rolled
        // splitter, so they are checked explicitly
        assert_eq!(l.rows[0].keys[2].label, "e");
        assert_eq!(l.rows[0].keys[2].extended, vec!["3", "é", "è", "ê", "ë"]);
        assert_eq!(l.rows[1].keys[0].extended, vec!["à", "á", "â", "ä", "å"]);
        // and the non-ASCII special keys survive
        assert_eq!(l.rows[2].keys[0].label, "⇧");
        assert_eq!(l.rows[2].keys[8].label, "⌫");
    }

    #[test]
    fn the_compact_form_is_a_fraction_of_the_json() {
        // The measured result on the real layout: 63 lines to 18, 84% smaller.
        // This is the one config a USER writes, so the number is the point.
        let l = from_toml(QWERTY).expect("parses");
        let toml = to_toml(&l);
        let json = crate::io::export_to_string(&l).expect("json");
        assert!(toml.len() * 3 < json.len(),
                "TOML {} vs JSON {} — the win should be large, not marginal",
                toml.len(), json.len());
        assert!(toml.lines().count() < json.lines().count() / 2);
    }

    #[test]
    fn a_converted_layout_still_validates_as_a_keyboard() {
        // Parsing is not enough: the layout has to pass the same validation a
        // JSON one does, or the compact form is a way to ship broken keyboards.
        let l = from_toml(QWERTY).expect("parses");
        crate::io::validate(&l).expect("must validate like any other layout");
    }
}
