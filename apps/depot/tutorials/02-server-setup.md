# Set up the server

From a bare Linux box to a depot that starts at boot, answers on the LAN,
and survives a reboot. About twenty minutes. Assumes Fedora for the
firewall and unit commands; the depot itself is the same everywhere.

## Choose the machine

The depot is a file server: it needs disk, a LAN address, and to be on
when clients need it. It does not need much CPU or memory (hashing a pack
uses one core briefly; serving is copying bytes).

Rough sizes to plan disk for:

| content | size |
|---------|------|
| every crate your projects use | 0.5–2 GB |
| a Fedora package set for one arch (what an installed desktop needs) | 10–30 GB |
| a whole Fedora + updates repo for one arch | 60–90 GB |
| a Rust toolchain per triple | ~150 MB, plus ~30 MB per extra `rust-std` |
| a NixOS system closure | 2–6 GB |

A phone with a big microSD qualifies. So does the Mac. Wired beats wifi
for the dnf pattern (hundreds of small requests).

## Get the binary

Build it, or copy the one file from a machine that did:

```sh
cd apps/depot && cargo build --release
ls -la target/release/depot            # ~2 MB, needs only glibc
# for a phone or an Alpine box (needs the target's rust-std installed):
cargo build --release --target aarch64-unknown-linux-musl
```

Install it where `PATH` finds it and the unit expects it:

```sh
sudo install -Dm755 target/release/depot /usr/local/bin/depot
depot --help | head -3
```

You should see the usage text. `/usr/local/bin` is for programs you
installed yourself; `/usr/bin` belongs to the package manager.

## Choose the root and initialise

The root is one directory holding everything. As a service it is
`/var/lib/depot`; for trying things out as yourself, `~/.local/share/depot`.
The service path is handled for you in the "make it a service" step; for
now, as your own user:

```sh
depot init
ls ~/.local/share/depot
cat ~/.local/share/depot/depot.toml
```

You should see `catalog.json  depot.toml  keys  objects  pub  tmp  trust`
and a config file where every key is commented. `keys/depot.key` is this
depot's signing key — it never leaves the box; `keys/depot.pub` is what
you give to depots that will unpack from here.

## Give it a name

Clients need something stable to type. Pick the address of your LAN
interface and a name for it:

```sh
ip -4 addr | grep inet                  # e.g. 192.168.1.20 on eth0/enp3s0/wlan0
```

Then either add `192.168.1.20 depot.lan` to `/etc/hosts` on every client,
or add an entry in your router's DNS. Bake the choice into the depot so
every generated client file uses it:

```sh
sed -i 's|^public_url = .*|public_url = "http://depot.lan:8000"|' ~/.local/share/depot/depot.toml
grep public_url ~/.local/share/depot/depot.toml
```

Without `public_url`, depot uses whatever `Host:` the client sent, which
is fine for browsing and wrong for files you copy to other machines.

## Run it by hand first

Before making it a service, see it work in the foreground where errors
are in front of you:

```sh
depot serve
```

In a browser on the same box: `http://localhost:8000/`. From another
machine on the LAN:

```sh
curl -sI http://192.168.1.20:8000/api/status | head -1
```

You should see `HTTP/1.1 200 OK` locally. From the other machine you will
probably see a hang or `No route to host` — that is the firewall, next
step. Stop the server with Ctrl-C when done; it logs `depot: stopped`.

## Open the firewall

```sh
sudo firewall-cmd --get-active-zones                 # note the zone your LAN interface is in
sudo firewall-cmd --permanent --add-port=8000/tcp    # default zone; add --zone=X if different
sudo firewall-cmd --reload
```

Start `depot serve` again and repeat the `curl` from the other machine.
You should see `200 OK` now. Do not open 8000 on an interface that faces
the internet: reads are unauthenticated by design.

## Make it a service

The unit runs depot as a throwaway user that owns `/var/lib/depot` and
nothing else, with no capabilities and a locked-down filesystem view.
`ExecStartPre=depot init` means the first start creates the root.

```sh
sudo install -Dm644 apps/depot/systemd/depot.service /etc/systemd/system/depot.service
sudo systemctl daemon-reload
sudo systemctl enable --now depot
systemctl status depot
journalctl -u depot -n 5
```

You should see `active (running)` and the `depot serving /var/lib/depot`
line. Copy `public_url` into `/var/lib/depot/depot.toml` as well (it is a
fresh root), then `sudo systemctl restart depot`.

Using the CLI against the service's root: `sudo DEPOT_ROOT=/var/lib/depot
depot status`. Files you create as root are re-owned to the service user
the next time it starts (systemd does this for `StateDirectory` under
`DynamicUser`), so after `sudo … depot ingest …` run `sudo systemctl
restart depot`. Or avoid the question: enable HTTP writes (next step) and
ingest with `curl`, which never touches the files as root.

## Enable writes over HTTP (optional)

Out of the box the depot is read-only over the network. Uploads, yanks
and pack pushes need a token file that does not exist until you make it:

```sh
sudo DEPOT_ROOT=/var/lib/depot depot admin-token --new
sudo systemctl restart depot            # re-owns the token file to the service user
```

You should see a 64-character token printed once. Anyone holding it can
add and yank content; keep it with the same care as an SSH key. Delete
`keys/admin.token` to turn writes off again; `--new` after that rotates it.

## Back it up

Everything derived can be rebuilt; two things cannot:

```sh
sudo rsync -a /var/lib/depot/{catalog.json,objects,keys,trust,depot.toml} /backup/depot/
```

`pub/` is hardlinks and generated files (`depot rebuild` recreates it);
`tmp/` is scratch. `objects/` is immutable: a backup tool that skips
unchanged files (rsync, borg, restic) will copy each object exactly once,
ever.

## Check it

```sh
sudo DEPOT_ROOT=/var/lib/depot depot status
sudo DEPOT_ROOT=/var/lib/depot depot verify
```

You should see the counts (all zero on a fresh depot), the signing key,
whether writes are enabled, and `ok: every object hashes to its name`.
The Overview tab in the UI shows the same. Next: fill it — "Fill the
depot" in this list.
