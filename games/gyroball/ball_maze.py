#!/usr/bin/env python3
"""
Rolling Ball Maze — tilt to steer.

Tilt parsing + the tilt→gravity model now live in the shared sensor-kit crate (built as the
`tilt-source` binary); this reads its "<gx> <gy>" gravity-direction stream from stdin instead of
parsing monitor-sensor itself.

Run with:  monitor-sensor --accel 2>&1 | tilt-source | python3 ball_maze.py
      (or just ./run.sh)
"""

import pygame
import sys
import threading
import math
import random
import time

# ── Physics constants ──────────────────────────────────────────────────────────
GRAVITY       = 0.4   # acceleration per frame applied by tilt
FRICTION      = 0.92  # velocity multiplier each frame
BALL_RADIUS   = 10
WALL_THICKNESS = 14

# ─── Shared sensor state ───────────────────────────────────────────────────────
# The tilt/orientation parsing and the tilt→gravity mapping now live in the shared sensor-kit crate,
# piped in through the `tilt-source` binary (see run.sh). This file just consumes its "<gx> <gy>"
# gravity-direction stream — one line per sensor change, +y = down, unit-scaled.
sensor_lock = threading.Lock()
grav_x, grav_y = 0.0, 0.0   # current gravity direction; scaled by GRAVITY in the physics step

# sensor-kit emits a unit gravity direction (same convention as the keyboard fallback's ±1.0); this
# scales how strongly tilt steers the ball. Tune on-device.
TILT_SENSITIVITY = 1.0


def sensor_reader():
    """Read the shared tilt-source stream ("<gx> <gy>" per change) and update the gravity vector."""
    global grav_x, grav_y
    for line in sys.stdin:
        parts = line.split()
        if len(parts) != 2:
            continue
        try:
            gx, gy = float(parts[0]), float(parts[1])
        except ValueError:
            continue
        with sensor_lock:
            grav_x, grav_y = gx * TILT_SENSITIVITY, gy * TILT_SENSITIVITY


# ─── Maze generation (recursive backtracker) ──────────────────────────────────
def generate_maze(cols, rows):
    """Return a set of wall segments as (x1,y1,x2,y2) tuples (in cell coords)."""
    visited = [[False]*rows for _ in range(cols)]
    walls = set()

    # Start with all walls
    for c in range(cols):
        for r in range(rows):
            if c < cols-1:
                walls.add((c, r, c+1, r, 'h'))   # horizontal passage right
            if r < rows-1:
                walls.add((c, r, c, r+1, 'v'))   # vertical passage down

    def neighbours(c, r):
        nb = []
        if c > 0:      nb.append((c-1, r))
        if c < cols-1: nb.append((c+1, r))
        if r > 0:      nb.append((c, r-1))
        if r < rows-1: nb.append((c, r+1))
        return nb

    stack = [(0, 0)]
    visited[0][0] = True
    removed = set()

    while stack:
        c, r = stack[-1]
        unvisited = [(nc, nr) for nc, nr in neighbours(c, r) if not visited[nc][nr]]
        if unvisited:
            nc, nr = random.choice(unvisited)
            visited[nc][nr] = True
            # Remove wall between (c,r) and (nc,nr)
            if nc == c+1: removed.add((c, r, c+1, r, 'h'))
            elif nc == c-1: removed.add((nc, r, c, r, 'h'))
            elif nr == r+1: removed.add((c, r, c, r+1, 'v'))
            elif nr == r-1: removed.add((c, nr, c, r, 'v'))
            stack.append((nc, nr))
        else:
            stack.pop()

    return walls - removed


