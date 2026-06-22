// src/voice_intent_pt.rs
//
// Portuguese intent parser. Mirrors voice_intent::parse (and the
// Spanish/French/Italian parsers) with Portuguese keywords. v0.1.872.
//
// Coverage: navigation, scrolling, quit, search, open-site, history,
// the "olha X" → cd shortcut, and the common politeness / wish /
// conditional prefixes and trailing softeners.
//
// Quality note: keyword lists lean toward Brazilian Portuguese but
// include European variants where they differ (e.g. "ecrã" is not
// used; navigation verbs are shared). The structure mirrors the other
// Romance parsers for easy diffing — extend the matches!/or_else chains.
//
// Falls back to the English parser for utterances that don't match
// Portuguese keywords — useful for mixed-language users who say e.g.
// "abrir GitHub" (Portuguese verb + proper noun).

use crate::voice_intent::Intent;

/// Strip Portuguese politeness prefixes, first-person wishes, and
/// trailing softeners.
fn normalize_pt(text: &str) -> String {
    let mut t = text.to_string();

    let prefixes: &[&str] = &[
        // Polite questions
        "você poderia por favor ",
        "voce poderia por favor ",
        "poderia por favor ",
        "pode por favor ",
        "poderia ",
        "pode ",
        // First-person wishes
        "eu gostaria de ",
        "gostaria de ",
        "eu quero ",
        "quero ",
        "eu preciso de ",
        "preciso de ",
        "eu preciso ",
        "preciso ",
        // Permissive / hortative
        "vamos ",
        "deixa eu ",
        "deixe-me ",
        "me deixe ",
        // Modal-only
        "por favor ",
        "por gentileza ",
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
        " por favor",
        " por gentileza",
        " obrigado",
        " obrigada",
        " rápido",
        " rapido",
        " agora mesmo",
        " agora",
        " se puder",
        " se você puder",
        " se voce puder",
        " se não se importar",
        " se nao se importar",
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

/// Parse Portuguese text into an Intent. Falls back to the English
/// parser for unrecognized utterances.
pub fn parse_pt(text: &str) -> Intent {
    let t = text.trim().to_lowercase();
    if t.is_empty() { return Intent::Unknown; }

    let t = normalize_pt(&t);
    if t.is_empty() { return Intent::Unknown; }
    let t = t.trim_end_matches('.').trim_end_matches('!').trim_end_matches('?');
    let t = t.trim();

    // Quit. ("tchau" is unambiguously a farewell in Portuguese, unlike
    // Italian "ciao", so it's safe to include.)
    if matches!(t,
        "sair" | "fechar" | "fechar app" | "fechar o aplicativo" | "fechar aplicativo"
        | "desligar" | "adeus" | "tchau" | "até logo" | "ate logo" | "até mais" | "ate mais"
    ) {
        return Intent::Quit;
    }

    // Navigation history.
    if matches!(t,
        "voltar" | "volte" | "retroceder" | "voltar atrás" | "voltar atras"
        | "ir para trás" | "ir para tras" | "para trás" | "para tras"
        | "página anterior" | "pagina anterior" | "anterior" | "retornar"
    ) {
        return Intent::GoBack;
    }
    if matches!(t,
        "avançar" | "avancar" | "avance" | "ir para frente" | "para frente"
        | "próxima" | "proxima" | "próxima página" | "proxima pagina"
        | "página seguinte" | "pagina seguinte" | "seguinte"
    ) {
        return Intent::GoForward;
    }
    if matches!(t,
        "recarregar" | "recarrega" | "atualizar" | "atualiza"
        | "recarregar página" | "recarregar pagina" | "recarregar a página" | "recarregar a pagina"
        | "atualizar página" | "atualizar pagina" | "tentar de novo"
    ) {
        return Intent::Reload;
    }

    // Scrolling.
    if matches!(t,
        "abaixo" | "desça" | "desca" | "descer" | "rolar para baixo" | "para baixo"
        | "um pouco abaixo" | "desça um pouco" | "desca um pouco"
    ) { return Intent::Scroll(60); }
    if matches!(t,
        "acima" | "suba" | "subir" | "rolar para cima" | "para cima"
        | "um pouco acima" | "suba um pouco"
    ) { return Intent::Scroll(-60); }
    if matches!(t,
        "página abaixo" | "pagina abaixo" | "uma página abaixo" | "uma pagina abaixo"
    ) { return Intent::ScrollPage(1); }
    if matches!(t,
        "página acima" | "pagina acima" | "uma página acima" | "uma pagina acima"
    ) { return Intent::ScrollPage(-1); }
    if matches!(t,
        "ir para o início" | "ir para o inicio" | "início" | "inicio"
        | "ao início" | "ao inicio" | "no topo" | "ir ao topo" | "topo"
    ) { return Intent::GoTop; }
    if matches!(t,
        "ir para o fim" | "fim" | "ao fim" | "final" | "ao final"
        | "lá embaixo" | "la embaixo" | "bem embaixo"
    ) { return Intent::GoBottom; }

    // "olha X" → cd into X (Portuguese parallel to Spanish "mira X").
    // Strict prefix — "olha aqui" ("look here") can appear legitimately.
    if let Some(rest) = t.strip_prefix("olha ")
        .or_else(|| t.strip_prefix("olhe "))
    {
        let rest = rest.trim();
        if !rest.is_empty() {
            return Intent::TerminalCommand(format!("cd {}", rest));
        }
    }

    // "Open <site>" — Portuguese verbs.
    if let Some(rest) = t.strip_prefix("abrir ")
        .or_else(|| t.strip_prefix("abra "))
        .or_else(|| t.strip_prefix("abre "))
        .or_else(|| t.strip_prefix("ir para "))
        .or_else(|| t.strip_prefix("vá para "))
        .or_else(|| t.strip_prefix("va para "))
        .or_else(|| t.strip_prefix("vá a "))
        .or_else(|| t.strip_prefix("visitar "))
        .or_else(|| t.strip_prefix("visite "))
        .or_else(|| t.strip_prefix("navegar para "))
        .or_else(|| t.strip_prefix("navegue para "))
        .or_else(|| t.strip_prefix("me leve para "))
        .or_else(|| t.strip_prefix("me leve a "))
        .or_else(|| t.strip_prefix("mostre "))
        .or_else(|| t.strip_prefix("mostra "))
        .or_else(|| t.strip_prefix("carregar "))
        .or_else(|| t.strip_prefix("carregue "))
    {
        let rest = rest.trim();
        if !rest.is_empty() {
            return crate::voice_intent::parse(&format!("open {}", rest));
        }
    }

    // Search the web. Portuguese verbs.
    if let Some(q) = t.strip_prefix("buscar ")
        .or_else(|| t.strip_prefix("busca "))
        .or_else(|| t.strip_prefix("busque "))
        .or_else(|| t.strip_prefix("procurar "))
        .or_else(|| t.strip_prefix("procure "))
        .or_else(|| t.strip_prefix("pesquisar "))
        .or_else(|| t.strip_prefix("pesquise "))
        .or_else(|| t.strip_prefix("pesquisar por "))
        .or_else(|| t.strip_prefix("encontrar "))
        .or_else(|| t.strip_prefix("encontre "))
    {
        let q = q.trim_start_matches("por ").trim();
        if !q.is_empty() { return Intent::SearchWeb(q.to_string()); }
    }

    crate::voice_intent::parse(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn olha_routes_to_cd() {
        match parse_pt("olha kwin") {
            Intent::TerminalCommand(cmd) => assert_eq!(cmd, "cd kwin"),
            other => panic!("expected TerminalCommand(cd kwin), got {:?}", other),
        }
        match parse_pt("olhe projetos") {
            Intent::TerminalCommand(cmd) => assert_eq!(cmd, "cd projetos"),
            other => panic!("expected TerminalCommand(cd projetos), got {:?}", other),
        }
    }

    #[test]
    fn scroll_basics() {
        assert!(matches!(parse_pt("abaixo"), Intent::Scroll(60)));
        assert!(matches!(parse_pt("desça"), Intent::Scroll(60)));
        assert!(matches!(parse_pt("acima"), Intent::Scroll(-60)));
        assert!(matches!(parse_pt("suba"), Intent::Scroll(-60)));
        assert!(matches!(parse_pt("página abaixo"), Intent::ScrollPage(1)));
        assert!(matches!(parse_pt("topo"), Intent::GoTop));
        assert!(matches!(parse_pt("fim"), Intent::GoBottom));
    }

    #[test]
    fn nav_basics() {
        assert!(matches!(parse_pt("voltar"), Intent::GoBack));
        assert!(matches!(parse_pt("voltar atrás"), Intent::GoBack));
        assert!(matches!(parse_pt("avançar"), Intent::GoForward));
        assert!(matches!(parse_pt("próxima página"), Intent::GoForward));
        assert!(matches!(parse_pt("recarregar"), Intent::Reload));
    }

    /// "ir para frente"/"ir para o início" must beat the "ir para "
    /// open-prefix (exact nav/top matches run first).
    #[test]
    fn ir_para_disambiguation() {
        assert!(matches!(parse_pt("ir para frente"), Intent::GoForward));
        assert!(matches!(parse_pt("ir para trás"), Intent::GoBack));
        assert!(matches!(parse_pt("ir para o início"), Intent::GoTop));
        assert!(matches!(parse_pt("ir para o fim"), Intent::GoBottom));
    }

    #[test]
    fn quit_basics() {
        assert!(matches!(parse_pt("sair"), Intent::Quit));
        assert!(matches!(parse_pt("fechar"), Intent::Quit));
        assert!(matches!(parse_pt("tchau"), Intent::Quit));
        assert!(matches!(parse_pt("adeus"), Intent::Quit));
    }

    #[test]
    fn search_basics() {
        match parse_pt("pesquisar gatos") {
            Intent::SearchWeb(q) => assert_eq!(q, "gatos"),
            other => panic!("expected SearchWeb(gatos), got {:?}", other),
        }
        match parse_pt("procurar receitas de bolo") {
            Intent::SearchWeb(q) => assert_eq!(q, "receitas de bolo"),
            other => panic!("expected SearchWeb(...), got {:?}", other),
        }
    }

    #[test]
    fn politeness_strip() {
        let r = parse_pt("por favor abrir github");
        assert!(!matches!(r, Intent::Unknown),
            "politeness prefix should be stripped, got {:?}", r);
    }

    #[test]
    fn trailing_softener_strip() {
        assert!(matches!(parse_pt("desça por favor"), Intent::Scroll(60)));
        assert!(matches!(parse_pt("voltar obrigado"), Intent::GoBack));
    }
}
