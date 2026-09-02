//! Super-resolution for rendered output, behind the `upscale` feature.
//!
//! The engine is `apps/dandere2x`'s headless core — block-matched residual
//! upscaling, where only the parts of a frame that changed go through the CNN.
//! That is the right shape for a visualiser: most of a beatspace frame is
//! unchanged background between beats, so the residual gate does most of the
//! work for free.
//!
//! **Off by default, and the refusal is the feature.** The same posture as
//! `opus-decode`: a build without it still compiles, and an upscale request
//! comes back with a message naming the flag rather than a link error. The
//! reason to keep it off is not size of code but size of MODELS — the useful
//! photo checkpoints are 67 MB each and are not in this repo.

/// Where the model came from, so a run can say it.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// Resolved from the operator's model directory.
    Found(std::path::PathBuf),
    /// The 2.5 MB default that dandere2x vendors.
    Vendored(std::path::PathBuf),
}

/// Why an upscale could not run. Every variant names the remedy, because
/// "upscaling failed" is not something an operator can act on.
#[derive(Debug, Clone, PartialEq)]
pub enum Unavailable {
    /// Built without the feature.
    NotCompiledIn,
    /// Feature on, no model found.
    NoModel { looked_in: Vec<String> },
}

impl Unavailable {
    pub fn message(&self) -> String {
        match self {
            Unavailable::NotCompiledIn =>
                "upscaling is not in this build — rebuild with `--features upscale` \
                 (or `upscale-gpu` for the wgpu path)".into(),
            Unavailable::NoModel { looked_in } => format!(
                "upscaling is available but no model was found. Put a .safetensors model in \
                 ~/models/upscale/ (realesr-animevideov3 is the small default; RealESRGAN_x2 \
                 is the 67 MB photo model), or set DANDERE2X_MODELS. Looked in: {}",
                looked_in.join(", ")),
        }
    }
}

/// Is upscaling usable right now, and with which model?
///
/// Returns `Err` with an actionable message in both the not-built and
/// no-model cases, so a caller has one thing to show the user either way.
#[cfg(feature = "upscale")]
pub fn probe() -> Result<Source, Unavailable> {
    // Model resolution belongs to dandere2x, not here — a second search path
    // would drift from the first, which is the failure this monorepo keeps
    // hitting. Ask it.
    let dirs: Vec<String> = d2x_core::model_search_dirs_display();
    match d2x_core::resolve_default_model() {
        Some((path, vendored)) => Ok(if vendored { Source::Vendored(path) } else { Source::Found(path) }),
        None => Err(Unavailable::NoModel { looked_in: dirs }),
    }
}

#[cfg(not(feature = "upscale"))]
pub fn probe() -> Result<Source, Unavailable> {
    Err(Unavailable::NotCompiledIn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_build_without_the_feature_says_which_flag_to_use() {
        // The test runs in both configurations; only the not-built branch has
        // a fixed answer, and that answer must name the flag.
        #[cfg(not(feature = "upscale"))]
        {
            let e = probe().unwrap_err();
            assert_eq!(e, Unavailable::NotCompiledIn);
            assert!(e.message().contains("--features upscale"), "{}", e.message());
        }
    }

    #[test]
    fn every_refusal_names_a_remedy_not_just_a_problem() {
        for e in [
            Unavailable::NotCompiledIn,
            Unavailable::NoModel { looked_in: vec!["/home/x/models".into()] },
        ] {
            let m = e.message();
            assert!(m.contains("--features") || m.contains("~/models"),
                "a refusal with no remedy is not actionable: {m}");
        }
    }

    #[test]
    fn the_no_model_message_lists_where_it_looked() {
        // Otherwise the operator cannot tell a wrong directory from a wrong
        // filename, which is the whole question at that moment.
        let m = Unavailable::NoModel { looked_in: vec!["/a".into(), "/b".into()] }.message();
        assert!(m.contains("/a") && m.contains("/b"), "{m}");
    }
}
