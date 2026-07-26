"""path.py -- pathing options over the world grid.

Grid A* across walkable cells (solid obstacles block), point-to-point routing, a
POI-to-POI route, and a greedy "patrol tour" that orders POIs into a sensible loop. All
agnostic: the host game feeds waypoints to its own mover. Also exposes `navigable` (can
you actually get from A to B) which the optimiser uses as a hard constraint.
"""
from __future__ import annotations
import heapq, math
from .score import _blocked_grid


def _cell_of(world, x, y):
    g = world.grid
    gx = int((x + world.size) / (2 * world.size) * (g - 1))
    gy = int((y + world.size) / (2 * world.size) * (g - 1))
    return max(0, min(g - 1, gx)), max(0, min(g - 1, gy))


def astar(world, start_xy, goal_xy, blk=None):
    """A* over 8-connected walkable cells. Returns a list of world (x,y) waypoints."""
    g = world.grid
    if blk is None:
        blk = _blocked_grid(world)
    s = _cell_of(world, *start_xy); t = _cell_of(world, *goal_xy)

    def nudge(c):  # if a cell is blocked, snap to the nearest open one
        if not blk[c[1]][c[0]]:
            return c
        for r in range(1, 6):
            for dj in range(-r, r + 1):
                for di in range(-r, r + 1):
                    ni, nj = c[0] + di, c[1] + dj
                    if 0 <= ni < g and 0 <= nj < g and not blk[nj][ni]:
                        return (ni, nj)
        return c
    s = nudge(s); t = nudge(t)

    def h(a, b):
        return math.hypot(a[0] - b[0], a[1] - b[1])
    openq = [(h(s, t), 0.0, s, None)]
    came = {}; gscore = {s: 0.0}
    while openq:
        _, gc, cur, parent = heapq.heappop(openq)
        if cur in came:
            continue
        came[cur] = parent
        if cur == t:
            break
        ci, cj = cur
        for di in (-1, 0, 1):
            for dj in (-1, 0, 1):
                if di == 0 and dj == 0:
                    continue
                ni, nj = ci + di, cj + dj
                if not (0 <= ni < g and 0 <= nj < g) or blk[nj][ni]:
                    continue
                step = 1.4142 if di and dj else 1.0
                ng = gc + step
                if ng < gscore.get((ni, nj), 1e18):
                    gscore[(ni, nj)] = ng
                    heapq.heappush(openq, (ng + h((ni, nj), t), ng, (ni, nj), cur))
    if t not in came:
        return None
    # reconstruct
    path = []; c = t
    while c is not None:
        path.append(world.to_world(*c)); c = came[c]
    path.reverse()
    return path


def navigable(world, a_xy, b_xy) -> bool:
    return astar(world, a_xy, b_xy) is not None


def route_pois(world, order):
    """Concatenate A* paths through a list of POIs (in the given order)."""
    blk = _blocked_grid(world)
    full = []
    for a, b in zip(order, order[1:]):
        seg = astar(world, (a.x, a.y), (b.x, b.y), blk)
        if seg is None:
            continue
        full.extend(seg if not full else seg[1:])
    return full


def patrol_tour(world, start_xy=(0.0, 0.0)):
    """Greedy nearest-neighbour ordering of the world's POIs into a patrol loop."""
    remaining = list(world.pois)
    order = []
    cx, cy = start_xy
    while remaining:
        remaining.sort(key=lambda p: math.hypot(p.x - cx, p.y - cy))
        nxt = remaining.pop(0)
        order.append(nxt); cx, cy = nxt.x, nxt.y
    return order


# ---- weighted / cost-based pathing: terrain costs, road networks, reachability ----

def cost_at(world, x, y):
    """Traversal cost of a cell: flat open ground is cheap; slopes + roughness cost more.
    A cost-aware planner then prefers natural valleys/flats -- roads that read as designed."""
    return 1.0 + world.slope_at(x, y) * 6.0 + world.roughness_at(x, y) * 3.0


def astar_cost(world, start_xy, goal_xy, blk=None):
    """A* that minimises accumulated TERRAIN COST (not just distance). Returns waypoints."""
    import heapq, math as _m
    g = world.grid
    if blk is None:
        blk = _blocked_grid(world)
    s = _cell_of(world, *start_xy); t = _cell_of(world, *goal_xy)
    if blk[s[1]][s[0]] or blk[t[1]][t[0]]:
        p = astar(world, start_xy, goal_xy, blk)   # fall back to plain A* if endpoints blocked
        return p

    def h(a, b):
        return _m.hypot(a[0] - b[0], a[1] - b[1])
    openq = [(h(s, t), 0.0, s)]; came = {s: None}; gsc = {s: 0.0}
    while openq:
        _, gc, cur = heapq.heappop(openq)
        if cur == t:
            break
        ci, cj = cur
        for di in (-1, 0, 1):
            for dj in (-1, 0, 1):
                if di == 0 and dj == 0:
                    continue
                ni, nj = ci + di, cj + dj
                if not (0 <= ni < g and 0 <= nj < g) or blk[nj][ni]:
                    continue
                wx, wy = world.to_world(ni, nj)
                step = (1.4142 if di and dj else 1.0) * cost_at(world, wx, wy)
                ng = gc + step
                if ng < gsc.get((ni, nj), 1e18):
                    gsc[(ni, nj)] = ng; came[(ni, nj)] = cur
                    heapq.heappush(openq, (ng + h((ni, nj), t), ng, (ni, nj)))
    if t not in came:
        return None
    path = []; c = t
    while c is not None:
        path.append(world.to_world(*c)); c = came[c]
    path.reverse()
    return path


def components(world):
    """Label connected walkable regions. Returns (labels grid, count). Lets you detect
    islands the player can't reach without a bridge/teleport."""
    g = world.grid; blk = _blocked_grid(world)
    lab = [[-1] * g for _ in range(g)]; nc = 0
    for j in range(g):
        for i in range(g):
            if blk[j][i] or lab[j][i] >= 0:
                continue
            stack = [(i, j)]; lab[j][i] = nc
            while stack:
                ci, cj = stack.pop()
                for di, dj in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                    ni, nj = ci + di, cj + dj
                    if 0 <= ni < g and 0 <= nj < g and not blk[nj][ni] and lab[nj][ni] < 0:
                        lab[nj][ni] = nc; stack.append((ni, nj))
            nc += 1
    return lab, nc


def build_roads(world, pois=None):
    """Connect the POIs into a ROAD NETWORK: a minimum-spanning tree over cost-aware paths,
    so every point of interest is reachable by a natural least-cost trail. Returns a list of
    polylines (each a list of (x,y) waypoints)."""
    pts = pois if pois is not None else world.pois
    if len(pts) < 2:
        return []
    blk = _blocked_grid(world)
    n = len(pts)
    # Prim's MST on straight-line distance, realise each kept edge as a cost-aware path
    import math as _m
    in_tree = [False] * n; in_tree[0] = True
    roads = []
    for _ in range(n - 1):
        best = None
        for a in range(n):
            if not in_tree[a]:
                continue
            for b in range(n):
                if in_tree[b]:
                    continue
                d = _m.hypot(pts[a].x - pts[b].x, pts[a].y - pts[b].y)
                if best is None or d < best[0]:
                    best = (d, a, b)
        if best is None:
            break
        _, a, b = best; in_tree[b] = True
        seg = astar_cost(world, (pts[a].x, pts[a].y), (pts[b].x, pts[b].y), blk)
        if seg:
            roads.append(seg)
    return roads
