//! Local model discovery, autocomplete and load feedback.
//!
//! The convention already exists in this monorepo: `~/models` holding
//! `.gguf` files, walked by `logscope::llm_backend::list_local_gguf`
//! into `LocalModel { name, path, size_bytes, params_billions }`. This
//! module deliberately mirrors that shape so the two agree, but
//! reimplements the walk with no dependencies (`llm_backend` uses
//! `dirs` and `regex`; this crate has none and should stay that way so
//! it drops into any binary).
//!
//! Two things are added that matter for a chat surface and are absent
//! there: the **quantisation** parsed from the filename, because
//! "which Q is this" is the first thing anyone asks; and a **load
//! state machine** so the box can say `loading…` and then
//! `model loaded: … in 3.2s` rather than freezing silently.
//!
//! Nothing here loads a model. Loading is the game's job (spawn a
//! llama.cpp server, or point at one already running); this owns the
//! list, the names, and what the player is told.

use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub struct LocalModel {
    /// File stem, e.g. `qwen2.5-7b-instruct-q4_k_m`.
    pub name: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    /// Parsed from the filename; 0.0 when absent.
    pub params_billions: f32,
    /// Parsed quantisation tag, e.g. `Q4_K_M`. None when absent.
    pub quant: Option<String>,
}

impl LocalModel {
    /// One line for the chat log or a card.
    pub fn describe(&self) -> String {
        let gb = self.size_bytes as f64 / 1e9;
        let mut s = self.name.clone();
        if self.params_billions > 0.0 { s.push_str(&format!("  {:.1}B", self.params_billions)); }
        if let Some(q) = &self.quant { s.push_str(&format!("  {q}")); }
        s.push_str(&format!("  {gb:.1} GB"));
        s
    }

    /// Rough guess at whether this fits a context budget on a machine
    /// with `vram_gb`. Deliberately crude and labelled as a guess: it
    /// is here so the box can warn before a 40 GB model is loaded on a
    /// 16 GB card, not to be precise.
    pub fn likely_fits(&self, vram_gb: f64) -> bool {
        (self.size_bytes as f64 / 1e9) * 1.15 < vram_gb
    }
}

/// `~/models`, the same path `llm_backend::default_models_dir` uses.
/// Resolved from `$HOME` rather than the `dirs` crate to stay
/// dependency-free; falls back to `.` exactly as that one does.
pub fn default_models_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("models")
}

/// Parameter count in billions from a filename: `qwen2.5-7b` -> 7.0,
/// `mixtral-8x7b` -> 56.0. Matches `llm_backend::estimate_params_billions`
/// semantics without regex.
pub fn estimate_params_billions(path: &str) -> Option<f32> {
    let name = Path::new(path).file_name()?.to_string_lossy().to_lowercase();
    let b = name.as_bytes();
    // scan for `<num>b`, and check for a preceding `<num>x`
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'b' { i += 1; continue; }
        // walk back over the number before this 'b'
        let mut s = i;
        let mut seen_digit = false;
        while s > 0 {
            let c = b[s - 1];
            if c.is_ascii_digit() { seen_digit = true; s -= 1; }
            else if c == b'.' && seen_digit { s -= 1; }
            else { break; }
        }
        if !seen_digit { i += 1; continue; }
        // the char after 'b' must not continue a word ("bert", "base")
        if b.get(i + 1).is_some_and(|c| c.is_ascii_alphabetic()) { i += 1; continue; }
        let num: f32 = match std::str::from_utf8(&b[s..i]).ok()?.parse() { Ok(v) => v, Err(_) => { i += 1; continue } };
        // multiplier form: `8x7b`
        if s > 0 && b[s - 1] == b'x' {
            let mut m = s - 1;
            let mut ms = m;
            while ms > 0 && b[ms - 1].is_ascii_digit() { ms -= 1; }
            if ms < m {
                if let Ok(mult) = std::str::from_utf8(&b[ms..m]).unwrap_or("").parse::<f32>() {
                    return Some(mult * num);
                }
            }
            let _ = &mut m;
        }
        return Some(num);
    }
    None
}

