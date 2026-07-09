#!/usr/bin/env python3
"""
Rolling Ball Maze - reads monitor-sensor --accel output from stdin
Run with: monitor-sensor --accel 2>&1 | python3 ball_maze.py
"""

import pygame
import sys
import threading
import math
import random
import time

# ── Physics constants ──────────────────────────────────────────────────────────
GRAVITY        = 0.4
FRICTION       = 0.92
BALL_RADIUS    = 10
WALL_THICKNESS = 14

# ── Tilt → gravity vector mapping ─────────────────────────────────────────────
TILT_GRAVITY = {
    "normal":      ( 0.0,  0.0),
    "face-up":     ( 0.0,  0.0),
    "face-down":   ( 0.0,  0.0),
    "tilted-down": ( 0.0,  1.0),
    "tilted-up":   ( 0.0, -1.0),
    "vertical":    ( 0.0,  0.8),
}

# Accelerometer orientation (left-up/right-up corrected — were inverted)
ORIENT_GRAVITY = {
    "normal":    ( 0.0,  0.0),
    "left-up":   ( 1.0,  0.0),   # fixed: was -1
    "right-up":  (-1.0,  0.0),   # fixed: was +1
    "bottom-up": ( 0.0,  1.0),
    "top-up":    ( 0.0, -1.0),
}

# ─── Shared sensor state ───────────────────────────────────────────────────────
sensor_lock = threading.Lock()
grav_x, grav_y = 0.0, 0.0
flip_horiz = False
flip_vert  = False


def sensor_reader():
    global grav_x, grav_y
    tilt_gx, tilt_gy   = 0.0, 0.0
    orient_gx, orient_gy = 0.0, 0.0

    for line in sys.stdin:
        line = line.strip()
        if "Tilt changed:" in line:
            tilt = line.split("Tilt changed:")[-1].strip()
            tilt_gx, tilt_gy = TILT_GRAVITY.get(tilt, (tilt_gx, tilt_gy))
        elif "Accelerometer orientation changed:" in line:
            orient = line.split("Accelerometer orientation changed:")[-1].strip()
            orient_gx, orient_gy = ORIENT_GRAVITY.get(orient, (orient_gx, orient_gy))

        with sensor_lock:
            raw_x = orient_gx + tilt_gx * 0.3
            raw_y = tilt_gy   + orient_gy * 0.3
            grav_x = -raw_x if flip_horiz else raw_x
            grav_y = -raw_y if flip_vert  else raw_y


# ─── Maze generation (recursive backtracker) ──────────────────────────────────
def generate_maze(cols, rows):
    visited = [[False]*rows for _ in range(cols)]
    walls = set()
    for c in range(cols):
        for r in range(rows):
            if c < cols-1: walls.add((c, r, c+1, r, 'h'))
            if r < rows-1: walls.add((c, r, c, r+1, 'v'))

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
            if   nc == c+1: removed.add((c,  r,  c+1, r,  'h'))
            elif nc == c-1: removed.add((nc, r,  c,   r,  'h'))
            elif nr == r+1: removed.add((c,  r,  c,   r+1,'v'))
            elif nr == r-1: removed.add((c,  nr, c,   r,  'v'))
            stack.append((nc, nr))
        else:
            stack.pop()

    return walls - removed


def maze_to_rects(maze_walls, cell_w, cell_h, ox, oy, cols, rows):
    rects = []
    t = WALL_THICKNESS
    h = t // 2
    rects.append(pygame.Rect(ox,                  oy,                   cols*cell_w, t))
    rects.append(pygame.Rect(ox,                  oy+rows*cell_h - t,   cols*cell_w, t))
    rects.append(pygame.Rect(ox,                  oy,                   t, rows*cell_h))
    rects.append(pygame.Rect(ox+cols*cell_w - t,  oy,                   t, rows*cell_h))
    for (c, r, nc, nr, kind) in maze_walls:
        if kind == 'h':
            rects.append(pygame.Rect(ox + nc*cell_w - h, oy + r*cell_h,  t, cell_h))
        else:
            rects.append(pygame.Rect(ox + c*cell_w,  oy + nr*cell_h - h, cell_w, t))
    return rects


