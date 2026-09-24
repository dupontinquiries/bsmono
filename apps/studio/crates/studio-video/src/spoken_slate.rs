//! spoken_slate — reading the board from what was SAID, not what was written.
//!
//! [`crate::slate_ocr`] reads the writing on the clapperboard. On most small
//! sets the same information is also spoken aloud, every take: "Cámara
//! grabando. Escena 12, plano 1, toma 2. ¡Acción!" A transcript of the camera's
//! scratch audio therefore already names the take, and the OCR reader is not
//! finished. This module turns that sentence into the same
//! [`slate_ocr::Board`], so [`slate_ocr::arrange`] and [`slate_ocr::by_setup`]
//! work unchanged whichever reader filled it in.
//!
//! ## What the recogniser actually hears
//!
//! Written against the transcripts of "A Fragile Thing" (whisper large-v3-turbo,
//! a bilingual Spanish/English crew). The slate is said in either language or
//! both, and misheard in ways that repeat:
//!
//! | said | transcribed as |
//! |---|---|
//! | escena | escena, scene, scena, scenna, sing, "c" |
//! | plano | plano, plan, plane |
//! | toma | toma, take |
//! | catorce / once / tres | catorce, once, tres — and "three", "3" |
//! | claqueta | claqueta, plaqueta, planeta, croqueta, braqueta |
//!
//! So keywords match a set of spellings, number WORDS in both languages are
//! numbers, and a misheard keyword lowers [`Board::sure`] instead of being
//! trusted like the real word.
//!
//! ## A number that disagrees with both neighbours is probably misheard
//!
//! "Scenna 3, in plan 1, take 1" was said "Escena 13" — the takes on either
//! side of it in camera order are scene 13, and nothing called scene 3 was
//! shot that night. A camera writes files in shooting order, and a scene is
//! shot as a run of takes, so a lone scene number inside a run of another
//! one is the recogniser's error far more often than the crew's.
//! [`suspicious`] finds those; it does not correct them, for the reason
//! `slate_ocr::MIN_SURE` gives — a take filed under the wrong scene is found
//! at the end of the edit — so the editor is asked instead.
//!
//! ## Whisper invents text over silence
//!
//! A take with no dialogue still gets a transcript: "Thank you.", subtitle
//! credits ("Sous-titrage ST' 501"), or one phrase repeated a dozen times.
//! [`hallucination`] says which, so a transcript search, a take list or a
//! language model reading the transcripts does not treat it as speech.

use crate::slate_ocr::Board;

/// Spellings that mean "scene", and how much each is worth.
const SCENE: &[(&str, f32)] = &[
    ("escena", 1.0), ("scene", 1.0), ("scena", 0.8), ("scenna", 0.7), ("scenario", 0.6),
    ("sing", 0.6), ("sin", 0.5), ("c", 0.5), ("cena", 0.6), ("scenes", 0.8),
];
const SHOT: &[(&str, f32)] = &[("plano", 1.0), ("plan", 0.8), ("plane", 0.7), ("planos", 0.8)];
const ROLL: &[(&str, f32)] = &[("roll", 1.0), ("rollo", 1.0), ("row", 0.6)];
const TAKE: &[(&str, f32)] = &[("toma", 1.0), ("take", 1.0), ("tome", 0.7), ("tomas", 0.8)];

fn number_word(w: &str) -> Option<u32> {
    const EN: &[&str] = &["zero", "one", "two", "three", "four", "five", "six", "seven", "eight",
        "nine", "ten", "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen",
        "seventeen", "eighteen", "nineteen", "twenty"];
    const ES: &[&str] = &["cero", "uno", "dos", "tres", "cuatro", "cinco", "seis", "siete",
        "ocho", "nueve", "diez", "once", "doce", "trece", "catorce", "quince", "dieciseis",
        "diecisiete", "dieciocho", "diecinueve", "veinte"];
    if let Ok(n) = w.parse::<u32>() { return Some(n); }
    if w == "una" || w == "on" { return Some(1); }  // "Plane on. Take on." = one
    EN.iter().position(|x| *x == w).or_else(|| ES.iter().position(|x| *x == w)).map(|n| n as u32)
}

