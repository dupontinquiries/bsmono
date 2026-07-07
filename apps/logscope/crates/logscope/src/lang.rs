//! lang.rs — what languages the sandbox understands, how to verify each, and — the property that matters
//! most for an autonomous swarm — whether each has a CHEAP DETERMINISTIC compile/validate gate.
//!
//! The whole evolutionary loop is only as affordable as its fitness floor: the stricter and cheaper the
//! bottom cull, the more aggressively branches can diverge, because bad artifacts die instantly instead of
//! burning judge tokens. So every language here is classified by whether it has that floor and how to reach
//! it. Deliberately narrow — only languages that STRENGTHEN the floor earn a place (see the module design
//! notes): Rust (the borrow-checked core), WGSL (validated in-process by naga, and checkable against a
//! numeric reference — the strongest floor of all), Zig (no GC, C-ABI, for the hardware/FFI frontier), plus
//! the structured-data formats whose parse IS a validation. Markdown carries no execution semantics, so it
//! has no gate and that's fine.

/// A language/format the sandbox can hold as an artifact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Rust,     // the type-and-borrow-checked core artifact language
    Wgsl,     // GPU shaders — validated in-process (naga), checkable vs a CPU reference
    Zig,      // hardware/FFI frontier: no GC, C ABI, comptime; lives at the unsafe boundary
    Toml,     // config (Cargo, hand-written) — parse = validation
    Json,     // structured output / blackboard entries — parse (+ optional schema) = validation
    Jsonl,    // append-only event streams (per-line JSON) — the swarm's on-disk event log
    Yaml,     // config — parse = validation
    Markdown, // prose / docs — no compile gate, and that's correct
    Other,    // unknown extension — no gate
}

/// How an artifact of a given language is verified — the tier the sandbox dispatches on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerifyKind {
    Cargo,         // subprocess: `cargo build`/`test` in a bubblewrap mirror (Rust)
    NagaValidate,  // IN-PROCESS: parse + validate WGSL with naga — no subprocess, hermetic, milliseconds
    Zig,           // subprocess: `zig build-obj` / `zig test`
    ParseValidate, // IN-PROCESS: serde/toml parse — a malformed doc fails here, cheaply
    None,          // no automated gate (Markdown / Other)
}

impl Lang {
    /// Classify by file extension (case-insensitive). Unknown → `Other`.
    pub fn from_path(path: &str) -> Lang {
        let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        match ext.as_str() {
            "rs" => Lang::Rust,
            "wgsl" => Lang::Wgsl,
            "zig" => Lang::Zig,
            "toml" => Lang::Toml,
            "json" => Lang::Json,
            "jsonl" | "ndjson" => Lang::Jsonl,
            "yaml" | "yml" => Lang::Yaml,
            "md" | "markdown" => Lang::Markdown,
            _ => Lang::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Lang::Rust => "Rust", Lang::Wgsl => "WGSL", Lang::Zig => "Zig", Lang::Toml => "TOML",
            Lang::Json => "JSON", Lang::Jsonl => "JSONL", Lang::Yaml => "YAML", Lang::Markdown => "Markdown",
            Lang::Other => "other",
        }
    }

    /// THE property the swarm's economics hinge on: does this language have a cheap, deterministic
    /// compile/validate gate that rejects a whole class of broken artifacts before any tokens are spent
    /// judging them? Everything but prose/unknown does.
    pub fn has_compile_gate(self) -> bool {
        !matches!(self, Lang::Markdown | Lang::Other)
    }

    /// Is the gate IN-PROCESS (naga/serde — no subprocess, no toolchain install, runs in milliseconds)?
    /// These are the cheapest culls of all and can run even in a locked-down environment.
    pub fn gate_in_process(self) -> bool {
        matches!(self.verify_kind(), VerifyKind::NagaValidate | VerifyKind::ParseValidate)
    }

    /// How to verify an artifact of this language.
    pub fn verify_kind(self) -> VerifyKind {
        match self {
            Lang::Rust => VerifyKind::Cargo,
            Lang::Wgsl => VerifyKind::NagaValidate,
            Lang::Zig => VerifyKind::Zig,
            Lang::Toml | Lang::Json | Lang::Jsonl | Lang::Yaml => VerifyKind::ParseValidate,
            Lang::Markdown | Lang::Other => VerifyKind::None,
        }
    }

