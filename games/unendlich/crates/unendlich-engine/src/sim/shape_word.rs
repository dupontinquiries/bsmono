//! `sim::shape_word` — parse a spoken bending phrase into a shape command.
//!
//! A terrain-bending technique is raycast + **voice word** + environment. This is
//! the voice-word half: it turns a recognised phrase like `"wide tall dome"` or
//! `"fast serpent break"` into a [`ShapeCmd`] — one [`Form`], its [`Qualifier`]s,
//! and an optional [`StateMod`] — which the shape generator then consumes with
//! the raycast hit and environment to emit block edits.
//!
//! Grammar (spec §12): a phrase is a bag of words in any order; exactly one must
//! be a form, any number may be qualifiers, and at most one a state modifier.
//! Unrecognised words are ignored here (the recognizer's confirm flow handles
//! genuinely misheard words upstream). `"hollow"` is an alias for the `Deep`
//! qualifier, per the doc.
//!
//! Pure/std-only. Case-insensitive; word order is preserved for qualifiers.

/// A bending shape form — Earth and Water vocabularies unified (some words, like
/// `Dome`/`Wall`/`Wave`/`Coil`, appear in both).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    // Earth
    Rise,
    Ramp,
    Bowl,
    Platform,
    Tunnel,
    Trench,
    Arch,
    Crown,
    Terrace,
    Fortress,
    // Water
    Spout,
    Veil,
    Orb,
    Whip,
    Cage,
    Ring,
    Serpent,
    Funnel,
    Mist,
    Bridge,
    // Shared
    Dome,
    Wall,
    Wave,
    Coil,
}

impl Form {
    fn from_word(w: &str) -> Option<Form> {
        Some(match w {
            "rise" | "pillar" | "column" => Form::Rise,
            "ramp" => Form::Ramp,
            "bowl" => Form::Bowl,
            "platform" => Form::Platform,
            "tunnel" => Form::Tunnel,
            "trench" => Form::Trench,
            "arch" => Form::Arch,
            "crown" => Form::Crown,
            "terrace" => Form::Terrace,
            "fortress" => Form::Fortress,
            "spout" => Form::Spout,
            "veil" => Form::Veil,
            "orb" | "sphere" => Form::Orb,
            "whip" => Form::Whip,
            "cage" => Form::Cage,
            "ring" => Form::Ring,
            "serpent" => Form::Serpent,
            "funnel" => Form::Funnel,
            "mist" => Form::Mist,
            "bridge" => Form::Bridge,
            "dome" => Form::Dome,
            "wall" => Form::Wall,
            "wave" => Form::Wave,
            "coil" => Form::Coil,
            _ => return None,
        })
    }

    /// Which vocabulary the form belongs to (shared forms report `None`).
    pub fn element(self) -> Option<Element> {
        use Form::*;
        Some(match self {
            Rise | Ramp | Bowl | Platform | Tunnel | Trench | Arch | Crown | Terrace | Fortress => Element::Earth,
            Spout | Veil | Orb | Whip | Cage | Ring | Serpent | Funnel | Mist | Bridge => Element::Water,
            Dome | Wall | Wave | Coil => return None,
        })
    }
}

/// The two terrain-bending vocabularies a form can belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Element {
    Earth,
    Water,
}

/// A size/shape qualifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Qualifier {
    Tall,
    Wide,
    Flat,
    Narrow,
    Deep,
    Rough,
    Smooth,
    Thin,
    Fast,
}

impl Qualifier {
    fn from_word(w: &str) -> Option<Qualifier> {
        Some(match w {
            "tall" => Qualifier::Tall,
            "wide" => Qualifier::Wide,
            "flat" => Qualifier::Flat,
            "narrow" => Qualifier::Narrow,
            "deep" | "hollow" => Qualifier::Deep, // 'hollow' aliases to Deep (spec §12)
            "rough" => Qualifier::Rough,
            "smooth" => Qualifier::Smooth,
            "thin" => Qualifier::Thin,
            "fast" => Qualifier::Fast,
            _ => return None,
        })
    }
}

/// The persistence/behaviour state a shape is created in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateMod {
    /// Sustained; does not decay until released.
    Hold,
    /// Animated / continuous motion.
    Flow,
    /// Strikes and shatters on impact.
    Break,
    /// Immediate ice conversion.
    Frozen,
}

impl StateMod {
    fn from_word(w: &str) -> Option<StateMod> {
        Some(match w {
            "hold" => StateMod::Hold,
            "flow" => StateMod::Flow,
            "break" => StateMod::Break,
            "frozen" => StateMod::Frozen,
            _ => return None,
        })
    }
}

