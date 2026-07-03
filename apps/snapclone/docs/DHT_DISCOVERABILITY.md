# DHT Discoverability and Key Signing

This document describes a minimal set of changes to SnapClone's DHT
discovery layer that significantly reduces who can find a user on
the network. It captures the design, the bootstrap problem, the
human-interaction model around introductions, and what's
deliberately out of scope for this iteration.

## TL;DR

Today, a user's stable identifier `sc-c1dd601b` is a global handle:
anyone who knows it can probe the DHT to see whether that user is
announced and look up their reachability addresses. Combined with
the fact that `sc-...` IDs get exchanged through QR codes, contact
swaps, and group introductions, this means an attacker who has ever
been a contact (or who scrapes IDs through other means) can keep
watching a user forever.

The fix is **per-contact salted DHT keys**. Each contact pair
derives a shared secret during their initial introduction; A's DHT
announce key for B is `H(sc-A || salt_AB)`. Strangers without a
salt can't compute any valid lookup key, so the user is invisible
on the DHT to anyone outside their explicit contact graph. Blocking
a contact rotates that pair's salt, and the blocked contact's
lookup key starts pointing at empty space — silently, without
revealing the block happened.

This is the 50-80% win. It doesn't solve all metadata leakage and
it doesn't replace cryptographic identity verification, but it
turns "can be probed by anyone who's seen my ID once" into "can
only be reached by people I've explicitly admitted."

## What the threat actually is

The current `sc-c1dd601b` identifier is derived from a stable
public key. Its hash is the DHT announce location. So:

  - Anyone with that ID can query the DHT and learn whether the
    user is currently online (presence leak)
  - Anyone with that ID can see what reachability addresses are
    being announced (network metadata)
  - A blocked user retains their copy of the ID forever and can
    keep probing — blocking at the application layer doesn't stop
    DHT-level observation
  - An attacker scraping IDs through accidental disclosures
    (screenshots, group invites, public profile pages elsewhere)
    accumulates a target list that they can passively watch over
    time

The crypto layer protects content. The DHT layer leaks presence
and reachability. This is the gap we're closing.

## What we don't try to fix

  - Traffic analysis at the DHT-watching adversary level. Someone
    sniffing all DHT traffic across many nodes can still see that
    *some* presence announcements are happening from *somewhere*,
    even if they can't tell who's announcing or where the announce
    is targeted. This is a fundamental limitation of public DHT
    infrastructure and would require onion-style routing to fix.
    Not in scope for this design.

  - Sybil attacks. The protocol can't prevent an attacker from
    creating arbitrarily many `sc-...` identities. The defense
    against that is contact-graph-bounded discovery (which this
    design supports) and social trust (which this design
    surfaces but doesn't enforce).

  - The introductions-from-strangers problem. There needs to be
    *some* path for a new contact to find you the first time, or
    the system is useless. We address this through a bootstrap
    channel separate from the main DHT — see "Bootstrap" below.

  - Mailbox content visibility. Encrypted blobs published to a
    user's mailbox DHT location remain visible (as encrypted
    blobs) to anyone watching that DHT slot. We don't fix this
    here. The salt-rotation design moves the slot, so old
    observers lose track, but doesn't make the new slot
    invisible to fresh observation.

## The mechanism

Today's flow:

```
A's DHT announce key  =  H(stable_id_A)
B looks up A          =  query DHT at H(stable_id_A)
```

Proposed flow:

```
A's DHT announce keys  =  { H(stable_id_A || salt_AB),
                            H(stable_id_A || salt_AC),
                            H(stable_id_A || salt_AD), ... }
B looks up A           =  query DHT at H(stable_id_A || salt_AB)
A stranger looks up A  =  has no salt, can't compute any valid key,
                          finds nothing
```

`salt_AB` is a 32-byte random value generated during the initial
introduction between A and B. Both sides store it locally,
keyed by contact identity. It never gets transmitted on the
DHT itself; it lives only on the two devices that share it.

### Storage cost

Each contact pair adds one 32-byte salt to each side's local
storage. For a user with 200 contacts that's 6.4 KB. Negligible.

### Announce cost

A user with N contacts now publishes to N DHT keys instead of 1.
This is the main cost of the design. Three mitigations:

  - Publish to all keys in a single batched DHT operation. The
    DHT protocol allows multiple value writes per network round
    trip; the wire cost scales sub-linearly with contact count.
  - Stagger announce timing. Not all contacts need fresh
    announcements at the same cadence. A user who hasn't been
    in touch with contact X for six months gets a much rarer
    announce to `H(... || salt_AX)`. Recency-weighted publishing.
  - Cap announce parallelism. For users with very large contact
    graphs (200+), publish in chunks rather than all at once.

For a typical user with 10-30 active contacts, this is well
within the cost envelope SnapClone already pays for periodic
announces.

