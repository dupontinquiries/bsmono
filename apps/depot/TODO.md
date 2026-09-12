# depot — TODO

Unverified or unbuilt, in the order they matter.

- [ ] **nix client end to end.** No nix in the sandbox. The layout follows
      `nix copy --to file://` and the narinfo/nix-cache-info formats; the
      base32 codec matches the known vector. Needs: `nix copy` a closure,
      `depot ingest nix`, then `nix build --substituters http://…/nix`.
- [x] ~~toolchain install.sh against real tarballs~~ — done 2026-09-12 with
      the 1.96.0 x86_64-gnu set: sha256 verified, installed, `rustc` works,
      tampered hash refused. Still untested: a musl host (the `ldd` detection
      branch) and `TRIPLE=` cross installs.
- [ ] **cross builds** for `aarch64-unknown-linux-musl` (phones) and
      `aarch64-unknown-linux-gnu` (Asahi). No C deps by default, so this
      should be `cargo build --target …` with the std for that triple; the
      optional `zstd` feature is the only thing that would need a cross CC.
- [ ] **flake.nix** — written from the `buildRustPackage` pattern, never
      evaluated.
- [ ] **UI in a browser.** Routes and JSON verified with curl; `node
      --check` passes; the JS has not been executed. Check: hash routing,
      filter boxes, yank button with a token, the Clients tab's checkbox →
      snippet refresh.
- [ ] **Graceful SIGTERM drain.** The accept loop polls a stop flag but
      nothing sets it (std has no signal handler). Safe today because every
      write is atomic; a `signal-hook`-free option is a `/proc/self`-free
      pipe trick via `sigaction` through libc — decide whether one libc
      call is worth it.
- [ ] **Lock held while hashing uploads.** `PUT /api/*` spools to disk
      without the lock but `ingest_*` re-hashes under it. For a 2 GB
      toolchain tarball that blocks API reads for seconds. Fix: hash in
      `spool`, pass the hash to `put_file` (it already accepts an expected
      hash) — then the lock hold is a rename.
- [ ] `comps.xml` / `updateinfo.xml` for dnf groups and advisories
      (Anaconda environment picker needs comps).
- [ ] rustup dist-server layout (channel manifest + dated paths) so
      `RUSTUP_DIST_SERVER` works unchanged — MANUAL §10.
- [ ] apk repositories for postmarketOS/Alpine (fifth projection:
      `APKINDEX.tar.gz` from `.apk` control segments).
- [ ] HTTPS listener behind `depot_httpd::Handler` if a fleet wants it.
