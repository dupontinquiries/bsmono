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
pub enum Group { Layout, Colors, Experimental }

impl Group {
    pub const ALL: [Group; 3] = [Group::Layout, Group::Colors, Group::Experimental];

    pub fn title(self) -> &'static str {
        match self {
            Group::Layout => "Layout",
            Group::Colors => "Colors",
            Group::Experimental => "Experimental",
        }
    }
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
        options: &["QWERTY", "Dvorak"], values: &["qwerty", "dvorak"],
    },
    Setting {
        key: "layout.geometry", label: "Geometry",
        help: "Staggered matches a physical keyboard; ortholinear is a strict grid.",
        group: Group::Layout,
        options: &["Staggered", "Ortholinear"], values: &["staggered", "ortho"],
    },
    Setting {
        key: "layout.density", label: "Density",
        help: "Compact reduces vertical padding for one-handed reach.",
        group: Group::Layout,
        options: &["Normal", "Compact"], values: &["normal", "compact"],
    },
    Setting {
        key: "layout.experimental_mode", label: "Mode",
        help: "Off = normal keyboard. Cardinal = 3×3 swipe grid. \
               Stenotype = 4-button steno (very experimental). \
               Words = 5-col cardinal with prose / C / Rust / Python clusters.",
        group: Group::Experimental,
        options: &["Off", "Cardinal", "Stenotype", "Words"],
        values: &["off", "cardinal", "stenotype", "words"],
    },
    Setting {
        key: "experimental.voice", label: "Voice input",
        help: "Dictate with the mic key.",
        group: Group::Experimental,
        options: &["Off", "On"], values: &["off", "on"],
    },
    Setting {
        key: "experimental.swipe_trail", label: "Swipe trail",
        help: "Draw the path your finger takes while swiping.",
        group: Group::Experimental,
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

    #[test]
    fn the_layout_group_holds_the_three_the_doc_names() {
        assert_eq!(group(Group::Layout).len(), 3);
        assert_eq!(group(Group::Layout)[0].label, "Letters", "declared order kept");
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
}
