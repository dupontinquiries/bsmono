//! toolreg.rs — the glue orchestra's registry: verified glue becomes a callable tool the whole swarm can
//! discover and invoke with flags, instead of every branch regenerating and recompiling the same logic.
//!
//! The core idea: a compiled tool with a flag interface is a FROZEN FITNESS RESULT — it passed the gates
//! once, so every later branch that calls it inherits that correctness for free (a memoization layer over
//! the fitness function). This is the minimal first version we agreed on: just PROMOTION + a discoverable
//! MENU + the reuse counter, no clustering/unification yet — let the reuse-vs-regenerate signal tell us
//! whether the fancier synthesis layer is worth building.
//!
//! Two hazards this guards:
//!   * Interface drift — once branches depend on a tool's flags, changing them breaks callers mid-swarm.
//!     So a tool's interface is APPEND-ONLY: re-promoting an existing name with a changed usage is rejected
//!     unless it's a new version (the `--help` contract is the stability boundary).
//!   * Premature promotion — [`should_promote`] gates on N independent branches converging on the same
//!     shape, so two coincidental rhymes don't mint a tool nobody can call.

/// A promoted, verified tool: a compiled binary with a flag contract, standing in a shared `bin/`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tool {
    pub name: String,     // the callable name (the binary in the shared bin/)
    pub version: u32,     // bumped when the interface must change (append-only otherwise)
    pub summary: String,  // one-line purpose (for the menu injected into branch context)
    pub usage: String,    // the flag contract — `--help` output; the STABLE interface callers depend on
    pub lang: crate::lang::Lang,
    pub calls: u32,       // reuse count — the promotion fitness signal (0 afterward = a bad promotion)
}

impl Tool {
    pub fn new(name: impl Into<String>, summary: impl Into<String>, usage: impl Into<String>, lang: crate::lang::Lang) -> Self {
        Self { name: name.into(), version: 1, summary: summary.into(), usage: usage.into(), lang, calls: 0 }
    }
}

/// Why a promotion was rejected (so the orchestra can log/act on it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromoteError {
    NotVerified,                  // the artifact didn't pass its compile/validate gate — can't promote
    NoGate,                       // the language has no fitness floor (e.g. Markdown) — nothing to trust
    InterfaceChanged(String),     // a tool with this name exists and the usage differs (would break callers)
}

#[derive(Default, Clone, Debug)]
pub struct ToolRegistry {
    tools: Vec<Tool>, // latest version per name (older versions kept for pinned callers)
}

impl ToolRegistry {
    pub fn new() -> Self { Self::default() }

