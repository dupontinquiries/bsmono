"""
PAPERBOUND — reliable input layer.

Lockstep is unforgiving: every deploy input must apply on the same tick on both
machines, or the sims diverge. UDP can drop packets, so deploy inputs need a
reliability layer on top — but a *tiny* one, since inputs are rare and already
scheduled a few ticks ahead (INPUT_DELAY), which gives us a natural resend
window.

This module adds two things, kept separate from the transport so it works over
UDP or the HTTP relay:

  1. ACK + RESEND — each outgoing deploy carries a monotonic `seq`. The sender
     keeps it in a small buffer and resends (a few times, short interval) until
     the peer acks that seq. The receiver dedupes by seq so a resent deploy is
     never applied twice, and acks every deploy it sees. A dropped packet self-
     heals within the INPUT_DELAY window instead of stalling the match.

  2. SAME-TICK BATCHING — deploys the local player makes in the same frame are
     coalesced into one `inputs` packet (the wire codec's OP_INPUTS), so a quick
     double-drop costs one datagram, not two.

The game drives it: wrap sends with ReliableOut.send(), flush once per frame,
feed incoming acks to note_ack(), and gate incoming deploys through
ReliableIn.accept() to dedupe.
"""
import time


class ReliableOut:
    """Sender side: sequences + buffers outgoing deploys, batches them within a
    short coalescing window (bounded by the lockstep input-delay budget), and
    resends the unacked until they're acknowledged.

    Coalescing: lockstep applies an input on tick T+INPUT_DELAY, so there's a
    slice of real time between issuing a deploy and when it must arrive. Holding a
    deploy for a few tens of ms lets a burst (e.g. dropping two cards fast, or the
    catch-up after an ink-starved pause) ride in ONE datagram instead of several.

    Aggressive batching: a single flush emits ONE 'bundle' packet that carries
    every pending deploy (across *all* ticks, not just the same one), any
    resends that are due, AND piggybacked acks — so instead of N deploy packets +
    an ack packet + a heartbeat, the peer gets one. Fewer datagrams means less
    per-packet overhead and, on mobile, fewer radio wake-ups (more idle = more
    battery). The bundle also serves as an implicit heartbeat."""
    def __init__(self, send_fn, retry=0.12, max_tries=8, coalesce=0.05, max_hold=0.12):
        self._send = send_fn          # a function(msg_dict) that actually transmits
        self.retry = retry            # seconds between resends of an unacked deploy
        self.max_tries = max_tries
        self.coalesce = coalesce      # how long to wait for more deploys to batch
        self.max_hold = max_hold      # hard cap on holding (stay within I-delay)
        self._seq = 0
        self._unacked = {}            # seq -> {"msg", "t_last", "tries"}
        self._batch = []              # deploys accumulated but not yet sent
        self._hold_since = None       # when the current batch started accumulating
        self._last_add = None         # when the last deploy was added
        self._pending_acks = []       # acks to piggyback on the next outgoing packet

    def queue_acks(self, seqs):
        """Stage acks to ride along on the next outgoing bundle instead of going
        in their own packet."""
        if seqs:
            self._pending_acks.extend(seqs)

    def send_deploy(self, msg, now=None):
        """Queue a deploy (assigns it a seq). It's held briefly to coalesce with
        any follow-up deploys, then sent by flush() when the window elapses. Pass
        `now` to share the caller's clock (must match the clock flush() uses)."""
        self._seq += 1
        msg = dict(msg); msg["seq"] = self._seq
        now = time.time() if now is None else now
        if self._hold_since is None:
            self._hold_since = now
        self._last_add = now
        self._batch.append(msg)
        return self._seq

    def _ready_to_send(self, now):
        """True once the coalescing window has elapsed (a quiet gap since the last
        deploy) or the hard hold cap is hit — so a lone deploy still goes promptly
        and a burst waits just long enough to merge."""
        if not self._batch:
            return False
        if now - (self._hold_since or now) >= self.max_hold:
            return True
        return now - (self._last_add or now) >= self.coalesce

    def _emit_bundle(self, deploys, acks):
        """Send one packet carrying many deploys (any ticks) + piggybacked acks.
        Falls back to the leanest shape: a lone deploy with no acks stays a plain
        'deploy'; otherwise a 'bundle'."""
        if len(deploys) == 1 and not acks:
            self._send(deploys[0])
            return
        if not deploys and acks:
            self._send({"t": "ack", "seqs": acks})
            return
        self._send({"t": "bundle",
                    "deploys": [{"seq": d["seq"], "tick": d.get("tick", 0),
                                 "key": d["key"], "x": d["x"], "y": d["y"],
                                 "team": d["team"]} for d in deploys],
                    "acks": acks})

    def flush(self, now=None, force=False):
        """Emit at most ONE packet: every pending deploy (across ticks) plus any
        piggybacked acks. Deploys enter the unacked buffer for resend. If there
        are only acks staged (no deploys), they still go out so the peer's resend
        timer clears promptly."""
        now = now or time.time()
        have_deploys = bool(self._batch)
        # hold deploys for the coalescing window unless forced; but if only acks
        # are pending, send them right away (they're tiny and time-sensitive).
        if have_deploys and not force and not self._ready_to_send(now):
            return
        acks = self._pending_acks
        self._pending_acks = []
        if not self._batch and not acks:
            return
        for m in self._batch:
            self._unacked[m["seq"]] = {"msg": m, "t_last": now, "tries": 1}
        self._emit_bundle(self._batch, acks)
        self._batch = []
        self._hold_since = None
        self._last_add = None
        self._hold_since = None
        self._last_add = None

    def note_ack(self, seq):
        """Peer confirmed this seq — stop resending it."""
        self._unacked.pop(seq, None)

    def note_acks(self, seqs):
        for s in (seqs or []):
            self._unacked.pop(s, None)

    def resend_due(self, now=None):
        """Resend any unacked deploy whose retry interval has elapsed, coalesced
        into ONE bundle packet (plus any staged acks) rather than one datagram per
        lost deploy. Call each frame. Returns the number resent."""
        now = now or time.time()
        due = []
        for seq, rec in list(self._unacked.items()):
            if now - rec["t_last"] >= self.retry:
                if rec["tries"] >= self.max_tries:
                    # give up eventually; lockstep will surface a stall elsewhere
                    self._unacked.pop(seq, None)
                    continue
                rec["t_last"] = now; rec["tries"] += 1
                due.append(rec["msg"])
        if due:
            acks = self._pending_acks
            self._pending_acks = []
            self._emit_bundle(due, acks)
        return len(due)

    @property
    def in_flight(self):
        return len(self._unacked)


