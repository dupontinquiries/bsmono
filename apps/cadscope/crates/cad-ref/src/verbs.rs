//! verbs.rs — the one place a reference request becomes an answer.
//!
//! Plan §17.3 says the language model may emit only a constrained query — kind, name, verb — and
//! that Rust performs the lookup and the measuring. That is a promise about *architecture*, and it
//! only holds if there is a single implementation of it. If the UI parses "volume of puma" one way
//! and `cad-liaison` translates a model's JSON another way, the boundary exists twice and will
//! drift, and the drift is exactly where an invented fact gets in.
//!
//! So both go through here. `RefRequest` is the constrained query — three variants, each carrying
//! nothing but a name. `execute` is the only way to turn one into a `RefOutcome`, and every failure
//! path is a typed `RefError` that says what was searched, never a silent empty result.
//!
//! What this module deliberately does not do is *apply* anything. It answers questions; turning an
//! answer into a document op is the caller's decision, made explicitly. A verb that quietly edited
//! the model would be a verb nobody could audit.

use crate::{BoardFit, FitReport, Index, MaterialMaps, RefError};

/// A constrained reference query. Deliberately small: this is the entire vocabulary a language
/// model is permitted to emit, and every field is a name the corpus can be asked about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefRequest {
    /// "make it the same volume as <model>"
    VolumeOf { model: String },
    /// "use the texture from <model>"
    TextureFrom { model: String },
    /// "design this to fit <doc>" — and, when the name turns out to be a board rather than a
    /// document, the same sentence answers with the board. Which one it is, is a corpus lookup
    /// rather than a second word the person has to remember.
    Fit { doc: String },
    /// "design around <board>" — the explicit board phrasing.
    Fits { board: String },
    /// "arms like <model>" — proportions, never geometry.
    Like { model: String },
}

impl RefRequest {
    /// The name being asked about, whichever verb it is.
    pub fn subject(&self) -> &str {
        match self {
            RefRequest::VolumeOf { model }
            | RefRequest::TextureFrom { model }
            | RefRequest::Like { model } => model,
            RefRequest::Fit { doc } => doc,
            RefRequest::Fits { board } => board,
        }
    }
}

/// Parse a typed command. Accepts the colon form used by the command palette (`:volume of puma`)
/// and the bare form, and tolerates the plural spellings people actually type.
///
/// Returns `None` rather than guessing at an unrecognised verb: offering the wrong reference is
/// worse than admitting the sentence was not understood.
pub fn parse_request(input: &str) -> Option<RefRequest> {
    let s = input.trim().trim_start_matches(':').trim();
    let lower = s.to_ascii_lowercase();
    // Connectives are never names. Without this, the shorter prefixes swallow them: "volume of"
    // matches the bare `volume ` form and yields a model helpfully called "of", which would then be
    // reported as not indexed — a confusing refusal for what is really an incomplete sentence.
    const NOT_A_NAME: [&str; 6] = ["of", "as", "from", "to", "the", "a"];
    let rest_after = |prefix: &str| -> Option<String> {
        let r = lower.strip_prefix(prefix)?;
        // Take the name from the ORIGINAL string so its case survives; corpus lookup is
        // case-insensitive but the name is echoed back to a person.
        let name = s[s.len() - r.len()..].trim();
        if name.is_empty() || NOT_A_NAME.contains(&name.to_ascii_lowercase().as_str()) {
            return None;
        }
        Some(name.to_string())
    };
    for p in ["volume of ", "same volume as ", "volume "] {
        if let Some(model) = rest_after(p) {
            return Some(RefRequest::VolumeOf { model });
        }
    }
    for p in ["textures from ", "texture from ", "texture of ", "textures "] {
        if let Some(model) = rest_after(p) {
            return Some(RefRequest::TextureFrom { model });
        }
    }
    for p in ["proportions of ", "shaped like ", "like "] {
        if let Some(model) = rest_after(p) {
            return Some(RefRequest::Like { model });
        }
    }
    for p in ["design around ", "around ", "board "] {
        if let Some(board) = rest_after(p) {
            return Some(RefRequest::Fits { board });
        }
    }
    for p in ["fits ", "fit "] {
        if let Some(doc) = rest_after(p) {
            return Some(RefRequest::Fit { doc });
        }
    }
    None
}

