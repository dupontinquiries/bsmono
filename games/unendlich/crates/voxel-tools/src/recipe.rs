//! The recognizer: a sentence to a form line, without a model.
//!
//! "A giant stone statue of Zeus holding a lightning bolt in a lunge
//! combat position" is one sentence with six decisions in it: the
//! FORM (statue), its SCALE (giant -> colossus), its POSE (lunge), a
//! PROP (a bolt, in the raised hand), a MATERIAL (stone), and WHO it
//! is (Zeus: bearded, and the plan's name). A 1B answered it with a
//! tower and a tree. Not because it lacked the words -- the words were
//! all right there -- but because turning them into coordinates is not
//! what it can do.
//!
//! So the words are read HERE. Every noun the dictionary has a form
//! for, every modifier a form takes, every material the palette
//! knows: a table, matched by whole word, producing the line the
//! dictionary needs. The model's reply is still used -- its lines join
//! the plan after -- but the shape the player asked for is guaranteed
//! by the recognizer, and the finishing words and the tuner do the
//! rest. For a 1B this is the difference between a pillar and a
//! statue; for a 70B it is a floor under a bad day.
//!
//! Deliberately not clever: no parsing, no grammar, whole-word lookup
//! in tables, first match wins, deterministic. If it recognises
//! nothing it says so and the model's plan stands alone.

use crate::dict::FORMS;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Recipe {
    pub form: String,
    pub style: Option<String>,
    pub size: String,
    pub block: Option<String>,
    pub kv: Vec<(String, String)>,
    pub name: Option<String>,
    /// The words that decided it, for the echo.
    pub because: Vec<String>,
}

impl Recipe {
    /// The IR line the dictionary needs.
    pub fn line(&self) -> String {
        let mut s = format!("form {} size={}", self.form, self.size);
        if let Some(st) = &self.style { s.push_str(&format!(" style={st}")); }
        if let Some(b) = &self.block { s.push_str(&format!(" block={b}")); }
        for (k, v) in &self.kv { s.push_str(&format!(" {k}={v}")); }
        s
    }
}

/// Nouns that name a form, with the form they name. Plurals and near
/// synonyms included; whole-word matched.
const NOUNS: &[(&[&str], &str)] = &[
    (&["statue", "statues", "figure", "monument", "effigy", "idol", "colossus", "gargoyle", "obelisk", "totem", "monolith"], "statue"),
    (&["house", "houses", "cottage", "cabin", "hut", "home", "dwelling", "lodge", "longhouse", "inn", "tavern"], "house"),
    (&["village", "hamlet", "settlement", "town"], "village"),
    (&["tower", "towers", "keep", "turret", "spire", "watchtower", "lighthouse"], "tower"),
    (&["castle", "fortress", "fort", "stronghold", "citadel"], "castle"),
    (&["temple", "shrine", "chapel", "church", "cathedral", "sanctuary"], "temple"),
    (&["colonnade", "columns", "pillars", "portico"], "colonnade"),
    (&["arch", "archway", "gate", "gateway"], "arch"),
    (&["nest", "lair", "den", "burrow"], "nest"),
    (&["cavern", "cave", "grotto", "chamber"], "cavern"),
    (&["wall", "walls", "rampart", "palisade"], "wall"),
];

/// Modifiers a form takes, as k=v.
const MODIFIERS: &[(&[&str], &str, &str)] = &[
    // statue poses
    (&["lunge", "lunging", "combat", "fighting", "attacking", "striking", "throwing", "hurling"], "pose", "lunge"),
    (&["seated", "sitting", "enthroned", "throne"], "pose", "seated"),
    (&["kneeling", "crouching", "crouched"], "pose", "kneeling"),
    (&["raised", "triumphant", "saluting", "arms_up", "victorious"], "pose", "arms_up"),
    // props
    (&["bolt", "lightning", "thunderbolt"], "hold", "bolt"),
    (&["sword", "blade"], "hold", "sword"),
    (&["staff", "spear", "trident", "sceptre", "scepter"], "hold", "staff"),
    (&["orb", "globe"], "hold", "orb"),
    (&["shield"], "hold", "shield"),
    // village
    (&["big", "large"], "count", "9"),
];