/// Lowercase words with accents folded and punctuation dropped, except a
/// dot inside a number ("2.1" is a scene).
fn words(text: &str) -> Vec<String> {
    let fold = |c: char| match c {
        'á' | 'à' => 'a', 'é' | 'è' => 'e', 'í' => 'i', 'ó' | 'ò' => 'o', 'ú' | 'ü' => 'u',
        'ñ' => 'n', c => c,
    };
    text.split(|c: char| c.is_whitespace() || matches!(c, ',' | '!' | '?' | '¡' | '¿' | ';' | ':' | '-' | '"'))
        .map(|w| w.to_lowercase().chars().map(fold).collect::<String>())
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .filter(|w| !w.is_empty())
        .collect()
}

fn lookup(w: &str, table: &[(&str, f32)]) -> Option<f32> {
    table.iter().find(|(k, _)| *k == w).map(|(_, s)| *s)
}

/// A setup letter for a numeric shot: plano 1 is A, plano 2 is B. Boards
/// name setups by letter, and `Board::label` concatenates scene and shot —
/// scene 12 plano 2 written "122" would read as scene 122.
pub fn shot_letter(n: u32) -> Option<String> {
    (1..=26).contains(&n).then(|| ((b'A' + (n - 1) as u8) as char).to_string())
}

/// How many words from the head of a transcript are searched. The slate is
/// the first thing said; searching further finds a scene number in dialogue.
pub const HEAD_WORDS: usize = 40;

/// Read the slate out of the head of a transcript. `None` when no scene
/// keyword followed by a number (or "extra …") is there.
pub fn read(transcript: &str) -> Option<Board> {
    let w: Vec<String> = words(transcript).into_iter().take(HEAD_WORDS).collect();
    let mut b = Board::default();
    let mut sures: Vec<f32> = Vec::new();
    let mut i = 0;
    while i < w.len() {
        let next_num = |j: usize| w.get(j).and_then(|x| number_word(x));
        if b.scene.is_none() {
            // "C2" — a scene word fused to its number
            if let Some(rest) = w[i].strip_prefix('c').filter(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_digit())) {
                b.scene = Some(rest.to_string());
                sures.push(0.5);
                i += 1;
                continue;
            }
            if let Some(s) = lookup(&w[i], SCENE) {
                if let Some(tok) = w.get(i + 1) {
                    if tok.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)
                        && tok.chars().all(|c| c.is_ascii_digit() || c == '.') {
                        b.scene = Some(tok.trim_end_matches('.').to_string());
                        sures.push(s);
                        i += 2;
                        continue;
                    }
                    if let Some(n) = number_word(tok) {
                        b.scene = Some(n.to_string());
                        sures.push(s * 0.9);  // a word is one more thing to mishear
                        i += 2;
                        continue;
                    }
                    if tok == "extra" {
                        // "escena extra, romance" / "escena extra de romance"
                        let skip = if w.get(i + 2).map(|x| x == "de").unwrap_or(false) { 1 } else { 0 };
                        let what = w.get(i + 2 + skip).filter(|x| number_word(x).is_none()
                            && lookup(x, SHOT).is_none() && lookup(x, TAKE).is_none());
                        b.scene = Some(match what {
                            Some(x) => format!("EXTRA {}", x.to_uppercase()),
                            None => "EXTRA".into(),
                        });
                        sures.push(s);
                        i += 2 + skip + what.is_some() as usize;
                        continue;
                    }
                }
            }
        } else {
            if b.shot.is_none() {
                if let (Some(s), Some(n)) = (lookup(&w[i], SHOT), next_num(i + 1)) {
                    b.shot = shot_letter(n);
                    b.other.push(format!("plano {n}"));
                    sures.push(s);
                    i += 2;
                    continue;
                }
            }
            if b.roll.is_none() {
                if let (Some(s), Some(n)) = (lookup(&w[i], ROLL), next_num(i + 1)) {
                    b.roll = Some(n.to_string());
                    sures.push(s);
                    i += 2;
                    continue;
                }
            }
            if b.take.is_none() {
                if let (Some(s), Some(n)) = (lookup(&w[i], TAKE), next_num(i + 1)) {
                    b.take = Some(n);
                    sures.push(s);
                    i += 2;
                    continue;
                }
            }
            if b.take.is_some() && (b.shot.is_some() || b.roll.is_some()) { break; }
        }
        i += 1;
    }
    b.scene.as_ref()?;
    b.sure = sures.iter().sum::<f32>() / sures.len() as f32;
    Some(b)
}

