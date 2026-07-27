//! activemodel.rs — ONE answer to "what am I talking to", and where it came from.
//!
//! Model choice was settable from three places — the picker, the physical dock, and the deployment tiles
//! — with a silent precedence rule buried in one function: the deployment wins when its plan is valid,
//! otherwise the picker does. So changing the picker while a plan was valid did nothing at all, and
//! nothing in the interface said why. Every control looked live and one of them was not.
//!
//! The fix is not only to collapse the sources. It is to make the RESOLUTION VISIBLE: a control that
//! cannot currently take effect has to say so, or it is lying about being a control. `Resolution` carries
//! both the winner and the loser, so the UI can render "picker: qwen3 (overridden by deployment tile)"
//! instead of showing a selector that quietly does nothing.

/// Where the active model came from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    /// A valid deployment plan named it. Outranks the picker.
    Deployment,
    /// The model picker / dock selection.
    Picker,
    /// Nothing is selected anywhere.
    Nothing,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Deployment => "deployment tile", Source::Picker => "picker", Source::Nothing => "—",
        }
    }
}

/// The resolved answer, plus what it displaced.
#[derive(Clone, PartialEq, Debug)]
pub struct Resolution {
    pub name: Option<String>,
    pub source: Source,
    /// The picker's value, when it lost to the deployment AND differs. `None` when the picker is either
    /// winning or agreeing — an "override" notice that fires when both say the same thing is noise, and
    /// noise is what teaches people to ignore notices.
    pub shadowed: Option<String>,
}

impl Resolution {
    /// Is a visible control currently unable to take effect?
    pub fn picker_is_inert(&self) -> bool { self.shadowed.is_some() }

    /// One line for the UI, next to the control that is being overridden.
    pub fn note(&self) -> Option<String> {
        self.shadowed.as_ref().map(|s| format!(
            "picker set to {s}, but the deployment tile selects {} — clear the deployment to use the picker",
            self.name.as_deref().unwrap_or("—")))
    }
}

/// Resolve the one active model.
///
/// `deployment` is `Some` only when the plan is VALID; an invalid plan must not shadow the picker,
/// because then a half-built deployment silently disables model selection and the user has no way to
/// tell that from a broken picker.
pub fn resolve(deployment: Option<&str>, picker: Option<&str>) -> Resolution {
    let picker = picker.map(str::trim).filter(|s| !s.is_empty() && *s != "—");
    match deployment.map(str::trim).filter(|s| !s.is_empty()) {
        Some(d) => Resolution {
            name: Some(d.to_string()),
            source: Source::Deployment,
            shadowed: picker.filter(|p| *p != d).map(str::to_string),
        },
        None => match picker {
            Some(p) => Resolution { name: Some(p.to_string()), source: Source::Picker, shadowed: None },
            None => Resolution { name: None, source: Source::Nothing, shadowed: None },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_valid_deployment_wins_but_SAYS_that_it_did() {
        // The reported symptom: change the picker, nothing happens, no explanation. The override is
        // legitimate; the silence was not.
        let r = resolve(Some("llama4-scout"), Some("qwen3-30b"));
        assert_eq!(r.name.as_deref(), Some("llama4-scout"));
        assert_eq!(r.source, Source::Deployment);
        assert!(r.picker_is_inert());
        let n = r.note().unwrap();
        assert!(n.contains("qwen3-30b") && n.contains("llama4-scout"));
        assert!(n.contains("clear the deployment"), "a notice without a way out is just a complaint");
    }

    #[test]
    fn agreement_is_not_reported_as_an_override() {
        // A notice that fires when both sources say the same thing is noise, and noise is what teaches
        // people to stop reading notices.
        let r = resolve(Some("qwen3-30b"), Some("qwen3-30b"));
        assert_eq!(r.source, Source::Deployment);
        assert!(!r.picker_is_inert());
        assert!(r.note().is_none());
    }

    #[test]
    fn an_invalid_deployment_does_not_shadow_the_picker() {
        // Callers pass None for an invalid plan. If a half-built deployment could shadow, model
        // selection would silently stop working and look exactly like a broken picker.
        let r = resolve(None, Some("qwen3-30b"));
        assert_eq!(r.name.as_deref(), Some("qwen3-30b"));
        assert_eq!(r.source, Source::Picker);
        assert!(!r.picker_is_inert());
    }

    #[test]
    fn nothing_selected_reads_as_nothing_rather_than_an_empty_name() {
        // The picker renders "—" for no selection; that placeholder must not become a model name.
        for p in [None, Some(""), Some("   "), Some("—")] {
            let r = resolve(None, p);
            assert_eq!(r.source, Source::Nothing, "picker {p:?}");
            assert!(r.name.is_none());
        }
    }

    #[test]
    fn a_deployment_with_no_picker_selection_is_not_an_override() {
        let r = resolve(Some("llama4-scout"), None);
        assert_eq!(r.name.as_deref(), Some("llama4-scout"));
        assert!(!r.picker_is_inert(), "nothing was displaced");
    }

    #[test]
    fn the_source_is_nameable_for_a_badge() {
        assert_eq!(Source::Deployment.label(), "deployment tile");
        assert_eq!(Source::Picker.label(), "picker");
        assert_ne!(Source::Deployment.label(), Source::Picker.label());
    }
}
