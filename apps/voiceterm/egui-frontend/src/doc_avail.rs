//! doc_avail — answering `doc_resolver`'s three questions from what this frontend holds.
//!
//! `doc_resolver::resolve` is pure and takes an `Availability` of three closures: is there a
//! BUNDLED man page for this key, is there a tldr entry for this name, is this name on PATH.
//! The resolver has 11 tests and was written probe-first; it was never mounted in this
//! frontend, so `TODO_doc_pane.md`'s "extract_query → resolve → spawn" flow had no resolve
//! step here.
//!
//! Kept out of `doc_resolver` on purpose — the resolver takes closures precisely so it can be
//! tested without a filesystem or a man page corpus, and answering the questions is this
//! layer's job.
//!
//! ## On PATH is asked, not assumed
//!
//! `command_search`'s index already records `on_path` per entry, but only for commands it has
//! a tldr page for. A command with no tldr entry is exactly the case where "is it installed"
//! decides between offering `--help` and offering a web search, so the fallback walks `$PATH`
//! rather than answering "no" for everything the index does not know.

/// The three facts, bound to this frontend's data.
///
/// `docs` is the tldr-backed index the app already carries (`default_command_docs`).
pub struct Facts<'a> {
    pub docs: &'a [crate::command_search::CommandDoc],
}

impl Facts<'_> {
    /// Is there a bundled man page for this key?
    ///
    /// ALWAYS FALSE IN THIS BUILD, and that is a fact rather than a stub. The bundled corpus
    /// lives in `manpages.rs`, which cannot be mounted here: its Qt-free half
    /// (`get`/`list_names`/`to_html`) shares a file with `to_qml`, which pulls in `qml_gen`
    /// and `scraper`, and a module cannot be half-included. `manpage_view`'s own docs record
    /// why the split has not happened — the app crate's build shells out to `qmake`.
    ///
    /// Answering `false` is the correct answer, not a degraded one: `resolve` then prefers
    /// tldr, the system man page or `--help`, all of which this build CAN reach. Answering
    /// `true` would route a query to an in-app renderer with nothing to render.
    pub fn bundled_man(&self, _key: &str) -> bool {
        false
    }

    /// Is there a tldr entry for this command name?
    pub fn has_tldr(&self, name: &str) -> bool {
        self.docs.iter().any(|d| d.name.eq_ignore_ascii_case(name))
    }

    /// Is this name an executable on `$PATH`?
    ///
    /// The index's own `on_path` first — it is already computed — then a `$PATH` walk for
    /// anything the index does not carry. A command with no tldr page is exactly where this
    /// question decides between `--help` and a web search, so answering "no" for everything
    /// unknown would send every unindexed tool to the browser.
    pub fn on_path(&self, name: &str) -> bool {
        if let Some(d) = self.docs.iter().find(|d| d.name.eq_ignore_ascii_case(name)) {
            return d.on_path;
        }
        on_path_now(name)
    }
}

/// Walk `$PATH` for an executable called `name`.
///
/// A name with a separator in it is refused rather than probed: `resolve` hands us the head
/// of a command line, and `../../bin/thing` is not a command name — treating it as one would
/// let a query decide which file gets stat-ed.
pub fn on_path_now(name: &str) -> bool {
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return false;
    }
    let Some(path) = std::env::var_os("PATH") else { return false };
    std::env::split_paths(&path).any(|dir| {
        let p = dir.join(name);
        // Metadata, not just existence: a directory called `git` on PATH is not `git`.
        std::fs::metadata(&p).map(|m| m.is_file()).unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_search::CommandDoc;

    fn doc(name: &str, on_path: bool) -> CommandDoc {
        CommandDoc {
            name: name.to_string(),
            summary: String::new(),
            examples: Vec::new(),
            on_path,
        }
    }

    /// Answering `false` is correct, not degraded: `resolve` then prefers tldr, system man
    /// or `--help`, all of which this build can reach, where `true` would route a query to an
    /// in-app renderer with nothing to render.
    #[test]
    fn this_build_claims_no_bundled_man_pages() {
        let f = Facts { docs: &[] };
        assert!(!f.bundled_man("git-rebase"));
        assert!(!f.bundled_man("ls"));
    }

    #[test]
    fn tldr_is_answered_from_the_index_case_insensitively() {
        let docs = vec![doc("git", true), doc("Cargo", false)];
        let f = Facts { docs: &docs };
        assert!(f.has_tldr("git"));
        assert!(f.has_tldr("GIT"), "a name is a name whatever its case");
        assert!(f.has_tldr("cargo"));
        assert!(!f.has_tldr("nushell"));
    }

    /// A command with no tldr page is exactly where "is it installed" decides between
    /// offering `--help` and offering a web search. Answering "no" for everything the index
    /// does not know would send every unindexed tool to the browser.
    #[test]
    fn on_path_falls_back_to_a_real_lookup_for_unindexed_commands() {
        let docs = vec![doc("git", true), doc("obscure", false)];
        let f = Facts { docs: &docs };
        // The index answers for what it carries, both ways.
        assert!(f.on_path("git"));
        assert!(!f.on_path("obscure"));
        // And something it does not carry is actually looked up: `sh` exists everywhere this
        // runs.
        assert!(f.on_path("sh"), "the fallback did not find /bin/sh");
        assert!(!f.on_path("definitely-not-a-real-command-xyz"));
    }

    /// `resolve` hands us the head of a command line. A path is not a command name, and
    /// treating it as one would let a query decide which file gets stat-ed.
    #[test]
    fn a_path_is_not_a_command_name() {
        assert!(!on_path_now("../../bin/sh"));
        assert!(!on_path_now("/bin/sh"));
        assert!(!on_path_now(""));
        assert!(on_path_now("sh"));
    }
}
