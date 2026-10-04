//! entry — .desktop discovery and parsing.
//!
//! Hand-rolled rather than a crate for the usual reason in this tree: the spec
//! subset a launcher needs is ~100 lines, external desktop-entry crates have
//! churned their APIs twice in two years, and a visible parse loop is a
//! debuggable parse loop. What we implement, deliberately:
//!
//! * `[Desktop Entry]` plus the `[Desktop Action …]` groups it names, which
//!   are the "New Window" / "New Private Window" items a context menu offers.
//! * `Name[ll_CC]` / `Name[ll]` localisation from `$LC_MESSAGES`/`$LANG`.
//! * `NoDisplay` / `Hidden` honoured (both mean "not in a launcher").
//! * Later data dirs are SHADOWED by earlier ones, per the XDG precedence:
//!   `~/.local/share` overrides `/usr/share` for the same desktop-file id.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A secondary way to launch an app, from a `[Desktop Action …]` group.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DesktopAction {
    /// The action id as listed in `Actions=`.
    pub id: String,
    pub name: String,
    pub exec: String,
}

/// One launchable application.
/// What an entry launches. A search also finds things that are not apps: an app's own `[Desktop Action]` items and a
/// few system functions, as VIRTUAL entries that carry the parent's icon and keywords.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum EntryKind {
    #[default]
    App,
    /// `action` of the app with id `parent` ("New Private Window" of Firefox).
    Action { parent: String, action: String },
    /// A system function by its id in `SYSTEM_ACTIONS` ("lock").
    System(String),
}

/// The searchable non-app entries for `apps`: each app's actions, and Lock. Shut down, Restart and Log out are NOT here
/// on purpose: a partial query plus Enter must never be able to reboot a phone; they stay on the system row.
pub fn virtual_entries(apps: &[AppEntry]) -> Vec<AppEntry> {
    let mut out = Vec::new();
    for app in apps.iter().filter(|a| a.kind == EntryKind::App) {
        for act in &app.actions {
            out.push(AppEntry {
                id: format!("{}#{}", app.id, act.id),
                name: format!("{} \u{b7} {}", act.name, app.name),
                description: app.name.clone(),
                keywords: app.keywords.clone(),
                icon: app.icon.clone(),
                exec: act.exec.clone(),
                terminal: app.terminal,
                workdir: app.workdir.clone(),
                actions: Vec::new(),
                path: app.path.clone(),
                kind: EntryKind::Action { parent: app.id.clone(), action: act.id.clone() },
            });
        }
    }
    out.push(AppEntry {
        id: "system#lock".into(),
        name: "Lock screen".into(),
        description: "System".into(),
        keywords: "lock secure session".into(),
        icon: "system-lock-screen".into(),
        exec: "loginctl lock-session".into(),
        kind: EntryKind::System("lock".into()),
        ..Default::default()
    });
    out
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AppEntry {
    /// The desktop-file id: relative path under `applications/` with `/`
    /// flattened to `-` and `.desktop` stripped. This is the id compositors
    /// match windows by, so it is also the id favorites/recents persist.
    pub id: String,
    pub name: String,
    /// GenericName or Comment — whichever the file has, for ranking + tooltips.
    pub description: String,
    /// `Keywords=` (localised), space-joined: what the app is for, in words the name does not contain ("browser"
    /// finds Firefox). Ranked below the name and above the description.
    pub keywords: String,
    pub icon: String,
    pub exec: String,
    pub terminal: bool,
    /// `Path=`: the working directory to launch in. Ignoring this is a quiet
    /// way to break apps that expect to start somewhere specific — a game
    /// that loads assets relatively, a script wrapper.
    pub workdir: Option<PathBuf>,
    /// `Actions=` entries, in declared order.
    pub actions: Vec<DesktopAction>,
    /// The file this came from, for debugging "why is this entry weird".
    pub path: PathBuf,
    /// An app, or a virtual entry (an app's action, a system function) that only a search shows.
    pub kind: EntryKind,
}

/// The locale suffixes to try, most specific first: `de_DE`, then `de`.
/// Modifiers/encodings (`@euro`, `.UTF-8`) are stripped — the spec allows
/// matching them but launchers that try rarely ship files that use them.
fn locale_keys() -> Vec<String> {
    let raw = std::env::var("LC_MESSAGES")
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default();
    let base = raw.split(['.', '@']).next().unwrap_or("");
    let mut out = Vec::new();
    if !base.is_empty() && base != "C" && base != "POSIX" {
        out.push(base.to_string());
        if let Some(lang) = base.split('_').next() {
            if lang != base {
                out.push(lang.to_string());
            }
        }
    }
    out
}

/// Parse one .desktop file's `[Desktop Entry]` group into a key→value map,
/// with localised keys stored under `key[locale]` verbatim.
fn parse_groups(text: &str) -> Vec<(String, HashMap<String, String>)> {
    let mut groups: Vec<(String, HashMap<String, String>)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(group) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            groups.push((group.to_string(), HashMap::new()));
            continue;
        }
        let Some((_, map)) = groups.last_mut() else { continue }; // keys before any group
        if let Some(eq) = line.find('=') {
            let (k, v) = line.split_at(eq);
            map.insert(k.trim().to_string(), v[1..].trim().to_string());
        }
    }
    groups
}

