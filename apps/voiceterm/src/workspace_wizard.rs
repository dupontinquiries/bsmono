//! v0.1.710: Notes workspace setup wizard — the guided "start your own
//! zettelkasten" flow, with a voice walk-through. PURE core: the step state
//! machine, the user's config, the SCAFFOLD PLAN (what files/folders to
//! create), and the voice narration script. The actual file creation + QML UI
//! live in main.rs / main.qml; everything decision-shaped is here and tested.

// ── config the wizard collects ──

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdScheme { Timestamp, Folgezettel }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// everything in one folder, connected by [[links]] (Luhmann-pure).
    Flat,
    /// inbox/ + permanent/ + references/ + index/ (a "second brain" split).
    Structured,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WizardConfig {
    /// workspace root (absolute path).
    pub root: String,
    pub id_scheme: IdScheme,
    pub layout: Layout,
    /// seed an index/hub note.
    pub seed_index: bool,
    /// seed a "how to zettelkasten" primer note.
    pub seed_primer: bool,
    /// add a daily-note template.
    pub daily_template: bool,
    /// run the spoken walk-through.
    pub voice_walkthrough: bool,
}

impl Default for WizardConfig {
    fn default() -> Self {
        WizardConfig {
            root: String::new(),
            id_scheme: IdScheme::Timestamp,
            layout: Layout::Flat,
            seed_index: true,
            seed_primer: true,
            daily_template: false,
            voice_walkthrough: true,
        }
    }
}

// ── step state machine ──

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Welcome,
    ChooseLocation,
    IdScheme,
    Layout,
    SeedContent,
    Review,
    Done,
}

impl Step {
    pub const ORDER: [Step; 7] = [
        Step::Welcome, Step::ChooseLocation, Step::IdScheme, Step::Layout,
        Step::SeedContent, Step::Review, Step::Done,
    ];

    pub fn index(self) -> usize {
        Self::ORDER.iter().position(|s| *s == self).unwrap_or(0)
    }
    pub fn next(self) -> Step {
        let i = self.index();
        Self::ORDER.get(i + 1).copied().unwrap_or(Step::Done)
    }
    pub fn prev(self) -> Step {
        let i = self.index();
        if i == 0 { Step::Welcome } else { Self::ORDER[i - 1] }
    }
    /// step title for the wizard header.
    pub fn title(self) -> &'static str {
        match self {
            Step::Welcome => "Welcome",
            Step::ChooseLocation => "Where",
            Step::IdScheme => "Note IDs",
            Step::Layout => "Layout",
            Step::SeedContent => "Starter notes",
            Step::Review => "Review",
            Step::Done => "Done",
        }
    }
}

/// Can we advance from `step` given the config so far? Returns an error message
/// when not (e.g. no location chosen).
pub fn can_advance(step: Step, cfg: &WizardConfig) -> Result<(), String> {
    match step {
        Step::ChooseLocation if cfg.root.trim().is_empty() =>
            Err("Choose a folder for your notes first.".into()),
        _ => Ok(()),
    }
}

// ── filename slug ──

/// Slugify a title for a filename: lowercase, spaces→hyphens, drop punctuation,
/// collapse repeats, trim, cap length.
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    let mut last_hyphen = true; // suppress leading hyphen
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_hyphen = false;
        } else if !last_hyphen {
            out.push('-');
            last_hyphen = true;
        }
        if out.len() >= 50 { break; }
    }
    while out.ends_with('-') { out.pop(); }
    out
}

// ── scaffold plan ──

/// A file the wizard will create. `path` is RELATIVE to the workspace root.
#[derive(Debug, Clone, PartialEq)]
pub struct FileSpec {
    pub path: String,
    pub contents: String,
    /// true for a directory (contents ignored).
    pub is_dir: bool,
}

