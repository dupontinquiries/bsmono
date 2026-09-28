//! settings_model — `ui/settings_panel.rs`, ported as a model.
//!
//! The in-keyboard settings panel: three groups of choices, applied and
//! persisted the moment they are made.
//!
//! ## What ports is the model, not the widget tree
//!
//! The GTK file is 2,469 lines, and most of that is building rows, wiring
//! callbacks and generating CSS for colour swatches. The part worth carrying is
//! the **shape**: which settings exist, what each option means, and the two
//! rules the panel is built around.
//!
//! ## Persist on every toggle, with no Save button
//!
//! *"Changes are persisted to `~/.config/rusty-board/config.toml` on every
//! toggle so user choices survive restart without needing to hit a Save
//! button. Broadcast to the keyboard view immediately so the visible layout
//! switches without reopening the app."*
//!
//! For a keyboard that is the only workable design. The panel replaces the
//! keys, so a Save button would have to be tapped with the very keyboard you
//! just reconfigured — and a setting that does not apply until restart is
//! untestable by the person changing it.
//!
//! ## No secondary window, for two stated reasons
//!
//! *"(a) on-mobile, secondary windows are awful, and (b) staying inside our
//! layer surface means the settings UI gets the same docking"* behaviour. A
//! keyboard is a layer-shell surface; a popup would sit outside it and be
//! placed by the compositor rather than by the keyboard.

/// Which group a setting belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group { Layout, Typing, Colors, Llm, Experimental }

impl Group {
    pub const ALL: [Group; 5] =
        [Group::Layout, Group::Typing, Group::Colors, Group::Llm, Group::Experimental];

    pub fn title(self) -> &'static str {
        match self {
            Group::Layout => "Layout",
            Group::Typing => "Typing",
            // Fonts joined the colours; the key stays `Colors` so stored
            // values and the CLI's group name keep working.
            Group::Colors => "Look",
            Group::Llm => "LLM",
            Group::Experimental => "Experimental",
        }
    }
}

/// What kind of control a setting needs.
///
/// The GTK panel has exactly three row builders — `build_setting_row`,
/// `build_text_row`, `build_color_row` — and this is that distinction carried
/// across. It exists because the model was option-lists-only, which left two
/// whole groups unportable: the LLM rows are free text and the Colors tab is
/// hex strings, and faking either as a two-option toggle would be a lie about
/// what the control does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// One of `options`. The original and still the common case.
    Choice,
    /// Free text — an endpoint, a prompt.
    Text,
    /// Free text that must not be shown: an API key.
    ///
    /// Distinct from `Text` rather than a flag on it, because the difference
    /// changes what a UI may do — render dots, refuse to log the value, keep
    /// it out of a settings export — and a `bool` beside the value is easy to
    /// read past.
    Secret,
    /// A hex colour, or empty meaning "fall back to the theme".
    ///
    /// DECLARED, NOT YET USED: no `Setting` carries this kind. The GTK Colors
    /// tab builds its pickers from a per-key list rather than a static table,
    /// so porting them is a data question (which keys, in what order) and not
    /// just a row type. The variant is here because `Kind` should describe the
    /// three controls the predecessor actually has, and because the CLI's
    /// handling of it is written and tested either way -- but nothing is
    /// pretending the colour rows exist yet.
    ///
    /// Empty-is-default is the GTK panel's own convention: its "reset colors
    /// to theme defaults" button "empties every hex string". Carried rather
    /// than replaced with a sentinel colour, because an empty field and a
    /// colour that happens to equal the theme's are different states — one
    /// follows the theme when it changes and the other does not.
    Color,
    /// A font: an installed family name (resolved with fontconfig) or a path
    /// to a .ttf/.otf file. Empty is the built-in face. Its own kind because
    /// the control is a picker over the installed fonts, not a text box.
    Font,
}

/// One multiple-choice setting.
#[derive(Clone, Debug, PartialEq)]
pub struct Setting {
    /// Config key, e.g. `layout.letters`.
    pub key: &'static str,
    pub label: &'static str,
    /// The help line under the label. Carried across because it is where the
    /// options are actually explained — "Staggered matches a physical keyboard;
    /// ortholinear is a strict grid" is the sentence that makes the choice
    /// meaningful.
    pub help: &'static str,
    pub group: Group,
    /// Which control this needs. `Choice` for everything that came across
    /// before `Kind` existed.
    pub kind: Kind,
    /// Display names, in order.
    pub options: &'static [&'static str],
    /// Values written to the config file, parallel to `options`.
    pub values: &'static [&'static str],
}

