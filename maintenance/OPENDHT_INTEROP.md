# Building the reference node in the sandbox, and what it settled

`shared/snapcore/tests/opendht_interop.rs` puts `dhtnode` — the reference
implementation's own binary, from `vendor/opendht` — on the far end of a
loopback socket and drives our transport against it. It is the test
`DHT_CONVERGENCE.md` called "the one that cannot be faked", and until this
session nothing in the repository had run it.

## Build (about four minutes on one core)

```sh
apt-get update && apt-get install -y --no-install-recommends \
    pkg-config cmake g++ libncurses-dev libreadline-dev nettle-dev \
    libgnutls28-dev libargon2-dev libmsgpack-dev libmsgpack-cxx-dev \
    libfmt-dev libjsoncpp-dev libssl-dev
cp -r vendor/opendht /tmp/odht-src        # do NOT patch the vendored tree
# simdutf is not packaged on Ubuntu 24.04 and is used only by base64.cpp.
# Replace that one file with a plain implementation (an ~30-line table
# encoder) and set simdutf_FOUND TRUE in CMakeLists.txt — see the session
# notes for the exact patch; both edits live in the COPY.
mkdir /tmp/odht-build && cd /tmp/odht-build && cmake /tmp/odht-src \
    -DCMAKE_BUILD_TYPE=Release -DBUILD_TESTING=OFF -DOPENDHT_TOOLS=ON \
    -DOPENDHT_PYTHON=OFF -DOPENDHT_C=ON -DOPENDHT_PROXY_CLIENT=OFF \
    -DOPENDHT_PROXY_SERVER=OFF -DOPENDHT_PEER_DISCOVERY=OFF \
    -DOPENDHT_DOCUMENTATION=OFF -DOPENDHT_INDEX=OFF -DOPENDHT_DOWNLOAD_DEPS=OFF \
    -DCMAKE_CXX_FLAGS=-O1
make -j1 dhtnode        # FOREGROUND. A backgrounded make dies with the tool shell.
```

Then:

```sh
DHTNODE=/tmp/odht-build/tools/dhtnode \
  cargo test -p snapcore --test opendht_interop -- --ignored
```

The test is `#[ignore]` and FAILS when the binary is absent rather than
passing quietly — CLAUDE.md §5b.

## What it found

Read side by side with `network_engine.cpp` / `parsed_message.h` /
`value.cpp`, the first `dht-kit` was not the reference's format. Each of
these alone was fatal, and all were present at once:

| | reference | dht-kit, before |
|---|---|---|
| top-level `v` (user agent) | on every message | refused as unknown → every real message dropped |
| reply `sa` (requester's ip) | on every reply | refused |
| `token` | msgpack **bin** (`unpackBlob` throws on an int) | int, on **every** query → every query we sent rejected |
| `find` target | `target` | `h` |
| nodes | `n4`/`n6`, one blob of 26/38-byte records | `nodes`/`nodes6`, array of blobs |
| value | `{id, dat: {body: {type, data}}}` | `{id, d}` |
| put | `values: [...]` | `v: {...}` |
| `s` (client) / `w` (want) | sent | never sent |

The failure was invisible from inside the app because an empty lookup is
also what a healthy DHT returns for an offline peer. Three defects on the
same path had already been fixed in the previous session (framing, token
caching, id correction); they were correct and sat on a codec no node could
read.

## What the test proves, in order

1. `ping` answered — envelope parses both ways. Before the fix the reference
   threw on our integer token and sent nothing.
2. `sa` learned as our address (`WireUdpTransport::public_ip`).
3. The responder's real id replaces the guessed bootstrap id.
4. `find` against A lists B, which A learned by bootstrap — `n4` decodes.
5. `get` hands us a 32-byte token; `put` with it is stored, and the
   reference's own `g` reads it back under the id we derived. **With the
   token removed, the reference stores nothing and the test fails** —
   verified by breaking it.
6. A value the reference wrote (`p`, with `user_type`) decodes here.
7. A full `lookup_nodes` walks from A to B and stores on both.

Two raw datagrams captured from the reference are pinned in dht-kit
(`a_captured_reference_*`): they decode, and **re-encode byte-identically**.

## Still not covered

* `listen` / `update` pushes: encoded and decoded, not driven end-to-end.
  Our client polls.
* Values large enough to arrive as parts (`y: "p"`): decoded as `Msg::Parts`,
  never reassembled. Presence records are ~200 bytes.
* Anything over the real internet: `bootstrap.jami.net` is not reachable
  from this sandbox. Loopback proves the protocol, not the NAT.