fn parse_group(text: &str) -> HashMap<String, String> {
    parse_groups(text)
        .into_iter()
        .find(|(name, _)| name == "Desktop Entry")
        .map(|(_, map)| map)
        .unwrap_or_default()
}

/// The actions an entry declares, in `Actions=` order. An action missing its
/// group, its Name, or its Exec is skipped rather than shown as a dead menu
/// item — half-written action groups are common in the wild.
fn parse_actions(text: &str, map: &HashMap<String, String>, locales: &[String]) -> Vec<DesktopAction> {
    let Some(list) = map.get("Actions") else { return Vec::new() };
    let groups = parse_groups(text);
    let mut out = Vec::new();
    for id in list.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let Some((_, g)) = groups.iter().find(|(n, _)| n == &format!("Desktop Action {id}")) else {
            continue;
        };
        if parse_bool(g.get("NoDisplay")) {
            continue;
        }
        let (Some(name), Some(exec)) = (localized(g, "Name", locales), g.get("Exec")) else {
            continue;
        };
        out.push(DesktopAction { id: id.to_string(), name: name.to_string(), exec: exec.clone() });
    }
    out
}

/// Localised lookup: `Name[de_DE]` → `Name[de]` → `Name`.
fn localized<'a>(map: &'a HashMap<String, String>, key: &str, locales: &[String]) -> Option<&'a str> {
    for loc in locales {
        if let Some(v) = map.get(&format!("{key}[{loc}]")) {
            return Some(v);
        }
    }
    map.get(key).map(String::as_str)
}

fn parse_bool(v: Option<&String>) -> bool {
    matches!(v.map(String::as_str), Some("true") | Some("True") | Some("1"))
}

/// Parse a single file. `None` means "valid file, but not a launcher entry"
/// (wrong Type, NoDisplay, Hidden, or no Exec) — not an error.
pub fn parse_entry(id: &str, path: &Path, text: &str, locales: &[String]) -> Option<AppEntry> {
    let map = parse_group(text);
    if map.get("Type").map(String::as_str) != Some("Application") {
        return None;
    }
    if parse_bool(map.get("NoDisplay")) || parse_bool(map.get("Hidden")) {
        return None;
    }
    let exec = map.get("Exec")?.clone();
    let name = localized(&map, "Name", locales)?.to_string();
    let description = localized(&map, "GenericName", locales)
        .or_else(|| localized(&map, "Comment", locales))
        .unwrap_or("")
        .to_string();
    Some(AppEntry {
        id: id.to_string(),
        name,
        description,
        keywords: localized(&map, "Keywords", locales).map(|k| k.split(';').map(str::trim).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")).unwrap_or_default(),
        icon: map.get("Icon").cloned().unwrap_or_default(),
        exec,
        terminal: parse_bool(map.get("Terminal")),
        workdir: map.get("Path").filter(|p| !p.is_empty()).map(PathBuf::from),
        actions: parse_actions(text, &map, locales),
        path: path.to_path_buf(),
        kind: EntryKind::App,
    })
}

/// The `applications/` roots to scan, highest precedence first.
pub fn application_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    dirs.push(config_kit::data_home().join("applications"));
    let sys = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    for d in sys.split(':').filter(|d| !d.is_empty()) {
        dirs.push(PathBuf::from(d).join("applications"));
    }
    dirs
}