/// Compute the full set of files/dirs to create for `cfg`. Deterministic and
/// pure — the caller just writes each spec under cfg.root.
pub fn scaffold_plan(cfg: &WizardConfig) -> Vec<FileSpec> {
    let mut out = Vec::new();
    let dir = |p: &str| FileSpec { path: p.into(), contents: String::new(), is_dir: true };
    let file = |p: &str, c: String| FileSpec { path: p.into(), contents: c, is_dir: false };

    // folders for the structured layout
    if cfg.layout == Layout::Structured {
        for d in ["inbox", "permanent", "references", "index"] {
            out.push(dir(d));
        }
    }

    // where seed notes land
    let base = match cfg.layout {
        Layout::Flat => "",
        Layout::Structured => "permanent/",
    };
    let idx_base = match cfg.layout {
        Layout::Flat => "",
        Layout::Structured => "index/",
    };

    if cfg.seed_index {
        out.push(file(&format!("{}index.md", idx_base), index_note(cfg)));
    }
    if cfg.seed_primer {
        out.push(file(&format!("{}how-to-zettelkasten.md", base), primer_note(cfg)));
    }
    if cfg.daily_template {
        out.push(file("templates/daily.md", daily_template()));
    }

    // a workspace marker so the app recognizes this as a notes root
    out.push(file(".web2qml-notes", workspace_marker(cfg)));
    out
}

fn id_example(cfg: &WizardConfig) -> &'static str {
    match cfg.id_scheme {
        IdScheme::Timestamp => "202606071430",
        IdScheme::Folgezettel => "1a2",
    }
}

fn index_note(cfg: &WizardConfig) -> String {
    let links = if cfg.seed_primer {
        "\n- [[how-to-zettelkasten]] — read this first\n"
    } else { "\n" };
    format!(
        "# Index\n\nYour entry point. Link your hub notes here, then follow the \
         [[links]] outward.\n{}\nNew to this? Say **\"new note\"** or tap + to \
         create your first zettel. Link notes by typing `[[` and the name.\n",
        links
    )
}

fn primer_note(cfg: &WizardConfig) -> String {
    let id_line = match cfg.id_scheme {
        IdScheme::Timestamp =>
            "Each note gets a timestamp id (e.g. `202606071430`) — always unique, \
             no bookkeeping.",
        IdScheme::Folgezettel =>
            "Each note gets a Folgezettel id (e.g. `1`, `1a`, `1a1`) — the id \
             encodes where the thought sits in a thread.",
    };
    format!(
        "# How to zettelkasten\n\n\
         A zettelkasten is a web of small, atomic notes you link together.\n\n\
         **One idea per note.** If a note grows two ideas, split it.\n\n\
         **Link generously.** Type `[[` and a note name to connect them; the \
         links are the value.\n\n\
         **{}**\n\n\
         **Start messy, refine later.** Capture first; the structure emerges \
         from the links, not from folders.\n\n\
         Your first move: make a note about something you're thinking about \
         right now, then link it to [[index]].\n",
        id_line
    )
}

fn daily_template() -> String {
    "# {{date}}\n\n## Captured\n\n- \n\n## Linked\n\n- \n".to_string()
}

fn workspace_marker(cfg: &WizardConfig) -> String {
    let scheme = match cfg.id_scheme {
        IdScheme::Timestamp => "timestamp",
        IdScheme::Folgezettel => "folgezettel",
    };
    let layout = match cfg.layout {
        Layout::Flat => "flat",
        Layout::Structured => "structured",
    };
    format!("# web2qml notes workspace\nid_scheme = {}\nlayout = {}\nid_example = {}\n",
        scheme, layout, id_example(cfg))
}

// ── voice walk-through script ──

/// One spoken step: what the narrator says, and the phrases that advance it.
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceStep {
    pub step: Step,
    /// narration spoken when this step opens.
    pub say: String,
    /// example phrases the user can say here (for the on-screen hint).
    pub accepts: Vec<String>,
}

/// The full walk-through script. The QML plays `say` on each step and listens
/// for the `accepts` phrases (mapped to wizard actions by the dispatcher).
pub fn voice_script() -> Vec<VoiceStep> {
    let s = |step, say: &str, accepts: &[&str]| VoiceStep {
        step, say: say.to_string(),
        accepts: accepts.iter().map(|x| x.to_string()).collect(),
    };
    vec![
        s(Step::Welcome,
          "Let's set up your zettelkasten — a linked web of small notes. \
           Say next to begin, or cancel to stop.",
          &["next", "cancel"]),
        s(Step::ChooseLocation,
          "First, where should your notes live? Pick a folder, then say next.",
          &["choose folder", "next", "back"]),
        s(Step::IdScheme,
          "How should notes be named? Say timestamps for automatic unique ids, \
           or folgezettel for Luhmann-style branching ids.",
          &["timestamps", "folgezettel", "next", "back"]),
        s(Step::Layout,
          "Layout: say flat to keep every note in one folder connected by \
           links, or structured for separate inbox, permanent, and reference \
           folders.",
          &["flat", "structured", "next", "back"]),
        s(Step::SeedContent,
          "I can add starter notes: an index hub and a how-to primer. Say skip \
           starters to begin empty, or next to keep them.",
          &["keep starters", "skip starters", "add daily template", "next", "back"]),
        s(Step::Review,
          "Ready to create your workspace. Say create to build it, or back to \
           change anything.",
          &["create", "back"]),
        s(Step::Done,
          "Your zettelkasten is ready. Say new note to write your first one.",
          &["new note", "open index"]),
    ]
}