/// What a reference request produced. `Refused` is a first-class outcome, not an error case to be
/// swallowed — most of the corpus cannot answer most questions, and saying so precisely is the
/// feature.
#[derive(Debug)]
pub enum RefOutcome<'a> {
    /// A measured volume in cubic millimetres. The caller may turn this into `Op::MatchVolume`.
    Volume { name: String, mm3: f32 },
    /// The material maps a model references, including the ones nothing implements.
    Textures { model: String, maps: Vec<MaterialMaps<'a>> },
    Fit(FitReport),
    /// A board outline and its keep-outs, in millimetres.
    Board(BoardFit),
    /// A reference form's proportions, ready to be handed to Grow.
    Proportions(crate::shape::Proportions),
    Refused(RefError),
}

impl RefOutcome<'_> {
    pub fn refused(&self) -> Option<&RefError> {
        match self {
            RefOutcome::Refused(e) => Some(e),
            _ => None,
        }
    }

    /// A one-line summary for a status bar or a chat reply.
    pub fn summary(&self) -> String {
        match self {
            RefOutcome::Volume { name, mm3 } => format!("{name}: {mm3:.1} mm³"),
            RefOutcome::Textures { model, maps } => {
                let found: usize = maps.iter().map(|m| m.found()).sum();
                let missing = maps.iter().filter(|m| m.unresolved()).count();
                format!(
                    "{model}: {} material(s), {found} map(s){}",
                    maps.len(),
                    if missing > 0 { format!(", {missing} unresolved") } else { String::new() }
                )
            }
            RefOutcome::Board(b) => format!(
                "{}: {:.0}x{:.0} mm, {} component(s), {} keep-out(s)",
                b.board,
                b.size_mm.x,
                b.size_mm.y,
                b.components,
                b.keepouts.len()
            ),
            RefOutcome::Proportions(p) => format!(
                "{}: slenderness {:.2}, symmetry {:.2}/{:.2}, {} stations",
                p.name,
                p.slenderness(),
                p.symmetry,
                p.symmetry_worst,
                p.radii.len()
            ),
            RefOutcome::Fit(r) => format!(
                "{}: {} within, {} exceed, {} untestable",
                r.doc,
                r.count(crate::Verdict::Within),
                r.count(crate::Verdict::Exceeds),
                r.lines.len()
                    - r.count(crate::Verdict::Within)
                    - r.count(crate::Verdict::Exceeds)
            ),
            RefOutcome::Refused(e) => e.to_string(),
        }
    }
}

