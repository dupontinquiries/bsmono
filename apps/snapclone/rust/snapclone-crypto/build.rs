// build.rs — generates include/crypto/snapclone_crypto_ffi.h for C++ consumption.
//
// Config::from_root_or_default(crate_dir) reads cbindgen.toml from the crate
// root (which sets rename_variants = "QualifiedScreamingSnakeCase"), then we
// override the three fields we always want via with_language / with_include_guard
// / with_documentation on the Builder. No struct literals — no private-field errors.
//
// V32.33+: set SNAPCLONE_SKIP_CBINDGEN=1 to skip header generation. Useful when
// running `cargo test` in environments where re-running cargo metadata
// (which cbindgen does) trips over MSRV mismatches in transitive deps that
// don't affect the library code itself. The header is already committed
// to include/crypto/ so skipping just means "trust what's there".
fn main() {
    println!("cargo:rerun-if-env-changed=SNAPCLONE_SKIP_CBINDGEN");
    if std::env::var_os("SNAPCLONE_SKIP_CBINDGEN").is_some() {
        println!("cargo:warning=SNAPCLONE_SKIP_CBINDGEN set — skipping header regen");
        return;
    }

    let crate_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let out_dir   = std::path::PathBuf::from(&crate_dir)
        .parent().unwrap()   // rust/
        .parent().unwrap()   // project root
        .join("include/crypto");

    std::fs::create_dir_all(&out_dir).ok();

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
        .write_to_file(out_dir.join("snapclone_crypto_ffi.h"));

    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=cbindgen.toml");
}
