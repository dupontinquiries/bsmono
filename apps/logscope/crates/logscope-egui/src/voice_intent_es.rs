//! voice_intent_es.rs — Spanish command vocabulary for the same `VoiceIntent` the English parser produces.
//! Falls back to the English parser for anything unmatched, so a stray English phrase (or a Spanish
//! utterance not yet covered) still resolves instead of silently failing. Pure + unit-tested.

use crate::vimmode::Command;
use crate::voice_intent::VoiceIntent;

pub fn parse(utterance: &str) -> VoiceIntent {
    let original = utterance.trim();
    let t = original.trim_end_matches(['.', '!', '?', ',']).trim().to_lowercase();

    if matches!(t.as_str(), "detener recorrido" | "parar recorrido" | "terminar recorrido" | "aterrizar") { return VoiceIntent::Tour(false); }
    if matches!(t.as_str(), "recorrido" | "iniciar recorrido" | "empezar recorrido" | "hacer el recorrido") { return VoiceIntent::Tour(true); }
    if matches!(t.as_str(), "salir de cámara libre" | "salir camara libre" | "salir de la cámara libre") { return VoiceIntent::Freecam(false); }
    if matches!(t.as_str(), "cámara libre" | "camara libre" | "vuelo libre") { return VoiceIntent::Freecam(true); }
    if matches!(t.as_str(), "enviar" | "envíalo" | "envialo" | "mándalo" | "mandalo") { return VoiceIntent::Send; }
    if matches!(t.as_str(), "modo código" | "modo codigo") { return VoiceIntent::CodeMode(true); }
    if matches!(t.as_str(), "modo prosa" | "modo texto") { return VoiceIntent::CodeMode(false); }

    let words: Vec<&str> = t.split_whitespace().collect();
    if let Some(n) = goto_target(&words) { return VoiceIntent::Command(Command::Goto(n)); }
    if matches!(t.as_str(), "ramificar aquí" | "ramificar" | "rama aquí" | "crear rama" | "crear una rama aquí") { return VoiceIntent::Command(Command::Branch); }
    if matches!(t.as_str(), "podar" | "podar aquí" | "recortar") { return VoiceIntent::Command(Command::Prune); }
    if matches!(t.as_str(), "restaurar" | "despodar" | "deshacer poda") { return VoiceIntent::Command(Command::Unprune); }
    if matches!(t.as_str(), "vista general" | "resumen" | "alejar" | "ver todo") { return VoiceIntent::Command(Command::Overview); }
    if matches!(t.as_str(), "mostrar el palacio" | "palacio" | "vista espacial" | "modo espacial") { return VoiceIntent::Command(Command::Spatial); }
    if matches!(t.as_str(), "volver al chat" | "vista lineal" | "modo texto plano") { return VoiceIntent::Command(Command::Linear); }
    if matches!(t.as_str(), "mejor" | "mostrar mejor" | "el mejor") { return VoiceIntent::Command(Command::Best); }
    if matches!(t.as_str(), "detener" | "parar" | "cancelar" | "alto") { return VoiceIntent::Command(Command::Stop); }

    crate::voice_intent::parse(original) // fall back to English (handles code mishears too)
}

fn goto_target(words: &[&str]) -> Option<usize> {
    let joined = words.join(" ");
    let rest = ["ir a ", "ve a ", "saltar a ", "enfocar en ", "enfocar ", "nodo ", "ir al nodo "]
        .iter().find_map(|p| joined.strip_prefix(p))?;
    parse_number(rest.trim())
}

/// Spanish 0-30. Compounds 21-29 are single fused words (veintiuno…); 31+ would need "treinta y uno" —
/// left unsupported (falls to dictation) rather than risk a wrong guess at that pattern.
fn parse_number(s: &str) -> Option<usize> {
    let s = s.trim();
    if let Ok(n) = s.parse::<usize>() { return Some(n); }
    const WORDS: &[(&str, usize)] = &[
        ("cero", 0), ("uno", 1), ("dos", 2), ("tres", 3), ("cuatro", 4), ("cinco", 5), ("seis", 6),
        ("siete", 7), ("ocho", 8), ("nueve", 9), ("diez", 10), ("once", 11), ("doce", 12), ("trece", 13),
        ("catorce", 14), ("quince", 15), ("dieciséis", 16), ("dieciseis", 16), ("diecisiete", 17),
        ("dieciocho", 18), ("diecinueve", 19), ("veinte", 20), ("veintiuno", 21), ("veintidós", 22),
        ("veintidos", 22), ("veintitrés", 23), ("veintitres", 23), ("veinticuatro", 24), ("veinticinco", 25),
        ("veintiséis", 26), ("veintiseis", 26), ("veintisiete", 27), ("veintiocho", 28), ("veintinueve", 29),
        ("treinta", 30),
    ];
    WORDS.iter().find(|(w, _)| *w == s).map(|&(_, n)| n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goto_and_commands() {
        assert!(matches!(parse("ir a doce"), VoiceIntent::Command(Command::Goto(12))));
        assert!(matches!(parse("nodo siete"), VoiceIntent::Command(Command::Goto(7))));
        assert!(matches!(parse("enfocar en veintiuno"), VoiceIntent::Command(Command::Goto(21))));
        assert!(matches!(parse("ramificar aquí"), VoiceIntent::Command(Command::Branch)));
        assert!(matches!(parse("podar"), VoiceIntent::Command(Command::Prune)));
        assert!(matches!(parse("vista general"), VoiceIntent::Command(Command::Overview)));
    }

    #[test]
    fn camera_send_and_english_fallback() {
        assert!(matches!(parse("iniciar recorrido"), VoiceIntent::Tour(true)));
        assert!(matches!(parse("cámara libre"), VoiceIntent::Freecam(true)));
        assert!(matches!(parse("envíalo"), VoiceIntent::Send));
        // unmatched Spanish falls back to the English parser, which dictates it
        match parse("esto no es un comando reconocido") {
            VoiceIntent::Dictate(s) => assert_eq!(s, "esto no es un comando reconocido"),
            _ => panic!("unrecognized phrase should fall through to dictation via the English fallback"),
        }
        // a stray English command still works through the fallback (mixed-language friendly)
        assert!(matches!(parse("go to five"), VoiceIntent::Command(Command::Goto(5))));
    }
}
