# Prepare a client

Pointing one machine at the depot: cargo, dnf, a Rust toolchain, nix —
each is a small file in a known place. The Clients tab of the UI writes
them for you; this walks through what they do so you can fix them when
something changes.

## Reach the depot
%check: reachable

Before configuring anything, prove the client can see the server:

```sh
getent hosts depot.lan                          # name → address (else add it to /etc/hosts)
curl -sI http://depot.lan:8000/api/status | head -1
```

You should see `HTTP/1.1 200 OK`. Anything else is the network, not
the depot: `Could not resolve host` — the name; a hang — the firewall on
the server; `Connection refused` — depot is not running there.

## The shortcut: bootstrap.sh

The depot generates a script that writes every client file for the
things you ask for. Always read it first: `DRY=1` prints without writing.

```sh
curl -fsSL "http://depot.lan:8000/bootstrap.sh?crates=1&rpm=fedora-42-aarch64&toolchain=1" -o bootstrap.sh
DRY=1 sh bootstrap.sh
sudo sh bootstrap.sh
```

You should see each file's path and contents, then `done`. The remaining
steps explain each file; skip to "Check it" if the script did what you
wanted.

## cargo

One file, and cargo fetches from the depot while still calling the
registry `crates-io` in `Cargo.lock` — so lock files made online and
offline are identical:

```sh
mkdir -p ~/.cargo && cat >> ~/.cargo/config.toml <<'EOF'
[source.crates-io]
replace-with = "depot"
[source.depot]
registry = "sparse+http://depot.lan:8000/crates/index/"
EOF
cd ~/src/myproject && cargo fetch
```

You should see `Updating depot index` and downloads from the depot. Do
**not** add `net.offline = true`: it forbids every fetch, including from
the depot. `failed to get 'foo' as a dependency` means the depot lacks
that crate or version — it has to be ingested on the online side.

## dnf

One `.repo` file per depot repo, and every internet repo disabled so
dnf does not fail trying to reach it:

```sh
sudo tee /etc/yum.repos.d/depot-fedora-42-aarch64.repo <<'EOF'
[depot-fedora-42-aarch64]
name=depot mirror: fedora-42-aarch64
baseurl=http://depot.lan:8000/rpm/fedora-42-aarch64/
enabled=1
gpgcheck=1
gpgkey=http://depot.lan:8000/rpm/fedora-42-aarch64/RPM-GPG-KEY
metadata_expire=1h
skip_if_unavailable=False
EOF
sudo dnf config-manager setopt 'fedora*.enabled=0' 'updates*.enabled=0' '*openh264*.enabled=0'
sudo dnf makecache
dnf repoquery --repo 'depot-*' bash
```

You should see the repo's metadata download and `bash-…` listed. Use
`gpgcheck=1` whenever the depot has a key attached (the Clients tab tells
you). If the original repo files had `priority=` or `module_hotfixes=`
lines (Asahi's COPRs, for instance), copy them into this file — the
generated snippet does not carry them.

## A Rust toolchain without rustup

The depot serves the standalone installers plus a script that fetches,
verifies and runs them for your machine's triple:

```sh
curl -fsSL http://depot.lan:8000/toolchains/install.sh -o i.sh
sh i.sh 1.96.0 "$HOME/.local"                   # or /usr/local as root
export PATH="$HOME/.local/bin:$PATH"
rustc --version && cargo --version
```

You should see the version printed after each component downloads and
its sha256 matches `index.json`. To cross-compile for a phone, add its
standard library into the same prefix:

```sh
COMPONENTS=rust-std TRIPLE=aarch64-unknown-linux-musl sh i.sh 1.96.0 "$HOME/.local"
```

## nix

Two lines in `nix.conf`: the depot first, then the public cache for boxes
that can still reach it; and the key the paths were signed with:

```sh
sudo tee -a /etc/nix/nix.conf <<'EOF'
substituters = http://depot.lan:8000/nix https://cache.nixos.org
trusted-public-keys = depot-1:<public key from the online side> cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY=
EOF
sudo systemctl restart nix-daemon
nix path-info --store http://depot.lan:8000/nix /nix/store/<a path you copied>
```

You should see the path printed (it exists in the cache). On NixOS these
go in `nix.settings.substituters` / `nix.settings.trusted-public-keys`
instead. `… is not signed by any trusted key` means the key line is
missing or wrong.

## Reinstalling a machine from the depot

For a standard Fedora box (x86_64, or aarch64 that boots a Fedora ISO —
not Apple Silicon), the netinstall image can install from the depot's
repo with a kickstart that lists packages explicitly (there is no
`comps.xml`, so groups do not resolve):

```
inst.repo=http://depot.lan:8000/rpm/fedora-42-x86_64/  inst.ks=http://…/ks.cfg
```

The repo must contain the whole install set — the "set an installed
system needs" step of "Fill the depot" gets you there. Apple Silicon
uses the Asahi installer, which needs a static-file mirror depot does
not offer yet; a running Asahi box updates from the depot fine.

## Check it, and what to do when it fails

```sh
curl -sI http://depot.lan:8000/crates/index/config.json | head -1     # cargo entry point
curl -sI http://depot.lan:8000/rpm/fedora-42-aarch64/repodata/repomd.xml | head -1
```

Both should be `200 OK`. Then one real operation per tool: `cargo fetch`
in a project, `dnf repoquery bash`, `rustc --version`. When one fails,
the server's journal (`journalctl -u depot -n 20` on the depot) tells you
whether the request arrived and with which status: a `404` there is a
missing package, not a broken client.
