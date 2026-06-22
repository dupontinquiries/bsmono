// src/voice_intent_fr.rs
//
// French intent parser. Mirrors voice_intent::parse with French
// keywords. v0.1.115.
//
// Falls back to English parser for unrecognized utterances.

use crate::voice_intent::Intent;

fn normalize_fr(text: &str) -> String {
    let mut t = text.to_string();

    let prefixes: &[&str] = &[
        // Polite questions (pourrais-tu, pourriez-vous, peux-tu, ...)
        "pourrais-tu s'il te plaît ",
        "pourrais-tu s'il te plait ",
        "pourriez-vous s'il vous plaît ",
        "pourriez-vous s'il vous plait ",
        "peux-tu s'il te plaît ",
        "peux-tu s'il te plait ",
        "pouvez-vous s'il vous plaît ",
        "pouvez-vous s'il vous plait ",
        "pourrais-tu ",
        "pourriez-vous ",
        "peux-tu ",
        "pouvez-vous ",
        // First-person wishes
        "je voudrais ",
        "j'aimerais ",
        "je veux ",
        "j'ai besoin de ",
        "je dois ",
        // Permissive
        "laisse-moi ",
        "permettez-moi de ",
        "allons ",
        // Modal-only
        "s'il te plaît ",
        "s'il te plait ",
        "s'il vous plaît ",
        "s'il vous plait ",
        "stp ",
        "svp ",
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
        " s'il te plaît",
        " s'il te plait",
        " s'il vous plaît",
        " s'il vous plait",
        " stp",
        " svp",
        " merci",
        " vite",
        " rapidement",
        " maintenant",
        " tout de suite",
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

pub fn parse_fr(text: &str) -> Intent {
    let t = text.trim().to_lowercase();
    if t.is_empty() { return Intent::Unknown; }

    let t = normalize_fr(&t);
    if t.is_empty() { return Intent::Unknown; }
    let t = t.trim_end_matches('.').trim_end_matches('!').trim_end_matches('?');
    let t = t.trim();

    // Quit
    if matches!(t,
        "quitter" | "fermer" | "ferme" | "fermer l'application" | "fermer l'app"
        | "éteindre" | "eteindre" | "au revoir" | "bye"
    ) {
        return Intent::Quit;
    }

    // Navigation history
    if matches!(t,
        "retour" | "retourner" | "page précédente" | "page precedente"
        | "précédent" | "precedent" | "revenir" | "reculer" | "en arrière" | "en arriere"
    ) {
        return Intent::GoBack;
    }
    if matches!(t,
        "suivant" | "avancer" | "aller en avant" | "page suivante" | "en avant"
    ) {
        return Intent::GoForward;
    }
    if matches!(t,
        "recharger" | "actualiser" | "rafraîchir" | "rafraichir"
        | "recharger la page" | "actualiser la page" | "réessayer" | "reessayer"
    ) {
        return Intent::Reload;
    }

    // Scrolling
    if matches!(t,
        "défiler vers le bas" | "defiler vers le bas" | "vers le bas"
        | "descendre" | "un peu plus bas" | "plus bas" | "bas"
    ) { return Intent::Scroll(60); }
    if matches!(t,
        "défiler vers le haut" | "defiler vers le haut" | "vers le haut"
        | "monter" | "un peu plus haut" | "plus haut" | "haut"
    ) { return Intent::Scroll(-60); }
    if matches!(t,
        "page suivante en bas" | "une page en bas" | "page vers le bas"
    ) { return Intent::ScrollPage(1); }
    if matches!(t,
        "page précédente en haut" | "page precedente en haut"
        | "une page en haut" | "page vers le haut"
    ) { return Intent::ScrollPage(-1); }
    if matches!(t,
        "aller en haut" | "tout en haut" | "haut de la page" | "le début" | "le debut"
        | "au début" | "au debut" | "début" | "debut"
    ) { return Intent::GoTop; }
    if matches!(t,
        "aller en bas" | "tout en bas" | "bas de la page" | "la fin"
        | "à la fin" | "a la fin" | "fin"
    ) { return Intent::GoBottom; }

    // "Open <site>" — French verbs
    if let Some(rest) = t.strip_prefix("ouvrir ")
        .or_else(|| t.strip_prefix("ouvre "))
        .or_else(|| t.strip_prefix("aller à "))
        .or_else(|| t.strip_prefix("aller a "))
        .or_else(|| t.strip_prefix("va à "))
        .or_else(|| t.strip_prefix("va a "))
        .or_else(|| t.strip_prefix("visiter "))
        .or_else(|| t.strip_prefix("visite "))
        .or_else(|| t.strip_prefix("naviguer vers "))
        .or_else(|| t.strip_prefix("amène-moi à "))
        .or_else(|| t.strip_prefix("amene-moi a "))
        .or_else(|| t.strip_prefix("montre-moi "))
        .or_else(|| t.strip_prefix("charger "))
        .or_else(|| t.strip_prefix("charge "))
    {
        let rest = rest.trim();
        if !rest.is_empty() {
            return crate::voice_intent::parse(&format!("open {}", rest));
        }
    }

    // Search the web. French verbs.
    if let Some(q) = t.strip_prefix("chercher ")
        .or_else(|| t.strip_prefix("cherche "))
        .or_else(|| t.strip_prefix("rechercher "))
        .or_else(|| t.strip_prefix("recherche "))
        .or_else(|| t.strip_prefix("trouver "))
        .or_else(|| t.strip_prefix("trouve "))
        .or_else(|| t.strip_prefix("googler "))
        .or_else(|| t.strip_prefix("google "))
    {
        let q = q.trim_start_matches("pour ").trim();
        if !q.is_empty() { return Intent::SearchWeb(q.to_string()); }
    }

    // Fall back to English parser.
    crate::voice_intent::parse(text)
}
