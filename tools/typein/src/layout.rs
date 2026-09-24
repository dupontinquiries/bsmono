//! Working out which keyboard layout the compositor is mapping with.
//!
//! In order, because each source is more authoritative than the next:
//!
//! 1. `TYPEIN_LAYOUT` / `TYPEIN_VARIANT` — an explicit override.
//! 2. `DOTOOL_XKB_LAYOUT` / `DOTOOL_XKB_VARIANT` — already set by the caller.
//! 3. cosmic-comp's `xkb_config`. This is the authority when cosmic is the
//!    compositor, because it is the thing performing the mapping.
//! 4. `XKB_DEFAULT_LAYOUT` / `XKB_DEFAULT_VARIANT`.
//! 5. `localectl status`, the X11 rows.
//! 6. `/etc/default/keyboard`.
//! 7. `us`, admitted as a guess.
//!
//! Parsing is pure: callers read the files and pass the text in.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub layout: String,
    pub variant: String,
    /// Where it came from, for `--check` to show.
    pub source: String,
}

/// A layout string may list several ("us,dvorak"); dotool simulates one, and
/// the first is where a compositor starts.
pub fn first(s: &str) -> String {
    s.split(',').next().unwrap_or("").trim().to_string()
}

/// cosmic-comp's `xkb_config`, a RON struct.
pub fn from_cosmic_ron(text: &str) -> Option<Layout> {
    let field = |name: &str| -> Option<String> {
        let at = text.find(&format!("{name}:"))?;
        let rest = &text[at + name.len() + 1..];
        let open = rest.find('"')?;
        let close = rest[open + 1..].find('"')?;
        Some(rest[open + 1..open + 1 + close].to_string())
    };
    let layout = field("layout").filter(|l| !l.trim().is_empty())?;
    Some(Layout { layout: first(&layout), variant: first(&field("variant").unwrap_or_default()), source: "cosmic-comp xkb_config".into() })
}

/// The X11 rows of `localectl status`.
pub fn from_localectl(text: &str) -> Option<Layout> {
    let row = |name: &str| text.lines().find_map(|l| l.trim().strip_prefix(name).map(|v| v.trim().to_string()));
    let layout = row("X11 Layout:").filter(|l| !l.is_empty())?;
    Some(Layout { layout: first(&layout), variant: first(&row("X11 Variant:").unwrap_or_default()), source: "localectl".into() })
}

/// `/etc/default/keyboard`, shell-style assignments.
pub fn from_etc_default_keyboard(text: &str) -> Option<Layout> {
    let var = |name: &str| {
        text.lines().find_map(|l| {
            let l = l.trim();
            let v = l.strip_prefix(name)?.strip_prefix('=')?;
            Some(v.trim().trim_matches('"').to_string())
        })
    };
    let layout = var("XKBLAYOUT").filter(|l| !l.is_empty())?;
    Some(Layout { layout: first(&layout), variant: first(&var("XKBVARIANT").unwrap_or_default()), source: "/etc/default/keyboard".into() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosmics_own_config_is_read() {
        let ron = r#"(
    rules: "",
    model: "",
    layout: "us",
    variant: "dvorak",
    options: None,
)"#;
        let l = from_cosmic_ron(ron).unwrap();
        assert_eq!(l.layout, "us");
        assert_eq!(l.variant, "dvorak", "the variant is what makes warp come out as ,apl");
        // An empty layout is not an answer; fall through to the next source.
        assert!(from_cosmic_ron(r#"( layout: "", variant: "" )"#).is_none());
        assert!(from_cosmic_ron("not ron at all").is_none());
    }

    #[test]
    fn several_layouts_means_the_first_one() {
        let l = from_cosmic_ron(r#"( layout: "us,de", variant: "dvorak," )"#).unwrap();
        assert_eq!((l.layout.as_str(), l.variant.as_str()), ("us", "dvorak"));
        assert_eq!(first("de,us"), "de");
        assert_eq!(first(""), "");
    }

    #[test]
    fn localectl_and_etc_default_are_parsed() {
        let s = "System Locale: LANG=en_US.UTF-8\n   X11 Layout: ch\n    X11 Model: pc105\n  X11 Variant: de_nodeadkeys\n";
        let l = from_localectl(s).unwrap();
        assert_eq!((l.layout.as_str(), l.variant.as_str()), ("ch", "de_nodeadkeys"));
        assert!(from_localectl("VC Keymap: (unset)").is_none());
        let k = from_etc_default_keyboard("XKBMODEL=\"pc105\"\nXKBLAYOUT=\"fr\"\nXKBVARIANT=\"\"\n").unwrap();
        assert_eq!((k.layout.as_str(), k.variant.as_str()), ("fr", ""));
        assert!(from_etc_default_keyboard("XKBMODEL=\"pc105\"").is_none());
    }
}