impl Setting {
    /// Index of a stored value, defaulting to zero.
    ///
    /// A config file holding a value this build does not know — written by a
    /// newer version, or hand-edited — selects the FIRST option rather than
    /// failing. A keyboard that will not open because of one bad line is
    /// unusable in the most literal sense: you cannot type the fix.
    pub fn index_of(&self, value: &str) -> usize {
        self.values.iter().position(|v| *v == value).unwrap_or(0)
    }

    pub fn value_at(&self, index: usize) -> &'static str {
        self.values.get(index).copied().unwrap_or(self.values[0])
    }

    /// Cycle to the next option — the panel's rows are tap-to-toggle.
    pub fn next(&self, current: &str) -> &'static str {
        let i = self.index_of(current);
        self.value_at((i + 1) % self.values.len().max(1))
    }
}

/// Every setting the panel exposes.
pub const SETTINGS: &[Setting] = &[
    Setting {
        key: "layout.letters", label: "Letters",
        help: "Base letter layout. QWERTY or Dvorak.",
        group: Group::Layout,
        kind: Kind::Choice,
        options: &["QWERTY", "Dvorak"], values: &["qwerty", "dvorak"],
    },
    Setting {
        key: "layout.geometry", label: "Geometry",
        help: "Staggered matches a physical keyboard; ortholinear is a strict grid.",
        group: Group::Layout,
        kind: Kind::Choice,
        options: &["Staggered", "Ortholinear"], values: &["staggered", "ortho"],
    },
    Setting {
        key: "layout.density", label: "Density",
        help: "Compact reduces vertical padding for one-handed reach.",
        group: Group::Layout,
        kind: Kind::Choice,
        options: &["Normal", "Compact"], values: &["normal", "compact"],
    },
    Setting {
        key: "layout.hand", label: "One-handed",
        help: "Compact the board toward one thumb. Off is full width. The freed strip \
               beside it stays live. GTK's own note says \"a future setting will let the \
               user choose\" the dominant side — this is that setting.",
        group: Group::Layout,
        kind: Kind::Choice,
        options: &["Off", "Left", "Right"], values: &["off", "left", "right"],
    },
    Setting {
        key: "layout.experimental_mode", label: "Mode",
        help: "Off = normal keyboard. Cardinal = 3×3 swipe grid. \
               Stenotype = 4-button steno (very experimental). \
               Words = 5-col cardinal with prose / C / Rust / Python clusters.",
        group: Group::Experimental,
        kind: Kind::Choice,
        options: &["Off", "Cardinal", "Stenotype", "Words"],
        values: &["off", "cardinal", "stenotype", "words"],
    },
    // ── Typing: ported from the GTK panel, help text verbatim ───────────
    //
    // DECLARATION ORDER IS THE DEFAULT. `Setting::next` cycles from
    // `values[0]` and `settings_cli::Reset` goes there, so the first option
    // must be what a fresh install has. Checked against `src/config/mod.rs`
    // rather than assumed, and four of these six needed reordering:
    // swipe_typing, swipe_auto_space and auto_hide_on_no_focus all default
    // TRUE, and swipe_speed_mode defaults to "normal", not the first of
    // slow/normal/fast. Writing them Off-first would have made `reset` turn
    // swipe typing off — a "restore defaults" that changes the default.
    //
    // `src/ui/settings_panel.rs` carries about two dozen settings; this model
    // arrived with seven, all Layout plus two Experimental. The whole swipe
    // group was missing, which is the group that changes how the keyboard
    // FEELS -- and the help lines are the part worth carrying exactly, because
    // they are where each choice is actually explained.
    Setting {
        key: "typing.swipe", label: "Swipe typing",
        help: "Drag across letters to type whole words. Off → spacebar drag becomes arrow keys.",
        group: Group::Typing,
        kind: Kind::Choice,
        options: &["On", "Off"], values: &["on", "off"],
    },
    Setting {
        key: "typing.swipe_speed", label: "Swipe speed",
        help: "How hard you pause to mark a letter. Slow = gentle pauses count; Fast = only near-stops.",
        group: Group::Typing,
        kind: Kind::Choice,
        options: &["Normal", "Slow", "Fast"], values: &["normal", "slow", "fast"],
    },
    Setting {
        key: "typing.lenient_swipe", label: "Lenient swipe",
        help: "Accept faster, sloppier swipes. Trades precision for forgiveness.",
        group: Group::Typing,
        kind: Kind::Choice,
        options: &["Off", "On"], values: &["off", "on"],
    },
    Setting {
        key: "typing.auto_space", label: "Auto-space swipes",
        help: "Add a space automatically between consecutive swipe-typed words.",
        group: Group::Typing,
        kind: Kind::Choice,
        options: &["On", "Off"], values: &["on", "off"],
    },
    Setting {
        key: "typing.auto_cap", label: "Auto-capitalize",
        help: "Capitalize the first letter after a period or at start of input.",
        group: Group::Typing,
        kind: Kind::Choice,
        options: &["Off", "On"], values: &["off", "on"],
    },
    Setting {
        key: "typing.auto_hide", label: "Auto-hide on no focus",
        help: "Hide the keyboard when no text field has focus.",
        group: Group::Typing,
        kind: Kind::Choice,
        options: &["On", "Off"], values: &["on", "off"],
    },
    // Key repeat: the GTK config's `key_repeat` / `repeat_delay_ms` /
    // `repeat_interval_ms`, as two controls (key_hold.rs applies them).
    Setting {
        key: "typing.key_repeat", label: "Key repeat",
        help: "Hold backspace or an arrow key to repeat it.",
        group: Group::Typing,
        kind: Kind::Choice,
        options: &["On", "Off"], values: &["on", "off"],
    },
    Setting {
        key: "typing.repeat_speed", label: "Repeat speed",
        help: "How soon a held key starts repeating, and how fast.",
        group: Group::Typing,
        kind: Kind::Choice,
        options: &["Normal", "Slow", "Fast"], values: &["normal", "slow", "fast"],
    },
    // ── Look: fonts ──────────────────────────────────────────────────────
    Setting {
        key: "look.font", label: "Key font",
        help: "The face key labels are drawn in. Symbols it lacks still come from the built-in fallbacks.",
        group: Group::Colors,
        kind: Kind::Font,
        options: &[""], values: &[""],
    },
    Setting {
        key: "look.font_size", label: "Key label size",
        help: "Label size relative to the key.",
        group: Group::Colors,
        kind: Kind::Choice,
        options: &["Normal", "Small", "Large", "Larger"], values: &["normal", "small", "large", "larger"],
    },
    // ── Colors ───────────────────────────────────────────────────────────
    //
    // The GTK panel's Colors tab is mostly hex-string pickers with a "reset to
    // theme defaults" button, and a hex field is not a `Setting` -- this model
    // is option lists, deliberately ("what ports is the model, not the widget
    // tree"). Glass effect is the one Colors row that IS a choice between
    // named options, so it is the one that fits here. The pickers need their
    // own type and are not faked as a two-option toggle.
    Setting {
        key: "colors.glass", label: "Glass effect",
        help: "Translucent keyboard with refraction gradient. Wallpaper shows through.",
        group: Group::Colors,
        kind: Kind::Choice,
        options: &["Off", "On"], values: &["off", "on"],
    },
    // ── LLM: the GTK panel's two text rows ──────────────────────────────
    //
    // `build_text_row` is used exactly twice in settings_panel.rs, for these
    // two. There is no system-prompt or timeout ROW -- `llm.system_prompt` and
    // `llm.timeout_ms` exist in the config and are set elsewhere, so porting
    // them here would invent UI the predecessor does not have.
    Setting {
        key: "llm.endpoint", label: "LLM endpoint",
        help: "URL of an OpenAI-compatible chat completions API.",
        group: Group::Llm,
        kind: Kind::Text,
        // config/mod.rs: "http://localhost:11434/v1/chat/completions"
        options: &["http://localhost:11434/v1/chat/completions"],
        values: &["http://localhost:11434/v1/chat/completions"],
    },
    Setting {
        key: "llm.api_key", label: "LLM API key",
        help: "Bearer token (leave blank for local servers).",
        group: Group::Llm,
        kind: Kind::Secret,
        // Blank is the default and a meaningful value: local servers want no
        // token at all, which is why the help says so.
        options: &[""], values: &[""],
    },
    Setting {
        key: "experimental.voice", label: "Voice input",
        help: "Dictate with the mic key.",
        group: Group::Experimental,
        kind: Kind::Choice,
        options: &["Off", "On"], values: &["off", "on"],
    },
    Setting {
        key: "experimental.swipe_trail", label: "Swipe trail",
        help: "Draw the path your finger takes while swiping.",
        group: Group::Experimental,
        kind: Kind::Choice,
        options: &["Off", "On"], values: &["off", "on"],
    },
];

