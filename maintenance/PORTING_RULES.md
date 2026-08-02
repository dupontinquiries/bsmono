# Porting rules, written after getting both wrong

Two failures, both fair criticism, both worth stating as rules because the
reasoning that produced them felt sensible at the time.

## 1. Do not invent UI structure

I added a `Terminal / Mail / Web / Graph / Archive / Cards` tab bar to voiceterm
so that ported modules could be reached. It compiled, it was tested, and it was
**wrong**: the engine's own `terminal_workspace::PaneKind` says a pane is
`Terminal` or `Web`, and voiceterm is *"native terminal tabs + pane split-tree"*.
Mail and the graph are not siblings of the terminal — they are things a pane can
hold, or overlays.

A tab bar beside a pane tree gives the app **two competing navigation models**,
and the result looks like a different product. Reverted.

**The rule:** when a ported module has nowhere to live, that is information. It
means either the original reaches it some other way (a pane kind, a drawer, a
long-press) or it does not belong in this app. Inventing a container is the one
move that is always wrong, because it cannot be checked against anything.

**How to tell:** if the original's source has no equivalent of the container you
are about to add, do not add it. `PaneKind` had two variants and I added six
tabs; that gap was visible before I wrote a line.

## 2. Wiring to empty data is not functionality

`wiring_check.py` reports a module as wired when the app *references* it. Every
one of these passes that check and shows nothing:

```rust
let topo = Topology::new(&[], &[]);          // an empty graph
let pages: Vec<ArchivedPage> = Vec::new();   // no pages, ever
tracks: Vec::new(),                          // a library with no music
```

A view connected to a hardcoded empty vector is a **connected placeholder**. It
is strictly worse than an unwired module, because the checker now says it is
done.

**The rule:** a surface is wired when it is connected to the app's real state —
the workspace, the session, the catalogue, the roster. If there is no real
source yet, the honest report is "needs a data source", not a green tick.

**The next tool** should check what a view is *fed*, not just what it names.
`wiring_check.py` measures reachability and stops there, which is how 26 dead
modules became 0 dead modules and no new functionality.

## What "feature parity" actually requires

Three things, and I have been counting the first as if it were all three:

1. the behaviour exists and is tested — done, several thousand lines of it;
2. it is reachable through the **original's** navigation, not a new one;
3. it is fed by real data.

Only (1) is finished.