# ─── Calibration overlay ──────────────────────────────────────────────────────
def draw_calibration(screen, font_big, font_sm, W, H, fh, fv):
    overlay = pygame.Surface((W, H), pygame.SRCALPHA)
    overlay.fill((0, 0, 0, 210))
    screen.blit(overlay, (0, 0))

    panel_w, panel_h = 440, 290
    px = (W - panel_w) // 2
    py = (H - panel_h) // 2
    pygame.draw.rect(screen, (14, 18, 32), (px, py, panel_w, panel_h), border_radius=14)
    pygame.draw.rect(screen, (60, 80, 150), (px, py, panel_w, panel_h), 2, border_radius=14)

    title = font_big.render("⚙  CALIBRATION", True, (150, 185, 255))
    screen.blit(title, title.get_rect(centerx=W//2, top=py+18))

    pygame.draw.line(screen, (38, 52, 100), (px+20, py+62), (px+panel_w-20, py+62))

    def pill_toggle(label, state, row_y):
        lbl = font_sm.render(label, True, (190, 205, 235))
        screen.blit(lbl, (px + 30, row_y + 4))
        pill_x = px + panel_w - 115
        pill_w, pill_h = 74, 28
        col = (55, 195, 115) if state else (55, 55, 80)
        pygame.draw.rect(screen, col, (pill_x, row_y, pill_w, pill_h), border_radius=14)
        pygame.draw.rect(screen, (120, 145, 200), (pill_x, row_y, pill_w, pill_h), 1, border_radius=14)
        kx = pill_x + pill_w - 23 if state else pill_x + 5
        pygame.draw.circle(screen, (240, 248, 255), (kx + 9, row_y + 14), 11)
        vt = font_sm.render("ON" if state else "OFF", True,
                             (215, 255, 215) if state else (110, 115, 135))
        screen.blit(vt, (pill_x + (9 if state else 18), row_y + 6))

    pill_toggle("Flip Horizontal  [H]", fh, py + 88)
    pill_toggle("Flip Vertical    [V]", fv, py + 140)

    for i, line in enumerate([
        "H / V  — toggle axes instantly",
        "C  or  ESC — close this menu",
        "Arrow keys are unaffected by flip",
    ]):
        s = font_sm.render(line, True, (70, 90, 145))
        screen.blit(s, s.get_rect(centerx=W//2, top=py + 200 + i*26))


# ─── Main ─────────────────────────────────────────────────────────────────────
def main():
    global flip_horiz, flip_vert, grav_x, grav_y

    pygame.init()
    W, H = 900, 700
    screen   = pygame.display.set_mode((W, H))
    pygame.display.set_caption("🔴 Ball Maze — tilt to roll!")
    clock    = pygame.time.Clock()
    font     = pygame.font.SysFont("monospace", 18)
    font_big = pygame.font.SysFont("monospace", 26, bold=True)

    COLS, ROWS = 12, 9
    CELL_W, CELL_H = 64, 64
    OX = (W - COLS * CELL_W) // 2
    OY = (H - ROWS * CELL_H) // 2

    maze_walls = generate_maze(COLS, ROWS)
    wall_rects = maze_to_rects(maze_walls, CELL_W, CELL_H, OX, OY, COLS, ROWS)

    goal_x = OX + (COLS-1)*CELL_W + CELL_W//2
    goal_y = OY + (ROWS-1)*CELL_H + CELL_H//2
    GOAL_R = BALL_RADIUS + 4

    bx = float(OX + CELL_W//2)
    by = float(OY + CELL_H//2)
    vx = vy = 0.0
    won      = False
    win_time = 0.0
    show_cal = False

    thread = threading.Thread(target=sensor_reader, daemon=True)
    thread.start()

    def reset():
        nonlocal bx, by, vx, vy, won, maze_walls, wall_rects
        maze_walls = generate_maze(COLS, ROWS)
        wall_rects = maze_to_rects(maze_walls, CELL_W, CELL_H, OX, OY, COLS, ROWS)
        bx = float(OX + CELL_W//2)
        by = float(OY + CELL_H//2)
        vx = vy = 0.0
        won = False

    glow_surf = pygame.Surface((BALL_RADIUS*6, BALL_RADIUS*6), pygame.SRCALPHA)

    key_vecs = {pygame.K_LEFT: (-1,0), pygame.K_RIGHT: (1,0),
                pygame.K_UP:   (0,-1), pygame.K_DOWN:  (0, 1)}
    keys_held = set()

    while True:
        clock.tick(60)

        for event in pygame.event.get():
            if event.type == pygame.QUIT:
                pygame.quit(); sys.exit()

            if event.type == pygame.KEYDOWN:
                k = event.key
                if k == pygame.K_ESCAPE:
                    if show_cal: show_cal = False
                    else: pygame.quit(); sys.exit()
                elif k == pygame.K_c:
                    show_cal = not show_cal
                elif k == pygame.K_r and not show_cal:
                    reset()
                elif k == pygame.K_h:
                    with sensor_lock:
                        flip_horiz = not flip_horiz
                elif k == pygame.K_v:
                    with sensor_lock:
                        flip_vert = not flip_vert
                elif k in key_vecs:
                    keys_held.add(k)

            if event.type == pygame.KEYUP:
                keys_held.discard(event.key)

        # Arrow-key gravity (raw, no flip applied)
        akx = aky = 0.0
        for k in keys_held:
            dx, dy = key_vecs[k]
            akx += dx; aky += dy

        if not won:
            with sensor_lock:
                sgx, sgy = grav_x, grav_y

            gx = sgx + akx
            gy = sgy + aky

            vx = (vx + gx * GRAVITY) * FRICTION
            vy = (vy + gy * GRAVITY) * FRICTION

            bx += vx
            br = pygame.Rect(bx-BALL_RADIUS, by-BALL_RADIUS, BALL_RADIUS*2, BALL_RADIUS*2)
            for wr in wall_rects:
                if br.colliderect(wr):
                    bx = wr.left - BALL_RADIUS if vx > 0 else wr.right + BALL_RADIUS
                    vx *= -0.3
                    br = pygame.Rect(bx-BALL_RADIUS, by-BALL_RADIUS, BALL_RADIUS*2, BALL_RADIUS*2)

            by += vy
            br = pygame.Rect(bx-BALL_RADIUS, by-BALL_RADIUS, BALL_RADIUS*2, BALL_RADIUS*2)
            for wr in wall_rects:
                if br.colliderect(wr):
                    by = wr.top - BALL_RADIUS if vy > 0 else wr.bottom + BALL_RADIUS
                    vy *= -0.3
                    br = pygame.Rect(bx-BALL_RADIUS, by-BALL_RADIUS, BALL_RADIUS*2, BALL_RADIUS*2)

            if math.hypot(bx - goal_x, by - goal_y) < GOAL_R + BALL_RADIUS - 4:
                won = True
                win_time = time.time()

        # ── Draw ──────────────────────────────────────────────────────────────
        screen.fill((8, 8, 12))

        for c in range(COLS+1):
            pygame.draw.line(screen, (20,20,28), (OX+c*CELL_W, OY), (OX+c*CELL_W, OY+ROWS*CELL_H))
        for r in range(ROWS+1):
            pygame.draw.line(screen, (20,20,28), (OX, OY+r*CELL_H), (OX+COLS*CELL_W, OY+r*CELL_H))

        pulse = 0.6 + 0.4*math.sin(time.time()*4)
        pygame.draw.circle(screen, (int(50*pulse), int(220*pulse), int(100*pulse)), (goal_x, goal_y), GOAL_R)
        pygame.draw.circle(screen, (200, 255, 220), (goal_x, goal_y), GOAL_R//2)

        for wr in wall_rects:
            pygame.draw.rect(screen, (45, 55, 80), wr)
            pygame.draw.rect(screen, (70, 90, 130), wr, 1)

        gr = BALL_RADIUS * 3
        glow_surf.fill((0,0,0,0))
        for i in range(gr, 0, -1):
            pygame.draw.circle(glow_surf, (255,40,40, int(80*(i/gr)**2)), (gr,gr), i)
        screen.blit(glow_surf, (int(bx)-gr, int(by)-gr))
        pygame.draw.circle(screen, (220,30,30),   (int(bx), int(by)), BALL_RADIUS)
        pygame.draw.circle(screen, (255,160,160), (int(bx)-3, int(by)-3), BALL_RADIUS//3)

        with sensor_lock:
            gxd, gyd     = grav_x, grav_y
            fh_cur, fv_cur = flip_horiz, flip_vert

        hud_str = (f"grav: {gxd:+.2f} {gyd:+.2f}  "
                   f"flipH:{'ON ' if fh_cur else 'off'}  "
                   f"flipV:{'ON ' if fv_cur else 'off'}  "
                   f"[C]=cal  [R]=new  [ESC]=quit")
        screen.blit(font.render(hud_str, True, (80,100,140)), (12, 10))

        if won:
            elapsed = time.time() - win_time
            ov = pygame.Surface((W, H), pygame.SRCALPHA)
            ov.fill((0, 200, 100, int(min(180, elapsed*300))))
            screen.blit(ov, (0,0))
            txt = font_big.render("YOU WIN!  Press R for a new maze", True, (255,255,255))
            screen.blit(txt, txt.get_rect(center=(W//2, H//2)))

        if show_cal:
            draw_calibration(screen, font_big, font, W, H, fh_cur, fv_cur)

        pygame.display.flip()


if __name__ == "__main__":
    main()
