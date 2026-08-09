# Unknown fields: extension point or attack surface?

Asked of the two protocol kits, and the honest answer is **both — and which one
depends on a property of the protocol, not on taste.** Answering it properly
turned up a real bug in `nat-kit` as shipped.

## STUN answers the question in the spec

RFC 5389 §15 splits the attribute space at **0x8000**:

| range | class | what a receiver must do |
|---|---|---|
| 0x0000–0x7FFF | comprehension-**required** | understand it or reject the message |
| 0x8000–0xFFFF | comprehension-**optional** | may skip |

**`nat-kit` shipped skipping both.** The comment said "comprehension-optional"
and the code was `_ => {}`. That is a security bug, not a spec detail:
**MESSAGE-INTEGRITY is 0x0008** and USERNAME is 0x0006 — both in the required
range — so a parser that skips unknown attributes **accepts an authenticated
message while ignoring its authentication**, which is strictly worse than not
supporting authentication at all.

Fixed: `is_comprehension_optional()` and `StunError::UnknownRequired`. Four new
tests, including one that asserts an unimplemented MESSAGE-INTEGRITY stops the
parse.

## The DHT has no such marking, so the policy has to be chosen

`dht-kit` originally refused every unknown key. That sounded strict and was
wrong in the other direction: the reference sends **`want`** (which address
families to return) in its argument maps, which this kit does not implement — so
a real OpenDHT query would have been **dropped whole over a field we do not
need**. Strictness against a protocol you do not control is not safety; it is a
guarantee of eventual incompatibility.

The policy now, and the asymmetry is deliberate:

* **Inside `a` / `r` maps: skip unknown keys.** This is where the reference
  extends, and forward compatibility lives here.
* **At the top level: still refuse.** Those keys are the message's own frame —
  `t`, `y`, `q`, `a`, `r`, `e` — and something unrecognised there means we are
  not looking at the message we think we are.

## Why skipping is safe here, and where it would not be

Skipping is **structural**: the value is parsed far enough to know its length,
then discarded. Two tests pin what that buys —

* a truncated unknown (`bin16` promising 9,999 bytes, five present) still
  fails, rather than desynchronising the reader into parsing the next key from
  the middle of a value
* a **nested** unknown map containing its own `id` and `h` keys is skipped
  whole, and does not leak into the decoded message

Skipping would *not* be safe if an unknown field could carry meaning that
changes how the known fields should be read — which is exactly the case STUN
marks with its comprehension bit and the DHT does not mark at all. That is the
residual risk here, and it is the thing to re-check against a real capture:
**if OpenDHT ever adds an argument that alters the meaning of `values` or
`token`, skipping it silently is the failure mode.**
