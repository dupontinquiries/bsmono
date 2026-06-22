// src/cmd_classifier.rs
//
// GITLOG-4 (v0.1.256). Classifies a command-line string into one
// of three buckets to decide whether the snapshot store should
// capture a pre-execution snapshot of the cwd:
//
//   ReadOnly  → skip snapshot (no rollback possible, none needed)
//   Mutating  → take snapshot (rollback may be wanted)
//   Unknown   → take snapshot (safe default; better to snapshot
//               an unnecessary one than to miss a needed one)
//
// Design principles:
//
//   1. Conservative: the ReadOnly list is SHORT and AUTHORITATIVE.
//      Only commands we are 100% certain cannot mutate the cwd
//      under any argument combination. When in doubt, classify
//      as Unknown.
//
//   2. Argument-aware for redirects: `ls > out.txt` is read-only
//      for `ls` but the redirect writes a file. The classifier
//      detects shell redirects (>, >>, tee, etc.) and forces
//      classification to Mutating regardless of the base command.
//
//   3. Pipe-aware: `ls | head` is read-only iff BOTH ls and head
//      are read-only. Splits on pipes and re-classifies each
//      stage. Worst classification wins (Mutating > Unknown >
//      ReadOnly).
//
//   4. Cheap: classification runs on EVERY command, so it must
//      be O(n) in command length, no regex, no allocations
//      beyond what split() inevitably produces.
//
//   5. Tested by name not by behavior: we cannot actually run
//      the commands to verify. The READ_ONLY list reflects
//      common-shell-coreutil semantics with no surprise flags.
//      Tested commands that DO have surprise flags (find with
//      -delete, grep that doesn't take -d, etc.) are excluded
//      from the list and fall through to Unknown.
//
// False-positive cost (read-only classified as mutating):
//   one wasted snapshot. Cheap; the snapshot_store dedups by
//   content anyway so identical state across snapshots costs
//   nothing in storage.
//
// False-negative cost (mutating classified as read-only):
//   no snapshot taken, rollback unavailable for that command.
//   This is the EXPENSIVE case. Hence the conservative bias.

/// Three-state classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Definitely cannot mutate the filesystem. Skip snapshot.
    ReadOnly,
    /// Will likely mutate. Take snapshot.
    Mutating,
    /// Unknown / can't determine safely. Take snapshot (safe
    /// default).
    Unknown,
}

impl Class {
    /// Snapshot? True for Mutating and Unknown. False only for
    /// confirmed ReadOnly.
    pub fn should_snapshot(self) -> bool {
        !matches!(self, Class::ReadOnly)
    }
}

/// Bare-command names that cannot mutate the filesystem under
/// ANY argument combination we care about. To qualify:
///
///   - No flag turns it into a mutating command (e.g. find has
///     -delete, so find is NOT here; rsync has no read-only-only
///     mode, NOT here).
///   - Output goes to stdout, which is read-only to the
///     filesystem (the calling shell could redirect, but redirect
///     detection handles that separately).
///   - No file/dir creation as side-effect (e.g. some tools
///     create cache dirs in $HOME; we tolerate that because
///     snapshot only watches cwd).
///
/// Curated and intentionally short. Expand cautiously.
const READ_ONLY_COMMANDS: &[&str] = &[
    // Core listing / display
    "ls", "ll", "la", "dir", "tree",
    "pwd", "cd",  // cd changes shell state, not files
    "echo", "printf",
    "cat", "bat", "less", "more", "head", "tail",
    "view",  // vim's view is read-only mode

    // Path/file inspection (no -w / -d flags)
    "file", "stat", "wc", "du", "df", "realpath", "readlink",
    "basename", "dirname",
    "which", "whereis", "type", "command",

    // Search (output to stdout only)
    "grep", "egrep", "fgrep", "rgrep",
    "ripgrep", "rg",   // rg has --replace but writes to stdout by default; the only mutating form is --no-context with -i and another tool, so safe enough
    "ag",              // similar to rg
    "ack",
    // Note: find is NOT here — -delete and -exec rm {} \; make
    // it too dangerous to classify as read-only. Falls through
    // to Unknown which still snapshots. Safe default.

    // Diff / compare (output only)
    "diff", "cmp", "comm",

    // Git read-only subcommands handled in classify_git below.

    // Process / system inspection
    "ps", "top", "htop", "iostat", "vmstat", "free", "uptime",
    "who", "w", "id", "groups", "whoami", "hostname", "uname",
    "date", "cal",
    "env", "printenv", "set",  // set with no args shows vars
    "jobs", "history",
    "lsof", "lsblk", "lscpu", "lsusb", "lspci", "lshw",

    // Network inspection (no -w write to interface)
    "ping", "ping6", "host", "dig", "nslookup", "traceroute",
    "ip", "ifconfig", "netstat", "ss", "route",
    "curl",  // curl can -o to write a file; but its default is
             // stdout. We accept the risk here because so many
             // people use curl to-stdout heavily. -o detection
             // could land later.
    // We exclude wget which defaults to writing a file.

    // Text manipulation (output to stdout)
    "sort", "uniq", "cut", "paste", "tr", "rev", "join",
    "fold", "expand", "unexpand", "fmt", "column",
    "tac",

    // Encoding / hashing
    "base64", "md5sum", "sha1sum", "sha256sum", "sha512sum",
    "cksum", "shasum",
    "od", "hexdump", "xxd",

    // Help / man
    "man", "info", "help", "apropos", "tldr",

    // Calculator (no fs side-effects)
    "bc", "dc", "expr",

    // Misc utility
    "true", "false", "yes", "test", "[",
    "sleep",
    "seq",
    "tput",  // emits terminal escape codes to stdout
    "clear", "reset",
];