/// The narration for a specific step (for replaying / TTS).
pub fn narration_for(step: Step) -> String {
    voice_script().into_iter().find(|v| v.step == step)
        .map(|v| v.say).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_navigation() {
        assert_eq!(Step::Welcome.next(), Step::ChooseLocation);
        assert_eq!(Step::ChooseLocation.prev(), Step::Welcome);
        assert_eq!(Step::Done.next(), Step::Done); // clamps
        assert_eq!(Step::Welcome.prev(), Step::Welcome); // clamps
        assert_eq!(Step::Review.next(), Step::Done);
        assert_eq!(Step::Welcome.index(), 0);
        assert_eq!(Step::Done.index(), 6);
    }

    #[test]
    fn advance_requires_location() {
        let mut cfg = WizardConfig::default();
        assert!(can_advance(Step::ChooseLocation, &cfg).is_err());
        cfg.root = "/home/me/notes".into();
        assert!(can_advance(Step::ChooseLocation, &cfg).is_ok());
        // other steps don't gate
        assert!(can_advance(Step::IdScheme, &WizardConfig::default()).is_ok());
    }

    #[test]
    fn slugify() {
        assert_eq!(slug("My First Note"), "my-first-note");
        assert_eq!(slug("Hello, World!!!"), "hello-world");
        assert_eq!(slug("  leading/trailing  "), "leading-trailing");
        assert_eq!(slug(""), "");
    }

    #[test]
    fn scaffold_flat_default() {
        let cfg = WizardConfig { root: "/n".into(), ..Default::default() };
        let plan = scaffold_plan(&cfg);
        let paths: Vec<&str> = plan.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"index.md"));
        assert!(paths.contains(&"how-to-zettelkasten.md"));
        assert!(paths.contains(&".web2qml-notes"));
        // flat → no subfolders
        assert!(!plan.iter().any(|f| f.is_dir));
    }

    #[test]
    fn scaffold_structured_has_dirs() {
        let cfg = WizardConfig { root: "/n".into(), layout: Layout::Structured, ..Default::default() };
        let plan = scaffold_plan(&cfg);
        let dirs: Vec<&str> = plan.iter().filter(|f| f.is_dir).map(|f| f.path.as_str()).collect();
        assert!(dirs.contains(&"inbox"));
        assert!(dirs.contains(&"permanent"));
        // index note lands under index/
        assert!(plan.iter().any(|f| f.path == "index/index.md"));
        // primer under permanent/
        assert!(plan.iter().any(|f| f.path == "permanent/how-to-zettelkasten.md"));
    }

    #[test]
    fn scaffold_respects_seed_toggles() {
        let cfg = WizardConfig {
            root: "/n".into(), seed_index: false, seed_primer: false,
            daily_template: true, ..Default::default()
        };
        let plan = scaffold_plan(&cfg);
        assert!(!plan.iter().any(|f| f.path.contains("index.md")));
        assert!(!plan.iter().any(|f| f.path.contains("how-to")));
        assert!(plan.iter().any(|f| f.path == "templates/daily.md"));
    }

    #[test]
    fn marker_records_choices() {
        let cfg = WizardConfig { root: "/n".into(), id_scheme: IdScheme::Folgezettel,
            layout: Layout::Structured, ..Default::default() };
        let m = scaffold_plan(&cfg).into_iter().find(|f| f.path == ".web2qml-notes").unwrap();
        assert!(m.contents.contains("id_scheme = folgezettel"));
        assert!(m.contents.contains("layout = structured"));
    }

    #[test]
    fn voice_script_covers_all_steps() {
        let script = voice_script();
        assert_eq!(script.len(), Step::ORDER.len());
        for step in Step::ORDER {
            assert!(script.iter().any(|v| v.step == step), "missing {step:?}");
            assert!(!narration_for(step).is_empty());
        }
        // each step offers at least one phrase
        assert!(script.iter().all(|v| !v.accepts.is_empty()));
    }
}
