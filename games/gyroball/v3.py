#!/usr/bin/env python3
"""
Rolling Ball Maze — tilting maze game with LAN multiplayer
Run with: monitor-sensor --accel 2>&1 | python3 ball_maze.py [username]
Or solo:  python3 ball_maze.py [username]
"""

import pygame
import sys
import threading
import math
import random
import time
import socket
import json
import subprocess
import os
import struct

# ── Network config ─────────────────────────────────────────────────────────────
LAN_PORT      = 57321          # UDP port for multiplayer
BROADCAST_IP  = "255.255.255.255"
NET_TICK      = 0.05           # seconds between broadcasts (20 Hz)
PEER_TIMEOUT  = 5.0            # seconds before a peer is considered gone

# ── Physics ────────────────────────────────────────────────────────────────────
GRAVITY        = 0.4
FRICTION       = 0.92
BALL_RADIUS    = 10
WALL_THICKNESS = 14
COLS, ROWS     = 12, 9
CELL_W = CELL_H = 64

# ── Player colours (cycled by join order) ─────────────────────────────────────
PLAYER_COLORS = [
    (220,  50,  50),   # red
    ( 50, 160, 255),   # blue
    ( 60, 220,  80),   # green
    (255, 190,  30),   # yellow
    (200,  80, 255),   # purple
    (255, 120,  30),   # orange
    ( 30, 220, 210),   # cyan
    (255,  80, 160),   # pink
]

# ── Tilt / orientation maps ────────────────────────────────────────────────────
TILT_GRAVITY = {
    "normal":      ( 0.0,  0.0),
    "face-up":     ( 0.0,  0.0),
    "face-down":   ( 0.0,  0.0),
    "tilted-down": ( 0.0,  1.0),
    "tilted-up":   ( 0.0, -1.0),
    "vertical":    ( 0.0,  0.8),
}
ORIENT_GRAVITY = {
    "normal":    ( 0.0,  0.0),
    "left-up":   ( 1.0,  0.0),
    "right-up":  (-1.0,  0.0),
    "bottom-up": ( 0.0,  1.0),
    "top-up":    ( 0.0, -1.0),
}

# ── Shared sensor state ────────────────────────────────────────────────────────
sensor_lock = threading.Lock()
grav_x = grav_y = 0.0
flip_horiz = False
flip_vert  = False


def sensor_reader():
    global grav_x, grav_y
    tgx = tgy = ogx = ogy = 0.0
    for line in sys.stdin:
        line = line.strip()
        if "Tilt changed:" in line:
            tilt = line.split("Tilt changed:")[-1].strip()
            tgx, tgy = TILT_GRAVITY.get(tilt, (tgx, tgy))
        elif "Accelerometer orientation changed:" in line:
            orient = line.split("Accelerometer orientation changed:")[-1].strip()
            ogx, ogy = ORIENT_GRAVITY.get(orient, (ogx, ogy))
        with sensor_lock:
            rx = ogx + tgx * 0.3
            ry = tgy + ogy * 0.3
            grav_x = -rx if flip_horiz else rx
            grav_y = -ry if flip_vert  else ry


# ── Rotation lock helpers ──────────────────────────────────────────────────────
def get_rotation_locked():
    """Return True if auto-rotation is currently disabled."""
    # Try GNOME gsettings first
    try:
        r = subprocess.run(
            ["gsettings", "get",
             "org.gnome.settings-daemon.plugins.orientation", "active"],
            capture_output=True, text=True, timeout=2)
        if r.returncode == 0:
            return r.stdout.strip().lower() == "false"
    except Exception:
        pass
    # Try iio-sensor-proxy inhibit check via systemd (KDE / generic)
    try:
        r = subprocess.run(
            ["monitor-sensor", "--list-inhibitors"],
            capture_output=True, text=True, timeout=2)
        return "ball_maze" in r.stdout
    except Exception:
        pass
    return False


