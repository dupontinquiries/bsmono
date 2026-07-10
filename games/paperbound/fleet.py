"""
PAPERBOUND — fleet integration.

Paperbound ships its own LAN net stack (netplay + transport: lockstep over UDP/HTTP, mDNS
discovery). This module is the bridge to the *shared* fleet layer the Rust games use — the
`netcode` crate's DHT of player profiles + joinable sessions (games/shared/netcode). It does NOT
replace paperbound's transport; it lets a paperbound host *announce* itself to the fleet so friends
see "…is hosting Paperbound" in the games centre and can be handed back paperbound's own dial string.

The point of interop is the wire format: a `SessionRecord` encoded byte-for-byte the same as
`netcode::session::SessionRecord`, so whatever publishes to the DHT (a fleet node / the games centre)
and whatever paperbound emits agree. This module owns that codec and a small presence helper; the
actual DHT publish is delegated to a fleet endpoint when one is present (a running snapcore/OpenDHT
node), and is a no-op otherwise — paperbound still works standalone.

Kept to the stdlib.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass
from typing import Callable, Optional

# Must match games/shared/netcode/src/session.rs
WIRE_V = 1


def _put_str(s: str) -> bytes:
    """Length-prefixed (u32 LE) UTF-8, matching netcode::session::put_str."""
    b = s.encode("utf-8")
    return struct.pack("<I", len(b)) + b


def _take_str(buf: bytes, off: int) -> tuple[str, int]:
    (n,) = struct.unpack_from("<I", buf, off)
    off += 4
    s = buf[off : off + n].decode("utf-8")
    return s, off + n


@dataclass(frozen=True)
class SessionRecord:
    """Where + how to reach a live game session — the control-plane record friends resolve before
    their own transport dials in. Mirror of the Rust struct of the same name."""

    game: str
    host: str
    transport: str  # "udp" | "http" | … — paperbound's own transport name
    addr: str  # dial string for that transport ("ip:port")

    def encode(self) -> bytes:
        return bytes([WIRE_V]) + _put_str(self.game) + _put_str(self.host) + _put_str(self.transport) + _put_str(self.addr)

    @staticmethod
    def decode(data: bytes) -> Optional["SessionRecord"]:
        if not data or data[0] != WIRE_V:
            return None
        try:
            off = 1
            game, off = _take_str(data, off)
            host, off = _take_str(data, off)
            transport, off = _take_str(data, off)
            addr, off = _take_str(data, off)
        except (struct.error, UnicodeDecodeError, IndexError):
            return None
        return SessionRecord(game, host, transport, addr)


def session_key(token: str) -> str:
    """DHT key for a session record — namespaced exactly like netcode::session::session_key so a
    token can never collide with a player-id profile key in the shared store."""
    return f"session:{token}"


def session_token(host_id: str, game: str = "paperbound") -> str:
    """Deterministic token so a re-host reuses the same DHT key (matches bsgames' session_token)."""
    return f"{game}@{host_id}"


# A publisher writes (key, value_bytes) to the fleet DHT. Injected so this module needs no DHT deps;
# None means "no fleet node available" and announce/resolve degrade to no-ops (paperbound's own
# mDNS/UDP discovery still works).
Publisher = Callable[[str, bytes], None]
Resolver = Callable[[str], Optional[bytes]]


class FleetPresence:
    """Announce a paperbound host to the shared fleet and resolve a friend's session back to a dial
    target. `publish`/`resolve` bridge to a real DHT node when present; otherwise this is inert."""

    def __init__(self, host_id: str, publish: Optional[Publisher] = None, resolve: Optional[Resolver] = None):
        self.host_id = host_id
        self._publish = publish
        self._resolve = resolve

    def announce_host(self, transport: str, addr: str, game: str = "paperbound") -> str:
        """Publish a SessionRecord for this host's match; returns the token friends resolve. No-op on
        the DHT if no publisher is wired, but still returns the token (paperbound stays playable)."""
        token = session_token(self.host_id, game)
        rec = SessionRecord(game=game, host=self.host_id, transport=transport, addr=addr)
        if self._publish is not None:
            self._publish(session_key(token), rec.encode())
        return token

    def resolve_session(self, token: str) -> Optional[SessionRecord]:
        """Fetch a friend's session record by token (None if no resolver or not found)."""
        if self._resolve is None:
            return None
        raw = self._resolve(session_key(token))
        return SessionRecord.decode(raw) if raw else None


if __name__ == "__main__":
    # Self-test + emit a record for the cross-language check against the Rust codec.
    r = SessionRecord("paperbound", "kesslerd", "http", "10.0.0.5:7777")
    enc = r.encode()
    assert SessionRecord.decode(enc) == r, "python round-trip failed"
    assert session_key("paperbound@kesslerd") == "session:paperbound@kesslerd"
    import sys

    if len(sys.argv) > 1 and sys.argv[1] == "--hex":
        print(enc.hex())
    else:
        print("fleet.py self-test OK:", r)