/// A take whose scene disagrees with both neighbours in camera order while
/// the neighbours agree with each other.
#[derive(Debug, Clone, PartialEq)]
pub struct Suspect {
    /// Index into the list given to [`suspicious`].
    pub index: usize,
    pub heard: String,
    /// What the neighbours say it probably is.
    pub likely: String,
}

/// Find lone scene numbers inside a run of another scene. `boards` must be
/// in camera (file) order; unread takes are `None` and are skipped over, so
/// a take between two unread ones is compared with the nearest read ones.
pub fn suspicious(boards: &[Option<Board>]) -> Vec<Suspect> {
    let read: Vec<(usize, &str)> = boards.iter().enumerate()
        .filter_map(|(i, b)| b.as_ref().and_then(|b| b.scene.as_deref()).map(|s| (i, s)))
        .collect();
    read.windows(3).filter_map(|w| {
        let (p, (i, s), n) = (w[0].1, w[1], w[2].1);
        (p == n && s != p).then(|| Suspect { index: i, heard: s.to_string(), likely: p.to_string() })
    }).collect()
}

/// Why a transcript is probably not speech.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hallucination {
    /// A phrase whisper emits over silence ("Thank you.", subtitle credits).
    StockPhrase,
    /// One short phrase repeated back to back, over and over.
    Loop,
}

