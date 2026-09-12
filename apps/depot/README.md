# depot

An offline mirror for a LAN without internet: cargo crates (sparse
registry), Fedora RPMs (dnf repodata), a Nix binary cache, and Rust
toolchains — served by one static binary from one directory, and moved
between the online and offline sides as signed, content-deduplicated packs.

- **Manual:** [MANUAL.md](MANUAL.md) — also served by the running depot at
  `/manual` and printed by `depot manual`.
- **Why it is built this way:** [DECISIONS.md](DECISIONS.md) — the Linux
  and ecosystem defaults each choice rests on (`/decisions`, `depot decisions`).

## Layout

```
crates/depot-httpd   std-only threaded HTTP/1.1 — the transport seam (hyper/TLS would slot in here)
crates/depot-core    headless: store, catalog, the four projections, signed packs
crates/depot         the binary: CLI + `depot serve` (routes in web.rs) + embedded UI (ui/)
systemd/             hardened unit: DynamicUser + StateDirectory=depot, port 8000, no capabilities
flake.nix            nix build (unverified here — no nix in the sandbox)
```

The projections are derived **without the upstream tools**: no `createrepo_c`,
no `cargo-index` helper, no `nix` on the depot. The RPM and crate output is
tested byte-for-byte / field-for-field against those tools' real output
(`crates/depot-core/testdata/README.md` records the provenance).

## Quick start

```sh
cargo build --release
target/release/depot init                       # ~/.local/share/depot (or /var/lib/depot as root)
target/release/depot ingest crate ~/.cargo/registry/cache/*/
target/release/depot ingest rpm --repo fedora-42-x86_64 ./rpms
target/release/depot serve                      # http://<host>:8000/
```

Then on a device: `curl -fsSL http://<host>:8000/bootstrap.sh?all=1 | sh`
(read it first — it prints every file before writing it).

## Verification (2026-09-12)

Built and tested in a sandbox with the offline Rust 1.96 drop, `rpm`,
`rpmbuild`, `createrepo_c` and Ubuntu's `dnf` 4.14 from apt.

- `cargo test`: depot-httpd 8/8, depot-core 33/33, depot 2/2; no warnings.
- **RPM repodata** is byte-identical to `createrepo_c`'s for four packages
  (rpm upstream's `hello`/`foo` fixtures and an edge-case RPM built here).
  Break-testing three of the replicated filter rules fails the tests.
- **Crate index lines** are field-equal to index.crates.io's for two crates.
- **Real cargo** (clean `CARGO_HOME`, source replacement): resolves, locks
  with the crates-io source id unchanged, fetches, builds, runs.
- **Real dnf** (`--installroot`): makecache, repoquery, `provides` by path,
  `install` from the depot.
- curl: ETag/304, Range/206, HEAD, traversal refused, writes refused without
  the admin token; UI and manual served.
- `systemd-analyze verify` passes; offline security exposure 1.2.

Not verified: nix client, toolchain `install.sh` against a real tarball,
`flake.nix`, cross builds (musl/aarch64), the UI in a browser (JS not
executed here), graceful SIGTERM drain. See [TODO.md](TODO.md).
