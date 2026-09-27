//! geartooth -- the tongue of the Geartooth clan, the engineer-cats who
//! built the Quarry's machine and the Sentinels. The Sentinels still speak
//! it (Oremantle's lines are in it), and the engineers carved it on the
//! Quarry's walls: [`TABLETS`], placed by `mazeworld` and read out by the
//! HUD when a cat stands at one. Each tablet teaches one piece of it, so a
//! player who reads a few can follow what the Sentinel says.
//!
//! **Its sounds.** Nine vowels -- a e i o u, and three the clans do not
//! have: ä (the "a" of "cat"), ö and ü (rounded lips on "e" and "i"), and
//! y (a vowel said with the tongue pulled back, between "i" and "u") --
//! and two CLICKS, made with the tongue and no breath: `!`, a pop off the
//! roof of the mouth (the sound of a gear tooth dropping into place), and
//! `|`, a sharp tsk behind the teeth (a ratchet's catch). The consonants
//! are few: t k d, th, r (rolled), l m n, s h v.
//!
//! **Its grammar** (what the tablets teach):
//!
//! 1. The verb comes LAST: `tyr kör` -- the machine turns.
//! 2. Many is said by DOUBLING the first syllable: `!ük` a gear, `!ü!ük`
//!    gears.
//! 3. When is said by a CLICK before the verb: `!` for what was, `|` for
//!    what will be, nothing for now: `kör` turns, `!kör` turned, `|kör`
//!    will turn.
//! 4. Not is `-m` on the end (`-em` after a consonant): `|ö` kin, `|öm` a
//!    stranger; `tam` shuts, `tamem` does not shut.
//! 5. Whose is `-a` on the owner: `tharu-a !ön` -- the mountain's heart.
//!
//! The scripts in `voicescript` (Oremantle's part) spell it as here;
//! `dev/voice_script/geartooth_voice.py` speaks it (Kokoro cannot: no
//! clicks, none of the vowels).

/// A word: the Geartooth, and what it means.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Word {
    pub gt: &'static str,
    pub means: &'static str,
}

const fn w(gt: &'static str, means: &'static str) -> Word {
    Word { gt, means }
}

/// Every word the Quarry's tablets and the Sentinel use (roots only: the
/// grammar builds the rest).
pub const LEXICON: &[Word] = &[
    w("tyr", "the machine"),
    w("!ük", "a gear; a tooth"),
    w("doru", "stone"),
    w("yla", "water"),
    w("tharu", "mountain"),
    w("!ön", "heart"),
    w("|ö", "kin; our own"),
    w("mi", "cat; little one"),
    w("näk", "a lock (a room the machine works)"),
    w("räsk", "floor; ground"),
    w("kis", "thorn"),
    w("kör", "to turn"),
    w("ruu", "to roll"),
    w("luu", "to go down; to fall; to rest"),
    w("tam", "to shut"),
    w("hä", "to open"),
    w("vel", "to watch; to keep"),
    w("dän", "to build"),
    w("hy", "to be here; here"),
    w("ol", "always"),
    w("ra", "every"),
    w("vi", "you"),
    w("aa", "yes; it is so"),
];

/// A carved tablet on the Quarry's walls: the inscription, the clans'
/// reading of it, and what it shows of the tongue.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tablet {
    pub gt: &'static str,
    pub reading: &'static str,
    pub teaches: &'static str,
}

const fn t(gt: &'static str, reading: &'static str, teaches: &'static str) -> Tablet {
    Tablet { gt, reading, teaches }
}

/// The tablets, in the order a cat walking in from its base tends to meet
/// them (the first ones teach what the later ones use).
pub const TABLETS: &[Tablet] = &[
    t("tyr kör. tyr ol kör.", "The machine turns. The machine always turns.", "Their verb comes last."),
    t("!ü!ük kör.", "The gears turn.", "Many: the first sound said twice (!ük, a gear)."),
    t("doru !ruu. doru |ruu.", "The stone rolled. The stone will roll.", "A click before the verb: ! for what was, | for what will be."),
    t("näk tam: räsk luu.", "A lock shuts: the floor goes down.", "Where the machine works, the floor opens."),
    t("yla doru |ruu.", "Water will roll the stone.", "Who does it, what to, then the verb."),
    t("|ö hy. |öm hy vel.", "Kin are here. Strangers here are watched.", "Not: -m on the end (|ö, kin)."),
    t("ra !ük |ö.", "Every gear is kin.", "They built the Sentinels: the machine knows its own."),
    t("tharu-a !ön.", "The mountain's heart.", "Whose: -a on the owner."),
    t("kis !dänem.", "The thorns were not built.", "The thorns came after them."),
    t("vi |kör. aa.", "You will turn. Yes.", "The last tablet: everything the machine moves, it moves again."),
];