/// Quantisation tag from a filename, e.g. `Q4_K_M`, `Q8_0`, `F16`.
pub fn parse_quant(name: &str) -> Option<String> {
    let up = name.to_uppercase();
    for (i, _) in up.match_indices('Q') {
        let rest = &up[i..];
        // Q + digit, then trailing tag characters
        if rest.len() >= 2 && rest.as_bytes()[1].is_ascii_digit() {
            let tag: String = rest.chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            // reject bare "Q4" inside e.g. "SEQ40" by requiring a
            // separator before the Q
            let ok_before = i == 0 || !up.as_bytes()[i - 1].is_ascii_alphanumeric();
            if ok_before && tag.len() >= 2 { return Some(tag); }
        }
    }
    for tag in ["F16", "BF16", "F32"] {
        if up.contains(tag) { return Some(tag.to_string()); }
    }
    None
}

/// Walk `dir` for `.gguf` files. Bounded depth, sorted, deduped by
/// path -- the same contract as `llm_backend::list_local_gguf`.
pub fn list_local_gguf(dir: &Path) -> Vec<LocalModel> {
    let mut out = Vec::new();
    walk(dir, 0, &mut out);
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.path.cmp(&b.path)));
    out.dedup_by(|a, b| a.path == b.path);
    out
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<LocalModel>) {
    if depth > 6 { return; }           // symlink cycles / pathological trees
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() { walk(&p, depth + 1, out); continue; }
        if p.extension().and_then(|x| x.to_str()).map(|x| x.eq_ignore_ascii_case("gguf")) != Some(true) {
            continue;
        }
        // skip multi-part shards after the first: loading one loads all
        let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if let Some(pos) = stem.to_lowercase().find("-of-") {
            let before = &stem[..pos];
            if before.rsplit('-').next().is_some_and(|n| n.chars().all(|c| c.is_ascii_digit()))
                && !before.ends_with("00001") && !before.ends_with('1') {
                continue;
            }
        }
        let size = e.metadata().map(|m| m.len()).unwrap_or(0);
        out.push(LocalModel {
            params_billions: estimate_params_billions(&p.to_string_lossy()).unwrap_or(0.0),
            quant: parse_quant(&stem),
            name: stem,
            path: p,
            size_bytes: size,
        });
    }
}

/// Where a model is in its lifecycle, for chat feedback.
#[derive(Debug, Clone, PartialEq)]
pub enum LoadState {
    None,
    Loading { name: String },
    Loaded { name: String, took: Duration, ctx: u32 },
    Failed { name: String, why: String },
}

impl LoadState {
    /// The line to print in chat. This is the whole point of the state
    /// machine: a load that says nothing looks like a hang.
    pub fn message(&self) -> String {
        match self {
            LoadState::None => "no model loaded -- /model to list, /model <name> to load".into(),
            LoadState::Loading { name } => format!("loading {name}..."),
            LoadState::Loaded { name, took, ctx } =>
                format!("model loaded: {name}  ctx {ctx}  in {:.1}s", took.as_secs_f32()),
            LoadState::Failed { name, why } => format!("failed to load {name}: {why}"),
        }
    }
    pub fn is_ready(&self) -> bool { matches!(self, LoadState::Loaded { .. }) }
}

/// The model list plus what is loaded.
#[derive(Debug, Default)]
pub struct ModelRegistry {
    pub dir: PathBuf,
    pub models: Vec<LocalModel>,
    pub state: LoadState,
}

impl Default for LoadState { fn default() -> Self { LoadState::None } }

impl ModelRegistry {
    pub fn scan(dir: PathBuf) -> Self {
        let models = list_local_gguf(&dir);
        Self { dir, models, state: LoadState::None }
    }

    /// Rescan; returns the summary line for chat.
    pub fn rescan(&mut self) -> String {
        self.models = list_local_gguf(&self.dir);
        format!("{} .gguf in {}", self.models.len(), self.dir.display())
    }