/// A parsed bending command: one form, its qualifiers (in spoken order), and an
/// optional state modifier.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeCmd {
    pub form: Form,
    pub qualifiers: Vec<Qualifier>,
    pub state: Option<StateMod>,
}

/// Parse a spoken phrase into a [`ShapeCmd`]. Requires exactly one form word;
/// returns `None` if no form is present (a phrase with no shape can't be a
/// technique). Qualifiers accumulate in spoken order (deduplicated); the last
/// state modifier wins. Unknown words are ignored.
pub fn parse(phrase: &str) -> Option<ShapeCmd> {
    let mut form: Option<Form> = None;
    let mut qualifiers: Vec<Qualifier> = Vec::new();
    let mut state: Option<StateMod> = None;

    for raw in phrase.split_whitespace() {
        let w = raw.to_ascii_lowercase();
        if let Some(f) = Form::from_word(&w) {
            // first form wins; later form words are ignored (single-shape rule)
            if form.is_none() {
                form = Some(f);
            }
        } else if let Some(q) = Qualifier::from_word(&w) {
            if !qualifiers.contains(&q) {
                qualifiers.push(q);
            }
        } else if let Some(s) = StateMod::from_word(&w) {
            state = Some(s); // last state modifier wins
        }
        // else: unknown word → ignored (confirm flow handles real mishears)
    }

    form.map(|form| ShapeCmd { form, qualifiers, state })
}

impl ShapeCmd {
    pub fn has(&self, q: Qualifier) -> bool {
        self.qualifiers.contains(&q)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(p: &str) -> ShapeCmd {
        parse(p).unwrap()
    }

    #[test]
    fn parses_the_spec_examples() {
        assert_eq!(cmd("wide dome"), ShapeCmd { form: Form::Dome, qualifiers: vec![Qualifier::Wide], state: None });
        assert_eq!(
            cmd("wide tall dome"),
            ShapeCmd { form: Form::Dome, qualifiers: vec![Qualifier::Wide, Qualifier::Tall], state: None }
        );
        assert_eq!(
            cmd("fast serpent break"),
            ShapeCmd { form: Form::Serpent, qualifiers: vec![Qualifier::Fast], state: Some(StateMod::Break) }
        );
        assert_eq!(cmd("frozen bridge"), ShapeCmd { form: Form::Bridge, qualifiers: vec![], state: Some(StateMod::Frozen) });
        // 'hollow' aliases to Deep
        assert_eq!(cmd("hollow dome"), ShapeCmd { form: Form::Dome, qualifiers: vec![Qualifier::Deep], state: None });
        assert_eq!(
            cmd("tall spout hold"),
            ShapeCmd { form: Form::Spout, qualifiers: vec![Qualifier::Tall], state: Some(StateMod::Hold) }
        );
    }

    #[test]
    fn word_order_is_free() {
        // qualifiers/state/form in any order resolve to the same command shape
        let a = cmd("break wide tall wave");
        let b = cmd("wave tall wide break");
        assert_eq!(a.form, b.form);
        assert_eq!(a.state, b.state);
        assert!(a.has(Qualifier::Tall) && a.has(Qualifier::Wide));
        assert_eq!(a.form, Form::Wave);
    }

    #[test]
    fn a_phrase_with_no_form_is_not_a_technique() {
        assert!(parse("tall wide hold").is_none());
        assert!(parse("").is_none());
        assert!(parse("umm what").is_none(), "unknown words alone → no shape");
    }

    #[test]
    fn unknown_words_are_ignored_around_a_valid_form() {
        let c = cmd("please make a wide dome now");
        assert_eq!(c.form, Form::Dome);
        assert_eq!(c.qualifiers, vec![Qualifier::Wide]);
    }

    #[test]
    fn duplicates_dedupe_and_last_state_wins() {
        let c = cmd("wide wide dome hold flow");
        assert_eq!(c.qualifiers, vec![Qualifier::Wide], "qualifier deduped");
        assert_eq!(c.state, Some(StateMod::Flow), "last state modifier wins");
    }

    #[test]
    fn case_insensitive_and_form_element_tagging() {
        assert_eq!(cmd("WIDE DOME").form, Form::Dome);
        assert_eq!(Form::Rise.element(), Some(Element::Earth));
        assert_eq!(Form::Serpent.element(), Some(Element::Water));
        assert_eq!(Form::Dome.element(), None, "shared forms aren't element-tagged");
    }

    #[test]
    fn common_synonyms_resolve() {
        assert_eq!(cmd("pillar").form, Form::Rise, "pillar → Rise");
        assert_eq!(cmd("wide sphere hold").form, Form::Orb, "sphere → Orb");
    }
}
