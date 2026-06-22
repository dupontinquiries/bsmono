// src/voice_intent_de.rs
//
// German intent parser. Mirrors voice_intent::parse (and the
// Spanish/French/Italian/Portuguese parsers) with German keywords.
// v0.1.873.
//
// Coverage: navigation, scrolling, quit, search, open-site, history,
// the "schau X" → cd shortcut, and the common politeness / wish
// prefixes and trailing softeners.
//
// Two German-specific wrinkles beyond the Romance parsers:
//   * Separable verbs: "mach Google auf" (= open Google) puts the
//     particle "auf" at the end. Handled as a strip-prefix +
//     strip-suffix special case.
//   * Umlaut variation: Whisper usually emits proper umlauts (öffne)
//     but can produce ASCII (oeffne/offne). High-frequency commands
//     list both the umlaut form and an ASCII fallback (ö→oe, ü→ue,
//     ä→ae, ß→ss).
//
// Falls back to the English parser for unmatched utterances.

use crate::voice_intent::Intent;

/// Strip German politeness prefixes, first-person wishes, and trailing
/// softeners.
fn normalize_de(text: &str) -> String {
    let mut t = text.to_string();

    let prefixes: &[&str] = &[
        // Polite questions (du / formal Sie)
        "könntest du bitte ",
        "koenntest du bitte ",
        "kannst du bitte ",
        "könnten sie bitte ",
        "koennten sie bitte ",
        "können sie bitte ",
        "koennen sie bitte ",
        "könntest du ",
        "koenntest du ",
        "kannst du ",
        "könntest ",
        "koenntest ",
        // First-person wishes
        "ich möchte gerne ",
        "ich moechte gerne ",
        "ich möchte ",
        "ich moechte ",
        "ich würde gerne ",
        "ich wuerde gerne ",
        "ich will ",
        "ich brauche ",
        // Permissive
        "lass mich ",
        "lassen sie mich ",
        // Modal-only
        "bitte ",
    ];

    let mut changed = true;
    let mut iter = 0;
    while changed && iter < 5 {
        changed = false;
        iter += 1;
        for &p in prefixes {
            if t.starts_with(p) {
                t = t[p.len()..].trim_start().to_string();
                changed = true;
                break;
            }
        }
    }

    let suffixes: &[&str] = &[
        " bitte",
        " danke",
        " schnell",
        " sofort",
        " jetzt sofort",
        " jetzt",
        " wenn du kannst",
        " wenn es geht",
    ];
    let mut changed = true;
    let mut iter = 0;
    while changed && iter < 5 {
        changed = false;
        iter += 1;
        for &s in suffixes {
            if t.ends_with(s) {
                let cut = t.len() - s.len();
                t = t[..cut].trim_end().to_string();
                changed = true;
                break;
            }
        }
    }

    t.trim().to_string()
}

