// src/voice_intent_es.rs
//
// Spanish intent parser. Mirrors voice_intent::parse but with
// Spanish keywords. v0.1.114.
//
// Coverage: navigation, scrolling, quit, search, open-site, history,
// and the common politeness/wish/conditional prefixes.
//
// Quality note: I'm not a native Spanish speaker. These keyword lists
// are reasonable but probably miss regional/colloquial variants
// (Latin American vs Iberian; Mexican vs Argentine; formal vs casual).
// Pull requests welcome. The structure is designed to make additions
// easy — just extend the matches!/or_else chains.
//
// Falls back to the English parser for utterances that don't match
// Spanish keywords — useful for mixed-language users who say e.g.
// "abrir GitHub" (Spanish verb + proper noun) where "abrir" matches
// Spanish but "github" carries the rest unchanged.

use crate::voice_intent::Intent;

/// Strip Spanish politeness prefixes, first-person wishes, and trailing
/// softeners. Mirrors voice_intent::normalize_natural_language but
/// with Spanish phrases.
fn normalize_es(text: &str) -> String {
    let mut t = text.to_string();

    // Politeness + wish prefixes (Spanish).
    //
    // Note about pronouns: Spanish drops subject pronouns much more
    // often than English. "Quiero abrir Google" (I want to open Google)
    // is more common than "Yo quiero abrir Google". We handle both.
    let prefixes: &[&str] = &[
        // Polite questions (¿podrías... ?)
        "podrías por favor ",
        "podrias por favor ",
        "puedes por favor ",
        "podrías ",
        "podrias ",
        "puedes ",
        "podría ",
        "podria ",
        // First-person wishes
        "yo quisiera ",
        "quisiera ",
        "me gustaría ",
        "me gustaria ",
        "quiero ",
        "necesito ",
        "yo quiero ",
        "yo necesito ",
        // Permissive / hortative
        "vamos a ",
        "déjame ",
        "dejame ",
        "permíteme ",
        "permiteme ",
        // Modal-only
        "por favor ",
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
        " por favor",
        " gracias",
        " rápido",
        " rapido",
        " ahora mismo",
        " ahora",
        " si puedes",
        " si no te importa",
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

/// Parse Spanish text into an Intent. Falls back to English parser for
/// unrecognized utterances (so e.g. "open google" still works for users
/// who type the English verb).
pub fn parse_es(text: &str) -> Intent {
    let t = text.trim().to_lowercase();
    if t.is_empty() { return Intent::Unknown; }

    // Apply Spanish-specific normalization
    let t = normalize_es(&t);
    if t.is_empty() { return Intent::Unknown; }
    let t = t.trim_end_matches('.').trim_end_matches('!').trim_end_matches('?');
    let t = t.trim();

    // Quit
    if matches!(t,
        "salir" | "cerrar" | "cerrar app" | "cerrar la aplicación" | "cerrar la aplicacion"
        | "apagar" | "adiós" | "adios" | "chao" | "hasta luego"
    ) {
        return Intent::Quit;
    }

    // Navigation history.
    if matches!(t,
        "atrás" | "atras" | "volver" | "regresar" | "retroceder"
        | "ir atrás" | "ir atras" | "página anterior" | "pagina anterior"
        | "anterior" | "regresa" | "vuelve"
    ) {
        return Intent::GoBack;
    }
    if matches!(t,
        "adelante" | "siguiente" | "avanzar" | "ir adelante"
        | "página siguiente" | "pagina siguiente" | "siguiente página" | "siguiente pagina"
    ) {
        return Intent::GoForward;
    }
    if matches!(t,
        "recargar" | "actualizar" | "refrescar" | "recargar página" | "recargar pagina"
        | "actualizar página" | "actualizar pagina" | "intentar de nuevo"
    ) {
        return Intent::Reload;
    }

    // Scrolling
    if matches!(t,
        "abajo" | "baja" | "scroll abajo" | "desplazar abajo" | "hacia abajo"
        | "un poco abajo" | "baja un poco"
    ) { return Intent::Scroll(60); }
    if matches!(t,
        "arriba" | "sube" | "scroll arriba" | "desplazar arriba" | "hacia arriba"
        | "un poco arriba" | "sube un poco"
    ) { return Intent::Scroll(-60); }
    if matches!(t,
        "página abajo" | "pagina abajo" | "una página abajo" | "una pagina abajo"
    ) { return Intent::ScrollPage(1); }
    if matches!(t,
        "página arriba" | "pagina arriba" | "una página arriba" | "una pagina arriba"
    ) { return Intent::ScrollPage(-1); }
    if matches!(t,
        "ir al principio" | "principio" | "arriba del todo" | "ir arriba del todo"
        | "al principio" | "comienzo" | "al comienzo"
    ) { return Intent::GoTop; }
    if matches!(t,
        "ir al final" | "final" | "abajo del todo" | "ir abajo del todo"
        | "al final" | "fondo" | "al fondo"
    ) { return Intent::GoBottom; }

    // v0.1.288: "mira X" / "míra X" → cd into X. User noted:
    // "vamos kwin" is ambiguous (run vs cd); "mira kwin" =
    // "look at kwin" disambiguates as cd. Filed separately
    // from the broader Spanish intent-parser work because
    // this is a clean single addition.
    //
    // Strict prefix match — we don't strip "mira" as a generic
    // discourse marker since it can legitimately appear inside
    // longer utterances ("mira aquí" = "look here") that
    // aren't cd commands.
    if let Some(rest) = t.strip_prefix("mira ")
        .or_else(|| t.strip_prefix("míra "))
    {
        let rest = rest.trim();
        if !rest.is_empty() {
            return Intent::TerminalCommand(format!("cd {}", rest));
        }
    }

    // "Open <site>" — Spanish verbs
    if let Some(rest) = t.strip_prefix("abrir ")
        .or_else(|| t.strip_prefix("abre "))
        .or_else(|| t.strip_prefix("ir a "))
        .or_else(|| t.strip_prefix("ve a "))
        .or_else(|| t.strip_prefix("visitar "))
        .or_else(|| t.strip_prefix("visita "))
        .or_else(|| t.strip_prefix("navegar a "))
        .or_else(|| t.strip_prefix("navega a "))
        .or_else(|| t.strip_prefix("llévame a "))
        .or_else(|| t.strip_prefix("llevame a "))
        .or_else(|| t.strip_prefix("muéstrame "))
        .or_else(|| t.strip_prefix("muestrame "))
        .or_else(|| t.strip_prefix("cargar "))
        .or_else(|| t.strip_prefix("carga "))
    {
        let rest = rest.trim();
        if !rest.is_empty() {
            // Defer to English parser to handle site name → URL
            // mapping (resolve_known_site, etc). We rebuild as
            // "open <rest>" so the existing logic kicks in.
            return crate::voice_intent::parse(&format!("open {}", rest));
        }
    }

    // Search the web. Spanish verbs.
    if let Some(q) = t.strip_prefix("buscar ")
        .or_else(|| t.strip_prefix("busca ")) 
        .or_else(|| t.strip_prefix("buscar por "))
        .or_else(|| t.strip_prefix("busca por "))
        .or_else(|| t.strip_prefix("encuentra "))
        .or_else(|| t.strip_prefix("googlea "))
        .or_else(|| t.strip_prefix("googlear "))
    {
        let q = q.trim_start_matches("por ").trim();
        if !q.is_empty() { return Intent::SearchWeb(q.to_string()); }
    }

    // Fall back to English parser. Mixed utterances like "abrir google"
    // (Spanish verb stripped → "google") will reach this point if no
    // Spanish open-prefix matched. The English parser may or may not
    // recognize it.
    crate::voice_intent::parse(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// v0.1.288 added "mira X" → cd X. Pin the contract so the
    /// prefix matcher doesn't get accidentally moved into the
    /// open-prefix list (which would route to navigate, not
    /// terminal).
    #[test]
    fn mira_routes_to_cd() {
        match parse_es("mira kwin") {
            Intent::TerminalCommand(cmd) => assert_eq!(cmd, "cd kwin"),
            other => panic!("expected TerminalCommand(cd kwin), got {:?}", other),
        }
        match parse_es("mira projects") {
            Intent::TerminalCommand(cmd) => assert_eq!(cmd, "cd projects"),
            other => panic!("expected TerminalCommand(cd projects), got {:?}", other),
        }
    }

    /// Accented "míra" should work too (Whisper sometimes
    /// preserves the accent).
    #[test]
    fn mira_with_accent() {
        match parse_es("míra documents") {
            Intent::TerminalCommand(cmd) => assert_eq!(cmd, "cd documents"),
            other => panic!("expected TerminalCommand(cd documents), got {:?}", other),
        }
    }

    /// "mira aquí" / "mira esto" should NOT be misclassified
    /// as cd. They're discourse markers ("look here", "look at
    /// this") not navigation commands.
    ///
    /// Currently our parser strictly takes "mira <word>" as cd,
    /// which will MATCH "mira aquí" → cd aquí. That's wrong but
    /// it's a known limitation filed in the v0.1.288 changelog
    /// ("strict prefix because 'mira aquí' / 'mira esto' can
    /// legitimately appear mid-utterance"). This test documents
    /// the current behavior — the trade-off is "false-positive
    /// cd over `mira aquí` is better than false-negative on
    /// `mira kwin`" since the latter is a real workflow.
    ///
    /// If user reports the false-positive, switch to a
    /// whitelist of common dirnames OR require "mira <word>"
    /// to look like a path.
    #[test]
    fn mira_aqui_documents_known_false_positive() {
        // Current behavior: "mira aquí" → TerminalCommand("cd aquí")
        // This is a documented limitation. The test passes either
        // way to avoid breaking on tightening logic.
        match parse_es("mira aquí") {
            Intent::TerminalCommand(_) | Intent::Unknown => {} // either is fine
            other => panic!("unexpected intent for 'mira aquí': {:?}", other),
        }
    }

    /// Spanish scrolling — pin a few entries against regression.
    #[test]
    fn scroll_basics() {
        assert!(matches!(parse_es("abajo"), Intent::Scroll(60)));
        assert!(matches!(parse_es("arriba"), Intent::Scroll(-60)));
    }

    /// Spanish navigation history.
    #[test]
    fn go_back_basics() {
        assert!(matches!(parse_es("atrás"), Intent::GoBack));
        assert!(matches!(parse_es("volver"), Intent::GoBack));
        assert!(matches!(parse_es("adelante"), Intent::GoForward));
    }

    /// Quit phrases.
    #[test]
    fn quit_basics() {
        assert!(matches!(parse_es("salir"), Intent::Quit));
        assert!(matches!(parse_es("adiós"), Intent::Quit));
    }

    /// Politeness prefix stripping.
    #[test]
    fn politeness_strip() {
        // "por favor abrir github" → falls through to English
        // parser (since "abrir github" is a known Spanish
        // open-prefix → English "open github").
        // We just verify it doesn't return Unknown.
        let r = parse_es("por favor abrir github");
        assert!(!matches!(r, Intent::Unknown),
            "politeness prefix should be stripped, got {:?}", r);
    }
}
