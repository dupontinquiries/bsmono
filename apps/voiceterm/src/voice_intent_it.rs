// src/voice_intent_it.rs
//
// Italian intent parser. Mirrors voice_intent::parse (and the Spanish
// voice_intent_es / French voice_intent_fr parsers) with Italian
// keywords. v0.1.872.
//
// Coverage: navigation, scrolling, quit, search, open-site, history,
// the "guarda X" → cd shortcut, and the common politeness / wish /
// conditional prefixes and trailing softeners.
//
// Quality note: keyword lists are reasonable Standard Italian but
// probably miss regional/colloquial variants. The structure is
// designed to make additions easy — just extend the matches!/or_else
// chains. Romance-language shape closely parallels Spanish, so the two
// files stay deliberately structurally identical for easy diffing.
//
// Falls back to the English parser for utterances that don't match
// Italian keywords — useful for mixed-language users who say e.g.
// "apri GitHub" (Italian verb + proper noun) where "apri" matches
// Italian but "github" carries the rest unchanged.

use crate::voice_intent::Intent;

/// Strip Italian politeness prefixes, first-person wishes, and trailing
/// softeners. Mirrors voice_intent::normalize_natural_language but with
/// Italian phrases.
fn normalize_it(text: &str) -> String {
    let mut t = text.to_string();

    // Politeness + wish prefixes (Italian).
    //
    // Like Spanish, Italian drops subject pronouns freely: "voglio
    // aprire Google" (I want to open Google) is more natural than "io
    // voglio aprire Google". Both are handled (the "io " variants).
    let prefixes: &[&str] = &[
        // Polite questions (potresti... ?)
        "potresti per favore ",
        "puoi per favore ",
        "potresti per piacere ",
        "puoi per piacere ",
        "potresti ",
        "puoi ",
        "potrebbe ",
        // First-person wishes
        "io vorrei ",
        "vorrei ",
        "mi piacerebbe ",
        "voglio ",
        "io voglio ",
        "ho bisogno di ",
        "avrei bisogno di ",
        // Permissive / hortative
        "andiamo a ",
        "fammi ",
        "lasciami ",
        "permettimi di ",
        // Modal-only
        "per favore ",
        "per piacere ",
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

    // Trailing softeners.
    let suffixes: &[&str] = &[
        " per favore",
        " per piacere",
        " grazie",
        " velocemente",
        " veloce",
        " adesso",
        " subito",
        " ora",
        " se puoi",
        " se non ti dispiace",
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

/// Parse Italian text into an Intent. Falls back to the English parser
/// for unrecognized utterances (so e.g. "open google" still works for
/// users who type the English verb).
pub fn parse_it(text: &str) -> Intent {
    let t = text.trim().to_lowercase();
    if t.is_empty() { return Intent::Unknown; }

    // Apply Italian-specific normalization.
    let t = normalize_it(&t);
    if t.is_empty() { return Intent::Unknown; }
    let t = t.trim_end_matches('.').trim_end_matches('!').trim_end_matches('?');
    let t = t.trim();

    // Quit. (Deliberately omits bare "ciao" — it's a greeting as often
    // as a farewell, and a false Quit is costly.)
    if matches!(t,
        "esci" | "chiudi" | "chiudi app" | "chiudi l'applicazione"
        | "chiudi applicazione" | "spegni" | "arrivederci" | "addio" | "a dopo"
    ) {
        return Intent::Quit;
    }

    // Navigation history.
    if matches!(t,
        "indietro" | "torna indietro" | "torna" | "ritorna" | "vai indietro"
        | "pagina precedente" | "precedente"
    ) {
        return Intent::GoBack;
    }
    if matches!(t,
        "avanti" | "vai avanti" | "successivo" | "successiva"
        | "pagina successiva" | "prossima" | "prossima pagina" | "pagina seguente"
    ) {
        return Intent::GoForward;
    }
    if matches!(t,
        "ricarica" | "ricaricare" | "aggiorna" | "ricarica pagina" | "ricarica la pagina"
        | "aggiorna pagina" | "aggiorna la pagina" | "riprova"
    ) {
        return Intent::Reload;
    }

    // Scrolling.
    if matches!(t,
        "giù" | "scendi" | "scorri giù" | "verso il basso" | "in basso"
        | "un po' giù" | "scendi un po'"
    ) { return Intent::Scroll(60); }
    if matches!(t,
        "su" | "sali" | "scorri su" | "verso l'alto" | "in alto"
        | "un po' su" | "sali un po'"
    ) { return Intent::Scroll(-60); }
    if matches!(t,
        "pagina giù" | "una pagina giù" | "pagina in giù"
    ) { return Intent::ScrollPage(1); }
    if matches!(t,
        "pagina su" | "una pagina su" | "pagina in su"
    ) { return Intent::ScrollPage(-1); }
    if matches!(t,
        "vai all'inizio" | "inizio" | "all'inizio" | "in cima" | "vai in cima"
        | "tutto in alto"
    ) { return Intent::GoTop; }
    if matches!(t,
        "vai alla fine" | "fine" | "alla fine" | "in fondo" | "vai in fondo"
        | "tutto in basso"
    ) { return Intent::GoBottom; }

    // "guarda X" → cd into X. The Italian parallel to Spanish "mira X"
    // ("look at X" disambiguates as cd, vs the run-vs-cd ambiguity of a
    // bare directory name). Strict prefix match — we don't strip
    // "guarda" as a generic discourse marker since it can legitimately
    // appear in longer utterances ("guarda qui" = "look here").
    if let Some(rest) = t.strip_prefix("guarda ") {
        let rest = rest.trim();
        if !rest.is_empty() {
            return Intent::TerminalCommand(format!("cd {}", rest));
        }
    }

    // "Open <site>" — Italian verbs.
    if let Some(rest) = t.strip_prefix("apri ")
        .or_else(|| t.strip_prefix("aprire "))
        .or_else(|| t.strip_prefix("vai a "))
        .or_else(|| t.strip_prefix("vai su "))
        .or_else(|| t.strip_prefix("visita "))
        .or_else(|| t.strip_prefix("visitare "))
        .or_else(|| t.strip_prefix("naviga a "))
        .or_else(|| t.strip_prefix("naviga su "))
        .or_else(|| t.strip_prefix("portami a "))
        .or_else(|| t.strip_prefix("portami su "))
        .or_else(|| t.strip_prefix("mostrami "))
        .or_else(|| t.strip_prefix("carica "))
        .or_else(|| t.strip_prefix("caricare "))
    {
        let rest = rest.trim();
        if !rest.is_empty() {
            // Defer to the English parser for site name → URL mapping
            // (resolve_known_site, etc). Rebuild as "open <rest>".
            return crate::voice_intent::parse(&format!("open {}", rest));
        }
    }

    // Search the web. Italian verbs.
    if let Some(q) = t.strip_prefix("cerca ")
        .or_else(|| t.strip_prefix("cercare "))
        .or_else(|| t.strip_prefix("cerca per "))
        .or_else(|| t.strip_prefix("trova "))
        .or_else(|| t.strip_prefix("googla "))
        .or_else(|| t.strip_prefix("googlare "))
    {
        let q = q.trim_start_matches("per ").trim();
        if !q.is_empty() { return Intent::SearchWeb(q.to_string()); }
    }

    // Fall back to the English parser. Mixed utterances like "apri
    // google" (Italian verb stripped → "google") reach here if no
    // Italian open-prefix matched.
    crate::voice_intent::parse(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "guarda X" → cd X (the Italian parallel to Spanish "mira X").
    #[test]
    fn guarda_routes_to_cd() {
        match parse_it("guarda kwin") {
            Intent::TerminalCommand(cmd) => assert_eq!(cmd, "cd kwin"),
            other => panic!("expected TerminalCommand(cd kwin), got {:?}", other),
        }
        match parse_it("guarda progetti") {
            Intent::TerminalCommand(cmd) => assert_eq!(cmd, "cd progetti"),
            other => panic!("expected TerminalCommand(cd progetti), got {:?}", other),
        }
    }

    /// Scrolling — pin a few entries against regression.
    #[test]
    fn scroll_basics() {
        assert!(matches!(parse_it("giù"), Intent::Scroll(60)));
        assert!(matches!(parse_it("scendi"), Intent::Scroll(60)));
        assert!(matches!(parse_it("su"), Intent::Scroll(-60)));
        assert!(matches!(parse_it("sali"), Intent::Scroll(-60)));
        assert!(matches!(parse_it("pagina giù"), Intent::ScrollPage(1)));
        assert!(matches!(parse_it("in cima"), Intent::GoTop));
        assert!(matches!(parse_it("in fondo"), Intent::GoBottom));
    }

    /// Navigation history.
    #[test]
    fn nav_basics() {
        assert!(matches!(parse_it("indietro"), Intent::GoBack));
        assert!(matches!(parse_it("torna indietro"), Intent::GoBack));
        assert!(matches!(parse_it("avanti"), Intent::GoForward));
        assert!(matches!(parse_it("pagina successiva"), Intent::GoForward));
        assert!(matches!(parse_it("ricarica"), Intent::Reload));
    }

    /// "vai avanti" / "vai all'inizio" must beat the "vai a " open-prefix
    /// (exact history/top matches run first). Pins that ordering.
    #[test]
    fn vai_disambiguation() {
        assert!(matches!(parse_it("vai avanti"), Intent::GoForward));
        assert!(matches!(parse_it("vai indietro"), Intent::GoBack));
        assert!(matches!(parse_it("vai all'inizio"), Intent::GoTop));
        assert!(matches!(parse_it("vai alla fine"), Intent::GoBottom));
    }

    /// Quit phrases.
    #[test]
    fn quit_basics() {
        assert!(matches!(parse_it("esci"), Intent::Quit));
        assert!(matches!(parse_it("chiudi"), Intent::Quit));
        assert!(matches!(parse_it("arrivederci"), Intent::Quit));
        // bare "ciao" is intentionally NOT a quit (greeting collision).
        assert!(!matches!(parse_it("ciao"), Intent::Quit));
    }

    /// Search.
    #[test]
    fn search_basics() {
        match parse_it("cerca gatti") {
            Intent::SearchWeb(q) => assert_eq!(q, "gatti"),
            other => panic!("expected SearchWeb(gatti), got {:?}", other),
        }
        match parse_it("trova ricette di pasta") {
            Intent::SearchWeb(q) => assert_eq!(q, "ricette di pasta"),
            other => panic!("expected SearchWeb(...), got {:?}", other),
        }
    }

    /// Politeness prefix stripping: "per favore apri github" should not
    /// return Unknown (prefix stripped → "apri github" → open path).
    #[test]
    fn politeness_strip() {
        let r = parse_it("per favore apri github");
        assert!(!matches!(r, Intent::Unknown),
            "politeness prefix should be stripped, got {:?}", r);
    }

    /// Trailing softeners: "scendi per favore" should still scroll down.
    #[test]
    fn trailing_softener_strip() {
        assert!(matches!(parse_it("scendi per favore"), Intent::Scroll(60)));
        assert!(matches!(parse_it("indietro grazie"), Intent::GoBack));
    }
}
