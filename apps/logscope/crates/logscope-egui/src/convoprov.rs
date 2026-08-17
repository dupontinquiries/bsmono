//! convoprov.rs — showing WHICH model said what, and only where it changed.
//!
//! The operator switches models constantly — a single session in the logs moves between
//! Llama-3.2-1B, Qwen3.6-35B, DeepSeek-R1-Distill and LLaDA2 — and the chat records none of it. A
//! conversation whose first half came from a 1B and second half from a 35B looks like one model
//! having a bad day, and the replies that exposed the template-mismatch bugs were read for several
//! turns before anyone asked which model had produced them.
//!
//! The rule that makes this useful rather than noisy is the same one the diagnostics badge uses:
//! mark the CHANGE, not the state. Labelling every message with its model turns a conversation into
//! a table; labelling the switches turns it into a timeline, and a timeline is what someone reading
//! back through a session actually needs.

/// Where a marker belongs, and what it should say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Marker {
    /// First model of the conversation. Worth naming once so the reader has an anchor.
    Opened(String),
    /// The model changed between this message and the previous one.
    Switched { from: String, to: String },
}

/// For each message, the marker to draw ABOVE it, if any.
///
/// Takes the model that produced each message; `None` for messages with no model (the user's own
/// turns, or replies from before this was recorded). Unknown provenance is skipped rather than
/// guessed at — an assistant message with no recorded model is exactly the case where inferring
/// "same as before" would be wrong, because that is what an older session looks like.
pub fn markers(models: &[Option<&str>]) -> Vec<Option<Marker>> {
    let mut out = vec![None; models.len()];
    let mut current: Option<&str> = None;
    for (i, m) in models.iter().enumerate() {
        let Some(m) = m else { continue };
        match current {
            None => { out[i] = Some(Marker::Opened(m.to_string())); current = Some(m); }
            Some(prev) if prev != *m => {
                out[i] = Some(Marker::Switched { from: prev.to_string(), to: m.to_string() });
                current = Some(m);
            }
            _ => {}
        }
    }
    out
}

/// A model filename reduced to what distinguishes it in a chat margin.
///
/// GGUF names carry a quantisation, a shard suffix and an extension that are identical across the
/// files someone actually switches between — "Qwen3.6-35B-A3B-UD-Q4_K_M.gguf" and
/// "Qwen3.5-35B-A3B-UD-Q4_K_M.gguf" differ in one character before all that. Trimming the tail keeps
/// the part that differs; trimming the head would not.
pub fn short_model(name: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or(name);
    let base = base.strip_suffix(".gguf").unwrap_or(base);
    // Drop a trailing quantisation tag, which is noise when comparing two models in one session.
    let cut = ["-UD-Q4_K_M", "-Q4_K_M", "-Q4_K_S", "-Q8_0", "-Q4_0", "-Q6_K", "-Q5_K_M",
               "-MXFP4_MOE", "-IQ1_S", "-f16", "-F16"];
    let mut s = base.to_string();
    for c in cut {
        if let Some(t) = s.strip_suffix(c) { s = t.to_string(); break; }
    }
    s
}

impl Marker {
    /// One line for the margin.
    pub fn label(&self) -> String {
        match self {
            Marker::Opened(m) => short_model(m),
            // Both names, because "switched to Qwen3.6" leaves the reader scrolling up to find what
            // it switched FROM — which is the question a switch marker exists to answer.
            Marker::Switched { from, to } => format!("{} → {}", short_model(from), short_model(to)),
        }
    }
    pub fn is_switch(&self) -> bool { matches!(self, Marker::Switched { .. }) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_model_is_named_once() {
        let m = markers(&[Some("a.gguf"), None, Some("a.gguf"), Some("a.gguf")]);
        assert_eq!(m[0], Some(Marker::Opened("a.gguf".into())));
        assert!(m[1..].iter().all(|x| x.is_none()), "no marker while nothing changes: {m:?}");
    }

    /// The point of the whole module: mark the change, not the state.
    #[test]
    fn only_the_switch_is_marked() {
        let m = markers(&[Some("a.gguf"), Some("a.gguf"), Some("b.gguf"), Some("b.gguf"), Some("a.gguf")]);
        assert!(matches!(m[0], Some(Marker::Opened(_))));
        assert_eq!(m[1], None);
        assert_eq!(m[2], Some(Marker::Switched { from: "a.gguf".into(), to: "b.gguf".into() }));
        assert_eq!(m[3], None);
        assert_eq!(m[4], Some(Marker::Switched { from: "b.gguf".into(), to: "a.gguf".into() }),
            "switching BACK is still a switch");
    }

    /// User turns carry no model and must not break the run — a marker on every other message would
    /// be worse than none.
    #[test]
    fn messages_without_a_model_are_transparent() {
        let m = markers(&[None, Some("a.gguf"), None, Some("a.gguf"), None, Some("b.gguf")]);
        assert_eq!(m[0], None, "a user turn opens nothing");
        assert!(matches!(m[1], Some(Marker::Opened(_))));
        assert_eq!(m[3], None, "the user turn between did not change the model");
        assert!(m[5].as_ref().unwrap().is_switch());
    }

    /// An older conversation has no recorded models at all and must produce no markers, rather than
    /// inventing an "Opened" for whatever is loaded now.
    #[test]
    fn a_conversation_with_no_provenance_is_left_alone() {
        assert!(markers(&[None, None, None]).iter().all(|x| x.is_none()));
        assert!(markers(&[]).is_empty());
    }

    /// The distinguishing part of a GGUF name is at the FRONT; the quantisation tail is shared.
    #[test]
    fn shortening_keeps_what_differs_between_two_models() {
        let a = short_model("/home/k/models/Qwen3.6-35B-A3B-UD-Q4_K_M.gguf");
        let b = short_model("/home/k/models/Qwen3.5-35B-A3B-UD-Q4_K_M.gguf");
        assert_eq!(a, "Qwen3.6-35B-A3B");
        assert_ne!(a, b, "two models a session switches between must not render identically");
        assert_eq!(short_model("Llama-3.2-1B-Instruct-Q4_K_M.gguf"), "Llama-3.2-1B-Instruct");
        assert_eq!(short_model("plain-model"), "plain-model");
    }

    /// A switch label names both sides, or the reader scrolls up to find what it came from.
    #[test]
    fn a_switch_label_names_both_models() {
        let l = Marker::Switched { from: "Llama-3.2-1B-Instruct-Q4_K_M.gguf".into(),
                                   to: "Qwen3.6-35B-A3B-UD-Q4_K_M.gguf".into() }.label();
        assert_eq!(l, "Llama-3.2-1B-Instruct → Qwen3.6-35B-A3B");
    }
}
