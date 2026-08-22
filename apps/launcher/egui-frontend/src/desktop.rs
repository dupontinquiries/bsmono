//! desktop — freedesktop `.desktop` entries: parse (pure) and scan (fs seam).
//!
//! The parser is a deliberate subset of the spec — the subset a launcher grid
//! needs: `[Desktop Entry]` only, `Type=Application` only, `NoDisplay`/`Hidden`
//! respected, `Exec` field codes stripped, quoting per the spec's simplified
//! shell rules. Localised keys (`Name[de]`) are skipped in favour of the plain
//! key; wiring locale preference in is a later, contained change to
//! [`parse_desktop_entry`] alone.
//!
//! What is intentionally NOT here: icon-theme resolution (that is the view's
//! problem, behind [`AppEntry::icon`] as a plain name), and TryExec probing
//! (a launch-time concern).

use std::path::{Path, PathBuf};

/// One launchable application.
#[derive(Clone, Debug, PartialEq)]
pub struct AppEntry {
    /// Desktop-file id: filename minus `.desktop` (`org.kde.dolphin`). The
    /// stable identity used for dedupe and for the animation board's tile keys.
    pub id: String,
    pub name: String,
    /// `GenericName`, if present — the plasmoid's "Description" display modes.
    pub generic: String,
    /// `Exec` with field codes (`%f` `%u` …) already stripped.
    pub exec: String,
    /// Icon *name* per the icon-theme spec, unresolved.
    pub icon: String,
    /// `Keywords` + `GenericName`, lowercased — extra fuzzy-match surface.
    pub keywords: Vec<String>,
    pub terminal: bool,
}

impl AppEntry {
    /// The Exec line as argv, per the spec's quoting rules (double quotes,
    /// backslash escapes inside them). Empty when Exec was empty.
    pub fn argv(&self) -> Vec<String> {
        split_exec(&self.exec)
    }

    /// Everything fuzzy matching may look at, lowercased.
    pub fn haystacks(&self) -> impl Iterator<Item = String> + '_ {
        std::iter::once(self.name.to_lowercase())
            .chain(std::iter::once(self.id.to_lowercase()))
            .chain(self.keywords.iter().cloned())
    }
}

/// Parse one `.desktop` file body. `None` when the entry should not appear in
/// a launcher: wrong Type, NoDisplay/Hidden, or no usable Name/Exec.
pub fn parse_desktop_entry(id: &str, text: &str) -> Option<AppEntry> {
    let mut in_main = false;
    let (mut name, mut generic, mut exec, mut icon) = (None, None, None, None);
    let mut keywords: Vec<String> = Vec::new();
    let (mut typ, mut nodisplay, mut hidden, mut terminal) = (None, false, false, false);

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_main = section == "Desktop Entry";
            continue;
        }
        if !in_main {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim(), value.trim());
        // Localised variants (Name[fr]) are skipped: plain key wins for now.
        if key.contains('[') {
            continue;
        }
        match key {
            "Type" => typ = Some(value.to_string()),
            "Name" => name = Some(value.to_string()),
            "GenericName" => generic = Some(value.to_string()),
            "Exec" => exec = Some(strip_field_codes(value)),
            "Icon" => icon = Some(value.to_string()),
            "NoDisplay" => nodisplay = value.eq_ignore_ascii_case("true"),
            "Hidden" => hidden = value.eq_ignore_ascii_case("true"),
            "Terminal" => terminal = value.eq_ignore_ascii_case("true"),
            "Keywords" => {
                keywords.extend(value.split(';').filter(|k| !k.is_empty()).map(|k| k.to_lowercase()));
            }
            _ => {}
        }
    }

    if typ.as_deref() != Some("Application") || nodisplay || hidden {
        return None;
    }
    let name = name?;
    let exec = exec?;
    if name.is_empty() || exec.is_empty() {
        return None;
    }
    let generic = generic.unwrap_or_default();
    if !generic.is_empty() {
        keywords.push(generic.to_lowercase());
    }
    Some(AppEntry {
        id: id.to_string(),
        name,
        generic,
        exec,
        icon: icon.unwrap_or_default(),
        keywords,
        terminal,
    })
}

/// Remove Exec field codes. `%%` is a literal percent; every other `%x` token
/// disappears (a launcher passes no file/url arguments).
pub fn strip_field_codes(exec: &str) -> String {
    let mut out = String::with_capacity(exec.len());
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            Some(_) | None => {}
        }
    }
    // Collapse the double spaces stripping "%U" leaves behind.
    let mut collapsed = String::with_capacity(out.len());
    let mut last_space = false;
    for c in out.trim().chars() {
        let space = c == ' ';
        if !(space && last_space) {
            collapsed.push(c);
        }
        last_space = space;
    }
    collapsed
}

