//! modelorg.rs — treat `~/models` as a library rather than a pile of files.
//!
//! A flat directory of `.gguf` files hides the thing that actually matters: which files belong to the
//! same model. Split checkpoints arrive as `name-00001-of-00003.gguf`, and a set that is incomplete — or
//! that someone has "merged" with `cat` — looks exactly like a working model until a load dies deep in the
//! tensor table with something unhelpful like `missing tensor blk.36.ffn_up_exps.weight`.
//!
//! So this groups files into models, states each model's status honestly (complete, incomplete, needs
//! merging), and plans a tidy layout: `~/models/<family>/<model>/`. Planning is separated from doing —
//! [`plan_moves`] returns what WOULD happen and refuses to overwrite anything, so a reorganization can be
//! reviewed before a single file moves.
//!
//! On fetching: the shard filename tells us what is missing, but NOT which repository it came from. Rather
//! than guess a plausible-looking repo path, [`fetch_plan`] names the exact missing files and emits a
//! command with the repo left to the caller. A wrong download URL wastes tens of gigabytes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One file that is part of a split checkpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShardRef {
    pub file: String,
    pub index: u32,
    pub total: u32,
}

/// Parse the `-<index>-of-<total>` convention llama.cpp and HuggingFace both use. Returns the base name
/// (without the shard suffix or extension), the 1-based index, and the total.
pub fn parse_shard(filename: &str) -> Option<(String, u32, u32)> {
    let stem = filename.strip_suffix(".gguf")?;
    // ...-00001-of-00003
    let (head, total_s) = stem.rsplit_once("-of-")?;
    let (base, idx_s) = head.rsplit_once('-')?;
    let total = total_s.parse::<u32>().ok()?;
    let idx = idx_s.parse::<u32>().ok()?;
    if total == 0 || idx == 0 || idx > total { return None; }
    Some((base.to_string(), idx, total))
}

/// What state a model in the library is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// A single self-contained file.
    Single,
    /// Every shard present — ready to merge (or to load, if the loader follows shards).
    SplitComplete { total: u32 },
    /// Shards missing: loading or merging this will fail.
    SplitIncomplete { have: Vec<u32>, missing: Vec<u32>, total: u32 },
    /// Shard indices that do not form a clean 1..=total run (duplicates or disagreeing totals).
    SplitInconsistent { detail: String },
}

impl Status {
    /// Can this be loaded or merged as-is?
    pub fn is_usable(&self) -> bool {
        matches!(self, Status::Single | Status::SplitComplete { .. })
    }
}

/// A model in the library: one logical checkpoint, however many files it occupies.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelEntry {
    pub name: String,
    pub status: Status,
    /// Files in shard order (or the single file).
    pub files: Vec<String>,
    pub bytes: u64,
}

impl ModelEntry {
    /// The family folder this belongs under. Prefix matching on the filename, lowercased — deliberately
    /// simple and easily extended; anything unrecognised goes to `other` rather than being guessed at.
    pub fn family(&self) -> &'static str { family_of(&self.name) }
}

/// Map a model name to a family folder.
/// Rewrite a shard path to the FIRST shard of its set.
///
/// A split checkpoint's metadata and tensor index live in shard 1; the rest are payload. Handing shard 2
/// to a loader gives it a file with no header, and llama.cpp reports exactly that:
///
/// ```text
/// Load failed: failed to load GGUF '...Q4_K_M-00002-of-00002.gguf': null result from llama cpp
/// ```
///
/// The message is accurate and useless — the file is fine, it is simply not the entry point. Every
/// loader that understands shards expects the first and finds the others itself, so resolving here means
/// no backend has to know.
///
/// Returns None when the path is not a shard, so a caller can pass the original through unchanged
/// rather than needing a fallback.
pub fn resolve_to_first_shard(path: &str) -> Option<String> {
    let (dir, file) = match path.rfind('/') {
        Some(i) => (&path[..=i], &path[i + 1..]),
        None => ("", path),
    };
    let (stem, idx, total) = parse_shard(file)?;
    if idx == 1 { return None; }                       // already the entry point
    Some(format!("{dir}{stem}-{:05}-of-{:05}.gguf", 1, total))
}