    /// Promote a verified tool into the shared registry. `verified` is whether it passed its gate. Enforces:
    /// the language must HAVE a gate (a fitness floor), the artifact must have passed it, and the interface
    /// is append-only — an existing name with a changed `usage` is rejected (bump `tool.version` to evolve).
    pub fn promote(&mut self, tool: Tool, verified: bool) -> Result<(), PromoteError> {
        if !tool.lang.has_compile_gate() { return Err(PromoteError::NoGate); }
        if !verified { return Err(PromoteError::NotVerified); }
        if let Some(existing) = self.tools.iter().find(|t| t.name == tool.name) {
            if existing.version == tool.version && existing.usage != tool.usage {
                return Err(PromoteError::InterfaceChanged(tool.name)); // same version, different contract → break
            }
            if tool.version <= existing.version && existing.usage == tool.usage {
                return Ok(()); // idempotent re-promote of the same tool
            }
        }
        // insert or replace-with-newer-version
        if let Some(slot) = self.tools.iter_mut().find(|t| t.name == tool.name && t.version <= tool.version) {
            *slot = tool;
        } else {
            self.tools.push(tool);
        }
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&Tool> {
        self.tools.iter().filter(|t| t.name == name).max_by_key(|t| t.version)
    }

    /// Record that a branch invoked a tool — the reuse signal that tells us the promotion paid off.
    pub fn record_call(&mut self, name: &str) {
        if let Some(t) = self.tools.iter_mut().filter(|t| t.name == name).max_by_key(|t| t.version) {
            t.calls += 1;
        }
    }

    pub fn len(&self) -> usize { self.tools.len() }
    pub fn is_empty(&self) -> bool { self.tools.is_empty() }

    /// The discoverable menu injected into a branch's context so it reaches for an existing tool instead of
    /// regenerating the logic. One line per tool: `name  — summary  [usage]`.
    pub fn menu(&self) -> String {
        if self.tools.is_empty() { return "(no tools promoted yet)".into(); }
        let mut lines: Vec<&Tool> = self.tools.iter().collect();
        lines.sort_by(|a, b| a.name.cmp(&b.name));
        lines.iter().map(|t| format!("{}  — {}  [{}]", t.name, t.summary, t.usage)).collect::<Vec<_>>().join("\n")
    }

    /// Tools nobody has called since promotion — the MEASURABLE bad-promotion signal (feed back to raise the
    /// promotion threshold). A tool promoted long ago with zero calls was probably premature.
    pub fn unused(&self) -> Vec<&Tool> {
        self.tools.iter().filter(|t| t.calls == 0).collect()
    }
}

/// Should a converged pattern be promoted to a tool yet? Only when at least `threshold` INDEPENDENT branches
/// arrived at the same shape — guarding against minting a tool from two coincidental rhymes. The threshold
/// is a knob the orchestra can tune from the unused()-tool feedback.
pub fn should_promote(independent_convergences: usize, threshold: usize) -> bool {
    independent_convergences >= threshold.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::Lang;

    fn t(name: &str, usage: &str) -> Tool { Tool::new(name, "does a thing", usage, Lang::Rust) }

    #[test]
    fn promotes_only_verified_gated_tools() {
        let mut r = ToolRegistry::new();
        // unverified → rejected
        assert_eq!(r.promote(t("gguf-probe", "--header <f>"), false), Err(PromoteError::NotVerified));
        // a language with no gate → rejected (can't trust it)
        let md = Tool::new("notes", "x", "y", Lang::Markdown);
        assert_eq!(r.promote(md, true), Err(PromoteError::NoGate));
        // verified + gated → accepted, discoverable
        assert!(r.promote(t("gguf-probe", "--header <f>"), true).is_ok());
        assert_eq!(r.len(), 1);
        assert!(r.get("gguf-probe").is_some());
        assert!(r.menu().contains("gguf-probe"));
    }

    #[test]
    fn interface_is_append_only() {
        let mut r = ToolRegistry::new();
        r.promote(t("walk", "--root <dir>"), true).unwrap();
        // same name+version, DIFFERENT usage → rejected (would break callers)
        assert_eq!(r.promote(t("walk", "--root <dir> --depth <n>"), true), Err(PromoteError::InterfaceChanged("walk".into())));
        // idempotent re-promote of the identical tool is fine
        assert!(r.promote(t("walk", "--root <dir>"), true).is_ok());
        assert_eq!(r.len(), 1);
        // a NEW VERSION may change the interface
        let mut v2 = t("walk", "--root <dir> --depth <n>"); v2.version = 2;
        assert!(r.promote(v2, true).is_ok());
        assert_eq!(r.get("walk").unwrap().version, 2, "get returns the latest version");
        assert!(r.get("walk").unwrap().usage.contains("--depth"));
    }

    #[test]
    fn reuse_signal_and_bad_promotion_detection() {
        let mut r = ToolRegistry::new();
        r.promote(t("used", "-x"), true).unwrap();
        r.promote(t("never-called", "-y"), true).unwrap();
        r.record_call("used");
        r.record_call("used");
        assert_eq!(r.get("used").unwrap().calls, 2);
        // the uncalled tool is flagged as a (likely premature) promotion
        let dead = r.unused();
        assert_eq!(dead.len(), 1);
        assert_eq!(dead[0].name, "never-called");
    }

    #[test]
    fn promotion_threshold_guards_premature_unification() {
        assert!(!should_promote(1, 3), "one branch rhyming isn't convergence");
        assert!(!should_promote(2, 3));
        assert!(should_promote(3, 3), "three independent branches converged → promote");
        assert!(should_promote(9, 3));
        assert!(should_promote(1, 0), "threshold floors at 1");
    }
}
