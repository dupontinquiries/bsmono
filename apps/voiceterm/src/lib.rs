//! web2qml — the pure half of voiceterm, as a library.
//!
//! ## Why this file exists
//!
//! `tests/ai_training.rs` opens with `use web2qml::{dead_air, dumb_summary, …}`
//! and this crate had **no library target at all** — only `src/main.rs`. An
//! integration test under `tests/` can only import its own crate as a library,
//! so that file could never compile, and a fleet build reported it:
//!
//! ```text
//! error[E0433]: cannot find module or crate `web2qml` in this scope
//!   --> tests/ai_training.rs:29:5
//! error: could not compile `web2qml` (test "ai_training") due to 2 previous errors
//! ```
//!
//! The test was not wrong about what it wanted. Its own header states the rule
//! this file implements: *"every entry must be a pure module (no FFI / Qt /
//! unrelated crate deps) so the suite stays runnable in any minimal sandbox."*
//! Nine modules, each `mod`-declared unconditionally in `main.rs`, none with a
//! single non-std import between them. They were always the pure half; there
//! was just no door to them.
//!
//! ## What belongs here, and what must not
//!
//! Only modules that compile with no Qt, no qmetaobject, no whisper-rs and no
//! feature flags. All nine qualify: not one has a non-std `use` between them.
//!
//! ## BUT THE CRATE STILL NEEDS QT, and this target does not change that
//!
//! Stated plainly because the obvious reading of this file is wrong. The
//! MODULES are pure; the PACKAGE is not. `qmetaobject = "0.2"` is an
//! unconditional dependency, so `cargo build --lib` still runs qttypes'
//! build script and still fails on a machine without Qt:
//!
//! ```text
//! error: failed to run custom build command for `qttypes v0.2.12`
//! ```
//!
//! So this fixes the reported build failure — `tests/ai_training.rs` can now
//! resolve `web2qml::` on a machine that has Qt — and does NOT yet deliver the
//! goal that test's header states, "runnable in any minimal sandbox".
//!
//! Getting there means making qmetaobject optional behind a default-on feature,
//! the way this crate already treats whisper-rs (`voice`) and the way
//! beatspace treats cpal (`app`). That is a larger change than it sounds:
//! `speaker_embed`, `peer_handshake`, `audio_dialup` and `llm_provider` all
//! import qmetaobject too, and the binary would need `required-features`. It
//! is the right follow-up, on a machine that can compile the binary to check
//! the other half.
//!
//! ## The duplicate-compilation cost, stated
//!
//! `main.rs` keeps its own `mod dead_air;` and so on, so these nine modules are
//! compiled twice — once into the library, once into the binary. The clean fix
//! is for `main.rs` to `use web2qml::…` instead, which is a real refactor of a
//! file that needs Qt to build and therefore cannot be verified here. Nine
//! std-only modules is a small price for the test suite existing at all, and
//! the refactor belongs with the feature-gating above.

pub mod dead_air;
pub mod dumb_summary;
pub mod llm_coalesce;
pub mod llm_scheduler;
pub mod mic_pipeline_fsm;
pub mod mic_scheduler;
pub mod mic_topology;
pub mod util;
pub mod whisper_models;

#[cfg(test)]
mod purity {
    /// Every module this target exposes is std-only.
    ///
    /// This target exists so voiceterm's logic CAN eventually be tested without
    /// Qt — the package is not there yet (see the header), and the one thing
    /// that would make it unreachable is a module here growing a non-std
    /// import. Then the feature-gating follow-up would have nothing pure left
    /// to expose, and the failure would read as "the Qt gating did not work"
    /// rather than "someone added a dependency to a module that must not have
    /// one".
    ///
    /// Reads the sources rather than the module tree, because the property is
    /// about what they IMPORT, and a `cfg`-gated import still counts on the
    /// platform where the gate opens.
    #[test]
    fn every_exposed_module_is_std_only() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let exposed: Vec<&str> = include_str!("lib.rs")
            .lines()
            .filter_map(|l| l.strip_prefix("pub mod "))
            .filter_map(|l| l.strip_suffix(';'))
            .collect();
        assert_eq!(exposed.len(), 9, "module list changed; update this count deliberately");

        let mut impure = Vec::new();
        for m in &exposed {
            let src = std::fs::read_to_string(dir.join(format!("{m}.rs")))
                .unwrap_or_else(|e| panic!("{m}.rs: {e}"));
            for line in src.lines() {
                let t = line.trim();
                let Some(rest) = t.strip_prefix("use ") else { continue };
                // std, core, alloc and intra-crate paths are fine; anything
                // else is an external crate this target must not need.
                if rest.starts_with("std::") || rest.starts_with("core::")
                    || rest.starts_with("alloc::") || rest.starts_with("crate::")
                    || rest.starts_with("super::") || rest.starts_with("self::") {
                    continue;
                }
                impure.push(format!("{m}.rs: {t}"));
            }
        }
        assert!(
            impure.is_empty(),
            "these modules are exposed by lib.rs but import outside std, which defeats the \
purpose of this target — voiceterm's logic must stay testable on a machine with no Qt:\n  {}",
            impure.join("\n  ")
        );
    }
}
