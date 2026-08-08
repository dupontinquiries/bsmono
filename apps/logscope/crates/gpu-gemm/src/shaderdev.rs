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

#[cfg(test)]
mod moe_iq1s_shape_tests {
    use crate::{MATVEC_IQ1_S, MOE_MATVEC_IQ1_S};

    /// `MOE_MATVEC_IQ1_S` was produced by editing the dense kernel's source, and string surgery on a
    /// shader compiles whether or not it is correct — a wrong binding index or a missed `a[` produces
    /// valid WGSL that reads the wrong memory. These check the SHAPE before any hardware sees it.
    #[test]
    fn it_takes_SIX_bindings_where_the_pair_kinds_take_five() {
        // The reason LinKind::Iq1 was unreachable from the MoE arm: one interleaved block buffer plus
        // a codebook grid, instead of a (quant, scale) pair.
        assert_eq!(MOE_MATVEC_IQ1_S.matches("@group(0) @binding").count(), 6);
        assert_eq!(MATVEC_IQ1_S.matches("@group(0) @binding").count(), 5);
    }

    #[test]
    fn the_expert_index_comes_from_selw_FIRST_half() {
        // The same buffer MOE_TOPK fills and MOE_MATVEC_GLU_Q4 reads: indices first, weights second.
        // Reading the second half here would silently index by a bitcast float.
        assert!(MOE_MATVEC_IQ1_S.contains("let expert = selw[slot]"));
    }

    #[test]
    fn EVERY_activation_read_goes_through_abase() {
        // A single missed `a[` would read slot 0's activations for every expert — correct-looking
        // output, wrong for all but the first slot.
        let b = MOE_MATVEC_IQ1_S;
        assert_eq!(b.matches("a[").count(), b.matches("a[abase + ").count(),
            "an activation read escaped the offset");
    }

    #[test]
    fn output_is_PER_SLOT_not_per_row() {
        // Writing c[row] would have every expert overwrite the last one's result.
        assert!(MOE_MATVEC_IQ1_S.contains("c[slot * d.n + row]"));
        assert!(!MOE_MATVEC_IQ1_S.contains("c[row] ="));
    }

    #[test]
    fn the_BLOCK_DECODE_is_unchanged_from_the_validated_kernel() {
        // --gate-iq1s-kernel validates the arithmetic. If the decode drifted, that validation would
        // no longer cover this kernel and nothing would say so.
        for marker in ["fn wbyte(i: u32)", "let dsc = f16(whalf(blk))", "var delta = 0.125;"] {
            assert!(MOE_MATVEC_IQ1_S.contains(marker), "decode changed: {marker}");
            assert!(MATVEC_IQ1_S.contains(marker));
        }
    }

    #[test]
    fn the_slot_guard_exists() {
        // wg.z runs to the dispatch size, which may exceed the selected count.
        assert!(MOE_MATVEC_IQ1_S.contains("slot >= d.slots"));
    }
}