def set_rotation_locked(lock: bool):
    """Enable or disable auto-rotation on the system."""
    errors = []
    # GNOME gsettings
    val = "false" if lock else "true"
    try:
        subprocess.run(
            ["gsettings", "set",
             "org.gnome.settings-daemon.plugins.orientation", "active", val],
            timeout=2)
    except Exception as e:
        errors.append(str(e))
    # iio-sensor-proxy inhibit (non-GNOME)
    if lock:
        try:
            subprocess.Popen(
                ["busctl", "--user", "call",
                 "net.hadess.SensorProxy", "/net/hadess/SensorProxy",
                 "net.hadess.SensorProxy", "ClaimAccelerometer"],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        except Exception:
            pass
    # xrandr fallback — force normal orientation
    if lock:
        try:
            subprocess.run(["xrandr", "-o", "normal"], timeout=2)
        except Exception:
            pass
    return not errors


def detect_rotation_support():
    """Return True if any rotation-lock mechanism seems available."""
    for cmd in [["gsettings"], ["xrandr"], ["busctl"]]:
        try:
            subprocess.run(cmd, capture_output=True, timeout=1)
            return True
        except Exception:
            pass
    return False


# ── Maze generation ────────────────────────────────────────────────────────────
def generate_maze(cols, rows, seed=None):
    if seed is not None:
        random.seed(seed)
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
    rects += [
        pygame.Rect(ox,                  oy,                  cols*cell_w, t),
        pygame.Rect(ox,                  oy+rows*cell_h-t,    cols*cell_w, t),
        pygame.Rect(ox,                  oy,                  t, rows*cell_h),
        pygame.Rect(ox+cols*cell_w-t,    oy,                  t, rows*cell_h),
    ]
    for (c, r, nc, nr, kind) in maze_walls:
        if kind == 'h':
            rects.append(pygame.Rect(ox+nc*cell_w-h, oy+r*cell_h,  t, cell_h))
        else:
            rects.append(pygame.Rect(ox+c*cell_w,  oy+nr*cell_h-h, cell_w, t))
    return rects


# ── Networking ─────────────────────────────────────────────────────────────────
class NetManager:
    """
    UDP broadcast peer discovery + state sync.
    Each packet: JSON with {uid, name, color_idx, x, y, won, maze_seed}
    Host = first player; sends maze_seed so all clients use same maze.
    """
    def __init__(self, username, color_idx):
        self.uid       = f"{socket.gethostname()}_{os.getpid()}"
        self.name      = username
        self.color_idx = color_idx
        self.peers     = {}       # uid → {name, color_idx, x, y, won, last_seen}
        self.maze_seed = None     # set by host or received from host
        self.is_host   = False
        self.lock      = threading.Lock()
        self._running  = True

        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
        self.sock.settimeout(0.1)
        try:
            self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
        except AttributeError:
            pass
        self.sock.bind(("", LAN_PORT))

        # Assign ourselves color index based on join order
        threading.Thread(target=self._recv_loop, daemon=True).start()
        threading.Thread(target=self._send_loop, daemon=True).start()
        threading.Thread(target=self._reap_loop, daemon=True).start()

        # Wait briefly to see if a host exists
        time.sleep(0.4)
        with self.lock:
            self.is_host = (self.maze_seed is None)
            if self.is_host:
                self.maze_seed = random.randint(0, 2**31)

    def send_state(self, x, y, won):
        msg = json.dumps({
            "uid":       self.uid,
            "name":      self.name,
            "color_idx": self.color_idx,
            "x":         round(x, 2),
            "y":         round(y, 2),
            "won":       won,
            "maze_seed": self.maze_seed,
            "is_host":   self.is_host,
        }).encode()
        try:
            self.sock.sendto(msg, (BROADCAST_IP, LAN_PORT))
        except Exception:
            pass

    def _recv_loop(self):
        while self._running:
            try:
                data, addr = self.sock.recvfrom(4096)
                pkt = json.loads(data.decode())
                uid = pkt.get("uid")
                if uid == self.uid:
                    continue
                with self.lock:
                    # Accept maze seed from host
                    if pkt.get("is_host") and self.maze_seed is None:
                        self.maze_seed = pkt["maze_seed"]
                    # Register / update peer
                    existing = self.peers.get(uid, {})
                    cidx = pkt.get("color_idx", 0)
                    # Resolve color conflicts: bump ours if same
                    if cidx == self.color_idx:
                        self.color_idx = (self.color_idx + 1) % len(PLAYER_COLORS)
                    self.peers[uid] = {
                        "name":      pkt.get("name", "?"),
                        "color_idx": cidx,
                        "x":         pkt.get("x", 0),
                        "y":         pkt.get("y", 0),
                        "won":       pkt.get("won", False),
                        "last_seen": time.time(),
                    }
            except (socket.timeout, json.JSONDecodeError, OSError):
                pass

    def _send_loop(self):
        # Placeholder — actual sends called from game loop for fresh coords
        while self._running:
            time.sleep(NET_TICK)

    def _reap_loop(self):
        while self._running:
            now = time.time()
            with self.lock:
                dead = [uid for uid, p in self.peers.items()
                        if now - p["last_seen"] > PEER_TIMEOUT]
                for uid in dead:
                    del self.peers[uid]
            time.sleep(1.0)

    def get_peers(self):
        with self.lock:
            return dict(self.peers)

    def stop(self):
        self._running = False
        self.sock.close()


# ── Drawing helpers ────────────────────────────────────────────────────────────
def draw_ball(surface, color, x, y, radius, glow_cache):
    gr = radius * 3
    key = color
    if key not in glow_cache:
        gs = pygame.Surface((gr*2, gr*2), pygame.SRCALPHA)
        for i in range(gr, 0, -1):
            a = int(70 * (i/gr)**2)
            pygame.draw.circle(gs, (*color, a), (gr, gr), i)
        glow_cache[key] = gs
    surface.blit(glow_cache[key], (int(x)-gr, int(y)-gr))
    pygame.draw.circle(surface, color, (int(x), int(y)), radius)
    # specular highlight
    hi = tuple(min(255, c+120) for c in color)
    pygame.draw.circle(surface, hi, (int(x)-radius//3, int(y)-radius//3), radius//3)


def draw_label(surface, font, text, cx, cy, color):
    surf = font.render(text, True, color)
    r    = surf.get_rect(centerx=int(cx), bottom=int(cy) - BALL_RADIUS - 4)
    # shadow
    sh = font.render(text, True, (0, 0, 0))
    surface.blit(sh, (r.x+1, r.y+1))
    surface.blit(surf, r)


def draw_panel(surface, x, y, w, h, title, font_big, font_sm, rows_content):
    pygame.draw.rect(surface, (14, 18, 34), (x, y, w, h), border_radius=14)
    pygame.draw.rect(surface, (55, 75, 145), (x, y, w, h), 2, border_radius=14)
    t = font_big.render(title, True, (145, 180, 255))
    surface.blit(t, t.get_rect(centerx=x+w//2, top=y+16))
    pygame.draw.line(surface, (38, 52, 100), (x+18, y+58), (x+w-18, y+58))
    return y + 70   # content start y


def pill_toggle_draw(surface, font, label, state, lx, y, pill_x):
    lbl = font.render(label, True, (185, 205, 235))
    surface.blit(lbl, (lx, y + 5))
    pw, ph = 74, 28
    col = (55, 195, 115) if state else (50, 50, 78)
    pygame.draw.rect(surface, col, (pill_x, y, pw, ph), border_radius=14)
    pygame.draw.rect(surface, (110, 140, 200), (pill_x, y, pw, ph), 1, border_radius=14)
    kx = pill_x + pw - 23 if state else pill_x + 5
    pygame.draw.circle(surface, (240, 248, 255), (kx+9, y+14), 11)
    vt = font.render("ON" if state else "OFF", True,
                     (210, 255, 210) if state else (105, 110, 130))
    surface.blit(vt, (pill_x + (9 if state else 19), y+6))


def button_draw(surface, font, label, rect, hover=False, active=False):
    col   = (80, 120, 200) if active else ((55, 70, 110) if hover else (35, 45, 75))
    border= (130, 170, 255) if hover else (70, 90, 150)
    pygame.draw.rect(surface, col,    rect, border_radius=8)
    pygame.draw.rect(surface, border, rect, 1, border_radius=8)
    t = font.render(label, True, (220, 230, 255))
    surface.blit(t, t.get_rect(center=rect.center))
    return rect


# ── Calibration panel ──────────────────────────────────────────────────────────
def draw_calibration(surface, W, H, font_big, font_sm,
                     fh, fv, rot_locked, rot_supported,
                     mouse_pos, click):
    overlay = pygame.Surface((W, H), pygame.SRCALPHA)
    overlay.fill((0, 0, 0, 215))
    surface.blit(overlay, (0, 0))

    pw, ph = 460, 370 if rot_supported else 310
    px = (W - pw) // 2
    py = (H - ph) // 2

    cy = draw_panel(surface, px, py, pw, ph, "⚙  CALIBRATION",
                    font_big, font_sm, [])

    lx      = px + 30
    pill_x  = px + pw - 115

    pill_toggle_draw(surface, font_sm, "Flip Horizontal  [H]", fh, lx, cy,      pill_x)
    pill_toggle_draw(surface, font_sm, "Flip Vertical    [V]", fv, lx, cy + 52, pill_x)

    lock_btn_rect = None
    if rot_supported:
        pygame.draw.line(surface, (35, 48, 95),
                         (px+18, cy+105), (px+pw-18, cy+105))
        lock_label = ("🔒 Rotation Locked" if rot_locked
                      else "🔓 Lock Screen Rotation")
        lock_btn_rect = pygame.Rect(px + pw//2 - 140, cy + 120, 280, 38)
        hover = lock_btn_rect.collidepoint(mouse_pos)
        button_draw(surface, font_sm, lock_label, lock_btn_rect,
                    hover=hover, active=rot_locked)

    # hints
    hint_y = py + ph - 95
    pygame.draw.line(surface, (35, 48, 95), (px+18, hint_y-8), (px+pw-18, hint_y-8))
    for i, line in enumerate([
        "H / V — toggle axes    C or ESC — close",
        "Arrow keys are unaffected by flip settings",
    ]):
        s = font_sm.render(line, True, (65, 85, 140))
        surface.blit(s, s.get_rect(centerx=W//2, top=hint_y + i*26))

    clicked_lock = (click and lock_btn_rect is not None
                    and lock_btn_rect.collidepoint(mouse_pos))
    return clicked_lock


# ── Username prompt (shown on first launch) ───────────────────────────────────
def username_prompt(screen, clock, font_big, font_sm, W, H):
    name   = ""
    cursor = True
    ct     = time.time()
    while True:
        clock.tick(30)
        for ev in pygame.event.get():
            if ev.type == pygame.QUIT:
                pygame.quit(); sys.exit()
            if ev.type == pygame.KEYDOWN:
                if ev.key == pygame.K_RETURN and name.strip():
                    return name.strip()[:16]
                elif ev.key == pygame.K_BACKSPACE:
                    name = name[:-1]
                elif ev.unicode and len(name) < 16 and ev.unicode.isprintable():
                    name += ev.unicode
            # if ev.type == pygame.VIDEORESIZE:
            #     W, H = ev.w, ev.h
            #     screen = pygame.display.set_mode((W, H),
            #         pygame.RESIZABLE | pygame.DOUBLEBUF)

        if time.time() - ct > 0.5:
            cursor = not cursor
            ct = time.time()

        screen.fill((8, 8, 14))
        pw, ph = 420, 200
        px, py = (W-pw)//2, (H-ph)//2
        pygame.draw.rect(screen, (14, 18, 34), (px, py, pw, ph), border_radius=14)
        pygame.draw.rect(screen, (60, 80, 150), (px, py, pw, ph), 2, border_radius=14)

        t1 = font_big.render("🔴 Ball Maze", True, (200, 80, 80))
        screen.blit(t1, t1.get_rect(centerx=W//2, top=py+20))
        t2 = font_sm.render("Enter your username:", True, (150, 170, 220))
        screen.blit(t2, t2.get_rect(centerx=W//2, top=py+70))

        box = pygame.Rect(px+40, py+105, pw-80, 36)
        pygame.draw.rect(screen, (25, 30, 50), box, border_radius=6)
        pygame.draw.rect(screen, (70, 100, 180), box, 1, border_radius=6)
        disp = name + ("|" if cursor else " ")
        nt = font_sm.render(disp, True, (220, 230, 255))
        screen.blit(nt, nt.get_rect(midleft=(box.x+10, box.centery)))

        hint = font_sm.render("Press ENTER to play", True, (60, 80, 130))
        screen.blit(hint, hint.get_rect(centerx=W//2, top=py+158))
        pygame.display.flip()


# ── Main ──────────────────────────────────────────────────────────────────────
def main():
    global flip_horiz, flip_vert, grav_x, grav_y

    pygame.init()
    W, H  = 1000, 750
    flags = pygame.RESIZABLE | pygame.DOUBLEBUF
    screen = pygame.display.set_mode((W, H), flags)
    pygame.display.set_caption("🔴 Ball Maze — LAN Multiplayer")
    clock    = pygame.time.Clock()
    font     = pygame.font.SysFont("monospace", 17)
    font_big = pygame.font.SysFont("monospace", 24, bold=True)
    font_lbl = pygame.font.SysFont("monospace", 14)

    # Username
    cli_name = sys.argv[1] if len(sys.argv) > 1 else ""
    if not cli_name:
        cli_name = username_prompt(screen, clock, font_big, font, W, H)

    # Network (assigns color index)
    color_idx = random.randint(0, len(PLAYER_COLORS)-1)
    net = NetManager(cli_name, color_idx)
    my_color = PLAYER_COLORS[net.color_idx]

    # Wait for maze seed from host (or we are host)
    deadline = time.time() + 1.0
    while net.maze_seed is None and time.time() < deadline:
        time.sleep(0.05)

    maze_seed  = net.maze_seed or random.randint(0, 2**31)
    maze_walls = generate_maze(COLS, ROWS, seed=maze_seed)

    # Rotation lock
    rot_supported = detect_rotation_support()
    rot_locked    = False

    # Sensor thread
    sensor_thread = threading.Thread(target=sensor_reader, daemon=True)
    sensor_thread.start()

    # Glow cache shared across balls
    glow_cache = {}

    def compute_offsets():
        ox = (W - COLS*CELL_W) // 2
        oy = (H - ROWS*CELL_H) // 2
        return ox, oy

    OX, OY = compute_offsets()
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

    key_vecs  = {pygame.K_LEFT:(-1,0), pygame.K_RIGHT:(1,0),
                 pygame.K_UP:(0,-1),   pygame.K_DOWN:(0,1)}
    keys_held = set()

    net_timer = 0.0
    last_click = False

    def reset():
        nonlocal bx, by, vx, vy, won, maze_walls, wall_rects
        nonlocal OX, OY, goal_x, goal_y, maze_seed
        maze_seed = random.randint(0, 2**31)
        net.maze_seed = maze_seed
        maze_walls = generate_maze(COLS, ROWS, seed=maze_seed)
        OX, OY = compute_offsets()
        wall_rects = maze_to_rects(maze_walls, CELL_W, CELL_H, OX, OY, COLS, ROWS)
        goal_x = OX + (COLS-1)*CELL_W + CELL_W//2
        goal_y = OY + (ROWS-1)*CELL_H + CELL_H//2
        bx = float(OX + CELL_W//2)
        by = float(OY + CELL_H//2)
        vx = vy = 0.0
        won = False

    # ─────────────────────────────────────────────────────────────────────────
    while True:
        dt = clock.tick(60) / 1000.0
        net_timer += dt

        mouse_pos = pygame.mouse.get_pos()
        clicked   = False

        for event in pygame.event.get():
            if event.type == pygame.QUIT:
                net.stop(); pygame.quit(); sys.exit()

            # if event.type == pygame.VIDEORESIZE:
            #     W, H = event.w, event.h
            #     screen = pygame.display.set_mode((W, H), flags)
            #     OX, OY = compute_offsets()
            #     wall_rects = maze_to_rects(maze_walls, CELL_W, CELL_H, OX, OY, COLS, ROWS)
            #     goal_x = OX + (COLS-1)*CELL_W + CELL_W//2
            #     goal_y = OY + (ROWS-1)*CELL_H + CELL_H//2

            if event.type == pygame.MOUSEBUTTONDOWN and event.button == 1:
                clicked = True

            if event.type == pygame.KEYDOWN:
                k = event.key
                if k == pygame.K_ESCAPE:
                    if show_cal: show_cal = False
                    else: net.stop(); pygame.quit(); sys.exit()
                elif k == pygame.K_c:
                    show_cal = not show_cal
                elif k == pygame.K_r and not show_cal:
                    reset()
                elif k == pygame.K_h:
                    with sensor_lock: flip_horiz = not flip_horiz
                elif k == pygame.K_v:
                    with sensor_lock: flip_vert  = not flip_vert
                elif k in key_vecs:
                    keys_held.add(k)

            if event.type == pygame.KEYUP:
                keys_held.discard(event.key)

        # Arrow key gravity
        akx = aky = 0.0
        for k in keys_held:
            dx, dy = key_vecs[k]; akx += dx; aky += dy

        # Physics
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
                won = True; win_time = time.time()

        # Broadcast our state
        if net_timer >= NET_TICK:
            net_timer = 0.0
            net.color_idx = net.color_idx   # may have been bumped by recv
            my_color = PLAYER_COLORS[net.color_idx]
            net.send_state(bx - OX, by - OY, won)   # send relative to maze origin

        # ── Draw ─────────────────────────────────────────────────────────────
        screen.fill((8, 8, 13))

        # Grid hint
        for c in range(COLS+1):
            pygame.draw.line(screen, (18,20,28),
                             (OX+c*CELL_W, OY), (OX+c*CELL_W, OY+ROWS*CELL_H))
        for r in range(ROWS+1):
            pygame.draw.line(screen, (18,20,28),
                             (OX, OY+r*CELL_H), (OX+COLS*CELL_W, OY+r*CELL_H))

        # Goal
        pulse = 0.6 + 0.4*math.sin(time.time()*4)
        pygame.draw.circle(screen, (int(50*pulse),int(220*pulse),int(100*pulse)),
                           (goal_x, goal_y), GOAL_R)
        pygame.draw.circle(screen, (200,255,220), (goal_x, goal_y), GOAL_R//2)

        # Walls
        for wr in wall_rects:
            pygame.draw.rect(screen, (42, 52, 78), wr)
            pygame.draw.rect(screen, (65, 85, 128), wr, 1)

        # Remote players
        peers = net.get_peers()
        for uid, p in peers.items():
            pcol = PLAYER_COLORS[p["color_idx"] % len(PLAYER_COLORS)]
            px_  = OX + p["x"]
            py_  = OY + p["y"]
            draw_ball(screen, pcol, px_, py_, BALL_RADIUS, glow_cache)
            draw_label(screen, font_lbl, p["name"], px_, py_, pcol)
            if p["won"]:
                w = font_lbl.render("🏆", True, (255, 220, 50))
                screen.blit(w, w.get_rect(centerx=int(px_), top=int(py_)+BALL_RADIUS+2))

        # Local ball
        draw_ball(screen, my_color, bx, by, BALL_RADIUS, glow_cache)
        draw_label(screen, font_lbl, cli_name, bx, by, my_color)

        # HUD
        peer_count = len(peers)
        with sensor_lock:
            gxd, gyd     = grav_x, grav_y
            fh_cur, fv_cur = flip_horiz, flip_vert
        host_str = "HOST" if net.is_host else "CLIENT"
        hud = (f"grav:{gxd:+.2f},{gyd:+.2f}  "
               f"flipH:{'ON' if fh_cur else 'off'}  flipV:{'ON' if fv_cur else 'off'}  "
               f"[{host_str}] peers:{peer_count}  port:{LAN_PORT}  "
               f"[C]=cal  [R]=new  [ESC]=quit")
        screen.blit(font.render(hud, True, (70,90,135)), (10, 8))

        # Win overlay
        if won:
            elapsed = time.time() - win_time
            ov = pygame.Surface((W, H), pygame.SRCALPHA)
            ov.fill((0, 200, 100, int(min(180, elapsed*300))))
            screen.blit(ov, (0,0))
            txt = font_big.render("YOU WIN! 🏆  Press R for a new maze", True, (255,255,255))
            screen.blit(txt, txt.get_rect(center=(W//2, H//2)))

        # Calibration overlay
        if show_cal:
            lock_clicked = draw_calibration(
                screen, W, H, font_big, font,
                fh_cur, fv_cur, rot_locked, rot_supported,
                mouse_pos, clicked)
            if lock_clicked:
                rot_locked = not rot_locked
                set_rotation_locked(rot_locked)

        pygame.display.flip()


if __name__ == "__main__":
    main()
