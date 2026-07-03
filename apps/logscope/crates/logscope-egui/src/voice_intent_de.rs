//! voice_intent_de.rs — German command vocabulary. Falls back to the English parser for anything
//! unmatched. Pure + unit-tested.

use crate::vimmode::Command;
use crate::voice_intent::VoiceIntent;

pub fn parse(utterance: &str) -> VoiceIntent {
    let original = utterance.trim();
    let t = original.trim_end_matches(['.', '!', '?', ',']).trim().to_lowercase();

    if matches!(t.as_str(), "tour stoppen" | "tour beenden" | "landen") { return VoiceIntent::Tour(false); }
    if matches!(t.as_str(), "tour" | "tour starten" | "rundgang starten") { return VoiceIntent::Tour(true); }
    if matches!(t.as_str(), "freie kamera verlassen" | "freie kamera beenden") { return VoiceIntent::Freecam(false); }
    if matches!(t.as_str(), "freie kamera" | "freiflug") { return VoiceIntent::Freecam(true); }
    if matches!(t.as_str(), "senden" | "abschicken" | "sende es") { return VoiceIntent::Send; }
    if matches!(t.as_str(), "code modus" | "codemodus") { return VoiceIntent::CodeMode(true); }
    if matches!(t.as_str(), "text modus" | "textmodus" | "prosa modus") { return VoiceIntent::CodeMode(false); }

    let words: Vec<&str> = t.split_whitespace().collect();
    if let Some(n) = goto_target(&words) { return VoiceIntent::Command(Command::Goto(n)); }
    if matches!(t.as_str(), "zweig hier" | "verzweigen" | "hier verzweigen" | "zweig erstellen") { return VoiceIntent::Command(Command::Branch); }
    if matches!(t.as_str(), "beschneiden" | "hier beschneiden") { return VoiceIntent::Command(Command::Prune); }
    if matches!(t.as_str(), "wiederherstellen" | "beschneidung rückgängig") { return VoiceIntent::Command(Command::Unprune); }
    if matches!(t.as_str(), "übersicht" | "uebersicht" | "übersicht zeigen" | "alles zeigen") { return VoiceIntent::Command(Command::Overview); }
    if matches!(t.as_str(), "zeige den palast" | "palast" | "räumliche ansicht" | "raeumliche ansicht") { return VoiceIntent::Command(Command::Spatial); }
    if matches!(t.as_str(), "zurück zum chat" | "zurueck zum chat" | "lineare ansicht") { return VoiceIntent::Command(Command::Linear); }
    if matches!(t.as_str(), "bester" | "das beste zeigen" | "das beste") { return VoiceIntent::Command(Command::Best); }
    if matches!(t.as_str(), "stopp" | "anhalten" | "abbrechen") { return VoiceIntent::Command(Command::Stop); }

    crate::voice_intent::parse(original) // fall back to English
}

fn goto_target(words: &[&str]) -> Option<usize> {
    let joined = words.join(" ");
    let rest = ["gehe zu ", "geh zu ", "fokussiere auf ", "fokus auf ", "knoten "]
        .iter().find_map(|p| joined.strip_prefix(p))?;
    parse_number(rest.trim())
}

/// German 0-30. Compounds are single fused words with reversed unit-ten order (einundzwanzig = "one and
/// twenty" = 21), so this is a direct lookup rather than decomposition. Normalizes "ß" -> "ss" since
/// whisper transcripts vary on that. 31+ is left unsupported rather than risk a wrong guess.
fn parse_number(s: &str) -> Option<usize> {
    let s = s.trim().replace('ß', "ss");
    if let Ok(n) = s.parse::<usize>() { return Some(n); }
    const WORDS: &[(&str, usize)] = &[
        ("null", 0), ("eins", 1), ("ein", 1), ("zwei", 2), ("drei", 3), ("vier", 4), ("fünf", 5), ("fuenf", 5),
        ("sechs", 6), ("sieben", 7), ("acht", 8), ("neun", 9), ("zehn", 10), ("elf", 11), ("zwölf", 12),
        ("zwoelf", 12), ("dreizehn", 13), ("vierzehn", 14), ("fünfzehn", 15), ("fuenfzehn", 15),
        ("sechzehn", 16), ("siebzehn", 17), ("achtzehn", 18), ("neunzehn", 19), ("zwanzig", 20),
        ("einundzwanzig", 21), ("zweiundzwanzig", 22), ("dreiundzwanzig", 23), ("vierundzwanzig", 24),
        ("fünfundzwanzig", 25), ("fuenfundzwanzig", 25), ("sechsundzwanzig", 26), ("siebenundzwanzig", 27),
        ("achtundzwanzig", 28), ("neunundzwanzig", 29), ("dreissig", 30),
    ];
    WORDS.iter().find(|(w, _)| *w == s).map(|&(_, n)| n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goto_and_commands() {
        assert!(matches!(parse("gehe zu zwölf"), VoiceIntent::Command(Command::Goto(12))));
        assert!(matches!(parse("knoten sieben"), VoiceIntent::Command(Command::Goto(7))));
        assert!(matches!(parse("gehe zu einundzwanzig"), VoiceIntent::Command(Command::Goto(21))));
        assert!(matches!(parse("zweig hier"), VoiceIntent::Command(Command::Branch)));
        assert!(matches!(parse("beschneiden"), VoiceIntent::Command(Command::Prune)));
        assert!(matches!(parse("übersicht"), VoiceIntent::Command(Command::Overview)));
    }

    #[test]
    fn camera_send_and_english_fallback() {
        assert!(matches!(parse("tour starten"), VoiceIntent::Tour(true)));
        assert!(matches!(parse("freie kamera"), VoiceIntent::Freecam(true)));
        assert!(matches!(parse("senden"), VoiceIntent::Send));
        match parse("das ist kein befehl") {
            VoiceIntent::Dictate(s) => assert_eq!(s, "das ist kein befehl"),
            _ => panic!("unrecognized phrase should fall through to dictation"),
        }
        assert!(matches!(parse("go to five"), VoiceIntent::Command(Command::Goto(5))));
    }

    #[test]
    fn eszett_normalization() {
        assert!(matches!(parse("gehe zu dreißig"), VoiceIntent::Command(Command::Goto(30))), "ß spelling");
        assert!(matches!(parse("gehe zu dreissig"), VoiceIntent::Command(Command::Goto(30))), "ss spelling");
    }
}
