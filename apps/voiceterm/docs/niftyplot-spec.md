# Niftyplot Format Specification (Draft)

Status: **DRAFT**, pre-implementation. Subject to revision until
the first kind is implemented (target: v0.1.344+).

Niftyplot is the plain-text diagram format used by web2qml. It
lives in fenced code blocks marked `niftyplot` in markdown files,
or in standalone `.niftyplot` files for hand-authored diagrams.
A CLI tool (`niftyplot transpile`) converts to/from Mermaid for
interop with other ecosystems.

## Design principles

1. **Human readable first.** A user opening a `.niftyplot` block
   in helix should immediately understand the diagram WITHOUT
   running it through a renderer. Pixel coordinates appear only
   when they have meaning (hardware mm, gantt dates) — never for
   "where does this node sit on the canvas."

2. **One fence, one source of truth.** No sidecar files, no
   parallel comments inside a Mermaid block. The entire diagram
   description lives in ONE niftyplot block. Layout decisions are
   part of the diagram, not metadata about it.

3. **Geometric constraints over coordinates.** When the user
   wants three nodes in a triangle, they write
   `isosceles(A, B, C)` — not three pin coordinates. The
   constraint survives resizing, theming, and re-layout passes
   because the SHAPE is what's persisted, not the pixels.

4. **Relationships over absolutes.** When structure is stacked,
   nested, or perpendicular, authors describe the RELATIONSHIP
   (`parallel(top, middle, bottom)`, `intersect(side, top) at
   90°`, `between(M, A, B)`) and the renderer extrapolates the
   geometry. No integer depth assignments, no manual
   renumbering when inserting a layer. Tilt is derived from the
   relationships, not declared as a magic number.

5. **Five diagram kinds, dispatched by `%kind:` directive.** Each
   kind has its own grammar tuned to its domain:
   - `graph` — nodes + edges (was: Mermaid graph TD)
   - `gantt` — tasks with dates and dependencies
   - `hardware` — components, pins, traces, clearances
   - `sequence` — participants, messages, activations
   - `state` — states, transitions, conditions

6. **Permissive parsing.** Unknown directives render the diagram
   anyway, but mark affected nodes visually (warning tint) and
   emit a copy-able debug line under the diagram showing what
   was unrecognized.

## File layout

### As a fence block

````markdown
```niftyplot
%kind: graph
A → B → C
```
````

### As a standalone file

```
$ cat flow.niftyplot
%kind: graph
A → B → C
```

The format inside the fence and inside the standalone file is
identical. The fence form is preferred for content where the
diagram is part of a larger document; the standalone form is
preferred for authored diagrams that get reused (e.g. embedded
into multiple docs).

## Top-level directives

Any line beginning with `%` is a directive. Directives can
appear anywhere in the block; some are required, some are
optional, some are kind-specific.

| Directive | Applies to | Required? | Description |
|-----------|------------|-----------|-------------|
| `%kind:` | all | **yes** | One of: graph, gantt, hardware, sequence, state |
| `%title:` | all | no | Display title for the diagram |
| `%tilt:` | all | no | Explicit 3D tilt angle in degrees (0–75). Usually OMITTED — the renderer derives tilt from plane relationships (`parallel`, `facing`, etc.). Set this only when you want to override the derived angle |
| `%zoom:` | all | no | Initial zoom factor (e.g. `1.4`) |
| `%theme:` | all | no | Override theme: `light`, `dark`, `sepia`, `high-contrast` |
| `%layout` | graph | no | Begins a layout section (see Geometric Constraints) |
| `plane <name> { ... }` | all | no | Declares a named plane (subgraph in 3D space). See Planes and spatial relationships |
| `view plane <name>` | all | no | Show only this plane (and any anchors crossing it). Helpful for inspecting one slice of a multi-plane diagram |

Unknown directives produce a warning (rendered diagram is still
shown; affected nodes get a yellow tint and the warning text is
under the diagram, copy-able).

## Planes and spatial relationships

Many diagrams have structure that begs for Z-stacking: a PCB has
copper layers, an architecture has tiers, a sequence has nested
calls. Rather than expose this as integer "depth" assignments
(robotic, requires renumbering when you insert), niftyplot lets
you DESCRIBE the spatial relationships between regions and the
renderer extrapolates the geometry.

The unit of spatial structure is a **plane**: a flat region
that can hold nodes, traces, and other diagram content. A
plane is essentially a subgraph with a position in 3D space.
Authors don't assign coordinates to planes — they describe how
planes relate to each other.

### The vocabulary

| Relationship | Reads as | What the renderer does |
|---|---|---|
| `parallel(A, B, C)` | Three planes stacked along their shared normal | Layered stack with default spacing |
| `parallel(A, B, C) close` | Tightly stacked | Smaller Z-gap between planes |
| `parallel(A, B, C) exploded` | Clearly separated | Wider Z-gap between planes |
| `intersect(top, side)` | Two planes meeting along an edge | L-shape, both visible |
| `intersect(top, side) at 90°` | Perpendicular meeting | Right-angle L |
| `intersect(A, B) along <axis>` | Meet at a specific hinge axis | Hinge axis explicit |
| `coplanar(A, B)` | A and B are the same plane | Merged into one rendering |
| `between(M, A, B)` | M sits sandwiched between A and B | M's depth is midway |
| `behind(B, A)` | B sits behind A relative to viewer | A occludes B in flat view; separates under tilt |
| `nested(inner, in: outer)` | inner is contained in outer's region | inner renders at greater depth, occluded view |
| `facing(plane, viewer)` | This plane is the front | Camera angle derived to face this plane |
| `radial(center, A, B, C, …)` | A, B, C arranged around center | Spoke-and-hub in 3D, center plane is the hub |
| `perpendicular(A, B)` | A and B at 90° | Same as `intersect(A, B) at 90°` but no axis named |