class ReliableIn:
    """Receiver side: dedupes deploys by seq (so resends don't double-apply) and
    tracks which seqs to acknowledge."""
    def __init__(self, ack_window=4096):
        self._seen = set()
        self._order = []              # ring of seen seqs to bound memory
        self.ack_window = ack_window
        self.pending_acks = []        # seqs to acknowledge on the next flush

    def accept(self, seq):
        """True if this deploy is NEW (should be applied); False if a duplicate.
        Either way the seq is queued to be acked (a lost ack triggers a resend,
        which we must re-ack)."""
        self.pending_acks.append(seq)
        if seq in self._seen:
            return False
        self._seen.add(seq); self._order.append(seq)
        if len(self._order) > self.ack_window:
            old = self._order.pop(0); self._seen.discard(old)
        return True

    def drain_acks(self):
        """Return the seqs to ack and clear the pending list."""
        acks = self.pending_acks
        self.pending_acks = []
        return acks


if __name__ == "__main__":
    # self-test: dropped deploys resend and apply once; bursts coalesce into ONE
    # bundle across ticks; acks piggyback; the coalescing window holds then flushes.
    sent = []
    out = ReliableOut(lambda m: sent.append(m), retry=0.0, max_tries=5,
                      coalesce=0.05, max_hold=0.12)
    inp = ReliableIn()

    # two deploys (even on the same tick) in one flush → ONE bundle packet
    out.send_deploy({"t": "deploy", "tick": 10, "key": "crane", "x": 1, "y": 2, "team": 0})
    out.send_deploy({"t": "deploy", "tick": 10, "key": "gorilla", "x": 3, "y": 4, "team": 0})
    out.flush(force=True)
    assert len(sent) == 1 and sent[0]["t"] == "bundle" and len(sent[0]["deploys"]) == 2, sent
    assert out.in_flight == 2

    # receiver: both seqs new, duplicate rejected
    seqs = [d["seq"] for d in sent[0]["deploys"]]
    assert inp.accept(seqs[0]) is True and inp.accept(seqs[1]) is True
    assert inp.accept(seqs[0]) is False, "duplicate must be rejected"

    # deploys ACROSS DIFFERENT TICKS also batch into one bundle (the new win)
    sent.clear()
    out.send_deploy({"t": "deploy", "tick": 12, "key": "baobab", "x": 5, "y": 6, "team": 1})
    out.send_deploy({"t": "deploy", "tick": 20, "key": "crane", "x": 7, "y": 8, "team": 0})
    out.flush(force=True)
    assert len(sent) == 1 and sent[0]["t"] == "bundle" and len(sent[0]["deploys"]) == 2, sent
    ticks = sorted(d["tick"] for d in sent[0]["deploys"])
    assert ticks == [12, 20], ticks

    # a lone deploy with no acks stays a plain 'deploy' (leanest shape)
    sent.clear()
    out.send_deploy({"t": "deploy", "tick": 30, "key": "gorilla", "x": 0, "y": 0, "team": 0})
    out.flush(force=True)
    assert sent[-1]["t"] == "deploy" and sent[-1]["key"] == "gorilla"

    # nothing acked yet → resend fires, and ALL due resends coalesce into one bundle
    sent.clear()
    n = out.resend_due()
    assert n == 5, (n, out.in_flight)                 # 2 + 2 + 1 in flight
    assert len(sent) == 1 and sent[0]["t"] == "bundle", sent

    # acks piggyback: stage acks, then a flush with deploys carries them along
    sent.clear()
    out.queue_acks([101, 102])
    out.send_deploy({"t": "deploy", "tick": 40, "key": "crane", "x": 1, "y": 1, "team": 0})
    out.flush(force=True)
    assert sent[0]["t"] == "bundle" and sent[0]["acks"] == [101, 102], sent

    # ack everything → nothing in flight, no more resends
    for s in [d["seq"] for m in [] for d in []]:
        pass
    all_seqs = list(out._unacked.keys())
    out.note_acks(all_seqs)
    assert out.in_flight == 0 and out.resend_due() == 0

    # staged acks with NO deploys still go out (as a lean ack packet)
    sent.clear()
    out.queue_acks([7, 8])
    out.flush(force=True)
    assert sent and sent[0]["t"] == "ack" and sent[0]["seqs"] == [7, 8]

    # receiver re-acks duplicates (so a lost ack recovers)
    inp.accept(seqs[0])
    assert seqs[0] in inp.drain_acks() and inp.drain_acks() == []

    # ── coalescing window: a fresh out that holds briefly then batches a burst ──
    import time as _t
    sent2 = []
    out2 = ReliableOut(lambda m: sent2.append(m), retry=0.0, coalesce=0.03, max_hold=0.10)
    out2.send_deploy({"t": "deploy", "tick": 20, "key": "a", "x": 0, "y": 0, "team": 0})
    out2.flush()                        # window not elapsed yet → held, nothing sent
    assert sent2 == [], "should hold within the coalescing window"
    out2.send_deploy({"t": "deploy", "tick": 20, "key": "b", "x": 0, "y": 0, "team": 0})
    _t.sleep(0.04)                      # quiet gap longer than coalesce
    out2.flush()
    assert len(sent2) == 1 and sent2[0].get("t") == "bundle", sent2  # the burst merged
    assert len(sent2[0]["deploys"]) == 2

    # max_hold cap: even a steady trickle is forced out by the hard cap
    sent3 = []
    out3 = ReliableOut(lambda m: sent3.append(m), retry=0.0, coalesce=0.5, max_hold=0.05)
    out3.send_deploy({"t": "deploy", "tick": 30, "key": "c", "x": 0, "y": 0, "team": 0})
    _t.sleep(0.06)
    out3.flush()                        # coalesce huge, but max_hold forces send
    assert len(sent3) == 1, sent3

    print("✅ netreliable self-test: same-tick batching, dedupe on resend, "
          "ack stops resend, coalescing window holds+merges a burst, max_hold cap, "
          "re-ack on duplicate — all OK")
