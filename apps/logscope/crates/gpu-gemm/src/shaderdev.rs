//! shaderdev.rs — swap a shader without rebuilding, because only the string changed.
//!
//! Ten gate cycles went into one Q4_K bug and every one paid a full `cargo build --release` to alter
//! a WGSL literal. **The Rust never changed.** Shaders are text, compiled by the driver at pipeline
//! creation, so a run can read them from disk and skip the rebuild entirely.
//!
//! This is the "quick cycle interpreter" idea applied where it actually works: not to Rust, where
//! MIR interpretation is 100x slower than a debug build, but to the part of this codebase that is
//! *already* interpreted at runtime by the GPU driver.
//!
//! ## What it does NOT change
//!
//! The dispatch, the bindings, the dims struct. An override that alters the struct layout will fail
//! at pipeline creation with a naga error, which is the same feedback a rebuild would give and
//! arrives in a second instead of ninety.
//!
//! ## Why it is opt-in and loud
//!
//! A stale file silently overriding a fixed shader is exactly the class of bug this codebase has
//! spent a week on — a cached verdict outliving its code. So it is off unless
//! `LOGSCOPE_SHADER_DIR` is set, and it announces every override it applies.

/// Look for `{LOGSCOPE_SHADER_DIR}/{name}.wgsl` and return its contents if present.
///
/// `name` is the constant's own identifier — `Q4K_TO_Q4`, `MATVEC_Q4` — so the mapping from file to
/// shader needs no table to drift out of date.
pub fn override_for(name: &str) -> Option<String> {
    let dir = std::env::var("LOGSCOPE_SHADER_DIR").ok()?;
    let path = std::path::Path::new(&dir).join(format!("{name}.wgsl"));
    let text = std::fs::read_to_string(&path).ok()?;
    // ANNOUNCED, every time. A silent override is a cached verdict wearing a different hat, and this
    // codebase has already lost days to one of those.
    eprintln!("wgpu-engine: SHADER OVERRIDE — {name} loaded from {} ({} bytes). The built-in version \
is NOT in use; any result from this run describes the file, not the tree.",
        path.display(), text.len());
    Some(text)
}

/// Resolve a shader: the override if one exists, else the compiled-in source.
pub fn resolve<'a>(name: &str, builtin: &'a str) -> std::borrow::Cow<'a, str> {
    match override_for(name) {
        Some(s) => std::borrow::Cow::Owned(s),
        None => std::borrow::Cow::Borrowed(builtin),
    }
}

/// Write every built-in shader to a directory, as a starting point for editing.
///
/// Without this a user has to find the literal in an 18000-line file and copy it out by hand, which
/// is enough friction that the feature would go unused.
pub fn dump(dir: &std::path::Path, shaders: &[(&str, &str)]) -> std::io::Result<usize> {
    std::fs::create_dir_all(dir)?;
    for (name, src) in shaders {
        std::fs::write(dir.join(format!("{name}.wgsl")), src)?;
    }
    Ok(shaders.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_the_env_var_the_BUILT_IN_is_used() {
        // Off by default. A hot-reload that engaged silently would let a stale file describe a tree
        // it does not match.
        std::env::remove_var("LOGSCOPE_SHADER_DIR");
        assert_eq!(resolve("ANY", "builtin source"), "builtin source");
    }

    #[test]
    fn a_missing_file_falls_back_rather_than_failing() {
        // Overriding one shader must not require dumping all forty-seven.
        let d = std::env::temp_dir().join(format!("sd-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::env::set_var("LOGSCOPE_SHADER_DIR", &d);
        assert_eq!(resolve("NOT_PRESENT", "builtin"), "builtin");
        std::env::remove_var("LOGSCOPE_SHADER_DIR");
    }

    #[test]
    fn an_override_REPLACES_the_builtin() {
        let d = std::env::temp_dir().join(format!("sd2-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("PROBE.wgsl"), "// edited").unwrap();
        std::env::set_var("LOGSCOPE_SHADER_DIR", &d);
        assert_eq!(resolve("PROBE", "builtin"), "// edited");
        std::env::remove_var("LOGSCOPE_SHADER_DIR");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn DUMP_writes_a_file_per_shader_named_after_the_constant() {
        // The name IS the mapping, so no table can drift out of date — and finding a WGSL literal in
        // an 18000-line file by hand is enough friction to make the feature go unused.
        let d = std::env::temp_dir().join(format!("sd3-{}", std::process::id()));
        let n = dump(&d, &[("A", "src a"), ("B", "src b")]).unwrap();
        assert_eq!(n, 2);
        assert_eq!(std::fs::read_to_string(d.join("A.wgsl")).unwrap(), "src a");
        std::fs::remove_dir_all(&d).ok();
    }
}
