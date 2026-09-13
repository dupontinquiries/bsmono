# Fill the depot (the online side)

Gathering the crates, packages, toolchains and store paths your offline
machines will need, and loading them into a depot. This happens on a
machine that can reach the internet — the depot that is then packed and
carried over, or a laptop that uploads to the LAN depot directly.

## Decide what a "repo" is

depot keeps crates in one index (like crates.io), and RPMs in **repos**
you name. Name a repo by what it is for: distribution, release, arch —
`fedora-42-x86_64`, `fedora-42-aarch64`. Put `fedora` and `updates` in
the same repo: dnf takes the newest version across everything it sees.
Give a repo of its own to anything with its own signing key (a COPR, a
third-party repo) so `gpgcheck=1` keeps working per key.

```sh
depot status          # what is here now
```

You should see zero counts on a fresh depot. Nothing to create up front:
a repo exists the moment you ingest into it.

## Crates: what cargo already downloaded
%check: has_crates

Every `.crate` cargo has ever fetched on a machine sits in its registry
cache. Fetching for a project pulls its whole dependency graph, so the
cache of a machine that builds your projects *is* the mirror:

```sh
cd ~/src/myproject
cargo fetch                                              # host target
cargo fetch --target aarch64-unknown-linux-musl          # a phone's deps may differ
depot ingest crate ~/.cargo/registry/cache/*/
depot status | grep crates
```

You should see one line per crate (`crate serde@1.0.219 (…)`), with
`[already had it]` on repeats. Check one from the served index:

```sh
curl -s http://localhost:8000/crates/index/se/rd/serde | tail -c 200
```

## RPMs: the set an installed system needs
%check: has_rpm

For a full reload you want everything the target has installed, with
dependencies resolved, for the target's arch. On a box of the same Fedora
release (any arch — `--forcearch` resolves for another one):

```sh
# 1. on the TARGET, the list of package names
dnf repoquery --installed --qf '%{name}\n' | sort -u > pkgs.txt
# 2. on the ONLINE box, download them with all dependencies
dnf download --resolve --alldeps --forcearch=aarch64 --destdir ./rpms $(cat pkgs.txt)
ls rpms | wc -l
# 3. ingest
depot ingest rpm --repo fedora-42-aarch64 ./rpms
```

You should see a `rpm fedora-42-aarch64/<nevra>` line per package and
`repodata rebuilt` at the end. Repodata is written once per `ingest`
call, not per package — ingest a directory, not a loop of files.

## RPMs: a whole repository

If disk is not the constraint, mirror the entire repo once; later weeks
carry only the delta (next tutorial):

```sh
dnf reposync --repo=fedora --repo=updates --arch=aarch64 --newest-only \
             --download-metadata -p /srv/reposync
depot ingest rpm --repo fedora-42-aarch64 /srv/reposync/fedora/Packages/*/ /srv/reposync/updates/Packages/*/
```

`--newest-only` skips superseded versions. This takes hours the first
time and is idempotent: rerun it and only new packages download.

## Attach the signing key
%check: rpm_keys

dnf verifies each package against the repo's GPG key. Fedora's is already
on every Fedora box; a COPR's is one download away:

```sh
depot rpm gpg-key --repo fedora-42-aarch64 /etc/pki/rpm-gpg/RPM-GPG-KEY-fedora-42-primary
curl -sO https://download.copr.fedorainfracloud.org/results/@asahi/kernel/pubkey.gpg
depot ingest rpm --repo asahi-kernel ./copr-rpms
depot rpm gpg-key --repo asahi-kernel pubkey.gpg
depot rpm describe --repo asahi-kernel "Asahi kernel COPR, mirrored 2026-09"
```

You should see `gpg key <hash>` for each. The RPM tab now shows
"attached" instead of a warning, and the generated `.repo` files switch
from `gpgcheck=0` to `gpgcheck=1`.

## Rust toolchains
%check: has_toolchains

`maintenance/pack-rust.sh` downloads and checksums the standalone
component tarballs for a version and the triples you name; depot ingests
its output directory:

```sh
maintenance/pack-rust.sh 1.96.0 x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu aarch64-unknown-linux-musl
depot ingest toolchain rust-install/
curl -s http://localhost:8000/toolchains/index.json | grep -c '"path"'
```

You should see one entry per component per triple (three per triple for
`rustc`, `cargo`, `rust-std`). For cross-compiling, only `rust-std` of the
other triple is needed on top of a host toolchain.

## Nix store paths
%check: has_nix

On a machine with nix, sign the paths you will ship (offline nix refuses
unsigned paths by default), copy them to a directory, ingest:

```sh
nix key generate-secret --key-name depot-1 > ~/.config/nix/depot-1.sec        # once
nix key convert-secret-to-public < ~/.config/nix/depot-1.sec                 # give to clients
nix store sign --key-file ~/.config/nix/depot-1.sec --recursive nixpkgs#hello
nix copy --to file:///tmp/nixcache nixpkgs#hello
depot ingest nix /tmp/nixcache
```

You should see `N store paths (N new NARs)`. A path signed by
cache.nixos.org keeps that signature too, and every default `nix.conf`
already trusts it.

## Check the result
%check: has_content

```sh
depot status
depot verify
```

You should see the counts you expect and `ok`. Open the UI's Overview;
each tab lists what its clients will see. If this depot is the one that
serves the LAN, you are done. If it is the online side of an air gap,
continue with "Move content across the air gap".