/// Is this path a shard that is NOT the first?
///
/// Used by the picker: listing every shard as a separate model invites selecting one that cannot load,
/// which is how the error above happens in the first place.
pub fn is_secondary_shard(path: &str) -> bool {
    let file = path.rsplit('/').next().unwrap_or(path);
    matches!(parse_shard(file), Some((_, idx, total)) if total > 1 && idx > 1)
}

pub fn family_of(name: &str) -> &'static str {
    let n = name.to_ascii_lowercase();
    const FAMILIES: &[(&str, &str)] = &[
        ("llama", "llama"), ("meta-llama", "llama"),
        ("qwen", "qwen"), ("gemma", "gemma"), ("mistral", "mistral"), ("mixtral", "mistral"),
        ("phi", "phi"), ("glm", "glm"), ("deepseek", "deepseek"), ("yi-", "yi"),
        ("command-r", "cohere"), ("granite", "granite"), ("olmo", "olmo"), ("smol", "smol"),
    ];
    for (needle, folder) in FAMILIES {
        if n.starts_with(needle) || n.contains(&format!("-{needle}")) { return folder; }
    }
    "other"
}

/// Group a flat listing of `(filename, bytes)` into models. Pure — the caller does the directory read, so
/// grouping and status logic are testable without a filesystem.
pub fn scan(files: &[(String, u64)]) -> Vec<ModelEntry> {
    let mut shards: BTreeMap<String, (Vec<ShardRef>, u64)> = BTreeMap::new();
    let mut singles: Vec<ModelEntry> = Vec::new();

    for (f, sz) in files {
        if !f.ends_with(".gguf") { continue; }
        match parse_shard(f) {
            Some((base, index, total)) => {
                let e = shards.entry(base).or_insert_with(|| (Vec::new(), 0));
                e.0.push(ShardRef { file: f.clone(), index, total });
                e.1 += sz;
            }
            None => singles.push(ModelEntry {
                name: f.trim_end_matches(".gguf").to_string(),
                status: Status::Single,
                files: vec![f.clone()],
                bytes: *sz,
            }),
        }
    }

    let mut out = singles;
    for (base, (mut refs, bytes)) in shards {
        refs.sort_by_key(|r| r.index);
        let totals: Vec<u32> = { let mut t: Vec<u32> = refs.iter().map(|r| r.total).collect(); t.sort(); t.dedup(); t };
        let status = if totals.len() != 1 {
            Status::SplitInconsistent { detail: format!("shards disagree on the total: {totals:?}") }
        } else {
            let total = totals[0];
            let have: Vec<u32> = { let mut h: Vec<u32> = refs.iter().map(|r| r.index).collect(); h.dedup(); h };
            if have.len() != refs.len() {
                Status::SplitInconsistent { detail: "duplicate shard indices".into() }
            } else {
                let missing: Vec<u32> = (1..=total).filter(|i| !have.contains(i)).collect();
                if missing.is_empty() { Status::SplitComplete { total } }
                else { Status::SplitIncomplete { have, missing, total } }
            }
        };
        out.push(ModelEntry {
            name: base,
            status,
            files: refs.into_iter().map(|r| r.file).collect(),
            bytes,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Where a model should live: `<root>/<family>/<name>/`.
pub fn target_dir(root: &Path, e: &ModelEntry) -> PathBuf {
    root.join(e.family()).join(&e.name)
}

/// A single planned relocation.
#[derive(Clone, Debug, PartialEq)]
pub struct Move {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// Plan the reorganization without performing it. Files already in the right place are omitted, and a move
/// whose destination already exists is skipped and reported in `conflicts` — reorganizing a model library
/// must never clobber a file, because re-downloading one is measured in tens of gigabytes.
pub fn plan_moves(root: &Path, entries: &[ModelEntry], exists: &dyn Fn(&Path) -> bool) -> (Vec<Move>, Vec<String>) {
    let mut moves = Vec::new();
    let mut conflicts = Vec::new();
    for e in entries {
        let dir = target_dir(root, e);
        for f in &e.files {
            let from = root.join(f);
            let to = dir.join(f);
            if from == to { continue; }
            if exists(&to) {
                conflicts.push(format!("{} already exists — not overwriting", to.display()));
                continue;
            }
            moves.push(Move { from, to });
        }
    }
    (moves, conflicts)
}

/// The command that merges a complete split set into one file. Returns None unless the set is complete —
/// merging an incomplete set produces a corrupt model, and `cat` produces one that *looks* loadable while
/// silently exposing only the first shard's tensors.
pub fn merge_plan(dir: &Path, e: &ModelEntry) -> Option<String> {
    match e.status {
        Status::SplitComplete { .. } => {
            let first = e.files.first()?;
            Some(format!("llama-gguf-split --merge {} {}",
                dir.join(first).display(), dir.join(format!("{}.gguf", e.name)).display()))
        }
        _ => None,
    }
}

/// What to fetch for an incomplete set. The shard names are derived exactly; the REPOSITORY is not, because
/// a filename does not determine it — so the repo is left as a placeholder rather than guessed. Downloading
/// tens of gigabytes from a wrong-but-plausible path is worse than being told to supply it.
pub fn fetch_plan(e: &ModelEntry, repo: Option<&str>) -> Option<Vec<String>> {
    let Status::SplitIncomplete { missing, total, .. } = &e.status else { return None };
    let repo_s = repo.unwrap_or("<owner/repo>");
    let mut cmds = Vec::new();
    for i in missing {
        let f = format!("{}-{:05}-of-{:05}.gguf", e.name, i, total);
        cmds.push(format!("huggingface-cli download {repo_s} {f} --local-dir ."));
    }
    Some(cmds)
}

/// A one-screen account of the library: what is usable, what needs action, and the action.
pub fn report(root: &Path, entries: &[ModelEntry]) -> Vec<String> {
    let mut out = Vec::new();
    for e in entries {
        let gib = e.bytes as f64 / (1u64 << 30) as f64;
        match &e.status {
            Status::Single => out.push(format!("  ok        {:<52} {gib:>7.1} GiB", e.name)),
            Status::SplitComplete { total } => {
                out.push(format!("  ok        {:<52} {gib:>7.1} GiB  ({total} shards)", e.name));
                if let Some(c) = merge_plan(&target_dir(root, e), e) {
                    out.push(format!("              merge: {c}"));
                }
            }
            Status::SplitIncomplete { have, missing, total } => {
                out.push(format!("  INCOMPLETE {:<51} {gib:>7.1} GiB  have {}/{total}, missing {missing:?}",
                    e.name, have.len()));
                if let Some(cmds) = fetch_plan(e, None) {
                    for c in cmds { out.push(format!("              fetch: {c}")); }
                }
            }
            Status::SplitInconsistent { detail } =>
                out.push(format!("  BROKEN    {:<52} {detail}", e.name)),
        }
    }
    if out.is_empty() { out.push("  (no .gguf files found)".into()); }
    out
}

/// IO shell: read a directory listing as `(filename, bytes)`.
pub fn list_dir(dir: &Path) -> Vec<(String, u64)> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if !e.path().is_file() { continue; }
            let name = e.file_name().to_string_lossy().to_string();
            let sz = e.metadata().map(|m| m.len()).unwrap_or(0);
            v.push((name, sz));
        }
    }
    v
}

/// Apply planned moves, creating destination directories. Returns what actually moved. Any single failure
/// is reported and the rest continue — a half-organized library is recoverable, an aborted one mid-way with
/// no record is not.
pub fn apply_moves(moves: &[Move]) -> (Vec<PathBuf>, Vec<String>) {
    let mut done = Vec::new();
    let mut errs = Vec::new();
    for m in moves {
        if let Some(parent) = m.to.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                errs.push(format!("mkdir {}: {e}", parent.display())); continue;
            }
        }
        match std::fs::rename(&m.from, &m.to) {
            Ok(()) => done.push(m.to.clone()),
            Err(e) => errs.push(format!("{} -> {}: {e}", m.from.display(), m.to.display())),
        }
    }
    (done, errs)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_secondary_shard_resolves_to_the_first() {
        // The reported failure: llama.cpp given shard 2 of 2 says "null result", which is accurate and
        // useless — the file is fine, it is simply not the entry point. Metadata and the tensor index
        // live in shard 1.
        let p = "/m/Llama4Scout/Llama-4-Scout-17B-16E-Instruct-Q4_K_M-00002-of-00002.gguf";
        assert_eq!(resolve_to_first_shard(p).as_deref(),
                   Some("/m/Llama4Scout/Llama-4-Scout-17B-16E-Instruct-Q4_K_M-00001-of-00002.gguf"));
    }

    #[test]
    fn the_first_shard_and_a_plain_file_resolve_to_nothing() {
        // None means "pass the original through", so a caller needs no fallback branch — and a plain
        // file must never be rewritten into a shard name that does not exist.
        assert!(resolve_to_first_shard("/m/x-00001-of-00002.gguf").is_none());
        assert!(resolve_to_first_shard("/m/plain.gguf").is_none());
        assert!(resolve_to_first_shard("/m/no-extension").is_none());
    }

    #[test]
    fn the_directory_is_preserved() {
        // Rewriting the name and losing the path would turn a load error into a file-not-found, which is
        // a worse error because it points at the wrong thing entirely.
        let r = resolve_to_first_shard("/deep/nested/dir/model-00003-of-00004.gguf").unwrap();
        assert!(r.starts_with("/deep/nested/dir/"), "{r}");
        assert!(r.ends_with("model-00001-of-00004.gguf"), "{r}");
    }

    #[test]
    fn only_secondary_shards_are_hidden_from_a_picker() {
        // Listing every shard as a separate model is how the wrong one gets selected. The FIRST must
        // stay visible — hiding all of them would make a split model unloadable.
        assert!(is_secondary_shard("/m/a-00002-of-00003.gguf"));
        assert!(is_secondary_shard("/m/a-00003-of-00003.gguf"));
        assert!(!is_secondary_shard("/m/a-00001-of-00003.gguf"), "the entry point stays listed");
        assert!(!is_secondary_shard("/m/plain.gguf"));
    }

    #[test]
    fn a_single_file_set_is_not_treated_as_a_shard() {
        // `-00001-of-00001` is a valid name for an unsplit model and must load normally.
        assert!(!is_secondary_shard("/m/a-00001-of-00001.gguf"));
        assert!(resolve_to_first_shard("/m/a-00001-of-00001.gguf").is_none());
    }

    use super::*;

    #[test]
    fn parses_the_shard_convention() {
        assert_eq!(parse_shard("Llama-4-Scout-17B-16E-Instruct-Q4_K_M-00001-of-00002.gguf"),
                   Some(("Llama-4-Scout-17B-16E-Instruct-Q4_K_M".into(), 1, 2)));
        // a plain model is not a shard
        assert_eq!(parse_shard("Llama-3.2-3B-Instruct-Q8_0.gguf"), None);
        // nonsense indices are rejected rather than half-parsed
        assert_eq!(parse_shard("model-00003-of-00002.gguf"), None);
        assert_eq!(parse_shard("model-00000-of-00002.gguf"), None);
    }

    #[test]
    fn groups_shards_and_leaves_singles_alone() {
        let files = vec![
            ("Qwen3-Coder-30B-A3B-Instruct-Q4_K_M.gguf".to_string(), 18_000_000_000u64),
            ("Llama-4-Scout-Q4_K_M-00001-of-00002.gguf".to_string(), 31_000_000_000),
            ("Llama-4-Scout-Q4_K_M-00002-of-00002.gguf".to_string(), 31_000_000_000),
        ];
        let e = scan(&files);
        assert_eq!(e.len(), 2, "three files, two models");
        let scout = e.iter().find(|m| m.name.starts_with("Llama-4-Scout")).unwrap();
        assert_eq!(scout.status, Status::SplitComplete { total: 2 });
        assert_eq!(scout.files.len(), 2);
        assert_eq!(scout.bytes, 62_000_000_000, "size is the whole set");
        let qwen = e.iter().find(|m| m.name.starts_with("Qwen3")).unwrap();
        assert_eq!(qwen.status, Status::Single);
    }

    #[test]
    fn an_incomplete_set_is_flagged_before_it_wastes_a_load() {
        let files = vec![
            ("Big-Model-Q4-00001-of-00003.gguf".to_string(), 10),
            ("Big-Model-Q4-00003-of-00003.gguf".to_string(), 10),
        ];
        let e = scan(&files);
        assert_eq!(e.len(), 1);
        match &e[0].status {
            Status::SplitIncomplete { have, missing, total } => {
                assert_eq!(have, &vec![1, 3]);
                assert_eq!(missing, &vec![2], "names the exact gap");
                assert_eq!(*total, 3);
            }
            s => panic!("expected incomplete, got {s:?}"),
        }
        assert!(!e[0].status.is_usable());
        // and it must not offer a merge for a set that would produce a corrupt model
        assert!(merge_plan(Path::new("/m"), &e[0]).is_none());
    }

    #[test]
    fn fetch_plan_names_missing_files_but_never_guesses_the_repo() {
        let files = vec![("Big-Model-Q4-00001-of-00003.gguf".to_string(), 10)];
        let e = scan(&files);
        let cmds = fetch_plan(&e[0], None).expect("incomplete set has a fetch plan");
        assert_eq!(cmds.len(), 2, "shards 2 and 3");
        assert!(cmds[0].contains("Big-Model-Q4-00002-of-00003.gguf"), "{}", cmds[0]);
        assert!(cmds[0].contains("<owner/repo>"), "repo is a placeholder, not a guess: {}", cmds[0]);
        // when the caller knows the repo, it is used verbatim
        let cmds = fetch_plan(&e[0], Some("unsloth/Llama-4-Scout-GGUF")).unwrap();
        assert!(cmds[0].contains("unsloth/Llama-4-Scout-GGUF"));
    }

    #[test]
    fn disagreeing_shard_totals_are_broken_not_merely_incomplete() {
        let files = vec![
            ("M-00001-of-00002.gguf".to_string(), 1),
            ("M-00002-of-00003.gguf".to_string(), 1),
        ];
        let e = scan(&files);
        assert!(matches!(e[0].status, Status::SplitInconsistent { .. }));
        assert!(!e[0].status.is_usable());
    }

    #[test]
    fn families_route_to_folders_and_unknowns_are_not_guessed() {
        assert_eq!(family_of("Llama-4-Scout-17B"), "llama");
        assert_eq!(family_of("Qwen3-Coder-30B-A3B"), "qwen");
        assert_eq!(family_of("gemma-4-12b-it"), "gemma");
        assert_eq!(family_of("GLM-4.7-Q4"), "glm");
        assert_eq!(family_of("ornith-9b"), "other", "unrecognised goes to other, not a guess");
    }

    #[test]
    fn move_plan_is_reviewable_and_never_overwrites() {
        let root = Path::new("/models");
        let files = vec![("Qwen3-Coder-30B-Q4.gguf".to_string(), 1)];
        let e = scan(&files);
        // nothing exists at the destination → a clean single move
        let (moves, conflicts) = plan_moves(root, &e, &|_| false);
        assert_eq!(moves.len(), 1);
        assert_eq!(moves[0].to, PathBuf::from("/models/qwen/Qwen3-Coder-30B-Q4/Qwen3-Coder-30B-Q4.gguf"));
        assert!(conflicts.is_empty());
        // destination occupied → refuse, and say so, rather than clobber tens of GiB
        let (moves, conflicts) = plan_moves(root, &e, &|_| true);
        assert!(moves.is_empty());
        assert_eq!(conflicts.len(), 1);
        assert!(conflicts[0].contains("not overwriting"));
    }

    #[test]
    fn merge_command_targets_the_first_shard() {
        let files = vec![
            ("M-00001-of-00002.gguf".to_string(), 1),
            ("M-00002-of-00002.gguf".to_string(), 1),
        ];
        let e = scan(&files);
        let cmd = merge_plan(Path::new("/models/other/M"), &e[0]).unwrap();
        assert!(cmd.starts_with("llama-gguf-split --merge"));
        assert!(cmd.contains("M-00001-of-00002.gguf"), "merge starts from shard 1: {cmd}");
        assert!(cmd.contains("/models/other/M/M.gguf"), "{cmd}");
    }

    #[test]
    fn report_surfaces_action_for_each_problem() {
        let files = vec![
            ("Good-Q4.gguf".to_string(), 1 << 30),
            ("Half-Q4-00001-of-00002.gguf".to_string(), 1 << 30),
        ];
        let e = scan(&files);
        let r = report(Path::new("/models"), &e).join("\n");
        assert!(r.contains("ok        Good-Q4"));
        assert!(r.contains("INCOMPLETE"));
        assert!(r.contains("fetch:"), "a problem is reported with its remedy:\n{r}");
    }
}