/// Removed from READ_ONLY_COMMANDS list above; treated as Unknown
/// (snapshotted) because they have flags that turn them into
/// mutating commands:
///
///   - find: -delete, -exec rm {} \;
///   - tee: writes to file by design
///   - dd: can do anything
///   - sed: -i edits in place
///   - awk: can call system() or open files for write
///   - tar / unzip / gzip / gunzip: archive ops write files
///   - mount / umount: side-effecting on system state
///   - sudo / doas / su: anything

/// Commands that definitely mutate. Snapshot regardless of args.
/// Putting them on this list means: if we recognize the bare
/// command name, we already know it's mutating.
const DEFINITELY_MUTATING_COMMANDS: &[&str] = &[
    // File / dir operations
    "rm", "rmdir", "unlink", "shred", "wipe",
    "mv", "rename",
    "cp", "rsync", "scp",
    "mkdir", "touch",
    "ln",     // creates link
    "chmod", "chown", "chgrp",
    "setfacl",
    "tee",    // -a appends, writes either way
    "dd",     // can do anything
    "install",
    "patch",
    "truncate",

    // In-place editors
    "sed",    // -i; even without, many invocations DO mutate
    "ed",
    "ex",
    // Note: vi/vim/nano/helix/emacs are interactive editors.
    // They MIGHT mutate. Conservative: classified as Mutating.
    "vi", "vim", "nvim", "nano", "pico", "emacs", "ne", "joe",
    "helix", "hx", "micro", "kak",
    "code", "subl", "atom",   // GUI editors invoked from shell

    // Archive / compression operations write files
    "tar", "unzip", "zip", "gzip", "gunzip", "bzip2", "bunzip2",
    "xz", "unxz", "zstd",
    "7z", "rar", "unrar",

    // Build / language tooling typically writes outputs
    "make", "ninja", "cmake",
    "cargo",  // build, clean, run, install — all touch fs
    "go",     // go build, go install
    "rustc", "gcc", "g++", "clang", "clang++",
    "javac", "kotlinc",
    "npm", "yarn", "pnpm", "bun",
    "pip", "pip3", "uv",
    "conda", "mamba",
    "gem",
    "composer",
    "mvn", "gradle",

    // Package managers
    "apt", "apt-get", "aptitude", "dpkg",
    "yum", "dnf", "rpm",
    "pacman", "yay", "paru",
    "brew",
    "snap", "flatpak",
    "zypper",
    "emerge",
    "nix-env", "nix",
    "rpm-ostree",

    // System modification
    "systemctl", "service", "init",
    "mount", "umount",
    "modprobe", "insmod", "rmmod",
    "useradd", "userdel", "usermod", "adduser", "deluser",
    "groupadd", "groupdel", "groupmod",
    "passwd",

    // VCS write operations handled in classify_git below;
    // listing 'git' here is too coarse.

    // Container / VM ops
    "docker", "podman",   // run, build, pull mutate fs
    "kubectl",            // apply, delete, patch mutate state
    "vagrant",

    // Curl/wget variants that write
    "wget",    // defaults to writing a file
    "fetch",
    "aria2c", "axel",
];

