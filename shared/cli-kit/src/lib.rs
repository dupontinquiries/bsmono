//! cli-kit — the small argument-parsing helpers every app in the portfolio re-implements.
//!
//! Not a framework. Just the four things a hand-rolled `std::env::args` loop keeps
//! re-deriving: is a flag present, what's an option's value (`--k v` or `--k=v`), the
//! repeated values of an option, and the leftover positionals. Lifted verbatim in spirit
//! from gitmind's CLI so apps stop copying it.
//!
//! ```
//! # use cli_kit::Cli;
//! let cli = Cli::from(["fleet", "build", "apps/x", "--target", "musl", "-v"]);
//! assert_eq!(cli.subcommand(), Some("fleet"));
//! assert_eq!(cli.opt("--target"), Some("musl"));
//! assert!(cli.flag("-v"));
//! assert_eq!(cli.positionals(), vec!["fleet", "build", "apps/x"]);
//! ```

/// A parsed argument vector. Holds the raw args; queries are computed on demand so the
/// same vector answers flags, options, and positionals without a builder step.
#[derive(Debug, Clone)]
pub struct Cli {
    args: Vec<String>,
}

/// A bare option name becomes its long form (`model` -> `--model`); dashed names pass through.
fn long_form(name: &str) -> String {
    if name.starts_with('-') { name.to_string() } else { format!("--{name}") }
}

impl Cli {
    /// Build from the real process args (skips argv[0]).
    pub fn from_env() -> Cli {
        Cli { args: std::env::args().skip(1).collect() }
    }

    /// The raw args, unchanged.
    pub fn args(&self) -> &[String] {
        &self.args
    }

    /// True if a bare flag (`-v`, `--json`) appears anywhere.
    pub fn flag(&self, name: &str) -> bool {
        let long = long_form(name);
        self.args.iter().any(|a| a == name || *a == long)
    }

    /// How many times a flag appears — for repeatable flags like `-v -v -v` (verbosity).
    pub fn count(&self, name: &str) -> usize {
        let long = long_form(name);
        self.args.iter().filter(|a| **a == name || **a == long).count()
    }

    /// `-h` / `--help` present anywhere.
    pub fn wants_help(&self) -> bool {
        self.args.iter().any(|a| a == "-h" || a == "--help")
    }

    /// The value of an option in either `--name value` or `--name=value` form; first wins. The
    /// name may be given bare (`opt("target")`) or dashed (`opt("--target")`) — both match the
    /// long flag, so a missing dash can't silently drop the option.
    pub fn opt(&self, name: &str) -> Option<&str> {
        let long = long_form(name);
        let (eq1, eq2) = (format!("{name}="), format!("{long}="));
        let mut it = self.args.iter();
        while let Some(a) = it.next() {
            if a == name || *a == long {
                return it.next().map(|s| s.as_str());
            }
            if let Some(v) = a.strip_prefix(&eq1).or_else(|| a.strip_prefix(&eq2)) {
                return Some(v);
            }
        }
        None
    }

    /// Parse an option into `T`, falling back to `default` if absent or unparsable.
    pub fn opt_parse<T: std::str::FromStr>(&self, name: &str, default: T) -> T {
        self.opt(name).and_then(|s| s.parse().ok()).unwrap_or(default)
    }

    /// A required option: its value, or `Err(message)` naming the missing flag.
    pub fn require(&self, name: &str) -> Result<&str, String> {
        self.opt(name).ok_or_else(|| format!("missing required option {}", long_form(name)))
    }

    /// All values of a repeatable option (`--tag a --tag b` or `--tag=a`).
    pub fn opt_all(&self, name: &str) -> Vec<&str> {
        let long = long_form(name);
        let (eq1, eq2) = (format!("{name}="), format!("{long}="));
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.args.len() {
            let a = &self.args[i];
            if a == name || *a == long {
                if let Some(v) = self.args.get(i + 1) {
                    out.push(v.as_str());
                    i += 2;
                    continue;
                }
            } else if let Some(v) = a.strip_prefix(&eq1).or_else(|| a.strip_prefix(&eq2)) {
                out.push(v);
            }
            i += 1;
        }
        out
    }

    /// Positionals: args that are neither a `-flag` nor consumed as an option value. A
    /// token following a `--opt` (without `=`) is treated as that option's value and
    /// excluded. Conservative: unknown `--opt value` pairs are still skipped so stray
    /// values don't masquerade as positionals.
    pub fn positionals(&self) -> Vec<&str> {
        self.positionals_impl(None)
    }

    /// Positionals when the caller knows exactly which options take a value. This removes the
    /// heuristic's ambiguity: a boolean flag (e.g. `--mock`) followed by a bare token no longer
    /// swallows that token as its value, so `--mock script.txt` yields `["script.txt"]`. List the
    /// value-taking option names *without* dashes.
    pub fn positionals_with_values(&self, value_opts: &[&str]) -> Vec<&str> {
        self.positionals_impl(Some(value_opts))
    }