### Lookup cost

Unchanged. B still does one DHT query to find A — just at a
different key than before. No scaling change for the lookup side.

## Block as salt rotation

When A blocks B:

  1. A's app deletes `salt_AB` from local storage
  2. A stops publishing to `H(stable_id_A || salt_AB)`
  3. Other salts are unaffected — A's other contacts still find A
     normally
  4. B, holding the old salt, continues to query
     `H(stable_id_A || salt_AB)` and finds nothing
  5. B has no way to distinguish "A is offline" from "A blocked me"

This is the strongest property of the design. Blocking is silent
and total at the network layer. B cannot circumvent it without
somehow obtaining a new salt, which requires A's explicit
cooperation (re-introduction).

If A wants to fully reset blocked-by visibility, A rotates all
salts (effectively a soft identity rotation) and re-introduces
themselves to all legitimate contacts. Painful but available as
a recovery option for severe cases.

### Unblock

If A later unblocks B, A regenerates `salt_AB` and re-introduces
the salt to B through whatever channel makes sense (in-person,
through a mutual contact, via QR). There's no "automatic unblock"
because there's no preserved state to restore — the old salt is
gone. This is intentional: blocked relationships shouldn't have
a "just hit the button to undo" path that an abuser could
socially-engineer.

## The bootstrap problem

If discovery requires a shared salt, how do two people who've
never met establish that salt?

There are four legitimate bootstrap paths, ordered from highest
trust to lowest:

### 1. In-person introduction (highest trust)

A and B are physically together. They open an "add contact" flow
that uses a local side channel (QR code on the screen, Bluetooth
proximity, NFC if available, audible tone). A Diffie-Hellman
exchange happens through that channel, both devices derive the
same `salt_AB`, both confirm a short verification code matches on
both screens.

This is the gold standard — same security level as Signal's
safety-number verification, same usability as exchanging
business cards. Trust is rooted in "I saw the other person's
face when this happened."

A future doc on the Bluetooth side channel will cover the
technical mechanics. For this design, the relevant property is
that an in-person side channel can produce a shared secret that
neither the public DHT nor an internet adversary ever sees.

### 2. Introduction by a mutual contact (medium trust)

Alice already has `salt_AB` with Bob and `salt_AC` with Charlie.
Alice introduces Bob to Charlie. Mechanically:

  1. Alice sends Bob a signed introduction: "this is Charlie's
     identity material + a fresh ephemeral key + my signature"
  2. Alice sends Charlie the symmetric thing about Bob
  3. Bob and Charlie use the ephemeral keys to perform DH and
     derive `salt_BC`
  4. Bob and Charlie each verify Alice's signature on the
     introduction packet they received

Charlie's trust in Bob is rooted in Charlie's trust in Alice.
This is transitive trust, which has well-known failure modes
(Alice might be wrong, Alice might be compromised), so:

  - Introductions are NEVER automatic. Both Bob and Charlie see
    a UI prompt: "Alice introduced Bob/Charlie to you. Accept?"
  - The trust path is visible. The UI shows who vouched and when
  - Acceptance is consent. Until both sides accept, no salt is
    stored, no DHT lookup is added

### 3. Public-key fingerprint exchange (medium-low trust)

A and B can't meet in person and don't share a mutual contact.
They exchange identity fingerprints through some out-of-band
channel they both trust (email, phone call, video meeting where
they read the codes to each other). After fingerprint
verification, they exchange salts through a one-time encrypted
DHT publish.

This is what most messaging apps fall back to. It's vulnerable
to active MITM during the fingerprint exchange but the user's
explicit verification step in their chosen channel is the
defense. Same security level as Signal's "scan code" flow with
people you can't physically meet.

### 4. Bootstrap from a public identifier (lowest trust)

Some users will need a way to be reachable by people who know
only their stable `sc-...` ID — for example, a published
business contact, a public figure, a support account.

For this case, the user can opt-in to publishing a single
"open" announce at `H(stable_id_A || PUBLIC)` where `PUBLIC`
is a well-known constant. This restores today's behavior —
anyone with the ID can find them — but it's an explicit
choice, not a default, and it can be turned off without
disturbing any salted contacts.

This is also the right mechanism for the in-person bootstrap
to start: the new contact's first DHT lookup uses the public
key, and the response contains the first message of the
salt-establishment handshake. The public lookup is only
needed for the initial connection; subsequent lookups use
the established salt.

## Verifying authenticity at introduction time

Every bootstrap path produces a shared salt. None of them prove
that the person on the other end of the introduction is who they
claim to be socially. Three distinct attack vectors:

### Impersonation at in-person introduction

A and "Bob" meet in person and exchange salts. But "Bob" was
actually B, an attacker pretending to be Bob. A's app now has a
salt for someone it thinks is Bob.