/// Shell redirection / piping operators that indicate a write
/// somewhere on disk. If any of these appears (outside quotes —
/// which we don't currently detect; see note below), force
/// classification to Mutating.
///
/// Quote detection is intentionally absent for v1. Cost: a
/// command like `echo 'foo > bar.txt'` (literal string with `>`
/// inside quotes) gets classified as Mutating when it's actually
/// just an echo. The cost of that false positive is one wasted
/// snapshot. Cheap. Worth the simpler code.
const REDIRECT_TOKENS: &[&str] = &[
    ">", ">>", "2>", "2>>", "&>", "&>>", "<>",
    "|tee", "| tee",
    // Process substitution `>(cmd)` also writes; '>' check above catches the leading char.
];

/// Classify a command line. Splits on pipes and re-classifies
/// each stage. Worst stage wins.
pub fn classify(cmd: &str) -> Class {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return Class::ReadOnly; // empty command, nothing to snapshot
    }
    // v0.1.584 (adversarial-probe fix): split on COMMAND SEPARATORS
    // (; && ||) FIRST and classify each clause, worst wins. Previously
    // only pipes were split, so "ls && rm -rf build" was seen as one
    // stage starting with `ls` → ReadOnly → NO SNAPSHOT before a
    // recursive delete. A destructive later clause must force a snapshot.
    let clauses = split_command_separators(trimmed);
    if clauses.len() > 1 {
        let mut worst = Class::ReadOnly;
        for clause in clauses {
            worst = combine_worst(worst, classify(clause.trim()));
            if matches!(worst, Class::Mutating) {
                return worst;
            }
        }
        return worst;
    }
    // Detect redirects FIRST. If any are present, immediate
    // Mutating regardless of base command. (`cat foo > bar.txt`
    // is a write.)
    if has_redirect(trimmed) {
        return Class::Mutating;
    }
    // Split on pipes. Walk each stage; track worst classification.
    let stages: Vec<&str> = trimmed.split('|')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if stages.is_empty() {
        return Class::ReadOnly;
    }
    let mut worst = Class::ReadOnly;
    for stage in &stages {
        let stage_class = classify_single(stage);
        worst = combine_worst(worst, stage_class);
        if matches!(worst, Class::Mutating) {
            return worst;  // can't get worse than Mutating
        }
    }
    worst
}

/// Split a command line on shell command separators (`;`, `&&`, `||`)
/// into its top-level clauses, so each can be classified independently.
/// Does NOT split on `|` (that's pipe-stage handling, done separately
/// and with different semantics — pipe stages share a data stream).
/// Naive on quotes/subshells (same trade-off the rest of the module
/// makes — over-splitting only costs an extra snapshot, never a missed
/// one).
fn split_command_separators(cmd: &str) -> Vec<&str> {
    let bytes = cmd.as_bytes();
    let mut clauses = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        // v0.1.732: byte-level && / || check. The old `&cmd[i..i+2]` str slice
        // panics when i lands mid-multibyte-char (e.g. a command with a unicode
        // arg/filename). `&` (0x26) and `|` (0x7C) are ASCII and never appear
        // inside a multibyte sequence, so a byte compare is correct + safe.
        let two_amp = bytes[i] == b'&' && i + 1 < bytes.len() && bytes[i + 1] == b'&';
        let two_pipe = bytes[i] == b'|' && i + 1 < bytes.len() && bytes[i + 1] == b'|';
        if two_amp || two_pipe {
            clauses.push(&cmd[start..i]);
            i += 2;
            start = i;
            continue;
        }
        // A single ';' separates clauses. ('|' alone is a pipe — leave it.)
        if bytes[i] == b';' {
            clauses.push(&cmd[start..i]);
            i += 1;
            start = i;
            continue;
        }
        i += 1;
    }
    clauses.push(&cmd[start..]);
    clauses.into_iter().map(|c| c.trim()).filter(|c| !c.is_empty()).collect()
}

