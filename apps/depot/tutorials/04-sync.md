# Move content across the air gap

Two depots: one that can see the internet, one on the offline LAN. A
**pack** carries what the offline one lacks — signed, so a stick from a
stranger is refused, and deduplicated, so the weekly delta is the delta.

## How a pack works

`depot pack` writes one file: a manifest (which crates, packages, paths
and objects), a signature over the manifest, then the objects. `depot
unpack` checks the signature before reading a single object, then checks
each object's bytes against its name as it lands. The receiver's
**have-list** — every object hash it already holds — is what makes packs
small: nothing on the list is included.

```sh
depot pack --help | grep -A3 "^SYNC"
```

You should see the four commands: `have`, `pack`, `unpack`, `inspect`.

## Exchange keys — once

The offline depot must be told which public key to accept:

```sh
# on the ONLINE depot
depot status | grep "signing key"            # prints the 64-hex public key
cat ~/.local/share/depot/keys/depot.pub      # the same, as a file

# on the OFFLINE depot (type the hex, or carry the .pub file over)
depot trust online 5dc90e81…                 # any name you like
depot status | grep trusted
```

You should see `trusted     online`. The private key never moves. If the
online depot is ever rebuilt with a new key, repeat this step and the
old trust entry can be deleted from `trust/`.

## Offline: say what you have

```sh
depot have --out have.txt
wc -l have.txt
```

You should see one hash per line — 64 characters each, about 65 bytes per
object, so a depot with 20 000 objects produces a 1.3 MB file. Carry it
to the online side (the same stick, on the way out).

## Online: pack the difference

```sh
depot pack --out 2026-09-12.depot --have have.txt --all
depot inspect 2026-09-12.depot
```

You should see `N objects, X of artifacts, Y on disk; M skipped (receiver
has them)`. `--all` is every kind; narrow it with `--crates`, `--rpm
fedora-42-aarch64`, `--nix`, `--toolchains`. Without `--have` the pack
contains everything selected, which is right for the very first fill and
wasteful after. `inspect` reads only the manifest — use it to check a pack
before carrying it.

## Offline: unpack

```sh
depot unpack 2026-09-12.depot
depot status
```

You should see `signed by online`, then object and catalog counts. The
served tree is rebuilt at the end; clients see the new content on their
next `dnf makecache` / `cargo fetch`. The `.depot` file can be deleted
now: everything in it is in `objects/`.

If you see `signature matches none of the trusted key(s)`, the key step
was skipped or the online depot's key changed. If you see `pack ended
before object … arrived`, the copy was truncated — copy it again; nothing
partial was recorded.

## The same thing over a wire

When the two depots can reach each other for a while (a phone hotspot,
the offline depot carried to the office), skip the stick:

```sh
# on the OFFLINE depot: ask the online one for a pack of what I lack
depot have | curl -sS --data-binary @- "http://online.lan:8000/api/pack?select=all" -o sync.depot
depot unpack sync.depot
```

Or push into a depot that has an admin token:

```sh
curl -X PUT -H "Authorization: Bearer $TOKEN" --data-binary @sync.depot http://depot.lan:8000/api/packs
```

You should see the same report as a CLI unpack, as JSON. The pack is
generated on the fly and streamed; nothing is stored on the online side.

## Make it routine

A weekly cycle on a stick is: `have` on the way out, `pack --have` at the
online end, `unpack` back home. Two housekeeping commands:

```sh
depot gc            # objects nothing references (after yanks / repo removals) — lists only
depot gc --delete   # remove them
depot verify        # after any doubt about the disk or the stick
```

You should see `0 unreferenced object(s)` normally. Yanked crate versions
are *not* unreferenced — yanked means "marked", the file stays, as on
crates.io.
