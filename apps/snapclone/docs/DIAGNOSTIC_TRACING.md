# Diagnostic tracing

## The problem this solves

A line like

```
2026-07-27T22:38:53.482  snapclone.ui.swipenav  [I]  [WHEEL] v=-20 h=0 subTabbed=1 atVLimit=0
```

was essential while the swipe/wheel hand-off was being built. It is noise now:
it fires on every wheel notch, costs a format plus a log write each time, and
buries the one line that matters when a real bug report comes in.

Deleting it would be equally wrong. The next person to debug a touch hand-off
needs exactly those lines — and they will be running an **installed build**,
with no compiler, no debug package, and no patience for "rebuild with `-DFOO`".

So these sites are kept, made free when off, and given three independent ways
to switch on.

## Channels

Diagnostics are grouped by *what you are debugging*, not by source directory:

| channel | covers |
|---|---|
| `input`   | swipe/wheel/gesture classification and hand-off |
| `layout`  | virtual keyboard, modal stack, dialog geometry |
| `camera`  | capture pipeline, focus/VCM, camera registry |
| `media`   | encode/record/transcode, viewer, thumbnails, export |
| `net`     | sockets, NAT/UPnP, relays, gossip, routing |
| `dht`     | DHT lookups plus identity/account resolution |
| `storage` | on-disk stores, inbox/outbox, databus, session |
| `voice`   | voice capture/playback, group-voice coordination |
| `ui`      | ambient visuals, conversation views, icons, wayfinder |
| `power`   | power and sleep-state policy |

Every channel is **off by default**. Warnings and errors are never gated —
tracing only ever governs info/debug.

## Turning it on

**While the app is running** — Settings → Debug → *Diagnostic tracing*: one
switch per channel, applied immediately, and **persisted**, so a channel you
enable now is still on after a restart. That last part is the point: it is how
you catch something that breaks *during startup*.

**At launch, on a build already installed** — no rebuild required:

```sh
SnapClone --trace=input,layout     # one or more channels
SnapClone --trace-all              # everything
SnapClone --trace=+net             # add to whatever is already saved
SnapClone --no-trace               # force off, ignore saved state
SnapClone --trace-minutes=60       # auto-off after N minutes (0 = never)
SnapClone --trace-help             # print the channel list and exit-safe help
SNAPCLONE_TRACE=input SnapClone    # same, via the environment
SNAPCLONE_TRACE_MINUTES=60 SnapClone
```

Precedence: CLI > environment > saved settings. A typed flag is more current
than a shell profile. A misspelled channel name is reported on stderr *with the
list of valid names* rather than silently tracing nothing — silence would send
you chasing a phantom.

**Auto-off.** Tracing disables itself after 30 minutes by default (configurable,
0 = until you turn it off) so a field build never quietly fills the disk because
somebody enabled a channel and walked away.

## Writing a trace site

```cpp
#include "debug/Trace.hpp"

SC_TRACE(Input, lcSwipeNav, "[WHEEL] v=%.0f h=%.0f", v, h);   // printf form
SC_TRACE_S(Layout, lcModal) << "presented" << widget;          // stream form
```

Cost when the channel is off: one relaxed atomic load and a predictable branch.
The arguments are **never evaluated** — no formatting, no `QString`, no varargs
marshalling. (This is the part a bare `qCInfo` gets wrong: it evaluates and
formats its arguments *before* Qt decides to drop the message. Filter rules
alone silence the output but not the work.) `tests/test_TraceChannels.cpp`
asserts this property directly with a counting stub.

`SC_TRACE_HOT` is for paths where even that branch is unwelcome — per-frame
paint, per-sample audio. It compiles to **nothing** unless the build sets
`-DSNAPCLONE_HOT_TRACE=ON`, so production pays literally zero while a
diagnostic build can still instrument a render path.

**Which to use:** if it can fire more than a few hundred times a second on a
busy machine, it is HOT. Wheel/touch/gesture events are not — they are bounded
by human input rate — so they use `SC_TRACE` and stay field-switchable.

Warnings and errors keep using `qCWarning`/`qCCritical` directly and are never
gated. A real problem must always reach the log.

## Design notes

`debug/TraceChannels.hpp` (the policy: channel table, spec parsing, filter-rule
generation, expiry arithmetic) is deliberately **Qt-free**, so
`tests/test_TraceChannels.cpp` builds it with a plain compiler — no Qt, no
display, no app. `debug/TraceControl.hpp` is the thin Qt glue: QSettings
persistence, `QLoggingCategory` application, a `QTimer` for auto-off, and a
`changed()` signal so the Settings checkboxes never lie about what is running.

**A bug fixed on the way in.** The previous `UiDebugFlags::apply()` emitted
filter rules only for the four categories it knew about — but
`QLoggingCategory::setFilterRules` *replaces* the entire rule set, so every call
silently reset every other category to Qt's defaults, including anything a CLI
flag had just enabled. `TraceMask::filterRules()` now emits rules for **every**
channel, enabled ones `true` and disabled ones explicitly `false`, and a test
asserts that completeness. `UiDebugFlags` remains as a thin delegate so the
legacy "mobile display debug" checkbox keeps working; it maps onto the `layout`
channel, which owns exactly the categories it used to list.

## Not yet done

The Rust side (`rust/snapclone-*`, and the egui frontend) uses `tracing` and has
its own filtering story; giving it the same channel names and the same
`SNAPCLONE_TRACE` env var would make one habit work across both halves of the
app. Worth doing when the egui port starts carrying real traffic.
