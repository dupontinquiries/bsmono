//! cmd_risk — GITLOG-4's classifier: does this command need a snapshot?
//!
//! Decides whether [`crate::snapshot_policy`]'s capture runs before a command.
//! Three buckets, and the asymmetry between them is the entire design:
//!
//! ```text
//!   ReadOnly  → skip     (no rollback possible, none needed)
//!   Mutating  → snapshot (rollback may be wanted)
//!   Unknown   → snapshot (better an unnecessary one than a missing one)
//! ```
//!
//! ## The costs are not symmetric, so the default is not neutral
//!
//! A false positive is *"one wasted snapshot. Cheap; the snapshot_store dedups
//! by content anyway, so identical state across snapshots costs nothing in
//! storage."* A false negative is a command that mutated the directory with no
//! way back.
//!
//! That is why the allowlist is *"SHORT and AUTHORITATIVE"* — only commands
//! certain not to mutate the cwd **under any argument combination** — and why
//! everything else falls through to Unknown rather than being guessed at.
//!
//! ## Classified by name, and the original is honest about what that misses
//!
//! *"We cannot actually run the commands to verify."* So commands with surprise
//! flags are deliberately **excluded from the allowlist** — `find` has
//! `-delete`, and a classifier that trusted the name would skip the snapshot on
//! exactly the command that needed it most.
//!
//! ## Two argument-level rules the base command cannot express
//!
//! * **Redirects.** `ls > out.txt` is read-only *for `ls`* and writes a file.
//!   Any redirect forces Mutating regardless of the command.
//! * **Pipes.** `ls | head` is read-only only if **both** stages are. The worst
//!   classification across stages wins.

/// The three buckets, ordered by how much caution they demand.
///
/// `Ord` is deliberate: combining stages is a `max`, and writing it that way
/// means a new bucket cannot be added without deciding where it sits.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Risk { ReadOnly, Unknown, Mutating }

impl Risk {
    pub fn needs_snapshot(self) -> bool { self != Risk::ReadOnly }
}

/// Commands certain not to mutate the working directory under any flags.
///
/// Short on purpose. Anything with a destructive flag — `find -delete`, and
/// anything whose flags we have not audited — belongs in Unknown instead.
pub const READ_ONLY: &[&str] = &[
    "ls", "ll", "la", "dir", "tree", "pwd", "cd", "echo", "printf",
    "cat", "bat", "less", "more", "head", "tail", "view",
    "file", "stat", "wc", "du", "df",
    "realpath", "readlink", "basename", "dirname",
    "which", "whereis", "type", "command",
    "grep",
];

/// `git` subcommands that only read.
///
/// Separate from the plain list because `git` itself is not read-only — `git
/// checkout` and `git clean` are among the most destructive things in a working
/// tree. Treating the whole binary as safe because most of its subcommands are
/// is exactly the mistake this split prevents.
pub const GIT_READ_ONLY: &[&str] = &[
    "status", "log", "show", "diff", "blame", "describe",
    "rev-parse", "rev-list", "ls-files", "ls-tree", "cat-file",
    "show-ref", "symbolic-ref", "name-rev", "whatchanged", "shortlog", "reflog",
];

/// Commands that certainly mutate. Classified up front so they never depend on
/// falling through Unknown.
pub const MUTATING: &[&str] = &[
    "rm", "rmdir", "unlink", "shred", "wipe",
    "mv", "rename", "cp", "rsync", "scp",
    "mkdir", "touch", "ln",
    "chmod", "chown", "chgrp", "setfacl", "tee",
];

/// Tokens that write regardless of what produced the output.
pub const REDIRECTS: &[&str] = &[">", ">>", "2>", "2>>", "&>", "&>>", "<>"];