def maze_to_rects(maze_walls, cell_w, cell_h, ox, oy, cols, rows):
    """Convert maze wall set into pygame Rects."""
    rects = []
    t = WALL_THICKNESS
    h = t // 2

    # Outer border
    rects.append(pygame.Rect(ox,                    oy,                     cols*cell_w, t))   # top
    rects.append(pygame.Rect(ox,                    oy+rows*cell_h - t,     cols*cell_w, t))   # bottom
    rects.append(pygame.Rect(ox,                    oy,                     t, rows*cell_h))   # left
    rects.append(pygame.Rect(ox+cols*cell_w - t,    oy,                     t, rows*cell_h))   # right

    for (c, r, nc, nr, kind) in maze_walls:
        if kind == 'h':
            # Vertical wall between cell (c,r) and (nc,r)
            x = ox + nc * cell_w
            y = oy + r  * cell_h
            rects.append(pygame.Rect(x - h, y, t, cell_h))
        else:
            # Horizontal wall between cell (c,r) and (c,nr)
            x = ox + c  * cell_w
            y = oy + nr * cell_h
            rects.append(pygame.Rect(x, y - h, cell_w, t))

    return rects


# ─── Main ─────────────────────────────────────────────────────────────────────
def main():
    pygame.init()
    W, H = 900, 700
    screen = pygame.display.set_mode((W, H))
    pygame.display.set_caption("🔴 Ball Maze — tilt your screen!")
    clock = pygame.time.Clock()
    font  = pygame.font.SysFont("monospace", 18)

    # Maze setup
    COLS, ROWS = 12, 9
    CELL_W, CELL_H = 64, 64
    OX = (W - COLS * CELL_W) // 2
    OY = (H - ROWS * CELL_H) // 2

    maze_walls = generate_maze(COLS, ROWS)
    wall_rects = maze_to_rects(maze_walls, CELL_W, CELL_H, OX, OY, COLS, ROWS)

    # Goal: bottom-right cell center
    goal_x = OX + (COLS - 1) * CELL_W + CELL_W // 2
    goal_y = OY + (ROWS - 1) * CELL_H + CELL_H // 2
    GOAL_R = BALL_RADIUS + 4

    # Ball starts at top-left cell center
    bx = float(OX + CELL_W // 2)
    by = float(OY + CELL_H // 2)
    vx, vy = 0.0, 0.0

    won       = False
    win_time  = 0.0

    # Start sensor thread
    t = threading.Thread(target=sensor_reader, daemon=True)
    t.start()

    def reset():
        nonlocal bx, by, vx, vy, won, maze_walls, wall_rects
        maze_walls = generate_maze(COLS, ROWS)
        wall_rects = maze_to_rects(maze_walls, CELL_W, CELL_H, OX, OY, COLS, ROWS)
        bx, by = float(OX + CELL_W // 2), float(OY + CELL_H // 2)
        vx, vy = 0.0, 0.0
        won = False

    # ── Glow surface for ball ──
    glow_surf = pygame.Surface((BALL_RADIUS*6, BALL_RADIUS*6), pygame.SRCALPHA)

    while True:
        dt = clock.tick(60) / 1000.0

        for event in pygame.event.get():
            if event.type == pygame.QUIT:
                pygame.quit(); sys.exit()
            if event.type == pygame.KEYDOWN:
                if event.key == pygame.K_r:
                    reset()
                if event.key == pygame.K_ESCAPE:
                    pygame.quit(); sys.exit()
            # Keyboard fallback for testing without sensor
            if event.type == pygame.KEYDOWN:
                if event.key == pygame.K_LEFT:
                    with sensor_lock:
                        global grav_x, grav_y
                        grav_x, grav_y = -1.0, 0.0
                if event.key == pygame.K_RIGHT:
                    with sensor_lock:
                        grav_x, grav_y = 1.0, 0.0
                if event.key == pygame.K_UP:
                    with sensor_lock:
                        grav_x, grav_y = 0.0, -1.0
                if event.key == pygame.K_DOWN:
                    with sensor_lock:
                        grav_x, grav_y = 0.0, 1.0
            if event.type == pygame.KEYUP:
                if event.key in (pygame.K_LEFT, pygame.K_RIGHT, pygame.K_UP, pygame.K_DOWN):
                    with sensor_lock:
                        grav_x, grav_y = 0.0, 0.0

        if not won:
            with sensor_lock:
                gx, gy = grav_x, grav_y

            vx = (vx + gx * GRAVITY) * FRICTION
            vy = (vy + gy * GRAVITY) * FRICTION

            # Move + collide X
            bx += vx
            ball_rect = pygame.Rect(bx - BALL_RADIUS, by - BALL_RADIUS,
                                    BALL_RADIUS*2, BALL_RADIUS*2)
            for wr in wall_rects:
                if ball_rect.colliderect(wr):
                    if vx > 0:
                        bx = wr.left - BALL_RADIUS
                    else:
                        bx = wr.right + BALL_RADIUS
                    vx = -vx * 0.3
                    ball_rect = pygame.Rect(bx - BALL_RADIUS, by - BALL_RADIUS,
                                            BALL_RADIUS*2, BALL_RADIUS*2)

            # Move + collide Y
            by += vy
            ball_rect = pygame.Rect(bx - BALL_RADIUS, by - BALL_RADIUS,
                                    BALL_RADIUS*2, BALL_RADIUS*2)
            for wr in wall_rects:
                if ball_rect.colliderect(wr):
                    if vy > 0:
                        by = wr.top - BALL_RADIUS
                    else:
                        by = wr.bottom + BALL_RADIUS
                    vy = -vy * 0.3
                    ball_rect = pygame.Rect(bx - BALL_RADIUS, by - BALL_RADIUS,
                                            BALL_RADIUS*2, BALL_RADIUS*2)

            # Win check
            dist = math.hypot(bx - goal_x, by - goal_y)
            if dist < GOAL_R + BALL_RADIUS - 4:
                won = True
                win_time = time.time()

        # ── Draw ──────────────────────────────────────────────────────────────
        screen.fill((8, 8, 12))

        # Grid hint (very dark)
        for c in range(COLS+1):
            x = OX + c * CELL_W
            pygame.draw.line(screen, (20, 20, 28), (x, OY), (x, OY + ROWS*CELL_H))
        for r in range(ROWS+1):
            y = OY + r * CELL_H
            pygame.draw.line(screen, (20, 20, 28), (OX, y), (OX + COLS*CELL_W, y))

        # Goal
        pulse = 0.6 + 0.4 * math.sin(time.time() * 4)
        goal_col = (int(50*pulse), int(220*pulse), int(100*pulse))
        pygame.draw.circle(screen, goal_col, (goal_x, goal_y), GOAL_R)
        pygame.draw.circle(screen, (200, 255, 220), (goal_x, goal_y), GOAL_R // 2)

        # Walls
        for wr in wall_rects:
            pygame.draw.rect(screen, (45, 55, 80), wr)
            pygame.draw.rect(screen, (70, 90, 130), wr, 1)

        # Ball glow
        gr = BALL_RADIUS * 3
        glow_surf.fill((0, 0, 0, 0))
        for i in range(gr, 0, -1):
            alpha = int(80 * (i / gr) ** 2)
            pygame.draw.circle(glow_surf, (255, 40, 40, alpha),
                               (gr, gr), i)
        screen.blit(glow_surf, (int(bx) - gr, int(by) - gr))

        # Ball
        pygame.draw.circle(screen, (220, 30, 30), (int(bx), int(by)), BALL_RADIUS)
        pygame.draw.circle(screen, (255, 160, 160), (int(bx)-3, int(by)-3), BALL_RADIUS//3)

        # HUD
        with sensor_lock:
            gx_disp, gy_disp = grav_x, grav_y
        hud = font.render(f"grav: {gx_disp:+.2f}  {gy_disp:+.2f}   [R]=new maze  [ESC]=quit", True, (80, 100, 140))
        screen.blit(hud, (12, 10))
        arrow_hint = font.render("↑↓←→ arrow keys also work", True, (50, 65, 95))
        screen.blit(arrow_hint, (12, 32))

        if won:
            elapsed = time.time() - win_time
            overlay = pygame.Surface((W, H), pygame.SRCALPHA)
            overlay.fill((0, 200, 100, int(min(180, elapsed * 300))))
            screen.blit(overlay, (0, 0))
            txt = font.render("YOU WIN!  Press R for a new maze", True, (255, 255, 255))
            screen.blit(txt, txt.get_rect(center=(W//2, H//2)))

        pygame.display.flip()


if __name__ == "__main__":
    main()