/// Walk one `applications/` root (recursively — vendor subdirs like `kde/`
/// exist and their files get `kde-` prefixed ids per the flattening rule).
fn scan_dir(root: &Path, prefix: &str, locales: &[String], out: &mut Vec<(String, AppEntry)>) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    for de in rd.flatten() {
        let path = de.path();
        let Some(fname) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if path.is_dir() {
            scan_dir(&path, &format!("{prefix}{fname}-"), locales, out);
        } else if let Some(stem) = fname.strip_suffix(".desktop") {
            let id = format!("{prefix}{stem}");
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Some(e) = parse_entry(&id, &path, &text, locales) {
                    out.push((id, e));
                }
            }
        }
    }
}

/// Scan every root; the FIRST occurrence of an id wins (XDG precedence).
/// A shadowing file that is NoDisplay still shadows — that is how users hide a
/// system app by dropping an override in `~/.local/share/applications`.
pub fn scan(dirs: &[PathBuf]) -> Vec<AppEntry> {
    let locales = locale_keys();
    let mut seen: HashMap<String, ()> = HashMap::new();
    let mut entries = Vec::new();
    for root in dirs {
        // Shadowing needs the id even for filtered-out files, so ids are
        // claimed per-root by filename, then the parsed survivors kept.
        let mut found = Vec::new();
        scan_dir(root, "", &locales, &mut found);
        // Also claim ids of files present but rejected (NoDisplay overrides):
        claim_all_ids(root, "", &mut seen, &mut entries, found);
    }
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    entries
}