    /// Autocomplete for `/model <partial>`. Prefix matches first, then
    /// substring, so typing `7b` finds `qwen2.5-7b-instruct` -- people
    /// remember the size, not the vendor prefix.
    pub fn complete(&self, partial: &str) -> Vec<&LocalModel> {
        let p = partial.trim().to_lowercase();
        if p.is_empty() { return self.models.iter().collect(); }
        let mut pre: Vec<&LocalModel> = Vec::new();
        let mut sub: Vec<&LocalModel> = Vec::new();
        for m in &self.models {
            let n = m.name.to_lowercase();
            if n.starts_with(&p) { pre.push(m); } else if n.contains(&p) { sub.push(m); }
        }
        pre.extend(sub);
        pre
    }

    /// Resolve a name to exactly one model. Ambiguity is an error with
    /// the candidates listed -- silently picking the first is how you
    /// load a 40 GB model by accident.
    pub fn resolve(&self, name: &str) -> Result<&LocalModel, String> {
        let c = self.complete(name);
        match c.len() {
            0 => Err(format!("no model matching '{name}' in {}", self.dir.display())),
            1 => Ok(c[0]),
            _ => {
                if let Some(exact) = c.iter().find(|m| m.name.eq_ignore_ascii_case(name)) {
                    return Ok(exact);
                }
                let names: Vec<&str> = c.iter().take(6).map(|m| m.name.as_str()).collect();
                Err(format!("'{name}' matches {} models: {}", c.len(), names.join(", ")))
            }
        }
    }

    /// The listing shown by a bare `/model`.
    pub fn listing(&self) -> String {
        if self.models.is_empty() {
            return format!("no .gguf files in {} -- put one there or pass a path",
                           self.dir.display());
        }
        let mut s = format!("{} models in {}:\n", self.models.len(), self.dir.display());
        for m in &self.models {
            let mark = match &self.state {
                LoadState::Loaded { name, .. } if *name == m.name => "* ",
                _ => "  ",
            };
            s.push_str(&format!("{mark}{}\n", m.describe()));
        }
        s
    }

    pub fn begin_load(&mut self, name: &str) -> Result<&LocalModel, String> {
        let m = self.resolve(name)?;
        let picked = m.name.clone();
        let path = m.path.clone();
        self.state = LoadState::Loading { name: picked.clone() };
        // return by re-resolving so the borrow ends before the mutation
        self.models.iter().find(|x| x.path == path)
            .ok_or_else(|| format!("model '{picked}' vanished during load"))
    }

    pub fn finish_load(&mut self, took: Duration, ctx: u32) -> String {
        if let LoadState::Loading { name } = self.state.clone() {
            self.state = LoadState::Loaded { name, took, ctx };
        }
        self.state.message()
    }

