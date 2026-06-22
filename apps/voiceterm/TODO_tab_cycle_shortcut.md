# TODO: {N}b web-tab cycling shortcut

## Request
A vim-style count-prefixed tab cycle for WEB tabs: `b` = next, `B` = prev,
with a count (`2b` = forward 2, `3B` = back 3). Keyboard.

## DONE — modeless {N}b landed (v0.1.719)
The true modeless count prefix is now wired:
- PURE: src/count_prefix.rs — CountPrefix accumulator (8 tests). feed(ch) →
  Absorbed(n) / Fire{cmd,count} / PassThrough. Handles leading-zero passthrough,
  multi-digit, cap (99), reset-on-other-key (vim semantics).
- Backend: tab_count_key(ch) qt_method feeds the accumulator, fires
  tab_cycle(count, forward) on b/B, returns the wire ("absorb\tN" / "fire\t
  count\tfwd" / "pass"). Plain _tab_count field holds the state. qt_struct clean
  (528 fields).
- QML: helixKeys (the browse-tab key handler) feeds EVERY key through
  tab_count_key first; absorb/fire → consume, pass → fall through to the
  existing single-letter nav (o/a/j/k/g/G/…). The handler's focus condition
  already excludes urlBar/findField/overlays, so it never steals digits from
  typing. Verified the wire contract end-to-end (3b→fwd3, 2B→back2, j→pass+
  reset, bare b→1). lint clean; harness 1372 + 28.
- Alt+b / Alt+Shift+b single-step shortcuts retained as the always-safe
  fallback.
RUNTIME-UNTESTED: keystroke routing / focus scoping needs a live GUI; the
accumulator logic + wire are unit-tested.

## Done (v0.1.642)
- PURE: src/tab_cycle.rs cycle_target(current, count, step, forward) →
  wrapped target index. Handles count>count (mods), single/no tab, stale
  index. 8 tests.
- Backend: tab_cycle(step, forward) qt_method applies it via tab_switch
  (3 sites; qt_struct clean, 443 fields).
- QML: Alt+b / Alt+Shift+b shortcuts (browse-tab gated) call
  tab_cycle(1, …). In chordRegistry. lint clean.

## Why Alt+b, not bare `b` (and the {N}b gap)
A bare unmodified `b` shortcut would clobber typing the letter "b"
anywhere in the browser (URL bar, form fields). The true MODELESS `{N}b`
(type a count then b, no modifier, like vim) needs a focus-gated key
handler that:
1. only captures when the browse VIEW (not a text input) has focus,
2. accumulates leading digits into a pending count,
3. on `b`/`B` fires tab_cycle(count||1, forward) and resets the count,
4. on any non-digit/non-b key resets the pending count.
That's a stateful Keys.onPressed over the browse area — build-gated
(can't verify keystroke routing / focus scoping in-sandbox, and a
mis-scoped handler eating keys is worse than the current Alt+b). The
backend already takes any step, so this is a QML-only addition.

### Sketch for the modeless handler (when building)
- Add a focus-scoped Item (or Keys attached to the browse root) active
  only when tabBar.currentIndex===0 AND no text input has activeFocus.
- property int _pendingCount: 0
- Keys.onPressed: if event.text matches /[0-9]/ → _pendingCount =
  _pendingCount*10 + digit (cap ~99), accept. If event.key is B →
  backend.tab_cycle(Math.max(1,_pendingCount), !(event.modifiers &
  Qt.ShiftModifier)); _pendingCount=0; accept. Else _pendingCount=0 (don't
  accept — let the key through).
- Verify it does NOT steal digits/b from the URL bar or page forms.
Keep Alt+b as the always-safe fallback.