/// The eight user-customisable palette entries.
pub const COLOR_KEYS: [&str; 8] = [
    "background", "key_fill", "key_fill_active", "text",
    "text_dim", "accent", "accent_alt", "border",
];

/// What a change produces.
///
/// Both, always — the panel applies AND persists on every toggle, and returning
/// one without the other is how a setting appears to work until restart.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub key: &'static str,
    pub value: String,
    /// Tell the keyboard view to re-layout now.
    pub broadcast: bool,
    /// Write the config file now.
    pub persist: bool,
}

/// Settings in one group, in declared order.
pub fn group(g: Group) -> Vec<&'static Setting> {
    SETTINGS.iter().filter(|s| s.group == g).collect()
}

/// Toggle a setting to its next value.
pub fn toggle(setting: &Setting, current: &str) -> Change {
    Change {
        key: setting.key,
        value: setting.next(current).to_string(),
        broadcast: true,
        persist: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(key: &str) -> &'static Setting {
        SETTINGS.iter().find(|s| s.key == key).expect("setting")
    }

    #[test]
    fn a_toggle_both_applies_and_persists() {
        // "Persisted on every toggle so user choices survive restart without
        // needing to hit a Save button. Broadcast immediately so the visible
        // layout switches without reopening the app." Returning one without the
        // other is how a setting appears to work until restart.
        let c = toggle(find("layout.letters"), "qwerty");
        assert_eq!(c.value, "dvorak");
        assert!(c.broadcast && c.persist);
    }

    #[test]
    fn a_two_option_setting_cycles_back() {
        let s = find("layout.geometry");
        assert_eq!(s.next("staggered"), "ortho");
        assert_eq!(s.next("ortho"), "staggered");
    }

    #[test]
    fn the_experimental_mode_has_four_options_not_two() {
        // Off, Cardinal, Stenotype, Words — a tap-to-toggle row that stopped at
        // two would make the last two unreachable.
        let s = find("layout.experimental_mode");
        assert_eq!(s.options.len(), 4);
        let mut v = "off";
        let mut seen = vec![v];
        for _ in 0..3 { v = s.next(v); seen.push(v); }
        assert_eq!(seen, vec!["off", "cardinal", "stenotype", "words"]);
        assert_eq!(s.next("words"), "off", "and wraps");
    }

    #[test]
    fn an_unknown_stored_value_selects_the_first_option_rather_than_failing() {
        // A config written by a newer build, or hand-edited. A keyboard that
        // will not open because of one bad line is unusable in the most literal
        // sense: you cannot type the fix.
        let s = find("layout.letters");
        assert_eq!(s.index_of("colemak"), 0);
        assert_eq!(s.index_of(""), 0);
        assert_eq!(s.next("colemak"), "dvorak", "and still toggles sensibly");
    }

    #[test]
    fn an_out_of_range_index_returns_the_first_value() {
        let s = find("layout.density");
        assert_eq!(s.value_at(99), "normal");
    }

    #[test]
    fn every_setting_has_as_many_values_as_options() {
        // A mismatch would show a label whose value does not exist, and write a
        // config the next launch cannot read.
        for s in SETTINGS {
            assert_eq!(s.options.len(), s.values.len(), "{}", s.key);
            assert!(!s.values.is_empty(), "{}", s.key);
        }
    }

    #[test]
    fn every_setting_explains_its_options() {
        // The help line is where the choice is actually made meaningful —
        // "Staggered matches a physical keyboard; ortholinear is a strict grid"
        // is the sentence, not the label.
        for s in SETTINGS {
            assert!(!s.help.is_empty(), "{}", s.key);
            assert!(s.help.len() > s.label.len(), "{} says nothing", s.key);
        }
    }

    #[test]
    fn config_keys_are_unique() {
        // Two settings sharing a key would silently overwrite each other on
        // save, and the loser would revert on every restart.
        let mut keys: Vec<&str> = SETTINGS.iter().map(|s| s.key).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), before);
    }

    #[test]
    fn config_values_are_lowercase_and_option_labels_are_not_forced_to_match() {
        // The file holds "qwerty" while the row shows "QWERTY": a config is
        // read by humans editing it and by a parser, and neither wants the
        // display casing.
        for s in SETTINGS {
            for v in s.values {
                assert_eq!(*v, v.to_lowercase(), "{} value {v}", s.key);
                assert!(!v.contains(' '), "{} value {v} has a space", s.key);
            }
        }
    }

    #[test]
    fn the_groups_partition_the_settings() {
        // A setting in no group is invisible; one in two would render twice.
        let counted: usize = Group::ALL.iter().map(|g| group(*g).len()).sum();
        assert_eq!(counted, SETTINGS.len());
    }

    /// Three from `CUSTOMIZATION.md` — letters, geometry, density — plus the one-handed
    /// anchor, which GTK's `rebuild_view` hardcodes to Right under a comment saying "a
    /// future setting will let the user choose".
    #[test]
    fn the_layout_group_holds_the_doc_three_plus_the_one_handed_anchor() {
        let g = group(Group::Layout);
        assert_eq!(g.len(), 4);
        assert_eq!(g[0].label, "Letters", "declared order kept");
        let keys: Vec<&str> = g.iter().map(|s| s.key).collect();
        for k in ["layout.letters", "layout.geometry", "layout.density", "layout.hand"] {
            assert!(keys.contains(&k), "{k} missing: {keys:?}");
        }
    }

    #[test]
    fn there_are_eight_customisable_colours() {
        // "eight color pickers for the user-customizable palette".
        assert_eq!(COLOR_KEYS.len(), 8);
        let mut k = COLOR_KEYS.to_vec();
        k.sort_unstable();
        k.dedup();
        assert_eq!(k.len(), 8, "and none repeats");
    }

    /// THE FIRST OPTION IS THE DEFAULT, because `next` cycles from there and
    /// `settings_cli::Reset` goes there. Checked against the GTK config's real
    /// defaults (`src/config/mod.rs`), not assumed.
    ///
    /// Four of the six Typing settings needed reordering to satisfy this:
    /// writing them alphabetically or Off-first would have made "restore
    /// defaults" turn swipe typing off.
    #[test]
    fn the_first_option_of_each_typing_setting_is_the_gtk_default() {
        for (key, want) in [
            ("typing.swipe", "on"),            // config: swipe_typing: true
            ("typing.swipe_speed", "normal"),  // config: "normal".to_string()
            ("typing.lenient_swipe", "off"),   // config: lenient_swipe: false
            ("typing.auto_space", "on"),       // config: swipe_auto_space: true
            ("typing.auto_cap", "off"),        // config: auto_capitalize: false
            ("typing.auto_hide", "on"),        // config: auto_hide_on_no_focus: Some(true)
            ("colors.glass", "off"),           // config: glass_effect: false
        ] {
            assert_eq!(find(key).values[0], want, "{key} resets to the wrong value");
        }
    }

    /// The help lines are the part worth carrying exactly -- they are where
    /// each choice is explained, and a paraphrase loses the distinction.
    #[test]
    fn the_swipe_speed_help_explains_what_the_setting_does() {
        let h = find("typing.swipe_speed").help;
        assert!(h.contains("pause"), "{h}");
        assert!(h.contains("near-stops"), "verbatim from the GTK panel: {h}");
    }

    /// Every group has at least one setting. Colors did not until Glass
    /// effect was ported -- the group existed as a tab with nothing in it,
    /// because the rest of the GTK Colors tab is hex pickers and a hex field
    /// is not an option list. An empty tab is a bug in the table.
    #[test]
    fn no_group_is_empty() {
        for g in Group::ALL {
            assert!(!group(g).is_empty(), "{:?} has no settings", g);
        }
    }
}