    pub fn fail_load(&mut self, why: impl Into<String>) -> String {
        if let LoadState::Loading { name } = self.state.clone() {
            self.state = LoadState::Failed { name, why: why.into() };
        }
        self.state.message()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("vox-models-{}-{:?}",
            std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos()));
        std::fs::create_dir_all(d.join("sub")).unwrap();
        for (name, bytes) in [
            ("qwen2.5-7b-instruct-q4_k_m.gguf", 4_400_000_000u64),
            ("llama-3.2-3b-instruct-q8_0.gguf", 3_400_000_000),
            ("mixtral-8x7b-instruct-q4_0.gguf", 26_000_000_000),
            ("notes.txt", 10),
        ] {
            let p = d.join(name);
            std::fs::write(&p, vec![0u8; 16]).unwrap();
            let _ = bytes;
        }
        std::fs::write(d.join("sub").join("phi-3-mini-f16.gguf"), vec![0u8; 16]).unwrap();
        d
    }

    #[test]
    fn params_are_parsed_including_the_mixture_form() {
        assert_eq!(estimate_params_billions("qwen2.5-7b-instruct.gguf"), Some(7.0));
        assert_eq!(estimate_params_billions("mixtral-8x7b.gguf"), Some(56.0));
        assert_eq!(estimate_params_billions("llama-3.2-3b.gguf"), Some(3.0));
        assert_eq!(estimate_params_billions("bert-base.gguf"), None, "'base' is not a size");
        assert_eq!(estimate_params_billions("model.gguf"), None);
    }

    #[test]
    fn quant_tags_are_parsed_and_false_positives_rejected() {
        assert_eq!(parse_quant("qwen-7b-q4_k_m"), Some("Q4_K_M".into()));
        assert_eq!(parse_quant("llama-q8_0"), Some("Q8_0".into()));
        assert_eq!(parse_quant("phi-3-mini-f16"), Some("F16".into()));
        assert_eq!(parse_quant("seq40-model"), None, "Q inside a word is not a quant");
        assert_eq!(parse_quant("plain-model"), None);
    }

    #[test]
    fn scan_finds_gguf_recursively_and_ignores_other_files() {
        let d = tmp();
        let r = ModelRegistry::scan(d.clone());
        let names: Vec<&str> = r.models.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(r.models.len(), 4, "{names:?}");
        assert!(names.contains(&"phi-3-mini-f16"), "subdirectory walked: {names:?}");
        assert!(!names.iter().any(|n| n.contains("notes")));
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn autocomplete_prefers_prefix_then_substring() {
        let d = tmp();
        let r = ModelRegistry::scan(d.clone());
        let c = r.complete("qwen");
        assert_eq!(c[0].name, "qwen2.5-7b-instruct-q4_k_m");
        // people type the size, not the vendor
        let by_size = r.complete("7b");
        assert!(by_size.len() >= 2, "substring match finds both 7b models");
        assert!(r.complete("").len() == 4, "empty lists everything");
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn ambiguous_names_are_an_error_listing_candidates() {
        let d = tmp();
        let r = ModelRegistry::scan(d.clone());
        let e = r.resolve("7b").unwrap_err();
        assert!(e.contains("matches 2 models"), "{e}");
        assert!(e.contains("qwen"), "{e}");
        assert!(r.resolve("qwen").is_ok(), "unambiguous prefix resolves");
        assert!(r.resolve("nope").unwrap_err().contains("no model matching"));
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn exact_name_wins_over_ambiguity() {
        let d = tmp();
        std::fs::write(d.join("qwen.gguf"), vec![0u8; 16]).unwrap();
        let r = ModelRegistry::scan(d.clone());
        // "qwen" now prefix-matches two, but one is exactly it
        assert_eq!(r.resolve("qwen").unwrap().name, "qwen");
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn load_states_produce_the_chat_lines() {
        let d = tmp();
        let mut r = ModelRegistry::scan(d.clone());
        assert!(r.state.message().contains("no model loaded"));
        r.begin_load("qwen").unwrap();
        assert!(r.state.message().starts_with("loading qwen"));
        assert!(!r.state.is_ready());
        let msg = r.finish_load(Duration::from_millis(3200), 16384);
        assert!(msg.starts_with("model loaded: qwen2.5-7b-instruct-q4_k_m"), "{msg}");
        assert!(msg.contains("ctx 16384") && msg.contains("3.2s"), "{msg}");
        assert!(r.state.is_ready());
        // the listing marks the loaded one
        assert!(r.listing().lines().any(|l| l.starts_with("* ")));
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn a_failed_load_says_why() {
        let d = tmp();
        let mut r = ModelRegistry::scan(d.clone());
        r.begin_load("llama").unwrap();
        let msg = r.fail_load("server exited: out of memory");
        assert!(msg.contains("failed to load llama"), "{msg}");
        assert!(msg.contains("out of memory"), "{msg}");
        assert!(!r.state.is_ready());
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn empty_dir_says_what_to_do() {
        let d = std::env::temp_dir().join(format!("vox-empty-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let r = ModelRegistry::scan(d.clone());
        assert!(r.listing().contains("no .gguf files"));
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn default_dir_is_home_models() {
        let d = default_models_dir();
        assert!(d.ends_with("models"), "{d:?}");
    }
}
