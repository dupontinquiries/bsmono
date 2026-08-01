# OpenDHT: measured, and the case for not porting it

## What it costs, from its own tree

```
29,346 lines of C++ (src + include)
native deps: fmt, nettle, gnutls, msgpack, jsoncpp, asio, argon2, simdutf
```

The repo ships a `rust/` crate, which is worth being precise about: it is **FFI bindings over the C API**
(`libc`, `os_socketaddr`), not a port. It does not remove the native dependency, it wraps it — so
adopting it means building libopendht and its eight dependencies for every target.

The hard part is cross-compiling **GnuTLS and nettle for Android**, which is exactly the case that
motivated this: a phone reaching an M2 Ultra.

## Why the requirement is smaller than a DHT

A DHT solves *find an arbitrary peer among millions, with no server*. That is a real problem and it is
not the one here.

`trustkit` establishes that an unregistered machine gets **nothing** — no compute, no build, no handoff.
So the peer set is enumerated in advance, by a person, deliberately. **Discovering unknown peers is work
whose output the trust model immediately discards.** The DHT's central value is the thing we already
forbade.

What remains: given a peer ID you have already vouched for, find where it is now. That is **rendezvous,
not routing**, and it is a few hundred lines rather than thirty thousand.

## What was built instead

`rendezvous` resolves a registered peer to the cheapest live address:

```
Loopback → Lan → LastKnown → Relay
```

Three properties worth stating, because they are where this could go wrong:

* **Resolution takes the registry as a parameter.** A caller has no way to ask about an unregistered
  id. The security argument is expressed in the signature rather than in a comment.
* **A discovery response can only ever *locate*, never *introduce*.** An unregistered responder is
  shown to the operator and nothing is sent to it — that is how you notice a machine you meant to
  register, and equally how you notice one you did not. On a café LAN, "a machine answered the
  broadcast" is what an attacker also does.
* **Stale addresses are dropped, not deprioritised.** A laptop that changed networks has a last-known
  address that now belongs to someone else, so trying it means connecting to an unknown host — worse
  than failing to connect.

## What is still missing, honestly

`rendezvous` decides *which* address to use. It does not yet **do** any of:

* **LAN multicast** — the actual announce/listen. Small: a UDP multicast socket and a message format.
  This alone covers phone↔M2 at home, which is most of the stated use.
* **NAT traversal** — needed only for travelling. This is the part where a hosted rendezvous point or
  something like WireGuard becomes the pragmatic answer, and where OpenDHT's machinery would have been
  earning its keep if the peer set were unknown. It is not.
* **Transport and authentication** — a registered peer must prove it is that peer. This is the piece
  that genuinely needs crypto, and it needs *one* primitive (a signed challenge), not a TLS stack.

## The recommendation

Do the LAN case first. It is small, it covers the common situation, and it will surface the message
format and the authentication requirement against something real. If travelling turns out to need more
than a known relay address, revisit — but revisit with a measurement of what failed, not with a library
chosen before the requirement was known.