    /// The external toolchain binary a subprocess gate needs (so the sandbox can check availability / build
    /// the argv). None for in-process gates and for languages with no gate.
    pub fn tool_binary(self) -> Option<&'static str> {
        match self.verify_kind() {
            VerifyKind::Cargo => Some("cargo"),
            VerifyKind::Zig => Some("zig"),
            _ => None,
        }
    }

    /// Can this artifact be graded against a reference for CORRECTNESS (not just validity)? WGSL kernels
    /// against a CPU reference and Rust against tests are the swarm's dream fitness targets — objective,
    /// cheap, deterministic. (The reference itself lives elsewhere; this just flags the capability.)
    pub fn reference_checkable(self) -> bool {
        matches!(self, Lang::Rust | Lang::Wgsl | Lang::Zig)
    }
}

/// The languages the sandbox actively verifies (have a compile/validate gate), for surfacing in UI/help.
pub fn gated_languages() -> &'static [Lang] {
    &[Lang::Rust, Lang::Wgsl, Lang::Zig, Lang::Toml, Lang::Json, Lang::Jsonl, Lang::Yaml]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_by_extension() {
        assert_eq!(Lang::from_path("src/lib.rs"), Lang::Rust);
        assert_eq!(Lang::from_path("kernels/gemm.wgsl"), Lang::Wgsl);
        assert_eq!(Lang::from_path("ffi/bind.zig"), Lang::Zig);
        assert_eq!(Lang::from_path("Cargo.toml"), Lang::Toml);
        assert_eq!(Lang::from_path("out.JSON"), Lang::Json); // case-insensitive
        assert_eq!(Lang::from_path("events.jsonl"), Lang::Jsonl);
        assert_eq!(Lang::from_path("config.yml"), Lang::Yaml);
        assert_eq!(Lang::from_path("README.md"), Lang::Markdown);
        assert_eq!(Lang::from_path("mystery.xyz"), Lang::Other);
        assert_eq!(Lang::from_path("no_extension"), Lang::Other);
    }

    #[test]
    fn fitness_floor_is_correct() {
        // everything but prose/unknown has a cheap deterministic gate
        for l in gated_languages() { assert!(l.has_compile_gate(), "{} should have a gate", l.label()); }
        assert!(!Lang::Markdown.has_compile_gate());
        assert!(!Lang::Other.has_compile_gate());
        // wgsl + the data formats gate in-process (cheapest cull, no toolchain, runs anywhere)
        assert!(Lang::Wgsl.gate_in_process(), "naga validates WGSL in-process");
        assert!(Lang::Json.gate_in_process() && Lang::Yaml.gate_in_process() && Lang::Toml.gate_in_process());
        assert!(!Lang::Rust.gate_in_process(), "cargo is a subprocess");
        assert!(!Lang::Zig.gate_in_process());
    }

    #[test]
    fn verify_dispatch_and_toolchain() {
        assert_eq!(Lang::Rust.verify_kind(), VerifyKind::Cargo);
        assert_eq!(Lang::Wgsl.verify_kind(), VerifyKind::NagaValidate);
        assert_eq!(Lang::Zig.verify_kind(), VerifyKind::Zig);
        assert_eq!(Lang::Json.verify_kind(), VerifyKind::ParseValidate);
        assert_eq!(Lang::Markdown.verify_kind(), VerifyKind::None);
        // only subprocess gates name a binary
        assert_eq!(Lang::Rust.tool_binary(), Some("cargo"));
        assert_eq!(Lang::Zig.tool_binary(), Some("zig"));
        assert_eq!(Lang::Wgsl.tool_binary(), None);
        assert_eq!(Lang::Markdown.tool_binary(), None);
    }

    #[test]
    fn reference_checkable_is_the_dream_target() {
        // code that can be graded for correctness vs a reference (objective fitness)
        assert!(Lang::Wgsl.reference_checkable() && Lang::Rust.reference_checkable() && Lang::Zig.reference_checkable());
        assert!(!Lang::Markdown.reference_checkable() && !Lang::Json.reference_checkable());
    }
}