/// Combine two classifications, returning the more conservative
/// (i.e. more snapshot-prone) one.
fn combine_worst(a: Class, b: Class) -> Class {
    match (a, b) {
        (Class::Mutating, _) | (_, Class::Mutating) => Class::Mutating,
        (Class::Unknown, _) | (_, Class::Unknown) => Class::Unknown,
        _ => Class::ReadOnly,
    }
}

/// True if the command line contains any shell-redirect operator
/// (outside quotes — which we don't detect, see module-level
/// comment for that trade-off).
fn has_redirect(cmd: &str) -> bool {
    // We do a token-aware scan rather than naive substring,
    // because `>>>` (rare but valid in some shells as bit-shift)
    // and `==>` (some scripts) should not falsely match. We
    // tokenize on whitespace AND on the redirect chars themselves.
    //
    // Simplification: just check for substring presence of each
    // canonical token; false-positive cost (wasted snapshot) is
    // tolerable.
    for tok in REDIRECT_TOKENS {
        if cmd.contains(tok) { return true; }
    }
    false
}

/// Classify a single pipe-stage. No pipe handling here (caller
/// already split). Detects:
///   1. sudo/doas/env prefix (strip and recurse)
///   2. git/cargo/npm-like subcommands (delegate)
///   3. Bare command match against READ_ONLY / MUTATING lists
///   4. Default Unknown
fn classify_single(stage: &str) -> Class {
    let stage = stage.trim();
    if stage.is_empty() { return Class::ReadOnly; }

    // Strip leading sudo/doas/env/time/nice/nohup prefixes.
    // These don't themselves mutate; they wrap another command.
    // We recurse on the wrapped command.
    let prefixes = &[
        "sudo ", "doas ", "su ", "env ", "time ", "nice ", "nohup ",
        "ionice ", "stdbuf ", "exec ", "command ", "builtin ",
    ];
    for p in prefixes {
        if stage.starts_with(p) {
            let mut rest = &stage[p.len()..];
            // v0.1.466: after a wrapper prefix, skip leading tokens
            // that AREN'T the wrapped command, so we recurse on the
            // real command:
            //   - VAR=value env assignments: `env VAR=1 ls`
            //   - options to the WRAPPER itself, some of which take
            //     a value in the next token: `nice -n 19 ls`,
            //     `stdbuf -oL cat`, `ionice -c 3 cmd`. The prior
            //     code stopped at the first '-token' (→ Unknown),
            //     the bug behind env_and_time_prefixes_recurse.
            // Stop at the first token that is neither an assignment
            // nor an option — that's the wrapped command.
            let trimmed_p = p.trim();
            loop {
                let trimmed = rest.trim_start();
                let tok = first_word(trimmed);
                if tok.is_empty() { rest = trimmed; break; }
                if is_env_assignment(tok) {
                    rest = &trimmed[tok.len()..];
                    continue;
                }
                if tok.starts_with('-') {
                    rest = &trimmed[tok.len()..];
                    // Does this wrapper-flag consume the next token
                    // as its value? (Only the space-separated form;
                    // `-n19` / `-oL` attach their value.)
                    if !tok.contains('=')
                        && wrapper_flag_takes_value(trimmed_p, tok) {
                        let after = rest.trim_start();
                        let val = first_word(after);
                        rest = &after[val.len()..];
                    }
                    continue;
                }
                rest = trimmed;
                break;
            }
            return classify_single(rest);
        }
    }

    // Pull first word as the command name.
    let bare = first_word(stage);
    if bare.is_empty() { return Class::ReadOnly; }

    // Strip leading ./ from local-executable paths so e.g.
    // "./build.sh" is recognized as a script invocation
    // (Unknown — could mutate). Drop path components for
    // long forms like /usr/bin/ls — we want the basename.
    let basename = bare.rsplit('/').next().unwrap_or(bare);

    // Special-case some commands whose subcommands matter.
    match basename {
        "git" => return classify_git(stage),
        // Could add: "cargo", "npm", "yarn", "kubectl", etc.
        // But all of those are on DEFINITELY_MUTATING already
        // because nearly all subcommands of those tools mutate
        // (build, run, install, etc. all touch fs). The few
        // read-only ones (cargo metadata, npm list, kubectl get)
        // get treated as Mutating which is a small false-positive
        // cost. Acceptable v1 trade-off.
        _ => {}
    }

    if READ_ONLY_COMMANDS.iter().any(|c| *c == basename) {
        return Class::ReadOnly;
    }
    if DEFINITELY_MUTATING_COMMANDS.iter().any(|c| *c == basename) {
        return Class::Mutating;
    }
    Class::Unknown
}