/// Parse German text into an Intent. Falls back to the English parser
/// for unrecognized utterances.
pub fn parse_de(text: &str) -> Intent {
    let t = text.trim().to_lowercase();
    if t.is_empty() { return Intent::Unknown; }

    let t = normalize_de(&t);
    if t.is_empty() { return Intent::Unknown; }
    let t = t.trim_end_matches('.').trim_end_matches('!').trim_end_matches('?');
    let t = t.trim();

    // Quit.
    if matches!(t,
        "beenden" | "schließen" | "schliessen" | "schließe" | "schliesse"
        | "app schließen" | "app schliessen" | "anwendung schließen" | "anwendung schliessen"
        | "ausschalten" | "tschüss" | "tschuess" | "tschau" | "auf wiedersehen"
    ) {
        return Intent::Quit;
    }

    // Navigation history.
    if matches!(t,
        "zurück" | "zurueck" | "geh zurück" | "geh zurueck" | "gehe zurück" | "gehe zurueck"
        | "zurückgehen" | "zurueckgehen" | "vorherige seite" | "vorherige"
    ) {
        return Intent::GoBack;
    }
    if matches!(t,
        "vorwärts" | "vorwaerts" | "weiter" | "geh weiter" | "gehe weiter"
        | "nächste seite" | "naechste seite" | "nächste" | "naechste" | "vorwärts gehen" | "vorwaerts gehen"
    ) {
        return Intent::GoForward;
    }
    if matches!(t,
        "neu laden" | "neuladen" | "aktualisieren" | "aktualisiere"
        | "seite neu laden" | "seite aktualisieren" | "erneut versuchen" | "nochmal versuchen"
    ) {
        return Intent::Reload;
    }

    // Scrolling.
    if matches!(t,
        "runter" | "herunter" | "nach unten" | "scroll runter" | "scrolle runter"
        | "runterscrollen" | "ein bisschen runter" | "etwas runter"
    ) { return Intent::Scroll(60); }
    if matches!(t,
        "hoch" | "hinauf" | "nach oben" | "scroll hoch" | "scrolle hoch"
        | "hochscrollen" | "ein bisschen hoch" | "etwas hoch"
    ) { return Intent::Scroll(-60); }
    if matches!(t,
        "seite runter" | "eine seite runter" | "seite nach unten"
    ) { return Intent::ScrollPage(1); }
    if matches!(t,
        "seite hoch" | "eine seite hoch" | "seite nach oben"
    ) { return Intent::ScrollPage(-1); }
    if matches!(t,
        "zum anfang" | "anfang" | "ganz nach oben" | "zum anfang gehen"
        | "nach ganz oben" | "an den anfang"
    ) { return Intent::GoTop; }
    if matches!(t,
        "zum ende" | "ende" | "ganz nach unten" | "zum ende gehen"
        | "nach ganz unten" | "ans ende"
    ) { return Intent::GoBottom; }

    // "schau X" / "schau dir X an" → cd into X (German parallel to the
    // Spanish "mira X"). Strip the leading verb and the trailing
    // separable particle "an".
    if let Some(rest) = t.strip_prefix("schau dir ").or_else(|| t.strip_prefix("schau ")) {
        let rest = rest.trim().trim_end_matches(" an").trim();
        if !rest.is_empty() {
            return Intent::TerminalCommand(format!("cd {}", rest));
        }
    }

    // Separable "mach X auf" (= open X), incl. the filler "mach mal X auf".
    if let Some(rest) = t.strip_prefix("mach ") {
        if let Some(target) = rest.strip_suffix(" auf") {
            let target = target.trim().trim_start_matches("mal ").trim();
            if !target.is_empty() {
                return crate::voice_intent::parse(&format!("open {}", target));
            }
        }
    }

    // "Open <site>" — German verbs.
    if let Some(rest) = t.strip_prefix("öffne ")
        .or_else(|| t.strip_prefix("oeffne "))
        .or_else(|| t.strip_prefix("öffnen "))
        .or_else(|| t.strip_prefix("oeffnen "))
        .or_else(|| t.strip_prefix("geh zu "))
        .or_else(|| t.strip_prefix("gehe zu "))
        .or_else(|| t.strip_prefix("besuche "))
        .or_else(|| t.strip_prefix("besuchen "))
        .or_else(|| t.strip_prefix("navigiere zu "))
        .or_else(|| t.strip_prefix("navigier zu "))
        .or_else(|| t.strip_prefix("bring mich zu "))
        .or_else(|| t.strip_prefix("zeig mir "))
        .or_else(|| t.strip_prefix("zeige mir "))
        .or_else(|| t.strip_prefix("lade "))
        .or_else(|| t.strip_prefix("lad "))
    {
        let rest = rest.trim();
        if !rest.is_empty() {
            // Defer to the English parser for site name → URL mapping.
            return crate::voice_intent::parse(&format!("open {}", rest));
        }
    }

    // Search the web. German verbs.
    if let Some(q) = t.strip_prefix("suche nach ")
        .or_else(|| t.strip_prefix("such nach "))
        .or_else(|| t.strip_prefix("suche "))
        .or_else(|| t.strip_prefix("suchen "))
        .or_else(|| t.strip_prefix("finde "))
        .or_else(|| t.strip_prefix("google "))
        .or_else(|| t.strip_prefix("googeln "))
    {
        let q = q.trim_start_matches("nach ").trim();
        if !q.is_empty() { return Intent::SearchWeb(q.to_string()); }
    }

    crate::voice_intent::parse(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schau_routes_to_cd() {
        match parse_de("schau kwin") {
            Intent::TerminalCommand(cmd) => assert_eq!(cmd, "cd kwin"),
            other => panic!("expected TerminalCommand(cd kwin), got {:?}", other),
        }
        // separable "schau dir X an"
        match parse_de("schau dir projekte an") {
            Intent::TerminalCommand(cmd) => assert_eq!(cmd, "cd projekte"),
            other => panic!("expected TerminalCommand(cd projekte), got {:?}", other),
        }
    }

    #[test]
    fn scroll_basics() {
        assert!(matches!(parse_de("runter"), Intent::Scroll(60)));
        assert!(matches!(parse_de("nach unten"), Intent::Scroll(60)));
        assert!(matches!(parse_de("hoch"), Intent::Scroll(-60)));
        assert!(matches!(parse_de("nach oben"), Intent::Scroll(-60)));
        assert!(matches!(parse_de("seite runter"), Intent::ScrollPage(1)));
        assert!(matches!(parse_de("zum anfang"), Intent::GoTop));
        assert!(matches!(parse_de("zum ende"), Intent::GoBottom));
    }

    #[test]
    fn nav_basics() {
        assert!(matches!(parse_de("zurück"), Intent::GoBack));
        assert!(matches!(parse_de("geh zurück"), Intent::GoBack));
        assert!(matches!(parse_de("vorwärts"), Intent::GoForward));
        assert!(matches!(parse_de("nächste seite"), Intent::GoForward));
        assert!(matches!(parse_de("neu laden"), Intent::Reload));
    }

    /// "geh zurück"/"geh weiter" must beat the "geh zu " open-prefix
    /// (exact history matches run first).
    #[test]
    fn geh_disambiguation() {
        assert!(matches!(parse_de("geh zurück"), Intent::GoBack));
        assert!(matches!(parse_de("geh weiter"), Intent::GoForward));
    }

    /// ASCII umlaut fallbacks (Whisper sometimes drops umlauts).
    #[test]
    fn ascii_umlaut_fallbacks() {
        assert!(matches!(parse_de("zurueck"), Intent::GoBack));
        assert!(matches!(parse_de("vorwaerts"), Intent::GoForward));
        assert!(matches!(parse_de("schliessen"), Intent::Quit));
    }

    /// Separable "mach X auf" → open.
    #[test]
    fn separable_mach_auf() {
        // "mach google auf" → open google (English parser resolves the
        // site → URL; we just assert it's not Unknown/cd).
        assert!(!matches!(parse_de("mach google auf"), Intent::Unknown));
        assert!(!matches!(parse_de("mach mal github auf"), Intent::Unknown));
    }

    #[test]
    fn quit_basics() {
        assert!(matches!(parse_de("beenden"), Intent::Quit));
        assert!(matches!(parse_de("schließen"), Intent::Quit));
        assert!(matches!(parse_de("tschüss"), Intent::Quit));
        assert!(matches!(parse_de("auf wiedersehen"), Intent::Quit));
    }

    #[test]
    fn search_basics() {
        match parse_de("suche katzen") {
            Intent::SearchWeb(q) => assert_eq!(q, "katzen"),
            other => panic!("expected SearchWeb(katzen), got {:?}", other),
        }
        match parse_de("suche nach kuchen rezepten") {
            Intent::SearchWeb(q) => assert_eq!(q, "kuchen rezepten"),
            other => panic!("expected SearchWeb(...), got {:?}", other),
        }
    }

    #[test]
    fn politeness_strip() {
        let r = parse_de("bitte öffne github");
        assert!(!matches!(r, Intent::Unknown),
            "politeness prefix should be stripped, got {:?}", r);
    }

    #[test]
    fn trailing_softener_strip() {
        assert!(matches!(parse_de("runter bitte"), Intent::Scroll(60)));
        assert!(matches!(parse_de("zurück danke"), Intent::GoBack));
    }
}