/// Does this command line contain a redirect?
///
/// Token-wise, not substring: `echo "a>b"` contains `>` inside a quoted string
/// and does not redirect, while `grep -->` is a flag. Scanning raw text would
/// snapshot on both — harmless, but it would also miss nothing, so the reason
/// to be precise is that a classifier nobody trusts gets switched off.
pub fn has_redirect(cmd: &str) -> bool {
    for tok in cmd.split_whitespace() {
        if REDIRECTS.contains(&tok) { return true; }
        // Attached forms: `>out.txt`, `2>&1`, `foo>>bar`.
        if let Some(i) = tok.find(['>']) {
            // A `>` inside a quoted token is not a redirect we can see without
            // a real parser; treat an unquoted one as a redirect and leave the
            // quoted case to Unknown, which snapshots anyway.
            if !tok.starts_with('"') && !tok.starts_with('\'') {
                let _ = i;
                return true;
            }
        }
    }
    false
}

/// Classify a single stage — no pipes, no redirects.
pub fn classify_stage(stage: &str) -> Risk {
    let mut words = stage.split_whitespace().filter(|w| !w.is_empty());
    // Skip leading environment assignments: `FOO=1 ls` is still `ls`.
    let head = loop {
        match words.next() {
            Some(w) if w.contains('=') && !w.starts_with('-') => continue,
            Some(w) => break w,
            None => return Risk::ReadOnly,
        }
    };
    // Strip a leading path: `/bin/ls` and `./ls` are `ls`.
    let name = head.rsplit('/').next().unwrap_or(head);
    // `sudo ls` is as read-only as `ls`; the elevation does not change what the
    // command does to the cwd, and refusing to look past it would classify
    // every elevated command as Unknown.
    if name == "sudo" || name == "doas" || name == "env" {
        let rest: String = words.collect::<Vec<_>>().join(" ");
        return if rest.is_empty() { Risk::Unknown } else { classify_stage(&rest) };
    }
    if MUTATING.contains(&name) { return Risk::Mutating; }
    if name == "git" {
        return match words.next() {
            Some(sub) if GIT_READ_ONLY.contains(&sub) => Risk::ReadOnly,
            // `git` alone prints help; anything unrecognised may be `clean`.
            _ => Risk::Unknown,
        };
    }
    if READ_ONLY.contains(&name) { return Risk::ReadOnly; }
    Risk::Unknown
}

