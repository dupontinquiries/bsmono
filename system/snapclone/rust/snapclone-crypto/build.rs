// build.rs — validates that the snapclone-crypto FFI surface is expressible in
// C, and (only on request) regenerates include/crypto/snapclone_crypto_ffi.h.
//
// WHY THE COMMITTED HEADER IS NOT THE GENERATED ONE
//
// The header in include/crypto/ is hand-maintained and DIVERGES from cbindgen's
// output on purpose. cbindgen.toml sets rename_variants =
// "QualifiedScreamingSnakeCase", so generation emits CRYPTO_ERROR_OK; the
// committed file instead declares CryptoError_Ok-style members plus a block of
// canonical CryptoError_* alias macros, with a comment explaining that
// cbindgen's variant naming differs across versions and configurations. The C++
// side includes those aliases.
//
// This build script used to write generation output straight into the source
// tree on every build. That silently destroyed the alias block and renamed
// every variant — a working-tree modification to a committed file, appearing
// with no warning, as a side effect of `cargo test`. Anyone who did not think
// to run `git status` afterwards committed the regression or discarded it by
// accident. The c8 parallel session hit it and worked around it by hand rather
// than fixing it ("left that committed header untouched rather than risk a
// mismatched regeneration"), which is the tell that the default was wrong: the
// escape hatch existed and the trap was still the default path.
//
// So the default is now OUT_DIR. cbindgen still runs, so the check it is
// actually valuable for — "can this Rust API be expressed as a C header at
// all" — still fires on every build and still fails the build when it cannot.
// What no longer happens is the write into the source tree.
//
// To deliberately regenerate the committed header (and then reconcile the
// alias block by hand, which is the part cbindgen cannot do for you):
//
//     SNAPCLONE_WRITE_FFI_HEADER=1 cargo build -p snapclone-crypto
//
// SNAPCLONE_SKIP_CBINDGEN=1 still skips generation entirely. That is for
// environments where re-running `cargo metadata` (which cbindgen does) trips
// over MSRV mismatches in transitive deps that do not affect the library code.
fn main() {
    println!("cargo:rerun-if-env-changed=SNAPCLONE_SKIP_CBINDGEN");
    println!("cargo:rerun-if-env-changed=SNAPCLONE_WRITE_FFI_HEADER");
    if std::env::var_os("SNAPCLONE_SKIP_CBINDGEN").is_some() {
        println!("cargo:warning=SNAPCLONE_SKIP_CBINDGEN set — skipping header generation");
        return;
    }

    let crate_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let write_committed = std::env::var_os("SNAPCLONE_WRITE_FFI_HEADER").is_some();

    let dest = if write_committed {
        let d = std::path::PathBuf::from(&crate_dir)
            .parent().unwrap()   // rust/
            .parent().unwrap()   // project root
            .join("include/crypto");
        std::fs::create_dir_all(&d).ok();
        println!("cargo:warning=SNAPCLONE_WRITE_FFI_HEADER set — overwriting the committed \
header. The hand-written CryptoError_* alias block will be lost; re-add it before committing.");
        d
    } else {
        // OUT_DIR: generation still validates the API, nothing in the source
        // tree is touched.
        std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR set by cargo"))
    };

    // Load cbindgen.toml (sets rename_variants = "QualifiedScreamingSnakeCase").
    // Falls back to defaults if the file is absent — cbindgen.toml is committed
    // to the repo so this should always succeed in normal builds.
    let config = cbindgen::Config::from_root_or_default(&crate_dir);

    cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(config)
        .with_language(cbindgen::Language::C)
        .with_include_guard("SNAPCLONE_CRYPTO_FFI_H")
        .with_documentation(true)
        .generate()
        .expect("cbindgen failed")
        .write_to_file(dest.join("snapclone_crypto_ffi.h"));

    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=cbindgen.toml");
}