/// Execute a request against the corpus. `part_extent_mm` is the current part's bounding extents,
/// needed only by `Fit`; passing `None` makes every fit line report as unmeasured rather than
/// silently passing.
pub fn execute<'a>(
    idx: &'a Index,
    req: &RefRequest,
    part_extent_mm: Option<[f32; 3]>,
) -> RefOutcome<'a> {
    match req {
        RefRequest::VolumeOf { model } => match idx.volume_mm3(model) {
            Ok(mm3) => RefOutcome::Volume { name: model.clone(), mm3 },
            Err(e) => RefOutcome::Refused(e),
        },
        RefRequest::TextureFrom { model } => {
            let maps = idx.textures_of_model(model);
            if maps.is_empty() {
                // Distinguish "no such model" from "a model that references no materials", because
                // they call for completely different next steps.
                let nearest = idx
                    .resolve(model, Some(crate::Kind::Model), 3)
                    .into_iter()
                    .map(|(e, _)| e.name.clone())
                    .collect::<Vec<_>>();
                return RefOutcome::Refused(if idx.exact(model).is_empty() {
                    RefError::NotIndexed { name: model.clone(), searched: idx.len(), nearest }
                } else {
                    RefError::NoGeometry { name: model.clone() }
                });
            }
            RefOutcome::Textures { model: model.clone(), maps }
        }
        RefRequest::Like { model } => match idx.like(model, 12) {
            Ok(p) => RefOutcome::Proportions(p),
            Err(e) => RefOutcome::Refused(e),
        },
        RefRequest::Fits { board } => match idx.fits(board) {
            Ok(b) => RefOutcome::Board(b),
            Err(e) => RefOutcome::Refused(e),
        },
        // "fits X" is natural for both a spec and a board. Rather than make someone remember which
        // word goes with which kind, resolve it by what X actually is in the corpus — a lookup, so
        // still deterministic, and still one implementation.
        RefRequest::Fit { doc }
            if idx.exact(doc).iter().any(|e| e.kind == crate::Kind::Niftyplot)
                && !idx.exact(doc).iter().any(|e| e.kind == crate::Kind::Doc) =>
        {
            match idx.fits(doc) {
                Ok(b) => RefOutcome::Board(b),
                Err(e) => RefOutcome::Refused(e),
            }
        }
        RefRequest::Fit { doc } => match idx.fit_check(doc, part_extent_mm) {
            Ok(r) => RefOutcome::Fit(r),
            Err(e) => RefOutcome::Refused(e),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_new_verbs_parse_in_the_words_people_use() {
        assert!(matches!(parse_request("arms like velgorn"), None), "a leading noun is not yet handled");
        assert!(matches!(parse_request("like velgorn"), Some(RefRequest::Like { model }) if model == "velgorn"));
        assert!(matches!(parse_request("shaped like Puma"), Some(RefRequest::Like { model }) if model == "Puma"));
        assert!(matches!(parse_request("proportions of puma"), Some(RefRequest::Like { .. })));
        assert!(matches!(parse_request("design around sensor_hub"), Some(RefRequest::Fits { board }) if board == "sensor_hub"));
        assert!(matches!(parse_request("board sensor_hub"), Some(RefRequest::Fits { .. })));
        // Incomplete sentences stay unparsed rather than naming a connective.
        assert!(parse_request("like").is_none());
        assert!(parse_request("design around the").is_none());
    }


    #[test]
    fn parses_the_spoken_forms() {
        assert_eq!(
            parse_request("volume of puma"),
            Some(RefRequest::VolumeOf { model: "puma".into() })
        );
        assert_eq!(
            parse_request(":same volume as Great_White_Shark"),
            Some(RefRequest::VolumeOf { model: "Great_White_Shark".into() }),
            "case must survive so the name can be echoed back"
        );
        assert_eq!(
            parse_request("  texture from  scorponok_body "),
            Some(RefRequest::TextureFrom { model: "scorponok_body".into() })
        );
        assert_eq!(
            parse_request("textures from arena"),
            Some(RefRequest::TextureFrom { model: "arena".into() })
        );
        assert_eq!(parse_request("fit enclosure_spec"), Some(RefRequest::Fit { doc: "enclosure_spec".into() }));
    }

    #[test]
    fn refuses_to_guess_at_an_unknown_verb() {
        assert_eq!(parse_request("make it look cool"), None);
        assert_eq!(parse_request("volume of"), None, "a verb with no subject is not a request");
        assert_eq!(parse_request("texture from"), None);
        assert_eq!(parse_request("volume as"), None);
        assert_eq!(parse_request(""), None);
        assert_eq!(parse_request(":"), None);
    }

    #[test]
    fn every_failure_is_a_typed_refusal_not_an_empty_answer() {
        let idx = Index::from_entries(Vec::new());
        for req in [
            RefRequest::VolumeOf { model: "ghost".into() },
            RefRequest::TextureFrom { model: "ghost".into() },
            RefRequest::Fit { doc: "ghost".into() },
        ] {
            let out = execute(&idx, &req, None);
            let e = out.refused().expect("an empty corpus must refuse, not return nothing");
            let msg = e.to_string();
            assert!(msg.contains("ghost"), "a refusal must name what was asked for: {msg}");
            assert!(msg.len() > 20, "a refusal must be useful: {msg}");
        }
    }

    #[test]
    fn subject_is_recoverable_for_every_variant() {
        assert_eq!(RefRequest::VolumeOf { model: "a".into() }.subject(), "a");
        assert_eq!(RefRequest::TextureFrom { model: "b".into() }.subject(), "b");
        assert_eq!(RefRequest::Fit { doc: "c".into() }.subject(), "c");
    }
}