The protocol can't fix this. Real-world identity verification
at the moment of first contact is fundamentally a social
problem. Mitigation: the same as any messaging app — A
discovers the imposture the first time real-Bob tries to talk
to A and they don't match. Trust the in-person moment.

### Fake transitive introduction

Alice claims to introduce Bob to Charlie, but the "Bob" in the
introduction is actually B. If Alice is honest but tricked,
this is the impersonation case above, propagated through
Alice's vouching. If Alice is dishonest, she's chosen to
introduce a known-fake identity.

Mitigation: introductions ARE NOT AUTOMATIC. Charlie sees the
introduction, sees that Alice vouched, but still makes a
manual decision. The UI surfaces who vouched and gives Charlie
a chance to check independently with Alice if anything looks
off.

### Replayed introduction

An attacker captures the network packets of a legitimate
introduction and replays them later to inject themselves.

Mitigation: standard cryptographic signature binding. The
introduction packet is signed by the introducer and binds to
the introducee's PUBLIC KEY, not just to claimed identity
strings. The attacker doesn't have the introducee's private
key, so the handshake fails when the attacker tries to act
on the replayed introduction.

## What the UI surfaces

Concrete UI implications:

  - **Add contact** screen presents the four bootstrap paths in
    order of trust. "In-person" is the top option, visually
    distinguished as recommended.
  - **Contact** detail screen shows how this contact was added
    ("in-person on date X" / "introduced by Alice on date Y" /
    "fingerprint verified on date Z" / "public ID lookup").
    This is the trust provenance and the user should be able to
    see it any time.
  - **Block** is a simple button with a single confirmation
    dialog. No "are you sure they'll know?" warning because the
    block IS invisible at the network layer.
  - **Introductions sent to you** appear as a separate inbox
    section, not as accepted contacts. Each one shows who
    vouched, when, and gives accept/decline buttons. No
    auto-accept.

## Migration from today

Existing contact pairs don't have salts. The migration path:

  1. Both sides upgrade to a SnapClone version that supports
     salted lookups
  2. On first mutual encounter after upgrade — either a snap
     exchange or a presence-check via the legacy DHT lookup —
     the apps detect that no salt exists and run a one-time
     salt-establishment exchange through the existing
     encrypted channel
  3. From that point forward, both sides publish and query
     using the new salt
  4. After some grace period (months), the legacy unsalted
     announce can be removed entirely

During the grace period, both lookup paths work — old clients
still find the user via the unsalted hash, new clients find
them via the salted hash. The user is more discoverable than
they'll eventually be, but no less so than today, so there's
no regression.

## Acceptance criteria

For this design to be considered shipped:

  - All four bootstrap paths exist as UI flows
  - In-person introduction works without requiring a network
    connection (proves the side channel is real)
  - Block is silent at the DHT layer — verified by having a
    blocked test client observe its DHT query return empty
    while a sibling test client still gets results
  - Migration from unsalted to salted works without losing any
    existing contacts
  - Public ID lookup is OPT-IN with a clear toggle in settings,
    default off, with explanatory text about what enabling it
    means
  - Salt storage is wiped by factory-reset and is per-contact
    wipeable from the contact detail screen

## What's deliberately deferred

  - **Group introductions.** "Alice introduces Bob, Charlie, and
    Diana to each other as a group" is a real use case but
    complicates the introduction UI substantially. First ship
    pairwise introductions, then think about groups.

  - **Trust path visualization.** Showing the full transitive
    chain ("Bob is trusted because Alice trusts Carla who
    trusts Bob") gets complicated and PGP-web-of-trust-ish
    quickly. Start with direct vouching only.

  - **Trust revocation propagation.** If Alice introduced Bob to
    Charlie and later Alice changes her mind, the protocol
    could propagate the revocation to Charlie. Useful but
    requires careful UX to avoid drama. Defer.

  - **Anti-Sybil heuristics.** Pattern-detection for "this new
    introduction looks like several previous introductions from
    the same network" is a real defense against abusers using
    serial fake identities, but it's a heuristic system with
    false-positive cost. Defer to a separate document on
    blocking and evasion detection.

  - **Onion-style routing through contact graph.** Discussed
    earlier and rejected for now — wrong threat model fit,
    much larger engineering scope. Not part of this design.

## Why this is the right next step

The 115% network tuning preset reduces background battery cost
without changing what the network reveals about you. The face
library reduces snap bandwidth without changing what the
network reveals about you. This change is the first one in the
roadmap that improves the privacy posture of who-can-find-whom,
and it does so with a relatively small protocol change layered
on top of existing primitives.

It also lays the groundwork for the in-person Bluetooth
onboarding flow (separate document) and the partner-pair-shared
face library (V32.34+). Both of those features need an
established "in-person verified contact" relationship type
that this design creates.
