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
- [x] ~~UI in a browser~~ — executed under jsdom against a live server
      (`tools/ui-test.js`, 18 checks: every view, yank-without-token alert,
      token save, verify button, Clients checkboxes → snippet + one-liner
      refresh, manual/decisions markdown). Not done: a real browser, the
      yank button WITH a token (needs a writable depot in the harness).
- [x] ~~Graceful SIGTERM drain~~ — `signal(2)` via libc sets the flag;
      verified: TERM mid-download, the download completed, "depot: stopped"
      logged. Remaining: an idle keep-alive connection delays exit by the
      keep-alive timeout (worker blocked in read); make handle_connection
      check the flag between requests if that ever matters.
- [x] ~~Lock held while hashing uploads~~ — uploads now stream straight
      into the store (hashed once, no lock), then `ingest_*_stored(hash)`
      runs under the lock: header read for RPMs, Cargo.toml scan for crates,
      a stat for toolchains. `put_file` short-circuits when the expected
      object exists. A rejected fresh upload is removed. Verified over HTTP
      with a 10 MB tarball; the crate/rpm catalog entries are asserted equal
      to the file-ingest path in `depot::tests`.
- [ ] `comps.xml` / `updateinfo.xml` for dnf groups and advisories
      (Anaconda environment picker needs comps).
- [ ] rustup dist-server layout (channel manifest + dated paths) so
      `RUSTUP_DIST_SERVER` works unchanged — MANUAL §10.
- [ ] apk repositories for postmarketOS/Alpine (fifth projection:
      `APKINDEX.tar.gz` from `.apk` control segments).
- [ ] HTTPS listener behind `depot_httpd::Handler` if a fleet wants it.
- [ ] Terminal: a websocket/SSE transport for lower latency (the `Terminals`
      API would not change); a fuller VT parser if editing on-device is
      wanted (voiceterm-egui's `term_grid` is the reference). Today it is a
      short-poll glovebox, deliberately.
- [ ] Terminal: not tested on a phone (touch keycaps, on-screen keyboard
      overlap) or as the unprivileged systemd user — only x86_64 /bin/sh here.