/// Style words to style names (the style table's own detector covers
/// most; these are the extra ones that only a form-level reading sees).
const STATUE_STYLES: &[(&[&str], &str)] = &[
    (&["giant", "colossal", "huge", "enormous", "towering", "gigantic", "massive"], "colossus"),
    (&["gargoyle", "gargoyles", "demon", "winged"], "gargoyle"),
    (&["idol", "totem"], "idol"),
    (&["obelisk", "needle", "monolith"], "obelisk"),
];

const SIZES: &[(&[&str], &str)] = &[
    (&["tiny", "little"], "tiny"),
    (&["small", "modest"], "small"),
    (&["large", "big", "grand", "great"], "large"),
    (&["huge", "giant", "colossal", "enormous", "vast", "sprawling", "gigantic", "massive"], "huge"),
];

const MATERIALS: &[(&[&str], &str)] = &[
    (&["stone"], "stone"), (&["marble"], "marble"), (&["brick", "bricks"], "stone_bricks"),
    (&["cobble", "cobblestone"], "cobblestone"), (&["sandstone"], "sandstone"),
    (&["wood", "wooden", "timber"], "oak_planks"), (&["oak"], "oak_planks"),
    (&["ice"], "ice"), (&["glass"], "glass"), (&["gold", "golden"], "gold_block"),
    (&["obsidian", "black"], "obsidian"), (&["snow"], "snow"),
];

/// Figures with a known look.
const FIGURES: &[(&str, &[(&str, &str)])] = &[
    ("zeus", &[("beard", "1"), ("hold", "bolt")]),
    ("poseidon", &[("beard", "1"), ("hold", "staff")]),
    ("odin", &[("beard", "1"), ("hold", "staff")]),
    ("thor", &[("beard", "1"), ("hold", "sword")]),
    ("athena", &[("hold", "shield")]),
    ("king", &[("beard", "1")]),
    ("knight", &[("hold", "sword")]),
    ("warrior", &[("hold", "sword")]),
    ("wizard", &[("beard", "1"), ("hold", "staff")]),
];