/// Spec-rule Exec splitting: whitespace-separated, double quotes group, and
/// inside quotes backslash escapes the next character.
pub fn split_exec(exec: &str) -> Vec<String> {
    let mut argv = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => in_quotes = !in_quotes,
            '\\' if in_quotes => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            c if c.is_whitespace() && !in_quotes => {
                if !cur.is_empty() {
                    argv.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        argv.push(cur);
    }
    argv
}

/// XDG application directories, precedence order: `$XDG_DATA_HOME` first, then
/// each of `$XDG_DATA_DIRS` (defaulting per the basedir spec).
pub fn application_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let data_home = std::env::var("XDG_DATA_HOME")
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(&home).join(".local/share"));
    dirs.push(data_home.join("applications"));
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    for d in data_dirs.split(':').filter(|d| !d.is_empty()) {
        dirs.push(Path::new(d).join("applications"));
    }
    dirs
}

/// Scan directories for `.desktop` files. First occurrence of an id wins, so
/// pass dirs in precedence order (what [`application_dirs`] returns). Result
/// is name-sorted, case-insensitive — the grid's resting order.
pub fn scan(dirs: &[PathBuf]) -> Vec<AppEntry> {
    let mut seen = std::collections::BTreeSet::new();
    let mut apps = Vec::new();
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            let Some(fname) = path.file_name().and_then(|f| f.to_str()) else { continue };
            let Some(id) = fname.strip_suffix(".desktop") else { continue };
            if !seen.insert(id.to_string()) {
                continue; // earlier (higher-precedence) dir already provided it
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            if let Some(app) = parse_desktop_entry(id, &text) {
                apps.push(app);
            }
        }
    }
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    apps
}

/// XDG icon theme roots, precedence order: `$XDG_DATA_HOME/icons` first, then
/// each of `$XDG_DATA_DIRS/icons`, then `/usr/share/pixmaps`'s sibling
/// convention is handled by [`crate::icontheme::find_icon`]'s unthemed
/// fallback (it looks at each root's `pixmaps/` sibling), not listed here.
pub fn icon_theme_dirs() -> Vec<PathBuf> {
    let mut dirs = application_dirs()
        .into_iter()
        .filter_map(|d| d.parent().map(|p| p.join("icons")))
        .collect::<Vec<_>>();
    dirs.dedup();
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIREFOX: &str = "\
[Desktop Entry]
Type=Application
Name=Firefox
GenericName=Web Browser
Exec=firefox %u
Icon=firefox
Keywords=internet;web;
";

    #[test]
    fn parses_a_normal_entry() {
        let app = parse_desktop_entry("firefox", FIREFOX).unwrap();
        assert_eq!(app.name, "Firefox");
        assert_eq!(app.exec, "firefox");
        assert_eq!(app.icon, "firefox");
        assert!(app.keywords.contains(&"web".to_string()));
        assert!(app.keywords.contains(&"web browser".to_string())); // GenericName folded in
        assert!(!app.terminal);
    }

    #[test]
    fn skips_nodisplay_hidden_and_non_applications() {
        for extra in ["NoDisplay=true", "Hidden=true", ""] {
            let text = format!(
                "[Desktop Entry]\nType={}\nName=X\nExec=x\n{}\n",
                if extra.is_empty() { "Link" } else { "Application" },
                extra
            );
            assert!(parse_desktop_entry("x", &text).is_none(), "case: {extra:?}");
        }
    }

    #[test]
    fn only_the_main_group_is_read() {
        let text = "\
[Desktop Entry]
Type=Application
Name=Real
Exec=real
[Desktop Action new-window]
Name=Shadow
Exec=shadow
";
        let app = parse_desktop_entry("x", text).unwrap();
        assert_eq!(app.name, "Real");
        assert_eq!(app.exec, "real");
    }

    #[test]
    fn field_codes_strip_cleanly() {
        assert_eq!(strip_field_codes("vlc --started-from-file %U"), "vlc --started-from-file");
        assert_eq!(strip_field_codes("app %f middle %u end"), "app middle end");
        assert_eq!(strip_field_codes("echo 100%% done"), "echo 100% done");
    }

    #[test]
    fn exec_splitting_honours_quotes_and_escapes() {
        assert_eq!(split_exec("a b c"), vec!["a", "b", "c"]);
        assert_eq!(split_exec(r#""/opt/My App/run" --flag"#), vec!["/opt/My App/run", "--flag"]);
        assert_eq!(split_exec(r#"x "say \"hi\"""#), vec!["x", r#"say "hi""#]);
    }

    #[test]
    fn localized_keys_do_not_clobber_plain_ones() {
        let text = "\
[Desktop Entry]
Type=Application
Name=Files
Name[de]=Dateien
Exec=nautilus
";
        assert_eq!(parse_desktop_entry("x", text).unwrap().name, "Files");
    }
}
