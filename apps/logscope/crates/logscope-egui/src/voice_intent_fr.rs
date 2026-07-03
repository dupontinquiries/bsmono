//! voice_intent_fr.rs — French command vocabulary. Falls back to the English parser for anything
//! unmatched. Pure + unit-tested.

use crate::vimmode::Command;
use crate::voice_intent::VoiceIntent;

pub fn parse(utterance: &str) -> VoiceIntent {
    let original = utterance.trim();
    let t = original.trim_end_matches(['.', '!', '?', ',']).trim().to_lowercase();

    if matches!(t.as_str(), "arrêter la visite" | "arreter la visite" | "fin de la visite" | "atterrir") { return VoiceIntent::Tour(false); }
    if matches!(t.as_str(), "visite" | "démarrer la visite" | "demarrer la visite" | "commencer la visite") { return VoiceIntent::Tour(true); }
    if matches!(t.as_str(), "quitter la caméra libre" | "quitter la camera libre" | "sortir de la caméra libre") { return VoiceIntent::Freecam(false); }
    if matches!(t.as_str(), "caméra libre" | "camera libre" | "vol libre") { return VoiceIntent::Freecam(true); }
    if matches!(t.as_str(), "envoyer" | "envoie" | "envoie le") { return VoiceIntent::Send; }
    if matches!(t.as_str(), "mode code") { return VoiceIntent::CodeMode(true); }
    if matches!(t.as_str(), "mode texte" | "mode prose") { return VoiceIntent::CodeMode(false); }

    let words: Vec<&str> = t.split_whitespace().collect();
    if let Some(n) = goto_target(&words) { return VoiceIntent::Command(Command::Goto(n)); }
    if matches!(t.as_str(), "branche ici" | "créer une branche" | "creer une branche" | "créer une branche ici") { return VoiceIntent::Command(Command::Branch); }
    if matches!(t.as_str(), "élaguer" | "elaguer" | "élaguer ici") { return VoiceIntent::Command(Command::Prune); }
    if matches!(t.as_str(), "restaurer" | "annuler l'élagage") { return VoiceIntent::Command(Command::Unprune); }
    if matches!(t.as_str(), "vue d'ensemble" | "vue densemble" | "dézoomer" | "dezoomer" | "tout voir") { return VoiceIntent::Command(Command::Overview); }
    if matches!(t.as_str(), "montrer le palais" | "palais" | "vue spatiale") { return VoiceIntent::Command(Command::Spatial); }
    if matches!(t.as_str(), "retour au chat" | "vue linéaire" | "vue lineaire") { return VoiceIntent::Command(Command::Linear); }
    if matches!(t.as_str(), "meilleur" | "montrer le meilleur" | "le meilleur") { return VoiceIntent::Command(Command::Best); }
    if matches!(t.as_str(), "arrêter" | "arreter" | "annuler" | "stop") { return VoiceIntent::Command(Command::Stop); }

    crate::voice_intent::parse(original) // fall back to English
}

fn goto_target(words: &[&str]) -> Option<usize> {
    let joined = words.join(" ");
    let rest = ["aller à ", "aller a ", "va à ", "va a ", "concentrer sur ", "nœud ", "noeud "]
        .iter().find_map(|p| joined.strip_prefix(p))?;
    parse_number(rest.trim())
}

/// French 0-30, tolerant of both hyphenated and space-separated compounds ("vingt-deux" / "vingt deux")
/// and the "et" joiner in 21 ("vingt et un"). 31+ is left unsupported (irregular tens beyond this point)
/// rather than risk a wrong guess.
fn parse_number(s: &str) -> Option<usize> {
    let s = s.trim();
    if let Ok(n) = s.parse::<usize>() { return Some(n); }
    const UNITS: &[(&str, usize)] = &[
        ("zéro", 0), ("zero", 0), ("un", 1), ("une", 1), ("deux", 2), ("trois", 3), ("quatre", 4), ("cinq", 5),
        ("six", 6), ("sept", 7), ("huit", 8), ("neuf", 9), ("dix", 10), ("onze", 11), ("douze", 12),
        ("treize", 13), ("quatorze", 14), ("quinze", 15), ("seize", 16), ("dix-sept", 17), ("dix sept", 17),
        ("dix-huit", 18), ("dix huit", 18), ("dix-neuf", 19), ("dix neuf", 19), ("vingt", 20),
    ];
    if let Some(&(_, n)) = UNITS.iter().find(|(w, _)| *w == s) { return Some(n); }
    // 21-29: "vingt" + optional "et" + unit (1-9)
    let parts: Vec<&str> = s.split([' ', '-']).filter(|p| !p.is_empty() && *p != "et").collect();
    if parts.len() == 2 && parts[0] == "vingt" {
        let u = UNITS.iter().find(|(w, _)| *w == parts[1]).map(|&(_, n)| n)?;
        if (1..=9).contains(&u) { return Some(20 + u); }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goto_and_commands() {
        assert!(matches!(parse("aller à douze"), VoiceIntent::Command(Command::Goto(12))));
        assert!(matches!(parse("nœud sept"), VoiceIntent::Command(Command::Goto(7))));
        assert!(matches!(parse("aller à vingt et un"), VoiceIntent::Command(Command::Goto(21))));
        assert!(matches!(parse("aller à vingt-deux"), VoiceIntent::Command(Command::Goto(22))));
        assert!(matches!(parse("branche ici"), VoiceIntent::Command(Command::Branch)));
        assert!(matches!(parse("élaguer"), VoiceIntent::Command(Command::Prune)));
        assert!(matches!(parse("vue d'ensemble"), VoiceIntent::Command(Command::Overview)));
    }

    #[test]
    fn camera_send_and_english_fallback() {
        assert!(matches!(parse("démarrer la visite"), VoiceIntent::Tour(true)));
        assert!(matches!(parse("caméra libre"), VoiceIntent::Freecam(true)));
        assert!(matches!(parse("envoyer"), VoiceIntent::Send));
        match parse("ceci n'est pas une commande") {
            VoiceIntent::Dictate(s) => assert_eq!(s, "ceci n'est pas une commande"),
            _ => panic!("unrecognized phrase should fall through to dictation"),
        }
        assert!(matches!(parse("go to five"), VoiceIntent::Command(Command::Goto(5))));
    }
}
