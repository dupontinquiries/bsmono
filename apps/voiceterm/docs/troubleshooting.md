# Troubleshooting

Operational notes for diagnosing common issues. Add to this doc
when a new investigation pattern proves useful.

## Performance profiling

When the app feels slow, run with profiling enabled and analyze
the log. The profiling captures frame timings, touch events, and
labeled state transitions in 3-second windows.

### Enabling

```
WEB2QML_PROFILE=1 cargo run --release --features voice
```

The flag is read once at startup. Profiling output goes to
`~/.config/web2qml/profiling.log` — one JSON line per 3s window.

When enabled, the terminal prints `[profile] enabled (WEB2QML_PROFILE=1) — writing to ~/.config/web2qml/profiling.log` at startup. If you don't see that line, the flag wasn't picked up.

### Log format

Each line is a JSON object:

```json
{
  "t": <unix_ms>,
  "fps_p": [min, p25, p50, p75, max],
  "dt_p": [<frame-times-ms percentiles>],
  "frames": <count>,
  "touches": [press, move, release],
  "events": ["page:about:blank", "altscreen:enter", "tab:term", ...],
  "page": "<active URL>"
}
```

`dt_p` is raw frame deltas in milliseconds. `fps_p` is computed from
those (inverse). Higher dt_p means slower frames; the percentiles
let you separate "always slow" from "occasional stutter."

### Labeled events captured

- `page:<url>` — page navigation
- `tab:term` / `tab:browse` — active tab changed (v0.1.244)
- `altscreen:enter` / `altscreen:exit` — TUI app activated/closed
  the alt-screen mode (v0.1.244)

These events help correlate frame drops with specific actions or
modes. If you see frame drops clustered near `altscreen:enter`,
the slowdown is TUI-related (see terminal performance arc in
`docs/todo.md`).

### Recommended recording protocol

For comparing against Konsole or another terminal:

1. Start the app with `WEB2QML_PROFILE=1`.
2. Wait ~10 seconds with the app idle (gets a clean baseline).
3. Run the same workload in both web2qml and the comparison
   terminal, alternating, with each pass ~30 seconds:
   - Shell: `cat /var/log/journal-export.log`, `seq 1 50000`,
     scroll through a paged file, continuous typing for 30s.
   - **TUI (primary focus per v0.1.241 feedback)**: open helix
     on a 1000-line file with rapid scrolling, htop refreshing
     for 30s with sort toggles, gitui navigating a large repo,
     TUI git log browsers.
4. Quit cleanly so the last window flushes.
5. Share `~/.config/web2qml/profiling.log`.

### Quick analysis

```
# Lines where median frame time > 20ms (= median <50fps)
jq 'select(.dt_p[2] > 20)' ~/.config/web2qml/profiling.log

# Frame stats during alt-screen mode
jq 'select(.events | any(. | startswith("altscreen")))' ~/.config/web2qml/profiling.log

# Events grouped
jq -r '.events[]' ~/.config/web2qml/profiling.log | sort | uniq -c | sort -rn
```

### What to look for

- **Median > 16.7ms sustained**: dropping below 60fps consistently.
  Look at events in the same window to find correlated activity.
- **p75 or max spikes**: occasional long frames. Could be GC,
  page render, or compositor stalls.
- **TUI workload vs shell workload comparison**: per the terminal
  performance arc, this is the primary signal for whether the
  slowdown is regime-specific (TUI-only) or general.

## Voice runtime issues

(reserved for future content)

## Reading-aloud not working

(reserved for future content)