A plane is defined by giving it a name and (optionally) a body:

```
plane top_copper {
    chip MCU at (10, 20)
    chip EEPROM at (60, 20)
    trace MCU.SDA → EEPROM.SDA
}

plane ground_plane {
    zone fill copper
}

plane bottom_copper {
    chip USB at (5, 5)
}

parallel(top_copper, ground_plane, bottom_copper)
```

The `parallel(…)` line tells the renderer these three planes
share a normal axis (perpendicular to all of them) and stack in
that order. The renderer assigns internal depths — the author
never types `at depth N`.

### Tilt is derived, not declared

Today's `%tilt: 35°` was a magic number. With planes, tilt
becomes an emergent property of the spatial relationships:

```
parallel(top, signal, ground, bottom) exploded
facing(top, viewer)
```

Reads as: "four parallel planes, separated enough to inspect;
the top one faces the viewer." The renderer derives the camera
angle that makes the top plane front-facing while still showing
the stack depth — typically 25–45° depending on canvas aspect.

Authors who DO want a specific angle can still override:

```
%tilt: 60°
```

…and that value wins (it's the explicit author intent). Without
it, the renderer chooses based on the spatial constraints.

### Intersections extrapolate to constraints

A relationship between two planes carries information about
ALL planes related to them. Example:

```
parallel(top, middle, bottom)
intersect(top, side_panel) at 90°
```

The renderer sees: three layers stacked (so they share a normal
axis), and `side_panel` is perpendicular to `top`. Because `top`
is parallel to `middle` and `bottom`, the renderer infers that
`side_panel` is ALSO perpendicular to `middle` and `bottom`.
The user didn't write `side_panel`'s relationship with the
other two; it was extrapolated.

This is how the format stays natural for humans while still
being mechanically precise. You only write the relationships
that aren't implied by transitivity.

### Per-kind interpretation

The same vocabulary maps to each kind's natural semantics:

| Kind | What `parallel(A, B, C)` typically describes |
|------|----------------------------------------------|
| graph | Architecture tiers — UI / service / data |
| hardware | Physical PCB layers — top copper / signal / ground / bottom |
| sequence | Nested call depths — user-visible / internal / database |
| state | Composite-state regions — parallel substate machines |
| gantt | Swimlanes shown as parallel planes (mostly used flat, but tilt-aware) |

Authors don't need to learn per-kind vocabulary. They write
`parallel`, `intersect`, `between`, etc. once, and the renderer
applies sensible defaults for the chosen kind.

### Anchors: stable cross-plane reference points

Anchors are named points where something crosses from one plane
to another. Hardware authors will recognize these as **vias**
(in fact `via` is the hardware-specific keyword that compiles
to anchor under the hood). For non-hardware kinds, anchors mark
significant handoffs:

```
anchor "GND_via_1" at MCU.GND crossing top_copper → ground_plane
anchor "request_in" at API.handler crossing api_tier → service_tier
```

The `at` clause says where the anchor lives in the source plane.
Forms:
- `at (x, y)` — explicit position in the source plane's coords
- `at <node>` — at a node's center (graph, sequence)
- `at <node>.<port>` — at a named port/pin (hardware)

The `crossing` chain lists the planes traversed, in order:

```
anchor "spine_route" crossing l1 → l2 → l3 → l4
```

For hardware specifically, `crossing all` is shorthand for
"every declared plane in parallel order" — useful for thermal
or through-hole structures.

Anchors are addressable from other constraints, so a later
declaration can reference an anchor by name:

```
anchor "request_in" at API.handler crossing api_tier → service_tier
trace UserSvc.input → "request_in" on service_tier
```

### Cross-plane geometry

The subgraph geometries (`venn`, `nested`, `siblings`, `stack`,
etc.) work on planes too. In fact, `parallel(A, B, C)` IS a
geometry — it lives in the same family as `circle(...)` and
`isosceles(...)`, just applied to planes rather than nodes.

So:

```
stack(top, middle, bottom)         # same as parallel + close
siblings(left_wing, right_wing)    # side-by-side, no Z separation
venn(public_layer, restricted_layer)  # overlapping regions
```

are all valid plane relationships, and authors can mix them
freely with node-level constraints.

### Implicit single plane

If a diagram declares NO `plane` blocks, everything lives in
one implicit plane named `default`. This is the common case for
small diagrams; layered structure only appears when the author
needs it. Tilt at this level is purely visual (no stack to
separate), matching today's behavior.

### Examples per kind

A four-tier architecture diagram in graph kind:

```niftyplot
%kind: graph
%title: "Service stack"

plane edge {
    LB: "Load balancer"
    CDN: "CDN"
}

plane application {
    API: "API gateway"
    Auth: "Auth service"
}

plane domain {
    Order: "Order service"
    Inventory: "Inventory service"
}

plane data {
    DB: cylinder "Postgres"
    Cache: cylinder "Redis"
}

parallel(edge, application, domain, data) exploded
facing(edge, viewer)

LB → API → Auth
API → Order → DB
API → Inventory → DB
Order → Cache

anchor "primary_entry" at LB crossing edge → application
```

Tilt is whatever the renderer picks to satisfy `facing(edge,
viewer)` while showing the four-plane depth. Flat view (if the
user dismisses tilt with the voice command "flatten") shows the
classical "boxes with arrows" architecture diagram.

## Conflict resolution

Authors can write contradictory constraints. The renderer never
fails outright — it renders the largest consistent subset and
warns about whatever it had to drop. Three resolution strategies
handle the three kinds of conflict, in order of strictness.

### Strategy 1: Tier precedence (across-tier)

When the SAME node is addressed by constraints at different
TIERS, the lower-numbered tier wins. The tier shows up in the
keyword the author writes — there's no `!important` syntax.

| Tier | Examples |
|------|----------|
| 0 — Hard pins | `pinned at (x, y)` |
| 1 — Membership | `group "X" includes A`, `nested(outer, inner)` |
| 2 — Relative | `A above B by 60`, `right of B by 80`, `B between A and C` |
| 3 — Geometric (nodes) | `isosceles(A,B,C)`, `circle(...)`, `venn(...)` |
| 4 — Spatial (planes) | `parallel(A,B,C)`, `intersect(...)`, `coplanar(...)` |
| 5 — Auto-layout | (implicit) |

Tier 0 always wins. Tier 5 only fills in unconstrained nodes.
In between, the lower number takes precedence. This is
ORDER-INSENSITIVE — pinning a node at the top of the file or
the bottom gives the same result.

```
%kind: graph
A → B → C
isosceles(A, B, C)       # tier 3
A above B by 60          # tier 2 — wins over isosceles for A's position
pin A at (40, 40)        # tier 0 — wins over both
```

Renders A at (40, 40); B is "below A by 60" satisfied; C is
auto-laid-out (isosceles broken because A is pinned).
Two constraints relaxed; both go in the debug footer.

### Strategy 2: Joint solving (within a tier)

Within a single tier, all constraints touching a node are
solved JOINTLY. The solver finds positions that satisfy as
many constraints as possible; if mathematically over-determined,
the least-constrained (highest weight) constraints are relaxed.

This is order-insensitive too. `circle(A, B, C, D)` and
`line(A, E, F) horizontal` both at tier 3 — A appears in both.
The solver picks a position for A that's on the circle AND on
the horizontal line. If those don't intersect, A's position is
chosen to minimize total constraint error; the worst-violated
constraint gets a warning.

### Strategy 3: Declaration-order with largest-working-subset (categorical contradictions)

When two constraints are LOGICALLY incompatible — not just
over-determined geometrically, but actually contradictory in
their structural claims — neither solving nor tier precedence
helps. Example:

```
parallel(A, B, C)       # claims A, B, C are three distinct planes
coplanar(A, B)          # claims A and B are the same plane
```

A and B can't be both "two distinct planes in a stack" AND "the
same plane." The contradiction is categorical.

For these cases, the rule is:

1. **Apply declarations in source order.** Each one is tested
   against the accumulated state for consistency.
2. **First declaration wins.** When the second arrives and
   contradicts the first, the second is dropped (or partially
   applied, if a subset of it is still consistent).
3. **Largest working subset.** If a multi-element directive
   like `parallel(A, B, C)` can only partially survive (e.g.
   because two of its planes were merged by an earlier
   `coplanar`), the renderer keeps the surviving subset and
   warns about what was lost.

Reversed order, reversed outcome:

```
coplanar(A, B)          # applied first: A and B merge into one effective plane
parallel(A, B, C)       # contradicts: A and B aren't separate. Falls back to parallel(merged_AB, C)
```

The author can read the file linearly and predict the result:
"what comes first wins; later contradictions are dropped or
trimmed."

Warning text examples:

> ⚠ `coplanar(A, B)` dropped — contradicts earlier
>   `parallel(A, B, C)` (line 12). Rendering with A, B, C as
>   three distinct planes. To merge A and B, remove the
>   parallel directive OR reorder so coplanar appears first.

> ⚠ `parallel(A, B, C)` partial — A and B are coplanar (line 11),
>   so only 2 effective planes remain. Rendering as
>   `parallel(merged_AB, C)`.

### Why three strategies

The strategies are listed in order of LOWEST AUTHOR EFFORT to
read correctly:

- **Tier precedence** is the easiest: read keyword → know
  who wins. Used for the most common case (node positioning).
- **Joint solving** requires no per-author reasoning: the
  solver figures it out. Used when constraints are
  mathematically composable.
- **Declaration order** requires the author to read top-to-
  bottom. Used only as a last resort when the conflict is
  CATEGORICAL — meaning no solver can reconcile it because
  the structural claims contradict.

Categorical contradictions are rare in practice; most files
won't hit strategy 3. When they do, the debug footer makes
the cause and resolution explicit.

### Redundant directives

A directive that has no effect (because no other planes/nodes
exist to relate to) is COMPILED with a warning. The renderer
applies the most reasonable interpretation; the warning tells
the author the line was unnecessary.

```
%kind: graph
A → B   # only nodes; no planes declared

facing(default, viewer)   # default plane is the only one — directive is redundant
```

Renders with the implicit default plane facing the viewer
(the only possible interpretation), and warns:

> ℹ `facing(default, viewer)` — diagram has only one plane;
>   directive is redundant. Renderer applied head-on camera
>   anyway. Remove this line OR add more planes for the
>   directive to be meaningful.

Same treatment for `parallel(A)` (single argument), `intersect(A)`
(single argument), `between(M, A, A)` (degenerate), etc.


## Kind 1: graph

The most common case — flowcharts, dependency diagrams, mind
maps. Replaces Mermaid `graph TD` / `flowchart` / etc.

### Nodes

A bare identifier is a node:

```
A
B
C
```

A node with a display label:

```
A: "Start of pipeline"
B: "Processing stage"
```

A node with a shape hint:

```
A as circle: "Start"
B as rect: "Stage"
C as diamond: "Decision"
D as cylinder: "Database"
E as cloud: "External service"
F as hexagon: "Async boundary"
```

Shapes available: `rect` (default), `circle`, `diamond`,
`cylinder`, `cloud`, `hexagon`, `parallelogram`, `note`.

### Edges

ASCII or Unicode arrows. Both are accepted; serializer emits
Unicode.

```
A → B            # Solid arrow
A => B           # Heavy arrow (high weight)
A -.-> B         # Dashed arrow
A ↔ B            # Bidirectional
A -- "label" → B # Edge with label
```

Chains:

```
A → B → C → D
```

Multi-targets (fanout):

```
A → {B, C, D}
{A, B} → C
```

### Subgraphs

Mermaid's subgraphs were limited — no overlap allowed, layout
inside them was awful, and you couldn't have a node belong to
two subgraphs. Niftyplot fixes all three.

```
group "Frontend" {
    A: "React UI"
    B: "API client"
    A → B
}

group "Backend" {
    C: "Server"
    D: "DB"
    C → D
}

B → C  # cross-group edge
```

Subgraphs can OVERLAP (a node can belong to multiple):

```
group "Auth" includes A, B
group "Public" includes A, C
group "Internal" includes B, C
```

Subgraph styling:

```
group "Frontend" {
    %style: dashed border, blue tint, 60% opacity
    A → B
}
```

### Geometric constraints

The headline feature. Instead of pin coordinates, describe the
SHAPE the user wants. Two families: NODE geometries (positioning
individual nodes) and SUBGRAPH geometries (relating groups).

#### Node geometries

```
isosceles(A, B, C)           # equilateral by default
isosceles(A, B, C) ratio 2   # taller than wide
rhombus(N1, N2, N3, N4)
circle(A, B, C, D, E)        # evenly distributed on a circle
arc(A, B, C, D) from 30° to 150°
line(A, B, C, D) horizontal
line(A, B, C, D) vertical
line(A, B, C, D) at 45°
grid(N1..N9) 3x3
grid(rows, cols) where rows={A,B,C} cols={X,Y,Z}
```

Node geometries compose:

```
hub = circle(A, B, C, D, E, F)
spokes = line(out1, out2, out3) right of hub by 200px
```

#### Subgraph geometries

```
venn(auth, public)               # two groups, overlap on shared nodes
venn(auth, public, internal)     # three-way Venn
nested(outer, inner)             # inner strictly inside outer
siblings(g1, g2, g3)             # side-by-side, no overlap
stack(top, middle, bottom)       # vertically stacked
tabs(g1, g2, g3)                 # toggle interaction; one visible at a time
overlay(background, foreground)  # fg drawn on top, slightly inset
```

Venn is the answer to Mermaid's overlapping-subgraph gap. When a
node belongs to multiple groups (`group "X" includes A` and
`group "Y" includes A`), `venn(X, Y)` produces the translucent-
overlapping-region rendering. Without `venn(...)`, the renderer
falls back to its primary-and-secondary-label scheme (the node
visually lives in its first-declared group; secondary memberships
get small chips/badges).

Subgraph geometries also compose:

```
top_row = siblings(frontend, backend, ops)
data_overlay = venn(backend, ops)
```

Here `siblings(frontend, backend, ops)` places three groups
side-by-side; `venn(backend, ops)` causes the right two to
visually overlap on the nodes they share, even though they're
also in the siblings arrangement. This works because the two
constraints address different things: positions are siblings,
overlap zones are venn.


### Layout sections

Pure positional overrides for cases where geometry doesn't fit:

```
%layout
    A: top-left
    B: center
    C: right of B by 60
    D: below B by 80
    E: pinned at (380, 88)   # escape hatch for exact pixels
```

Cardinal positions: `top-left`, `top`, `top-right`, `left`,
`center`, `right`, `bottom-left`, `bottom`, `bottom-right`.

Relative positions: `<direction> of <ref> by <distance>` where
`<direction>` ∈ {above, below, left, right, above-left,
above-right, below-left, below-right} and `<distance>` is in
pixels (default unit) or em / % of canvas.

Pixel escape hatch: `pinned at (x, y)` — the user dragged the
node here, web2qml round-trips the exact position.

### Example: a complete graph

```niftyplot
%kind: graph
%title: "Q4 build pipeline"
%tilt: 35°

A as circle: "Start"
B: "Build"
C: "Test"
D as diamond: "Pass?"
E: "Ship"
F as cloud: "Rollback"

A → B → C → D
D → E
D -.-> F  "if fail"

%layout
    A: top-left
    isosceles(B, C, D)
    E: right of D by 80
    F: below D by 100
```

A user reading this immediately sees: there's a pipeline from
A through D, B/C/D sit in a triangle (the parallel work zone),
E is the success branch and F is the failure branch. The
layout decisions are intentional and durable; resizing the
canvas doesn't break the visual story.

### Example: overlapping subgraphs with venn

Showing the subgraph geometry that addresses Mermaid's
overlap limitation:

```niftyplot
%kind: graph
%title: "Auth surface — Venn view"

A: "Login flow"
B: "Token refresh"
C: "MFA challenge"
D: "Anonymous browse"
E: "Admin override"
F: "Audit log"

group "Auth required" includes A, B, C, E, F
group "Anonymous OK" includes A, D
group "Admin only" includes E, F

venn("Auth required", "Anonymous OK", "Admin only")
```

A renders inside both "Auth required" and "Anonymous OK" (the
overlap zone of those two regions). E and F render inside both
"Auth required" and "Admin only" (a different overlap). D is
only in "Anonymous OK" — outside the auth bubble. The single
`venn(...)` declaration tells the renderer to use overlapping
translucent zones rather than the fallback primary-and-secondary
chip rendering.

Without the `venn(...)` line, the same three groups still
render — but each node visually lives in its FIRST-declared
group and shows small chips for secondary memberships. Both
renderings are correct; `venn` is the explicit opt-in for the
overlap visualization.

## Kind 2: gantt

Tasks with start dates, durations, dependencies, and milestones.
The fundamental unit is a TASK (not an x/y coordinate). The
display layout is derived from dates + dependencies — not stored.

### Tasks

```
design     : 2026-03-01 → 2026-03-14
build      : after design, 3 weeks
test       : after build, 10 days
ship       : 2026-04-21 (milestone)
```

The three time forms:

1. **Absolute range:** `<start> → <end>`. Both ISO dates.
2. **Anchored duration:** `after <task>, <duration>`. The task
   starts when its anchor ends (or, with `parallel with`, the
   same start). Duration in days/weeks/months.
3. **Milestone:** `<date> (milestone)`. A point-in-time event
   with no duration.

### Dependencies

The simplest form is `after <task>`. Multiple anchors:

```
test : after build, after security-review, 10 days
```

Means test starts at MAX(build_end, security-review_end) plus 0
days, lasts 10 days.

Conditional dependencies:

```
ship : after test if test.outcome = pass, else after rollback
```

### Lanes (swimlanes)

```
%lane: engineering
    design
    build
    test
%lane: release
    ship
    announcement
```

Tasks not in any `%lane:` block go to a default lane at the
bottom.

### Today + baselines

```
%today: 2026-03-15
%baseline: 2026-02-01   # plan from this date, compared to now
```

A baseline draws a faded "where we planned to be by now" ghost
behind the actual bars.

### Example: complete gantt

```niftyplot
%kind: gantt
%title: "Q1 product launch"
%today: 2026-03-15

design : 2026-03-01 → 2026-03-14
build  : after design, 3 weeks
test   : after build, 10 days
ship   : after test, milestone

%lane: engineering
    design
    build
    test
%lane: release
    ship
```

## Kind 3: hardware

Components on a board with positions, rotations, named pins, and
traces between them. Coordinate units are MILLIMETERS (the
hardware native unit) — not pixels.

Hardware is the kind where LAYERS are most literal: real PCBs
have multiple copper layers, ground planes, silkscreen, soldermask,
and physical depth. A multi-layer PCB description uses the
top-level `layer` blocks from "Layers and depth" above.

### Components

```
chip MCU at (10, 20) rotated 0°
chip EEPROM at (60, 20) rotated 90°
connector USB at (5, 5)
resistor R1 at (25, 30) (10kΩ)
capacitor C1 at (30, 30) (100nF)
```

Components without an explicit `layer` block go to depth 0 (top
copper), which is what most authors expect for a single-sided
breakout sketch.

### Pins

Pins are declared inside a component block:

```
chip MCU at (10, 20) {
    pin VCC : top-right
    pin GND : bottom-left
    pin SDA : right
    pin SCL : right + 5mm
    pin RST : left + 3mm
}
```

Pin position descriptors: cardinal positions, or `<cardinal> +
<offset>` for offset-from-edge.

### Traces

```
trace MCU.SDA → EEPROM.SDA
trace MCU.SCL → EEPROM.SCL
trace MCU.VCC → EEPROM.VCC width 0.3mm
trace MCU.GND → EEPROM.GND width 0.5mm ground
```

Traces inherit the layer of their source component unless
specified otherwise:

```
trace MCU.GND → EEPROM.GND on "ground plane"
```

### Vias (hardware-specific anchors)

A via is the hardware-specific name for a cross-layer anchor.
The syntax mirrors generic `anchor` (see "Layers and depth"),
but reads more naturally for PCB authors:

```
via "GND_1" at MCU.GND through "top copper" → "ground plane"
via "thermal_1" at (15, 25) through all layers
```

`through all layers` is shorthand for "every declared layer in
order."

### Zones

```
zone fill copper on "ground plane"
zone "keepout" : rect (40, 10) to (50, 30) on "top copper"
```

Zones now live inside (or reference) a specific layer rather
than carrying their own layer hint.

### Example: single-layer breakout

For simple single-sided sketches, no `layer` blocks needed:

```niftyplot
%kind: hardware
%title: "I2C breakout v1"
%units: mm

chip MCU at (10, 20) {
    pin VCC : top-right
    pin GND : bottom-left
    pin SDA : right
    pin SCL : right + 5
}

chip EEPROM at (60, 20) rotated 90° {
    pin VCC : top
    pin GND : bottom
    pin SDA : left
    pin SCL : left + 5
}

trace MCU.SDA → EEPROM.SDA width 0.2
trace MCU.SCL → EEPROM.SCL width 0.2
trace MCU.VCC → EEPROM.VCC width 0.4
trace MCU.GND → EEPROM.GND width 0.6 ground
```

### Example: four-layer PCB

When the design is genuinely multi-layer, declare planes and
describe their relationship:

```niftyplot
%kind: hardware
%title: "Sensor hub, 4-layer"
%units: mm

plane top_copper {
    chip MCU at (10, 20) {
        pin VCC : top-right
        pin GND : bottom-left
        pin SDA : right
        pin SCL : right + 5
    }
    chip SENSOR at (60, 20) rotated 90° {
        pin VCC : top
        pin GND : bottom
        pin SDA : left
        pin SCL : left + 5
    }
    trace MCU.SDA → SENSOR.SDA width 0.2
    trace MCU.SCL → SENSOR.SCL width 0.2
    trace MCU.VCC → SENSOR.VCC width 0.4
}

plane signal_1 {
    # routing on inner layer 1, mostly auto
}

plane ground_plane {
    zone fill copper
}

plane bottom_copper {
    connector USB at (5, 5)
}

parallel(top_copper, signal_1, ground_plane, bottom_copper) close
facing(top_copper, viewer)

via "MCU_GND" at MCU.GND crossing top_copper → ground_plane
via "SENSOR_GND" at SENSOR.GND crossing top_copper → ground_plane
via "USB_SIG" at USB crossing bottom_copper → signal_1
```

The renderer reads `parallel(...) close` and stacks the four
planes tightly (a PCB is thin); `facing(top_copper, viewer)`
makes the top layer front-facing. The user never typed a depth
number or a tilt angle — both are implied by the spatial
description. Vias punch between specific named planes; the
renderer draws small cylinders at each crossing.

If the user wants an exploded view (e.g. to inspect the inner
layers), they swap `close` for `exploded`:

```
parallel(top_copper, signal_1, ground_plane, bottom_copper) exploded
```

Same data, much wider Z separation. No tilt angle to tweak.

## Kind 4: sequence

Participants exchange messages over time. Layout: participants
left-to-right at the top; messages flow top-to-bottom.

### Participants

```
participant User
participant API as "API server"
participant DB as "PostgreSQL"
```

### Messages

```
User → API : POST /login
API → DB : SELECT user
DB → API : row
API → User : 200 OK + token
```

### Activations

Brackets denote when a participant is "active" (busy handling
the request):

```
User → API : POST /login {
    API → DB : SELECT user {
        DB → API : row
    }
    API → User : 200 OK + token
}
```

### Notes

```
note over User, API : "Before this point, no session exists"
note left of DB : "Replicated cluster"
```

## Kind 5: state

States and transitions. Optional positions; default is
hierarchical layout.

### States

```
state Idle
state Active
state Done (final)
state "Error recovery" as recovery
```

### Transitions

```
Idle → Active : on start
Active → Done : on complete
Active → recovery : on failure
recovery → Active : on retry
recovery → Done : after 3 retries
```

### Composite states

```
state Active {
    state Loading
    state Processing
    state Ready
    Loading → Processing → Ready
}
```

The outer state acts as a container; the substates are visible
only when the outer is expanded.

### Entry / exit actions

```
state Active {
    on entry : log "activating"
    on exit  : log "deactivating"
    ...
}
```

## CLI tool: `niftyplot`

A small companion tool, distributed alongside web2qml. Renders
and transpiles standalone `.niftyplot` files.

```
niftyplot render file.niftyplot               # PNG output
niftyplot render --format=svg file.niftyplot  # SVG output
niftyplot transpile --to=mermaid file.niftyplot
niftyplot transpile --to=tikz file.niftyplot
niftyplot transpile --from=mermaid foo.mmd > foo.niftyplot
niftyplot lint file.niftyplot                 # check for warnings
```

The transpiler is BEST-EFFORT: niftyplot has features Mermaid
doesn't (overlapping subgraphs, geometric constraints), so a
round-trip through Mermaid loses information. The tool warns
when info is lost.

## Error handling

Per user direction: **render the diagram even when the parser
encounters something it doesn't understand.** Mark affected
nodes visually (warning tint, typically yellow/orange), and
emit a debug section under the diagram showing what was
unrecognized.

Example: a typo in a geometric constraint:

```
%kind: graph
A → B → C
isoceles(A, B, C)   # typo — should be "isosceles"
```

The diagram renders with auto-layout (no geometric constraint
applied). Below the rendered diagram, a debug section appears:

> ⚠ 1 unrecognized directive
>
> Line 3: `isoceles(A, B, C)` — unknown geometry kind
> "isoceles". Did you mean `isosceles`?
>
> [Copy debug info]

The Copy button puts the full debug section on the clipboard so
the user can paste it into a search / report / chat.

## File extension

`.niftyplot` for standalone files. Fence keyword `niftyplot` for
embedded.

## Comments

`#` to end of line is a comment, anywhere outside a string
literal.

```
A → B   # this edge represents the main flow
%kind: graph   # required
```

## Mode (Mermaid migration)

A `%mode: legacy-mermaid` directive at the top of a block tells
the parser to interpret the contents as Mermaid syntax for that
block only. This lets users migrate documents incrementally:

````
```niftyplot
%mode: legacy-mermaid
graph TD
    A --> B
    B --> C
```
````

The CLI tool can also rewrite `mermaid` fences to `niftyplot`
fences with `%mode: legacy-mermaid` set, making the migration a
one-command sweep.

## Implementation roadmap

This spec is the discipline check before ANY code lands. The
goal: write down every kind's grammar so we can spot tensions
before they become refactor pain.

1. **v0.1.343 — spec** (this document). No implementation.
2. **v0.1.344 — graph kind, parser + renderer + tests.** No
   gantt/hardware/sequence/state yet. Geometric constraints
   for graph only.
3. **v0.1.345 — niftyplot CLI tool, transpile from Mermaid.**
   The migration path lands BEFORE any other kind so users can
   bring their existing diagrams.
4. **v0.1.346 — gantt kind.** Date arithmetic, lanes, baselines.
5. **v0.1.347 — sequence kind.** Activations, notes.
6. **v0.1.348 — state kind.** Composites, transitions.
7. **v0.1.349 — hardware kind.** Components, pins, traces, zones.

Order rationale: graph is the most-used and lowest-risk kind
(directly maps to existing cards-canvas state). Gantt comes
next because it has the most pressing user need ("when's this
shipping"). Hardware is last because it has the most domain-
specific vocabulary AND because the user mentioned wanting to
think more about CAD/hardware before pinning the design.

## Open questions

The spec is a draft. Things that may still change:

- **Geometric constraint solver.** A constraint like
  `isosceles(A, B, C)` needs a real solver — it can't just be
  "set A at (0,0), B at (100,0), C at (50, 87)" because the
  graph may also say A is `right of D by 60`, which constrains
  A's position. We need a small constraint propagation pass.
  Likely: implement geometries as soft constraints with high
  weight; pinned-at-pixels as hard constraints; auto-layout
  fills in unconstrained nodes.
- **Subgraph overlap rendering.** Mermaid avoided this because
  visually showing "this node is in TWO groups" is hard. Options:
  Venn-diagram-style overlapping translucent regions; thin
  colored borders that double up where groups overlap; or
  "primary group + secondary group label" — pick a primary
  visual home, annotate the secondary.
- **Conditional dependencies in gantt.** `if X then A else B`
  needs a scheduling semantics. Both branches drawn ghosted?
  Only the chosen branch? User toggles?
- **Hardware pin offset units.** `right + 5mm` — when the
  component is rotated 90°, does "right" still mean the
  pre-rotation right side of the component, or the post-
  rotation right side (which would be the original top)?
  The pre-rotation interpretation is less confusing for
  authors but harder to render.
- **CLI tool packaging.** Bundled with web2qml binary
  (single static binary, sub-commands)? Or separate
  `niftyplot` crate published independently? Ergonomics
  argue for separate; distribution argues for bundled.
- **Error-tint colors per theme.** Yellow on dark theme is
  fine; on light theme it's washed out. Probably need
  a small per-theme palette: dark → bright orange,
  light → muted red.
- **Where does `%today:` for gantt get its default?** System
  date is obvious but means a diagram looks different at
  different times of day across timezones. Maybe always
  UTC date as default; user can override.
- **Round-trip fidelity vs. mermaid.** If a user has a
  niftyplot diagram with `overlapping subgraphs +
  geometric constraints`, transpiling to mermaid LOSES
  both features. The CLI warns and emits the lossy version;
  do we also preserve the original niftyplot as a comment
  inside the mermaid output, so re-transpiling restores
  fidelity? Probably yes — it's how Pandoc handles
  format-specific features.

## Versioning

The niftyplot format is at version **0.1.0-draft**. Once
the first kind ships, the format moves to **0.1.0**. Until
1.0.0, breaking changes are permitted but documented in
this file under "Migration notes" sections.

## Editing surfaces

The format spec defines how niftyplot blocks read and render.
This section defines how authors EDIT them — by keyboard with
suggestion chips, by voice, and by direct drag manipulation on
the rendered preview.

### Suggestive helper

When the keyboard caret is inside a niftyplot fence, a row of
up to 5 suggestion chips appears below the editor. Chips are
context-aware: what they offer depends on what's around the
caret.

| Caret context | Chip suggestions |
|---|---|
| Empty block | `%kind:` then the five kind names |
| After `%kind:` | `graph`, `gantt`, `hardware`, `sequence`, `state` |
| Empty line in body (graph kind) | `plane <name> { … }`, `<id> → <id>`, `%layout`, `parallel(…)` |
| After typing a node id | `→ <existing>`, `as <shape>`, `: "label"` |
| After `→` | Existing node IDs sorted by proximity (recently typed first) |
| Typing a constraint keyword | Fuzzy-complete from the constraint vocabulary |
| Inside `at` clause | Cardinals: `top-left`, `top`, `center`, `right of <id> by 60`, … |
| After `pin` | Existing identifiers |

Suggestions are deterministic — no LLM, no scoring beyond
"fuzzy prefix match" + "recently used in this file." Tapping
a chip inserts the text at the caret and dismisses the row.

Voice equivalents: "read suggestions" reads the chip labels
aloud; "pick <N>" selects the Nth chip; "complete with
<phrase>" fuzzy-matches the phrase against current chips.

### Live preview of scoped subgraphs

When the caret is inside a `plane { ... }` or `group "X" { ... }`
block, the preview pane scopes to that block. The targeted
subgraph renders at full detail and full opacity; everything
else in the diagram dims to 20% opacity for context.

Detection algorithm: scan backwards from the caret through the
fence body, tracking brace depth, looking for the nearest
unclosed `plane` or `group` declaration. The innermost
unclosed scope is the focus target.

This serves the same purpose as folding in a code editor —
isolate what you're working on without losing where it sits.

Voice equivalents: "focus <name>" scopes to a named plane or
group regardless of caret position. "unfocus" returns to full
view. "focus current" follows the caret-detection algorithm.

### Voice command formats

Voice for editing diagrams uses a structured grammar:

```
<verb> [<subject>] [<modifier>...] [<target>]
```

Where:
- **verb** is one of a small fixed set
- **subject** identifies the diagram object being acted on
- **modifier** is a positional phrase or geometric shape name
- **target** is the other endpoint when applicable

Where possible, the voice verb maps DIRECTLY to a niftyplot
keyword. "Make triangle" emits `isosceles`. "Stack" emits
`parallel`. Users learn one vocabulary that works in both
typing and speaking modes.

#### Verb reference (v0.1.343 set)

| Voice phrasing | Niftyplot effect |
|---|---|
| "add node Alice" | Create node `Alice` |
| "add node Bob above Alice" | Create + `B above A by <default>` |
| "add node Bob below / left of / right of Alice" | Same with directional variants |
| "connect Alice to Bob" | Add edge `Alice → Bob` |
| "connect Alice to Bob with label says hello" | Edge + label |
| "remove node Bob" | Delete node and incident edges |
| "remove edge Alice to Bob" | Delete a specific edge |
| "rename Bob to Charlie" | Rename node |
| "move Alice above Bob" | Add/update relative constraint |
| "make A B C a triangle" | `isosceles(A, B, C)` |
| "make Alice Bob and Charlie a circle" | `circle(A, B, C)` |
| "make A B C a line" | `line(A, B, C) horizontal` |
| "stack top middle bottom" | `parallel(top, middle, bottom)` |
| "tilt thirty degrees" | `%tilt: 30°` |
| "flatten" | tilt → 0 (clears override) |
| "fit graph" / "recenter" | (existing) reset zoom + pan |
| "show only services" | `view plane services` |
| "show everything" | clear view scope |
| "focus services" | Set scope to plane "services" |
| "unfocus" | Clear scope |
| "read suggestions" | Read chip labels aloud |
| "pick first" / "pick second" / "pick <N>" | Select chip N |
| "what's wrong" | Read debug footer aloud |
| "undo" / "redo" | Standard undo/redo of last edit |
| "close graph" / "exit graph" | (existing) close cards canvas |

#### Identifier disambiguation

Node IDs come from the source text. Voice ASR may transcribe
"Alice" but the source has "alice". The matcher is
case-INSENSITIVE for node lookup. For unfamiliar/proper-noun
IDs the user can spell-mode: "add node spell A L I C E" forces
literal characters.

When two nodes have similar IDs ("Alice" and "Alyce"), the
system speaks back the match: "Added connection from Alice to
Bob. Did you mean Alyce?" — and offers an undo chip.

#### Modifier vocabulary

Directional phrases the parser recognizes:

- `above <X>` / `below <X>` / `left of <X>` / `right of <X>`
- `above and to the left of <X>` / similar combinations
- `between <X> and <Y>`
- `near <X>` / `far from <X>` (proxy for stronger/weaker
  force-directed pull)
- `<N> pixels from <X>` (explicit distance)

Shape phrases the parser recognizes (for `make`):

- `triangle` → `isosceles`
- `square` / `diamond` / `rhombus` → `rhombus`
- `circle` / `ring` → `circle`
- `line` → `line`
- `grid` → `grid`
- `stack` / `parallel` / `layered` → `parallel`
- `nested` / `inside` → `nested`
- `venn` / `overlapping` → `venn`

Plane / group nouns (for `make` and `focus`):

- `plane <name>` / `layer <name>` (synonyms)
- `group <name>` / `subgraph <name>` (synonyms)

#### Edge-case handling

- **Ambiguous targets**: "add node above" with no current
  selection — voice prompts: "Above which node?"
- **Missing identifiers**: "connect Alice to" cut off — the
  parser holds the partial command for 3 seconds awaiting
  completion; otherwise discards with a toast "Incomplete:
  connect to whom?"
- **Conflicting voice + typing**: voice command writes to the
  buffer at the current caret position; if the user is also
  typing, the voice insertion happens AFTER the user's next
  enter or after 1.5s of typing silence.

### Drag manipulation

(Already mostly implemented; documenting expectations.)

- Drag a node → pin it to the dragged position. Source file
  gains a `pinned at (x, y)` line for that node.
- Drag a node onto a plane region → reassign membership.
- Drag a node BETWEEN two planes → ambiguous; prompt with
  toast "Move to <plane1> or <plane2>?"
- Long-press a node → context menu (rename, remove, change
  shape, copy id).

