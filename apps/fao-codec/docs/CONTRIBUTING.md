# Contributing to FAO

Conventions for editing this code, especially for future-claude picking 
up work across sessions.

## Build

```bash
cd workspace
cargo r --release --bin fao-app --features=gpu
```

With library:
```bash
FAO_LIBRARY=~/fao-lib FAO_LIBRARY_RW=1 cargo r --release --bin fao-app
```

CLI:
```bash
cargo r --release --bin fao-cli -- library init ~/fao-lib
cargo r --release --bin fao-cli -- library list ~/fao-lib
cargo r --release --bin fao-cli -- library info ~/fao-lib <uuid>
cargo r --release --bin fao-cli -- library verify ~/fao-lib
```

Target macOS is Apple Silicon (M-series); target Linux is x86_64 with 
optional AVX2/SSE/NEON SIMD paths.

### Reference test systems

The development reference systems are big-memory: 64-192GB Linux dev 
boxes and a 64GB M4 Max. The codec runs fine on smaller machines, but 
performance tuning decisions are made with the big-memory regime in 
mind. If you're testing on a 16GB system, expect more `trimming UI 
caches` log lines and earlier ChunkStore eviction — that's by design.

If you're building a feature that should scale up with memory (e.g., 
larger feature-track windows, more canonical webs in cache), feel free 
to add a soft tuning knob via env var rather than hard-coding a 
constant — `FAO_MEMORY_PROFILE` or similar.

## Code style

- **Rust 2021 edition**, idiomatic but not over-engineered. Prefer 
  clarity over cleverness in the hot paths.
- **No clippy-driven refactors** without checking the asm — some 
  patterns clippy flags are deliberate for performance.
- **Single-file modules are fine.** v8_reconstruction_codec.rs is 
  6700 lines because splitting it across files would obscure the 
  pipeline. main.rs is 10k lines for the same reason (GTK4 callback 
  closures are hard to split cleanly).
- **Comments explain WHY, not WHAT.** Especially for the hot loops, 
  document the math and the constants.

## Hot path discipline

In encoder hot loops (per-frame, per-pixel, per-feature):
- No allocations
- No HashMap lookups (use Vec + index)
- Mutex<T> sharded per-worker, not global
- Pre-compute invariants outside iteration loops (see LK hoisting in 
  v8_reconstruction_codec.rs for the canonical example)

In decoder hot loops:
- Same as above
- Use incremental evaluation where the function is linear in screen 
  coords (see rasterize_triangle for the edge-walk pattern)

## Format versioning

Bitstream changes should:
- Add a new variant magic ('Z' inner version byte, or new V8XX trailer)
- Old decoders see the new variant and either skip it or fall back to 
  source-fetch
- New decoders read all variants

NEVER reuse existing magic bytes for new formats. Decoders need to 
distinguish them unambiguously.

## Testing

- Unit tests live in `#[cfg(test)] mod tests` at the bottom of each 
  module. Run with `cargo test`.
- Integration tests use small fixture .fao files. Not automated; run 
  by hand for now.
- Performance regressions: visual inspection of stats during real 
  encodes on representative footage (game capture for high-motion, 
  movie clips for low-motion).

## Documentation

- Bigger design docs live in `docs/` (this folder).
- Inline rustdoc for public APIs is encouraged but not yet uniform.
- Session-level work logs live in `docs/SESSION_LOG.md` — append at 
  the end after meaningful work.

## Working with claude on this code

Conventions that work well across sessions:

1. **Brace-balance check before packaging.** Run the brace counter 
   over all .rs files. The counter (in transcripts) handles raw 
   strings, char literals, line and block comments correctly.
2. **Anchor-based edits.** Use Python str.replace with multi-line 
   anchors that include surrounding context. Avoid line-number 
   targeting (line numbers drift across edits).
3. **Verify before assuming.** Compaction can lose the exact state 
   of in-flight edits; first thing after compact is usually 
   re-reading the file to confirm.
4. **Pack & present at session end.** zip the workspace into 
   /mnt/user-data/outputs/fao_codec_fixed.zip and present_files.
5. **Update SESSION_LOG.md** before packaging so future-claude has 
   context.

## Git not in use

The workspace is delivered as a zip rather than a git repo. Each 
session produces a fresh zip. Diff between zips manually if needed.

## Honest assessment habit

Avoid puffing up changes. Patterns:
- "X is shipped" only if X is actually in the workspace, not just 
  designed
- "10x faster" only if the math supports it on realistic loads, not 
  best-case microbench
- "Soon" is forbidden; estimate sessions instead