    fn positionals_impl(&self, value_opts: Option<&[&str]>) -> Vec<&str> {
        let mut out = Vec::new();
        let mut skip_next = false;
        for (i, a) in self.args.iter().enumerate() {
            if skip_next {
                skip_next = false;
                continue;
            }
            if a.starts_with('-') && a.len() > 1 {
                // does this option consume the next token as its value?
                let takes_value = if let Some(known) = value_opts {
                    // precise: only the declared value-options consume a value
                    let name = a.trim_start_matches('-');
                    let name = name.split('=').next().unwrap_or(name);
                    known.contains(&name)
                } else {
                    // heuristic (back-compat): any `--opt` (no '=') followed by a non-dash token
                    true
                };
                if takes_value && !a.contains('=') {
                    if let Some(next) = self.args.get(i + 1) {
                        if !next.starts_with('-') {
                            skip_next = true;
                        }
                    }
                }
                continue;
            }
            out.push(a.as_str());
        }
        out
    }

    /// The first positional — conventionally the subcommand.
    pub fn subcommand(&self) -> Option<&str> {
        self.positionals().into_iter().next()
    }

    /// Positionals after the first (the subcommand's own arguments).
    pub fn rest(&self) -> Vec<&str> {
        self.positionals().into_iter().skip(1).collect()
    }
}

impl<S: Into<String>, I: IntoIterator<Item = S>> From<I> for Cli {
    fn from(iter: I) -> Cli {
        Cli { args: iter.into_iter().map(Into::into).collect() }
    }
}

// ── free functions matching the hand-rolled `&[String]` helpers apps already have, so a
//    crate can drop its copies and call these with no other change ──

/// True if a bare flag appears in `args`.
pub fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

/// Value of an option in `--name value` or `--name=value` form (first wins).
pub fn opt<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == name {
            return it.next().map(|s| s.as_str());
        }
        if let Some(v) = a.strip_prefix(&format!("{name}=")) {
            return Some(v);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_and_both_option_forms() {
        let c = Cli::from(["build", "--target", "musl", "--features=a,b", "-v"]);
        assert!(c.flag("-v"));
        assert!(!c.flag("-q"));
        assert_eq!(c.opt("--target"), Some("musl"));
        assert_eq!(c.opt("--features"), Some("a,b"));
        assert_eq!(c.opt("--missing"), None);
    }

    #[test]
    fn repeatable_options() {
        let c = Cli::from(["x", "--tag", "one", "--tag=two", "--tag", "three"]);
        assert_eq!(c.opt_all("--tag"), vec!["one", "two", "three"]);
    }

    #[test]
    fn positionals_exclude_option_values() {
        let c = Cli::from(["fleet", "build", "apps/x", "--target", "musl", "--json"]);
        assert_eq!(c.positionals(), vec!["fleet", "build", "apps/x"]);
        assert_eq!(c.subcommand(), Some("fleet"));
        assert_eq!(c.rest(), vec!["build", "apps/x"]);
    }

    #[test]
    fn eq_form_does_not_consume_next() {
        let c = Cli::from(["run", "--mode=fast", "target1", "target2"]);
        assert_eq!(c.positionals(), vec!["run", "target1", "target2"]);
    }
}

#[cfg(test)]
mod value_opt_tests {
    use super::Cli;

    #[test]
    fn positionals_with_values_distinguishes_boolean_flags() {
        // boolean flag before a positional: heuristic wrongly eats it...
        let cli = Cli::from(vec!["--mock", "script.txt"]);
        assert!(cli.positionals().is_empty());
        // ...value-aware knows --mock takes no value, so the script is the positional
        assert_eq!(cli.positionals_with_values(&["model", "system"]), vec!["script.txt"]);
        // a declared value-opt still consumes its value
        let cli2 = Cli::from(vec!["--model", "big.gguf", "script.txt"]);
        assert_eq!(cli2.positionals_with_values(&["model"]), vec!["script.txt"]);
        // '=' form is always unambiguous
        let cli3 = Cli::from(vec!["--mock=1", "script.txt"]);
        assert_eq!(cli3.positionals_with_values(&["model"]), vec!["script.txt"]);
    }
}

#[cfg(test)]
mod ergonomics_tests {
    use super::Cli;

    #[test]
    fn bare_names_match_long_flags() {
        let c = Cli::from(["run", "--model", "big", "--mock", "--max", "1024"]);
        // bare names (the mistake that silently failed before) now match the long flags
        assert!(c.flag("mock"));
        assert_eq!(c.opt("model"), Some("big"));
        assert_eq!(c.opt_parse("max", 512usize), 1024);
        // dashed names still work exactly
        assert!(c.flag("--mock"));
        assert_eq!(c.opt("--model"), Some("big"));
        // opt_parse default when absent/unparsable
        assert_eq!(c.opt_parse("missing", 7u32), 7);
    }

    #[test]
    fn count_and_help_and_require() {
        let c = Cli::from(["x", "-v", "-v", "-v", "--help"]);
        assert_eq!(c.count("-v"), 3);
        assert!(c.wants_help());
        let c2 = Cli::from(["x", "--name", "ok"]);
        assert_eq!(c2.require("name"), Ok("ok"));
        assert!(c2.require("target").unwrap_err().contains("--target"));
    }

    #[test]
    fn check_does_not_match_check_file() {
        let c = Cli::from(["x", "--check-file", "/tmp/a", "--check", "cargo build"]);
        assert_eq!(c.opt("check"), Some("cargo build"));
        assert_eq!(c.opt("check-file"), Some("/tmp/a"));
    }
}