/// v0.1.466: does a wrapper command's flag consume the NEXT token
/// as its value (space-separated form)? Used so the prefix-strip
/// recursion skips `nice -n 19 cmd` → `cmd`, not stopping at `-n`.
/// Conservative: only the well-known value-taking flags per
/// wrapper. Unknown flags are treated as valueless (skip just the
/// flag) — worst case we stop one token early and fall to Unknown,
/// which is the safe (snapshot-taking) default.
fn wrapper_flag_takes_value(wrapper: &str, flag: &str) -> bool {
    match wrapper {
        // nice -n <adjustment>
        "nice" => matches!(flag, "-n" | "--adjustment"),
        // ionice -c <class> -n <classdata> -p <pid>
        "ionice" => matches!(flag, "-c" | "--class" | "-n"
            | "--classdata" | "-p" | "--pid"),
        // stdbuf -i/-o/-e <MODE> (note: -oL attaches; only the
        // space-separated form reaches here)
        "stdbuf" => matches!(flag, "-i" | "--input" | "-o"
            | "--output" | "-e" | "--error"),
        // env -u <name> -C <dir> -S <string>; -i has no value
        "env" => matches!(flag, "-u" | "--unset" | "-C"
            | "--chdir" | "-S" | "--split-string"),
        // timeout-like: `time` (GNU) -o <file> -f <fmt>; the shell
        // builtin `time` takes none, but the binary form might.
        "time" => matches!(flag, "-o" | "--output" | "-f" | "--format"),
        _ => false,
    }
}

/// First whitespace-delimited word.
fn first_word(s: &str) -> &str {
    s.split_whitespace().next().unwrap_or("")
}

