// whisper-rs-sys (whisper.cpp) and llama-cpp-sys-2 (llama.cpp) each statically vendor their OWN copy of
// ggml.c. Linking both — i.e. building `voice` together with `llamacpp` — defines every ggml_* symbol
// twice, and GNU ld aborts with "multiple definition of `ggml_fp16_to_fp32_row'" etc.
//
// The standard whisper.cpp + llama.cpp coexistence fix is to let the linker keep the FIRST definition and
// permit the duplicates. We only emit the flag when the `voice` feature is on (the whisper side that adds
// the second ggml); a llama.cpp-only or voice-only build has a single ggml and is unaffected, so this never
// weakens the normal build. Works with GNU ld and mold (both accept --allow-multiple-definition).
fn main() {
    if std::env::var_os("CARGO_FEATURE_VOICE").is_some() {
        println!("cargo:rustc-link-arg=-Wl,--allow-multiple-definition");
    }
}