fn claim_all_ids(
    root: &Path,
    prefix: &str,
    seen: &mut HashMap<String, ()>,
    entries: &mut Vec<AppEntry>,
    parsed: Vec<(String, AppEntry)>,
) {
    // First pass: every .desktop filename in this root claims its id.
    fn claim(root: &Path, prefix: &str, seen: &mut HashMap<String, ()>, fresh: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(root) else { return };
        for de in rd.flatten() {
            let path = de.path();
            let Some(fname) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if path.is_dir() {
                claim(&path, &format!("{prefix}{fname}-"), seen, fresh);
            } else if let Some(stem) = fname.strip_suffix(".desktop") {
                let id = format!("{prefix}{stem}");
                if !seen.contains_key(&id) {
                    seen.insert(id.clone(), ());
                    fresh.push(id);
                }
            }
        }
    }
    let mut fresh = Vec::new();
    claim(root, prefix, seen, &mut fresh);
    // Second pass: of the parsed entries, keep the ones whose id was fresh
    // in THIS root (not shadowed by an earlier root).
    for (id, e) in parsed {
        if fresh.iter().any(|f| f == &id) {
            entries.push(e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locales_de() -> Vec<String> {
        vec!["de_DE".into(), "de".into()]
    }

    #[test]
    fn a_minimal_application_entry_parses() {
        let t = "[Desktop Entry]\nType=Application\nName=Files\nExec=nautilus %U\nIcon=org.gnome.Nautilus\n";
        let e = parse_entry("org.gnome.Nautilus", Path::new("x"), t, &[]).unwrap();
        assert_eq!(e.name, "Files");
        assert_eq!(e.icon, "org.gnome.Nautilus");
        assert!(!e.terminal);
    }

    #[test]
    fn keywords_are_split_trimmed_localised_and_absent_means_empty() {
        let t = "[Desktop Entry]\nType=Application\nName=Fox\nExec=fox\nKeywords=web; browser ;;internet\nKeywords[fr]=toile;navigateur\n";
        assert_eq!(parse_entry("fox", Path::new("x"), t, &[]).unwrap().keywords, "web browser internet");
        assert_eq!(parse_entry("fox", Path::new("x"), t, &["fr".to_string()]).unwrap().keywords, "toile navigateur");
        let none = "[Desktop Entry]\nType=Application\nName=Fox\nExec=fox\n";
        assert_eq!(parse_entry("fox", Path::new("x"), none, &[]).unwrap().keywords, "");
    }

    #[test]
    fn desktop_actions_are_parsed_in_declared_order() {
        let text = "\
[Desktop Entry]
Type=Application
Name=Firefox
Exec=firefox %u
Actions=new-window;new-private-window;broken;

[Desktop Action new-private-window]
Name=New Private Window
Exec=firefox --private-window

[Desktop Action new-window]
Name=New Window
Exec=firefox --new-window

[Desktop Action broken]
Name=No Exec Here
";
        let e = parse_entry("firefox", Path::new("/x"), text, &[]).unwrap();
        let ids: Vec<&str> = e.actions.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["new-window", "new-private-window"],
            "Actions= order wins over group order, and an action with no Exec is dropped"
        );
        assert_eq!(e.actions[0].name, "New Window");
        assert_eq!(e.actions[1].exec, "firefox --private-window");
    }

    #[test]
    fn an_action_can_be_hidden_and_a_missing_group_is_not_a_dead_menu_item() {
        let text = "\
[Desktop Entry]
Type=Application
Name=App
Exec=app
Actions=hidden;absent

[Desktop Action hidden]
Name=Hidden
Exec=app --hidden
NoDisplay=true
";
        let e = parse_entry("app", Path::new("/x"), text, &[]).unwrap();
        assert!(e.actions.is_empty());
    }

    #[test]
    fn the_working_directory_is_kept_when_declared() {
        let with = "[Desktop Entry]\nType=Application\nName=Game\nExec=game\nPath=/opt/game\n";
        let e = parse_entry("game", Path::new("/x"), with, &[]).unwrap();
        assert_eq!(e.workdir.as_deref(), Some(Path::new("/opt/game")));

        let without = "[Desktop Entry]\nType=Application\nName=Game\nExec=game\nPath=\n";
        let e = parse_entry("game", Path::new("/x"), without, &[]).unwrap();
        assert_eq!(e.workdir, None, "an empty Path= is no Path= at all");
    }

    #[test]
    fn keys_outside_the_desktop_entry_group_stay_outside_it() {
        // The regression risk in parsing all groups instead of one: an action
        // group's Name must not overwrite the entry's.
        let text = "\
[Desktop Entry]
Type=Application
Name=Real Name
Exec=app
Actions=alt

[Desktop Action alt]
Name=Alternate
Exec=app --alt
Icon=other-icon
";
        let e = parse_entry("app", Path::new("/x"), text, &[]).unwrap();
        assert_eq!(e.name, "Real Name");
        assert_eq!(e.icon, "", "the action's Icon is not the entry's");
    }

    #[test]
    fn nodisplay_and_hidden_and_non_applications_are_not_entries() {
        for t in [
            "[Desktop Entry]\nType=Application\nName=X\nExec=x\nNoDisplay=true\n",
            "[Desktop Entry]\nType=Application\nName=X\nExec=x\nHidden=true\n",
            "[Desktop Entry]\nType=Link\nName=X\nURL=http://x\n",
            "[Desktop Entry]\nType=Application\nName=X\n", // no Exec
        ] {
            assert!(parse_entry("x", Path::new("x"), t, &[]).is_none(), "{t}");
        }
    }

    #[test]
    fn localisation_prefers_the_most_specific_key() {
        let t = "[Desktop Entry]\nType=Application\nExec=x\nName=Files\nName[de]=Dateien\nName[de_DE]=Dateien (DE)\n";
        let e = parse_entry("x", Path::new("x"), t, &locales_de()).unwrap();
        assert_eq!(e.name, "Dateien (DE)");
        let e = parse_entry("x", Path::new("x"), t, &["de".into()]).unwrap();
        assert_eq!(e.name, "Dateien");
    }

    #[test]
    fn only_the_desktop_entry_group_is_read() {
        // A Desktop Action's Name must not clobber the app's Name.
        let t = "[Desktop Entry]\nType=Application\nName=App\nExec=app\n[Desktop Action new]\nName=New Window\nExec=app --new\n";
        let e = parse_entry("x", Path::new("x"), t, &[]).unwrap();
        assert_eq!(e.name, "App");
        assert_eq!(e.exec, "app");
    }

    #[test]
    fn earlier_roots_shadow_later_ones_even_with_nodisplay() {
        // ~/.local override hiding a system app: the launcher must show neither.
        let tmp = std::env::temp_dir().join(format!("hatch-scan-{}", std::process::id()));
        let user = tmp.join("user/applications");
        let sys = tmp.join("sys/applications");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::create_dir_all(&sys).unwrap();
        std::fs::write(
            user.join("editor.desktop"),
            "[Desktop Entry]\nType=Application\nName=X\nExec=x\nNoDisplay=true\n",
        )
        .unwrap();
        std::fs::write(
            sys.join("editor.desktop"),
            "[Desktop Entry]\nType=Application\nName=Editor\nExec=editor\n",
        )
        .unwrap();
        std::fs::write(
            sys.join("term.desktop"),
            "[Desktop Entry]\nType=Application\nName=Terminal\nExec=term\n",
        )
        .unwrap();
        let entries = scan(&[tmp.join("user"), tmp.join("sys")]
            .map(|p: PathBuf| p.join("applications"))
            .to_vec());
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["Terminal"], "editor is shadowed-hidden: {names:?}");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