/// Say whether a transcript (or one segment of it) is whisper talking to
/// itself rather than someone talking. A loop is reported when one repeated
/// phrase covers at least half the words — on the film, a 310-word audition
/// transcript was 20 copies of "I don't know if I can do something", and
/// the flag is right to fire on it even though real dialogue surrounds it.
pub fn hallucination(text: &str) -> Option<Hallucination> {
    let t = text.trim().to_lowercase();
    let stripped: String = t.chars().filter(|c| c.is_alphanumeric() || c.is_whitespace()).collect();
    let stripped = stripped.split_whitespace().collect::<Vec<_>>().join(" ");
    const STOCK: &[&str] = &["thank you", "thanks for watching", "thank you so much",
        "please subscribe", "you", "bye"];
    if STOCK.contains(&stripped.as_str()) || t.contains("sous-titrage") || t.contains("subtitles by")
        || t.contains("amara.org") {
        return Some(Hallucination::StockPhrase);
    }
    // A loop: some phrase of 1..=8 words repeated at least 4 times in a row,
    // covering most of the text.
    let w: Vec<&str> = stripped.split_whitespace().collect();
    for len in 1..=8usize {
        if w.len() < len * 4 { break; }
        let mut i = 0;
        while i + len * 4 <= w.len() {
            let mut reps = 1;
            while i + (reps + 1) * len <= w.len() && w[i + reps * len..i + (reps + 1) * len] == w[i..i + len] {
                reps += 1;
            }
            if reps >= 4 && reps * len * 2 >= w.len() {
                return Some(Hallucination::Loop);
            }
            i += 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(s: &str) -> String { read(s).map(|b| b.label()).unwrap_or_else(|| "none".into()) }

    // Every line below is a real transcript head from the film's camera files.

    #[test]
    fn spanish_english_and_mixed_slates_read_the_same() {
        assert_eq!(label("Cámara grabando. Escena 12, toma 1, plano 1. Action."), "12A-1");
        assert_eq!(label("Camera recording. Scene 12. Plano 2. Toma 1. I'm not really"), "12B-1");
        assert_eq!(label("Camera grabando. Escena catorce. Plano uno. Toma uno. Action. Hi mom."), "14A-1");
        assert_eq!(label("- Camera grabando. - Escena once. Plano uno. Toma uno. - Action."), "11A-1");
        assert_eq!(label("Cámara grabando! Escena 1, Plano 3, toma 2! Action!"), "1C-2");
    }

    #[test]
    fn misheard_keywords_still_read_but_count_for_less() {
        let clean = read("Cámara grabando. Escena 13, plano 1, toma 1. Action.").unwrap();
        let misheard = read("Camera recording? Scenna 3, in plan 1, take 1. You know what?").unwrap();
        assert_eq!(misheard.label(), "3A-1");  // read as heard — see `suspicious`
        assert!(misheard.sure < clean.sure, "{} vs {}", misheard.sure, clean.sure);
        assert!(clean.sure >= crate::slate_ocr::MIN_SURE);
    }

    #[test]
    fn roll_is_kept_as_roll_and_fused_scene_words_read() {
        let b = read("The camera is recording. Give me a clap. C2, roll one, take two. Action! Cut.").unwrap();
        assert_eq!((b.scene.as_deref(), b.roll.as_deref(), b.take), (Some("2"), Some("1"), Some(2)));
        let b = read("Cámara grabando, dame la claqueta. Scene 2.1, roll 2, take 1. Action!").unwrap();
        assert_eq!((b.scene.as_deref(), b.roll.as_deref(), b.take), (Some("2.1"), Some("2"), Some(1)));
        // "Scene 7, take 1, roll 4": order of the fields does not matter
        let b = read("Cámara grabando. Plaqueta, cuéntala. Scene 7, take 1, roll 4. Action!").unwrap();
        assert_eq!((b.scene.as_deref(), b.roll.as_deref(), b.take), (Some("7"), Some("4"), Some(1)));
    }

    #[test]
    fn extra_scenes_and_unslated_takes() {
        let b = read("Cámara grabando. Escena extra, romance. Plano 2, toma 1. Action.").unwrap();
        assert_eq!(b.scene.as_deref(), Some("EXTRA ROMANCE"));
        assert_eq!((b.shot.as_deref(), b.take), (Some("B"), Some(1)));
        let b = read("Cámara grabando. Escena extra de Romance, Plano 2, Toma 2. Action.").unwrap();
        assert_eq!(b.scene.as_deref(), Some("EXTRA ROMANCE"));
        assert!(read("Cámara grabando. Claqueta. Action. Action.").is_none());
        assert!(read("I'm recording now the double. Canta the plaqueta. Action.").is_none());
        // a number in the dialogue far past the head is not a slate
        let late = format!("{} scene 4 take 2", "we talked for a while ".repeat(12));
        assert!(read(&late).is_none());
    }

    #[test]
    fn a_lone_scene_inside_a_run_is_flagged_not_changed() {
        let heads = [
            "Camera recording, claqueta. Scena 13, plano 1, take 1. Action.",
            "Camera recording? Scenna 3, in plan 1, take 1. You know what?",
            "Cámara grabando. Claqueta. Scene 13. Plano 1. Take 2. Action!",
            "Camera's recording. Scene 13. Plano 2. Take 1. Action.",
            "Cámara grabando, claqueta. Escena 3, plano 3, toma 1. Action.",
            "camera recording, claqueta escena 13, plano 3, toma 2 action",
        ];
        let boards: Vec<Option<Board>> = heads.iter().map(|h| read(h)).collect();
        let s = suspicious(&boards);
        assert_eq!(s.iter().map(|x| x.index).collect::<Vec<_>>(), vec![1, 4], "{s:?}");
        assert!(s.iter().all(|x| x.heard == "3" && x.likely == "13"));
        // flagging never edits the board
        assert_eq!(boards[1].as_ref().unwrap().scene.as_deref(), Some("3"));
        // a real scene change (a run of each) is not suspicious
        let runs: Vec<Option<Board>> = ["Escena 1, plano 1, toma 1", "Escena 1, plano 1, toma 2",
            "Escena 2, plano 1, toma 1", "Escena 2, plano 1, toma 2"].iter().map(|h| read(h)).collect();
        assert!(suspicious(&runs).is_empty());
    }

    #[test]
    fn hallucinations_are_named_and_speech_is_not() {
        assert_eq!(hallucination("Thank you."), Some(Hallucination::StockPhrase));
        assert_eq!(hallucination("Sous-titrage ST' 501"), Some(Hallucination::StockPhrase));
        assert_eq!(hallucination(&"Cámara grabando, dame la claqueta. ".repeat(12)),
                   Some(Hallucination::Loop));
        assert_eq!(hallucination(&"I'm not sure if you're going to go. ".repeat(9)),
                   Some(Hallucination::Loop));
        assert_eq!(hallucination("The sound of the sound of the sound of the sound of the sound of the sound."),
                   Some(Hallucination::Loop));
        // real dialogue, including a short repeated exclamation inside it
        assert_eq!(hallucination("Hi Max, how are you doing? Are you taking care of yourself? \
            Are you eating? Yes, I am. Yes, I am. Listen, I'm very excited."), None);
        assert_eq!(hallucination("Stop! Stop! You left us alone in this, alright? It was your fault!"), None);
    }
}