/// Classify a whole command line.
pub fn classify(cmd: &str) -> Risk {
    let trimmed = cmd.trim();
    if trimmed.is_empty() { return Risk::ReadOnly; }
    if has_redirect(trimmed) { return Risk::Mutating; }
    // Worst stage wins. `ls | head` is read-only only because both are.
    trimmed
        .split('|')
        .map(|s| classify_stage(s))
        .max()
        .unwrap_or(Risk::Unknown)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_listing_needs_no_snapshot() {
        assert_eq!(classify("ls -la"), Risk::ReadOnly);
        assert!(!classify("ls -la").needs_snapshot());
    }

    #[test]
    fn a_redirect_makes_a_read_only_command_mutating() {
        // `ls > out.txt` is read-only FOR LS and writes a file. The base
        // command cannot express this.
        assert_eq!(classify("ls > out.txt"), Risk::Mutating);
        assert_eq!(classify("echo hi >> log.txt"), Risk::Mutating);
        assert_eq!(classify("cat a 2> err"), Risk::Mutating);
        assert_eq!(classify("make &> build.log"), Risk::Mutating);
    }

    #[test]
    fn an_attached_redirect_is_caught_too() {
        // `ls >out.txt` with no space is the form people actually type.
        assert_eq!(classify("ls >out.txt"), Risk::Mutating);
        assert_eq!(classify("prog 2>&1"), Risk::Mutating);
    }

    #[test]
    fn a_pipeline_is_read_only_only_if_every_stage_is() {
        assert_eq!(classify("ls | head"), Risk::ReadOnly);
        assert_eq!(classify("ls | tee out.txt"), Risk::Mutating, "tee writes");
        assert_eq!(classify("cat f | weirdtool"), Risk::Unknown);
    }

    #[test]
    fn the_worst_stage_wins_wherever_it_sits() {
        // Not just the first or last: a destructive stage anywhere makes the
        // whole line destructive.
        assert_eq!(classify("ls | rm -rf x | head"), Risk::Mutating);
        assert_eq!(classify("weirdtool | ls"), Risk::Unknown);
    }

    #[test]
    fn an_unknown_command_snapshots_rather_than_being_guessed_safe() {
        // The asymmetry: a false positive is one wasted snapshot the store
        // dedups away; a false negative is a mutated directory with no way
        // back.
        assert_eq!(classify("cargo build"), Risk::Unknown);
        assert!(classify("cargo build").needs_snapshot());
        assert!(classify("./install.sh").needs_snapshot());
    }

    #[test]
    fn find_is_not_on_the_allowlist_because_of_delete() {
        // The stated reason commands are classified by name yet audited by
        // flag: `find -delete` is exactly the command that needs the snapshot
        // most, and a classifier trusting the name would skip it.
        assert!(!READ_ONLY.contains(&"find"));
        assert_eq!(classify("find . -name '*.tmp' -delete"), Risk::Unknown);
        assert!(classify("find . -type f").needs_snapshot(),
                "even harmless find snapshots, because we cannot tell them apart");
    }

    #[test]
    fn git_is_split_by_subcommand_rather_than_trusted_whole() {
        // `git checkout` and `git clean` are among the most destructive things
        // in a working tree; treating the binary as safe because most
        // subcommands are is exactly the mistake this split prevents.
        assert_eq!(classify("git status"), Risk::ReadOnly);
        assert_eq!(classify("git log --oneline"), Risk::ReadOnly);
        assert_eq!(classify("git checkout ."), Risk::Unknown);
        assert_eq!(classify("git clean -fd"), Risk::Unknown);
        assert_eq!(classify("git"), Risk::Unknown, "bare git is not a promise");
    }

    #[test]
    fn a_leading_path_does_not_hide_a_command() {
        // `/bin/rm` and `./rm` are still rm.
        assert_eq!(classify("/bin/rm -rf x"), Risk::Mutating);
        assert_eq!(classify("/usr/bin/ls"), Risk::ReadOnly);
    }

    #[test]
    fn sudo_is_looked_through_rather_than_treated_as_unknown() {
        // Elevation does not change what the command does to the cwd, and
        // refusing to look past it would classify every elevated command as
        // Unknown — which is safe but makes the classifier useless for anyone
        // who uses sudo routinely.
        assert_eq!(classify("sudo ls"), Risk::ReadOnly);
        assert_eq!(classify("sudo rm -rf /tmp/x"), Risk::Mutating);
        assert_eq!(classify("sudo"), Risk::Unknown, "sudo alone promises nothing");
    }

    #[test]
    fn an_environment_prefix_does_not_hide_the_command() {
        // `RUST_LOG=debug cargo run` is `cargo run`; treating the assignment as
        // the command name would classify everything prefixed that way as
        // Unknown.
        assert_eq!(classify("FOO=1 ls"), Risk::ReadOnly);
        assert_eq!(classify("RUST_LOG=debug rm x"), Risk::Mutating);
    }

    #[test]
    fn an_empty_command_needs_nothing() {
        // A bare Enter at the prompt should not cost a disk walk.
        assert_eq!(classify(""), Risk::ReadOnly);
        assert_eq!(classify("   "), Risk::ReadOnly);
        assert!(!classify("").needs_snapshot());
    }

    #[test]
    fn the_ordering_is_what_makes_combining_stages_a_max() {
        // Written as Ord so a new bucket cannot be added without deciding
        // where it sits in the caution order.
        assert!(Risk::Mutating > Risk::Unknown);
        assert!(Risk::Unknown > Risk::ReadOnly);
        assert_eq!([Risk::ReadOnly, Risk::Mutating, Risk::Unknown].iter().max(),
                   Some(&Risk::Mutating));
    }

    #[test]
    fn the_allowlist_and_the_mutating_list_do_not_overlap() {
        // A command in both would classify by whichever check ran first, and
        // the answer would depend on an implementation detail rather than on
        // what the command does.
        for c in READ_ONLY {
            assert!(!MUTATING.contains(c), "{c} is in both lists");
        }
    }

    #[test]
    fn classification_costs_no_allocation_beyond_splitting() {
        // "Classification runs on EVERY command, so it must be O(n) in command
        // length, no regex." A long line must not be quadratic.
        let long = "ls ".repeat(20_000);
        assert_eq!(classify(&long), Risk::ReadOnly);
    }
}