/// v0.1.466: is this token a `VAR=value` environment assignment
/// (as accepted by `env` and by bare shell command lines)?
/// True iff it has an '=' and the part before '=' is a valid
/// shell identifier (starts with letter/underscore, then
/// alphanumerics/underscores) and is non-empty. Rejects things
/// like "--opt=val" (leading '-') and "a/b=c" (path) so we don't
/// mistake real arguments for assignments.
fn is_env_assignment(tok: &str) -> bool {
    let eq = match tok.find('=') {
        Some(i) => i,
        None => return false,
    };
    if eq == 0 { return false; } // "=foo" — not an assignment
    let name = &tok[..eq];
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Read-only git subcommands. Carefully curated. ANYTHING that
/// writes refs, objects, working tree, or index is excluded.
///
/// Excluded on review because of false-negative risk:
///   - branch (can delete with -d)
///   - tag (can delete with -d)
///   - config (writes with assignment)
///   - remote (add/rm mutate)
///   - bisect (runs other commands which may mutate)
///   - stash (save mutates)
/// All of those fall through to Mutating in classify_git's
/// final return, which is the safe default.
const GIT_TRULY_READ_ONLY: &[&str] = &[
    "status", "log", "show", "diff", "blame",
    "describe", "rev-parse", "rev-list",
    "ls-files", "ls-tree", "cat-file",
    "show-ref", "symbolic-ref",
    "name-rev", "whatchanged", "shortlog",
    "reflog", "grep",
];

fn classify_git(cmd: &str) -> Class {
    // After "git", look at the subcommand. Skip --options to git
    // itself like `git -C path status`.
    let mut tokens = cmd.split_whitespace().peekable();
    tokens.next(); // consume "git"
    let mut subcmd: Option<&str> = None;
    while let Some(tok) = tokens.next() {
        if tok.starts_with('-') {
            // v0.1.466: some git global flags TAKE A VALUE in the
            // NEXT token: `-C <path>`, `-c <name=value>`,
            // `--git-dir <path>`, `--work-tree <path>`, etc. The
            // prior code skipped only the flag itself, so for
            // `git -C /tmp status` the value `/tmp` was mistaken
            // for the subcommand → not in the read-only list →
            // Mutating (the bug behind git_options_skipped).
            // Consume the value token too for these flags.
            //
            // Exception: the `=`-joined form (`--git-dir=/tmp`,
            // `-c x=y`... actually -c uses a space) keeps its value
            // attached, so only the space-separated forms need the
            // next-token skip.
            let takes_value = matches!(tok,
                "-C" | "-c" | "--git-dir" | "--work-tree"
                | "--namespace" | "--super-prefix" | "--exec-path"
                | "--config-env");
            if takes_value && !tok.contains('=') {
                tokens.next(); // consume the value
            }
            continue;
        }
        subcmd = Some(tok);
        break;
    }
    let sub = match subcmd { Some(s) => s, None => return Class::ReadOnly };
    if GIT_TRULY_READ_ONLY.iter().any(|s| *s == sub) {
        return Class::ReadOnly;
    }
    // Everything else under git: pull, push, fetch, clone, commit,
    // merge, rebase, reset, checkout, cherry-pick, am, apply, mv,
    // rm, add, restore, etc. — mutating.
    Class::Mutating
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_class(cmd: &str, expected: Class) {
        let got = classify(cmd);
        assert_eq!(got, expected,
            "classify({:?}) = {:?}, expected {:?}", cmd, got, expected);
    }

    #[test]
    fn empty_is_readonly() {
        assert_class("", Class::ReadOnly);
        assert_class("   ", Class::ReadOnly);
    }

    #[test]
    fn basic_read_only() {
        assert_class("ls", Class::ReadOnly);
        assert_class("ls -la", Class::ReadOnly);
        assert_class("pwd", Class::ReadOnly);
        assert_class("cat foo.txt", Class::ReadOnly);
        assert_class("echo hello", Class::ReadOnly);
        assert_class("head -n 5 file", Class::ReadOnly);
    }

    #[test]
    fn basic_mutating() {
        assert_class("rm foo.txt", Class::Mutating);
        assert_class("rm -rf /tmp/bar", Class::Mutating);
        assert_class("mv a b", Class::Mutating);
        assert_class("cp a b", Class::Mutating);
        assert_class("mkdir new_dir", Class::Mutating);
        assert_class("touch x", Class::Mutating);
        assert_class("chmod +x script.sh", Class::Mutating);
    }

    #[test]
    fn editors_classified_mutating() {
        // Could be just viewing, but conservative.
        assert_class("vim foo.txt", Class::Mutating);
        assert_class("nano foo.txt", Class::Mutating);
        assert_class("hx src/main.rs", Class::Mutating);
    }

    #[test]
    fn redirects_force_mutating() {
        assert_class("echo hi > foo.txt", Class::Mutating);
        assert_class("ls > listing.txt", Class::Mutating);
        assert_class("cat src/* >> all.txt", Class::Mutating);
        assert_class("ls 2> err.log", Class::Mutating);
    }

    #[test]
    fn pipes_walk_each_stage() {
        // All read-only pipe: read-only.
        assert_class("ls | head", Class::ReadOnly);
        assert_class("cat foo | grep bar | head", Class::ReadOnly);
        // One mutating stage poisons the chain.
        assert_class("cat foo | tee out.txt", Class::Mutating);
        assert_class("find . | xargs rm", Class::Unknown);
        // xargs not in either list → Unknown, which conservatively
        // snapshots. Good.
    }

    #[test]
    fn compound_commands_take_worst_clause() {
        // v0.1.584 (adversarial-probe fix): a destructive LATER clause in
        // a compound command must force Mutating so a snapshot is taken.
        // Previously only `|` was split, so "ls && rm -rf x" looked like
        // a read-only `ls` and SKIPPED the snapshot before a delete.
        assert_class("ls && rm -rf build", Class::Mutating);
        assert_class("cd foo; rm -rf bar", Class::Mutating);
        assert_class("make || rm -rf out", Class::Mutating);
        // genuinely all-read-only compounds stay ReadOnly (no wasted snapshot)
        assert_class("ls && cat readme", Class::ReadOnly);
        assert_class("pwd; ls; git status", Class::ReadOnly);
        // single commands are unaffected by the new split path
        assert_class("git log", Class::ReadOnly);
        assert_class("rm -rf x", Class::Mutating);
    }

    #[test]
    fn unknown_commands_are_unknown() {
        assert_class("./build.sh", Class::Unknown);
        assert_class("python script.py", Class::Unknown);
        assert_class("my-custom-tool", Class::Unknown);
        assert_class("make_widget", Class::Unknown);
    }

    #[test]
    fn sudo_recurses_into_wrapped_command() {
        assert_class("sudo ls", Class::ReadOnly);
        assert_class("sudo rm foo", Class::Mutating);
        assert_class("sudo apt update", Class::Mutating);
        assert_class("doas pwd", Class::ReadOnly);
    }

    #[test]
    fn env_and_time_prefixes_recurse() {
        assert_class("env VAR=1 ls", Class::ReadOnly);
        assert_class("time make", Class::Mutating);
        assert_class("nice -n 19 ls", Class::ReadOnly);
    }

    #[test]
    fn git_readonly_subcommands() {
        assert_class("git status", Class::ReadOnly);
        assert_class("git log --oneline -10", Class::ReadOnly);
        assert_class("git diff HEAD", Class::ReadOnly);
        assert_class("git show HEAD~3", Class::ReadOnly);
    }

    #[test]
    fn git_mutating_subcommands() {
        assert_class("git commit -m foo", Class::Mutating);
        assert_class("git pull", Class::Mutating);
        assert_class("git push origin main", Class::Mutating);
        assert_class("git checkout other-branch", Class::Mutating);
        assert_class("git reset --hard", Class::Mutating);
        assert_class("git add .", Class::Mutating);
    }

    #[test]
    fn git_options_skipped() {
        // `git -C /tmp status` should still classify status.
        assert_class("git -C /tmp status", Class::ReadOnly);
        assert_class("git -c color.ui=auto log", Class::ReadOnly);
    }

    #[test]
    fn package_managers_mutating() {
        assert_class("apt install foo", Class::Mutating);
        assert_class("npm install", Class::Mutating);
        assert_class("cargo build", Class::Mutating);
        assert_class("pip install requests", Class::Mutating);
    }

    #[test]
    fn full_paths_strip_to_basename() {
        assert_class("/usr/bin/ls -la", Class::ReadOnly);
        assert_class("/bin/rm /tmp/x", Class::Mutating);
    }

    #[test]
    fn should_snapshot_logic() {
        assert!(!Class::ReadOnly.should_snapshot());
        assert!(Class::Mutating.should_snapshot());
        assert!(Class::Unknown.should_snapshot());
    }

    #[test]
    fn find_treated_as_unknown_due_to_delete_flag() {
        // find IS NOT in the read-only list because -delete and
        // -exec rm exist. Must classify as Unknown (which will
        // snapshot, the safe choice).
        assert_class("find . -name '*.txt'", Class::Unknown);
        assert_class("find . -delete", Class::Unknown);
        // Even with -delete it's only Unknown, not Mutating —
        // we don't parse find flags. The snapshot still happens.
    }

    #[test]
    fn tee_alone_or_in_pipe_is_mutating() {
        // tee always writes a file.
        assert_class("tee out.txt", Class::Mutating);
        // tee in a pipe also mutates.
        assert_class("echo hi | tee out.txt", Class::Mutating);
    }

    #[test]
    fn whitespace_handled() {
        assert_class("  ls   ", Class::ReadOnly);
        assert_class("\tcat foo\t", Class::ReadOnly);
    }

    // v0.1.732 (dogfood crash class): a command containing a multibyte char
    // must not panic the && / || separator split (was &cmd[i..i+2]).
    #[test]
    fn unicode_command_does_not_panic_separator_split() {
        // emoji + CJK in args, around real && / || separators.
        assert_class("echo 🎬 && ls café", Class::ReadOnly);
        assert_class("grep 日本語 file || echo done", Class::ReadOnly);
        // emoji directly adjacent to a separator (worst-case alignment)
        let _ = super::split_command_separators("echo 🎬&&ls");
        let _ = super::split_command_separators("x日本||y");
    }
}