pub fn recognise(prose: &str) -> Option<Recipe> {
    let p = prose.to_lowercase();
    let words: Vec<&str> = p.split(|c: char| !c.is_alphanumeric() && c != '_').filter(|w| !w.is_empty()).collect();
    let has = |w: &str| words.contains(&w);
    let first = |table: &[(&[&str], &str)]| -> Option<(String, String)> {
        for (ws, v) in table { if let Some(w) = ws.iter().find(|w| has(w)) { return Some((v.to_string(), w.to_string())); } }
        None
    };
    // A form noun; or a figure alone ("a seated king in marble"), which
    // is a statue of that figure.
    let (form, noun) = match first(NOUNS) {
        Some(x) => x,
        None => { let who = FIGURES.iter().find(|(w, _)| has(w))?; ("statue".to_string(), who.0.to_string()) }
    };
    if FORMS.iter().all(|f| f.name != form) { return None; }
    let mut r = Recipe { form: form.clone(), size: "medium".into(), ..Default::default() };
    r.because.push(noun);
    if let Some((sz, w)) = first(SIZES) { r.size = sz; r.because.push(w); }
    if form == "statue" {
        if let Some((st, w)) = first(STATUE_STYLES) { r.style = Some(st); r.because.push(w); }
        // a giant statue is a colossus of medium footprint, not a
        // medium statue on a huge plinth
        if r.style.as_deref() == Some("colossus") { r.size = "medium".into(); }
    } else if let Some(st) = crate::style::from_prose(prose) {
        r.style = Some(st.name.to_string()); r.because.push(st.name.to_string());
    }
    if let Some((b, w)) = first(MATERIALS) { r.block = Some(b); r.because.push(w); }
    for (ws, k, v) in MODIFIERS {
        if let Some(w) = ws.iter().find(|w| has(w)) {
            if (k == &"pose" || k == &"hold") && form != "statue" { continue; }
            if k == &"count" && form != "village" { continue; }
            if !r.kv.iter().any(|(kk, _)| kk == k) { r.kv.push((k.to_string(), v.to_string())); r.because.push(w.to_string()); }
        }
    }
    if form == "statue" {
        for (who, traits) in FIGURES {
            if has(who) {
                r.name = Some(who.to_string());
                for (k, v) in *traits {
                    if !r.kv.iter().any(|(kk, _)| kk == k) { r.kv.push((k.to_string(), v.to_string())); }
                }
                r.because.push(who.to_string());
                break;
            }
        }
    }
    r.because.dedup();
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_zeus_sentence_becomes_a_lunging_bearded_colossus_with_a_bolt() {
        let r = recognise("a giant stone statue of Zeus holding a lightning bolt in a lunge combat position").unwrap();
        assert_eq!(r.form, "statue");
        assert_eq!(r.style.as_deref(), Some("colossus"));
        assert_eq!(r.block.as_deref(), Some("stone"));
        assert!(r.kv.contains(&("pose".into(), "lunge".into())), "{:?}", r.kv);
        assert!(r.kv.contains(&("hold".into(), "bolt".into())), "{:?}", r.kv);
        assert!(r.kv.contains(&("beard".into(), "1".into())), "{:?}", r.kv);
        assert_eq!(r.name.as_deref(), Some("zeus"));
        let line = r.line();
        assert!(line.starts_with("form statue size=medium style=colossus block=stone"), "{line}");
        let (plan, issues) = crate::plan::parse_plan_at(&format!("{line}\n"), Some((0, 65, 0)));
        assert!(issues.iter().all(|i| i.severity != crate::plan::Severity::Error), "{issues:?}");
        assert_eq!(plan.ops.len(), 1);
        assert!(crate::plan::line_is_valid_ir(&line));
    }

    #[test]
    fn other_sentences_land_on_their_forms() {
        assert_eq!(recognise("a cosy little cottage").unwrap().line(), "form house size=tiny style=cottage");
        let v = recognise("a big viking village").unwrap();
        assert_eq!(v.form, "village");
        assert_eq!(v.style.as_deref(), Some("nordic"));
        assert!(v.kv.contains(&("count".into(), "9".into())));
        assert_eq!(recognise("a huge sandstone castle").unwrap().line(), "form castle size=huge style=desert block=sandstone");
        assert_eq!(recognise("a gargoyle").unwrap().style.as_deref(), Some("gargoyle"));
        assert!(recognise("make it taller").is_none(), "no noun, nothing to recognise");
        assert!(recognise("a road to the river").is_none(), "roads are ops, not forms");
    }

    #[test]
    fn pose_and_prop_words_only_apply_to_statues() {
        let h = recognise("a house with a sword above the door").unwrap();
        assert!(h.kv.is_empty(), "{:?}", h.kv);
        let s = recognise("a statue with a sword").unwrap();
        assert!(s.kv.contains(&("hold".into(), "sword".into())));
    }

    #[test]
    fn every_recognised_line_is_valid_ir() {
        for prose in ["a statue", "a giant statue of a wizard", "a seated king in marble", "a stone tower",
                      "a temple", "a small nordic house", "an ice castle", "a lair", "a cave", "a golden idol",
                      "a knight statue holding a shield", "an obelisk"] {
            let r = recognise(prose).expect(prose);
            assert!(crate::plan::line_is_valid_ir(&r.line()), "{prose} -> {}", r.line());
        }
    }
}
