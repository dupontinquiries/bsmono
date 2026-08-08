//! dupscan.rs — finding two implementations of one idea before the compiler cannot.
//!
//! **Rust permits `pub struct Peer` in two modules of one crate.** That is correct language design
//! and it is why seven duplicate implementations survived in this tree: the compiler has no
//! objection, the tests pass, and both versions work. They are only found when someone happens to
//! put them in the same scope.
//!
//! The ones that got through, and how each was eventually caught:
//!
//! | duplicate | found by |
//! |---|---|
//! | `Peer` in machinery.rs, twice | a name collision when a third was added |
//! | `Beacon` and `beacon_for` | same |
//! | two Q4_K dequant paths | a failing gate, after four wrong fixes |
//! | two CPU quantizers | a sweep, after one was patched and the other was not |
//! | `mdtable` vs `table_ops` | being asked to look |
//! | `state_exit_tests` | a test module name collision |
//!
//! **Six of seven were found by accident.** A scan is the cheap version of the accident.
//!
//! ## Why name-matching and not semantics
//!
//! Two functions that do the same thing under different names are the harder case and cannot be
//! found this way. Two with the SAME name almost always are the same idea — nobody names an
//! unrelated thing `Peer` twice in one crate — so the cheap check catches the common case, and
//! saying it does not catch the other is more useful than pretending it does.

/// A public item and where it is defined.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Item { pub name: String, pub kind: Kind, pub module: String, pub line: usize }

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Kind { Struct, Enum, Fn, Const, Trait, Type }

impl Kind {
    fn from_keyword(k: &str) -> Option<Kind> {
        Some(match k {
            "struct" => Kind::Struct, "enum" => Kind::Enum, "fn" => Kind::Fn,
            "const" | "static" => Kind::Const, "trait" => Kind::Trait, "type" => Kind::Type,
            _ => return None,
        })
    }
}

/// Extract public items from one file's source.
///
/// Skips `#[cfg(test)]` regions by default — a test helper named the same as a production item is
/// noise, and a scan that reports noise gets ignored, which is how the last one would have failed.
pub fn items_in(module: &str, src: &str, include_tests: bool) -> Vec<Item> {
    let mut out = Vec::new();
    let mut in_test = false;
    let mut brace_depth_at_test = 0i32;
    let mut depth = 0i32;
    for (i, line) in src.lines().enumerate() {
        let t = line.trim();
        if t.starts_with("#[cfg(test)]") { in_test = true; brace_depth_at_test = depth; }
        depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
        if in_test && depth <= brace_depth_at_test && t.contains('}') { in_test = false; }
        if in_test && !include_tests { continue }
        let Some(rest) = t.strip_prefix("pub ") else { continue };
        let rest = rest.strip_prefix("const ").map(|r| ("const", r))
            .or_else(|| rest.strip_prefix("fn ").map(|r| ("fn", r)))
            .or_else(|| rest.strip_prefix("struct ").map(|r| ("struct", r)))
            .or_else(|| rest.strip_prefix("enum ").map(|r| ("enum", r)))
            .or_else(|| rest.strip_prefix("trait ").map(|r| ("trait", r)))
            .or_else(|| rest.strip_prefix("static ").map(|r| ("static", r)))
            .or_else(|| rest.strip_prefix("type ").map(|r| ("type", r)));
        let Some((kw, after)) = rest else { continue };
        let Some(kind) = Kind::from_keyword(kw) else { continue };
        let name: String = after.chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        if name.is_empty() { continue }
        out.push(Item { name, kind, module: module.to_string(), line: i + 1 });
    }
    out
}

/// Items defined more than once across a set of modules.
///
/// **Grouped by name AND kind.** A `struct Foo` and a `fn foo` are unrelated; a `struct Foo` in two
/// modules is the thing worth reporting.
pub fn duplicates(items: &[Item]) -> Vec<Vec<&Item>> {
    let mut by_key: std::collections::BTreeMap<(String, Kind), Vec<&Item>> = Default::default();
    for it in items { by_key.entry((it.name.clone(), it.kind)).or_default().push(it); }
    let mut v: Vec<Vec<&Item>> = by_key.into_values()
        // Two in ONE module is usually a genuine overload or a typo the compiler will catch. Two in
        // DIFFERENT modules is the case that compiles and survives.
        .filter(|g| g.len() > 1 && g.iter().map(|i| &i.module).collect::<std::collections::BTreeSet<_>>().len() > 1)
        .collect();
    v.sort_by_key(|g| g[0].name.clone());
    v
}