/// Every word in `text` (clicks kept, punctuation and case dropped), and
/// each reduced to its root as the grammar builds words: a tense click off
/// a verb, a doubled first syllable, a -m / -em, a -a owner.
pub fn roots(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in text.split(|c: char| c.is_whitespace() || c == '.' || c == ',' || c == ':' || c == '?' || c == '…') {
        let word: String = raw.chars().filter(|c| c.is_alphabetic() || *c == '!' || *c == '|' || *c == '-').flat_map(|c| c.to_lowercase()).collect();
        // (a click is said before a vowel: one at the END of a word is an
        // exclamation mark, not a click)
        let word = word.trim_end_matches(['!', '|']).trim_matches('-').to_string();
        if word.is_empty() {
            continue;
        }
        for part in word.split('-') {
            if part.is_empty() || part == "a" {
                continue;
            }
            out.push(root_of(part));
        }
    }
    out
}

fn known(w: &str) -> bool {
    LEXICON.iter().any(|x| x.gt == w)
}

fn root_of(w: &str) -> String {
    if known(w) {
        return w.to_string();
    }
    let mut cands: Vec<String> = vec![w.to_string()];
    // a tense click on a verb
    if let Some(r) = w.strip_prefix('!').or_else(|| w.strip_prefix('|')) {
        cands.push(r.to_string());
    }
    let mut more = Vec::new();
    for c in &cands {
        // not: -em, -m
        if let Some(r) = c.strip_suffix("em") {
            more.push(r.to_string());
        }
        if let Some(r) = c.strip_suffix('m') {
            more.push(r.to_string());
        }
        // many: a doubled first syllable (!ü!ük -> !ük)
        let cs: Vec<char> = c.chars().collect();
        for k in 1..cs.len() {
            let head: String = cs[..k].iter().collect();
            let rest: String = cs[k..].iter().collect();
            if rest.starts_with(&head) && known(&rest) {
                more.push(rest);
            }
        }
    }
    cands.extend(more);
    cands.into_iter().find(|c| known(c)).unwrap_or_else(|| w.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every word on every tablet, and in every Sentinel line, is a
    /// lexicon word or one the grammar builds from one -- nothing made up
    /// on the wall that the tongue cannot say.
    #[test]
    fn every_word_is_the_tongues() {
        let mut n = 0;
        for tb in TABLETS {
            for r in roots(tb.gt) {
                assert!(known(&r), "tablet '{}': '{r}' is not a word of the tongue", tb.gt);
                n += 1;
            }
        }
        let ore = crate::voicescript::part("oremantle").unwrap();
        for l in ore.lines {
            for r in roots(l.text) {
                assert!(known(&r), "Oremantle's {}: '{r}' is not a word of the tongue", l.key);
                n += 1;
            }
        }
        assert!(n > 40, "only {n} words checked");
    }

    /// The grammar's builders reduce to their roots.
    #[test]
    fn the_grammar_builds_from_roots() {
        assert_eq!(roots("!ü!ük"), vec!["!ük"]);
        assert_eq!(roots("|kör !ruu"), vec!["kör", "ruu"]);
        assert_eq!(roots("|öm tamem"), vec!["|ö", "tam"]);
        assert_eq!(roots("tharu-a !ön"), vec!["tharu", "!ön"]);
        assert_eq!(roots("xyzzy"), vec!["xyzzy"], "a non-word stays itself, to be caught");
        assert_eq!(roots("|Ö, hy! Tharu-a !ÖN!"), vec!["|ö", "hy", "tharu", "!ön"], "a closing ! is punctuation");
    }

    /// The tablets teach it: every rule of the grammar (verb last,
    /// doubling, the two tense clicks, not, whose) appears on some tablet,
    /// and both clicks and all four strange vowels are carved somewhere.
    #[test]
    fn the_tablets_teach_the_tongue() {
        let all: String = TABLETS.iter().map(|t| t.gt).collect::<Vec<_>>().join(" ");
        for (what, needle) in [("doubling", "!ü!ük"), ("past click", "!ruu"), ("future click", "|ruu"), ("not", "|öm"), ("whose", "-a ")] {
            assert!(all.contains(needle), "no tablet shows {what}");
        }
        for c in ['!', '|', 'ä', 'ö', 'ü', 'y'] {
            assert!(all.contains(c), "'{c}' is never carved");
        }
        assert!(TABLETS.len() >= 8);
        for tb in TABLETS {
            assert!(!tb.reading.is_empty() && !tb.teaches.is_empty());
        }
    }
}