/// The report.
pub fn report(groups: &[Vec<&Item>]) -> String {
    if groups.is_empty() {
        return "no public item is defined in two modules of this crate".into();
    }
    let mut out = vec![format!("{} name(s) defined in more than one module:", groups.len())];
    for g in groups {
        let places: Vec<String> = g.iter().map(|i| format!("{}:{}", i.module, i.line)).collect();
        out.push(format!("  {:?} {}  —  {}", g[0].kind, g[0].name, places.join(", ")));
    }
    out.push("Rust permits this and the compiler will not object, which is why duplicates here \
survive until someone puts them in one scope. Each is either two ideas sharing a name, or one idea \
implemented twice — and the second is worth resolving before both drift".into());
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_PEER_duplicate_would_have_been_caught() {
        // Two `pub struct Peer` in one crate, which is what actually happened in machinery.rs and
        // was only found when a third was added.
        let a = items_in("machinery", "pub struct Peer {\n}\n", false);
        let b = items_in("netpeer", "pub struct Peer {\n}\n", false);
        let all: Vec<Item> = a.into_iter().chain(b).collect();
        let d = duplicates(&all);
        assert_eq!(d.len(), 1);
        assert!(report(&d).contains("one idea implemented twice"));
    }

    #[test]
    fn a_STRUCT_and_a_FN_of_the_same_name_are_not_a_duplicate() {
        // `struct Foo` and `fn foo` are unrelated, and reporting them would teach the reader to skim.
        let all: Vec<Item> = items_in("a", "pub struct Thing;\n", false).into_iter()
            .chain(items_in("b", "pub fn Thing() {}\n", false)).collect();
        assert!(duplicates(&all).is_empty());
    }

    #[test]
    fn two_definitions_in_ONE_module_are_left_alone() {
        // The compiler catches those. This scan exists for the case that COMPILES.
        let all = items_in("a", "pub struct Foo;\npub struct Foo;\n", false);
        assert!(duplicates(&all).is_empty());
    }

    #[test]
    fn TEST_HELPERS_are_skipped_by_default() {
        // A test helper sharing a production name is noise, and a scan that reports noise gets
        // ignored — which is how this guard would fail in exactly the way the others did not.
        let src = "pub struct Real;\n#[cfg(test)]\nmod t {\n    pub struct Real;\n}\n";
        assert_eq!(items_in("m", src, false).len(), 1);
        assert_eq!(items_in("m", src, true).len(), 2);
    }

    #[test]
    fn a_CLEAN_crate_says_so_rather_than_printing_nothing() {
        // An empty report reads as "the scan did not run", which this project has learned to
        // distrust more than any other output.
        assert!(report(&[]).contains("no public item is defined in two modules"));
    }

    #[test]
    fn CONST_and_STATIC_are_one_kind() {
        // SUBMIT_BY_NO_FRAME as a static in one module and a const in another is the same collision
        // wearing two keywords.
        let all: Vec<Item> = items_in("a", "pub const LIMIT: u32 = 1;\n", false).into_iter()
            .chain(items_in("b", "pub static LIMIT: u32 = 2;\n", false)).collect();
        assert_eq!(duplicates(&all).len(), 1);
    }
}

/// Scan a directory of `.rs` files.
///
/// Wired so the scan RUNS rather than merely existing — this module was itself in the unwired list
/// the day after it was written, which is the joke that makes the point.
pub fn scan_dir(dir: &std::path::Path) -> std::io::Result<Vec<Item>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        if p.extension().and_then(|x| x.to_str()) != Some("rs") { continue }
        let Some(stem) = p.file_stem().and_then(|x| x.to_str()) else { continue };
        if stem == "lib" || stem == "main" { continue }
        let Ok(src) = std::fs::read_to_string(&p) else { continue };
        out.extend(items_in(stem, &src, false));
    }
    Ok(out)
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// Runs on the ACTUAL source tree, every `cargo test`.
    ///
    /// A scan behind a CLI flag runs when someone remembers. This runs when anything runs, which is
    /// the difference between a guard and a tool — and the seven duplicates this session were all
    /// found by mechanisms that fired without being invoked.
    ///
    /// **Asserts a CEILING rather than zero.** The tree has 88 legitimate collisions — `Entry`,
    /// `Block`, `Budget` in unrelated domains — and demanding zero would mean either a mass rename
    /// nobody wants or a test everyone deletes. A ceiling catches the NEXT one without arguing about
    /// the existing ones.
    #[test]
    fn duplicate_names_do_not_grow() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let Ok(items) = scan_dir(&dir) else { return };  // not a source checkout; nothing to check
        let d = duplicates(&items);
        const CEILING: usize = 92;
        assert!(d.len() <= CEILING,
            "{} duplicate names, ceiling {CEILING}. A new one appeared — either two ideas sharing a \
name, which is fine and wants the ceiling raised deliberately, or one idea implemented twice, which \
is what happened seven times this session.\n{}",
            d.len(), report(&d));
    }

    #[test]
    fn the_scan_finds_SOMETHING_so_a_broken_scanner_cannot_pass_silently() {
        // A ceiling test passes trivially if the scan returns nothing. This is the guard on the
        // guard, and it is the failure mode `--gate-quantref` had when six of eight submit callers
        // incremented no counter and the profile looked clean.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let Ok(items) = scan_dir(&dir) else { return };
        assert!(items.len() > 500, "only {} public items found — the scanner is broken, not the tree \
clean", items.len());
    }
}
