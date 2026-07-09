#!/usr/bin/env python3
"""
Rolling Ball Maze — tilting maze with LAN multiplayer
Run:  monitor-sensor --accel --compass 2>&1 | python3 ball_maze.py [username]
Solo: python3 ball_maze.py [username]
"""

import pygame
import sys, threading, math, random, time, socket, json, subprocess, os

# ── Network ────────────────────────────────────────────────────────────────────
LAN_PORT     = 57321
BROADCAST_IP = "255.255.255.255"
NET_TICK     = 0.05
PEER_TIMEOUT = 3.0

# ── Scale / geometry ───────────────────────────────────────────────────────────
SCALE          = 2
BALL_RADIUS    = 10 * SCALE          # 20
WALL_THICKNESS = 7  * SCALE          # 14 — kept same proportion
GOAL_R         = BALL_RADIUS + 4
COLS, ROWS     = 12, 9
CELL_W = CELL_H = 32 * SCALE         # 64

# ── Physics ────────────────────────────────────────────────────────────────────
GRAVITY  = 0.55
FRICTION = 0.90

# ── Colours ────────────────────────────────────────────────────────────────────
PLAYER_COLORS = [
    (230,  55,  55),  # red
    ( 55, 165, 255),  # blue
    ( 60, 225,  85),  # green
    (255, 195,  35),  # yellow
    (205,  85, 255),  # purple
    (255, 125,  35),  # orange
    ( 35, 225, 215),  # cyan
    (255,  85, 165),  # pink
]

# ── Sensor model (hand-held portrait = neutral) ────────────────────────────────
# ── Sensor model — impulse based ──────────────────────────────────────────────
#
# The tilt sensor emits coarse discrete states. Rather than treating them as
# sustained gravity (which fails for "up" because tilted-up is a tiny window),
# each state-change fires a velocity IMPULSE that gets drained by the game loop.
# Strong friction then slows the ball between kicks so it feels responsive but
# not sloppy.  Rapid tilt events (phone rocking) accumulate impulses naturally.
#
# Run with:  monitor-sensor --accel 2>&1 | python3 ball_maze.py [username]

# Magnitude of each tilt event impulse (tune up/down to taste)
TILT_IMPULSE_MAG  = 5.6
ORIENT_IMPULSE_MAG = 5.6

# Friction when ball has no active tilt — decelerates between impulses
FRICTION_NEUTRAL  = 0.80
# Friction while a tilt is held (keeps ball rolling when phone is tilted)
FRICTION_ACTIVE   = 0.92

# Direction each tilt/orient state kicks the ball
TILT_KICK = {
    "tilted-down": (0.0,  TILT_IMPULSE_MAG),   # top away  → ball rolls down
    "tilted-up":   (0.0, -TILT_IMPULSE_MAG),   # top toward → ball rolls up
}
ORIENT_KICK = {
    "left-up":  ( ORIENT_IMPULSE_MAG, 0.0),
    "right-up": (-ORIENT_IMPULSE_MAG, 0.0),
}

sensor_lock  = threading.Lock()
# Shared impulse state — both threads access via sensor_lock
_sensor = {
    "ix": 0.0,       # accumulated X impulse (drained each frame)
    "iy": 0.0,       # accumulated Y impulse
    "active": False, # True while a directional tilt state is held
}
flip_horiz = flip_vert = False
flip_vert = True


def sensor_reader():
    """
    Emit velocity impulses on each tilt/orientation event.
    Neutral states stop accumulating and set active=False for stronger friction.
    """
    prev_tilt = None

    for line in sys.stdin:
        line = line.strip()

        if "Tilt changed:" in line:
            state = line.split("Tilt changed:")[-1].strip()
            if state != prev_tilt:
                prev_tilt = state
                kick = TILT_KICK.get(state)
                with sensor_lock:
                    if kick:
                        kx, ky = kick
                        _sensor["ix"] += -kx if flip_horiz else kx
                        _sensor["iy"] += -ky if flip_vert  else ky
                        _sensor["active"] = True
                    else:
                        _sensor["active"] = False

        elif "Accelerometer orientation changed:" in line:
            state = line.split("Accelerometer orientation changed:")[-1].strip()
            kick = ORIENT_KICK.get(state)
            if kick:
                with sensor_lock:
                    kx, ky = kick
                    _sensor["ix"] += -kx if flip_horiz else kx
                    _sensor["iy"] += -ky if flip_vert  else ky


# ── Profile image ──────────────────────────────────────────────────────────────
def load_avatar(size=40):
    """Try to load the Linux user's profile picture. Returns a circular Surface or None."""
    username = os.environ.get("USER", os.environ.get("LOGNAME", ""))
    candidates = [
        os.path.expanduser("~/.face"),
        os.path.expanduser("~/.face.icon"),
        f"/var/lib/AccountsService/icons/{username}",
        os.path.expanduser("~/.config/user-dirs/face"),
        # KDE
        os.path.expanduser("~/.local/share/kaidan/accounts/default/avatar.jpg"),
    ]
    # Also scan AccountsService user file for custom path
    try:
        acct = f"/var/lib/AccountsService/users/{username}"
        if os.path.exists(acct):
            for line in open(acct):
                if line.startswith("Icon="):
                    candidates.insert(0, line.split("=",1)[1].strip())
    except Exception:
        pass

    img = None
    for path in candidates:
        if path and os.path.isfile(path):
            try:
                img = pygame.image.load(path).convert_alpha()
                break
            except Exception:
                continue
    if img is None:
        return None

    # Scale to size×size, then cut circular mask
    img = pygame.transform.smoothscale(img, (size, size))
    circle_surf = pygame.Surface((size, size), pygame.SRCALPHA)
    circle_surf.fill((0, 0, 0, 0))
    pygame.draw.circle(circle_surf, (255,255,255,255), (size//2, size//2), size//2)
    # Multiply alpha
    result = pygame.Surface((size, size), pygame.SRCALPHA)
    result.blit(img, (0, 0))
    result.blit(circle_surf, (0, 0), special_flags=pygame.BLEND_RGBA_MIN)
    return result


# ── Rotation lock ──────────────────────────────────────────────────────────────
def set_rotation_locked(lock: bool):
    val = "false" if lock else "true"
    try:
        subprocess.run(["gsettings","set",
            "org.gnome.settings-daemon.plugins.orientation","active",val], timeout=2)
    except Exception: pass
    if lock:
        try: subprocess.run(["xrandr","-o","normal"], timeout=2)
        except Exception: pass

def detect_rotation_support():
    for cmd in [["gsettings"],["xrandr"],["busctl"]]:
        try: subprocess.run(cmd, capture_output=True, timeout=1); return True
        except Exception: pass
    return False


# ── Maze ───────────────────────────────────────────────────────────────────────
def generate_maze(cols, rows, seed=None):
    if seed is not None:
        random.seed(seed)
    visited = [[False]*rows for _ in range(cols)]
    walls = set()
    for c in range(cols):
        for r in range(rows):
            if c < cols-1: walls.add((c,r,c+1,r,'h'))
            if r < rows-1: walls.add((c,r,c,r+1,'v'))
    def nb(c,r):
        n=[]
        if c>0:      n.append((c-1,r))
        if c<cols-1: n.append((c+1,r))
        if r>0:      n.append((c,r-1))
        if r<rows-1: n.append((c,r+1))
        return n
    stack=[(0,0)]; visited[0][0]=True; removed=set()
    while stack:
        c,r=stack[-1]
        uv=[(nc,nr) for nc,nr in nb(c,r) if not visited[nc][nr]]
        if uv:
            nc,nr=random.choice(uv); visited[nc][nr]=True
            if   nc==c+1: removed.add((c, r, c+1,r, 'h'))
            elif nc==c-1: removed.add((nc,r, c,  r, 'h'))
            elif nr==r+1: removed.add((c, r, c,  r+1,'v'))
            elif nr==r-1: removed.add((c, nr,c,  r, 'v'))
            stack.append((nc,nr))
        else: stack.pop()
    return walls-removed

def maze_to_rects(maze_walls, cw, ch, ox, oy, cols, rows):
    t=WALL_THICKNESS; h=t//2
    rects=[
        pygame.Rect(ox,           oy,           cols*cw, t),
        pygame.Rect(ox,           oy+rows*ch-t, cols*cw, t),
        pygame.Rect(ox,           oy,           t, rows*ch),
        pygame.Rect(ox+cols*cw-t, oy,           t, rows*ch),
    ]
    for (c,r,nc,nr,kind) in maze_walls:
        if kind=='h': rects.append(pygame.Rect(ox+nc*cw-h, oy+r*ch,  t, ch))
        else:         rects.append(pygame.Rect(ox+c*cw,  oy+nr*ch-h, cw, t))
    return rects


# ── Network ────────────────────────────────────────────────────────────────────
class NetManager:
    def __init__(self, username, color_idx):
        self.uid=f"{socket.gethostname()}_{os.getpid()}"
        self.name=username; self.color_idx=color_idx
        self.peers={}; self.maze_seed=None; self.is_host=False
        self.lock=threading.Lock(); self._running=True
        self.sock=socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
        self.sock.settimeout(0.1)
        try: self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
        except AttributeError: pass
        self.sock.bind(("", LAN_PORT))
        threading.Thread(target=self._recv_loop, daemon=True).start()
        threading.Thread(target=self._reap_loop, daemon=True).start()
        time.sleep(0.4)
        with self.lock:
            self.is_host=(self.maze_seed is None)
            if self.is_host: self.maze_seed=random.randint(0,2**31)

    def send_state(self,x,y,won):
        msg=json.dumps({"uid":self.uid,"name":self.name,"color_idx":self.color_idx,
            "x":round(x,2),"y":round(y,2),"won":won,
            "maze_seed":self.maze_seed,"is_host":self.is_host}).encode()
        try: self.sock.sendto(msg,(BROADCAST_IP,LAN_PORT))
        except Exception: pass

    def _recv_loop(self):
        while self._running:
            try:
                data,_=self.sock.recvfrom(4096)
                pkt=json.loads(data.decode()); uid=pkt.get("uid")
                if uid==self.uid: continue
                with self.lock:
                    if pkt.get("is_host") and self.maze_seed is None:
                        self.maze_seed=pkt["maze_seed"]
                    cidx=pkt.get("color_idx",0)
                    if cidx==self.color_idx:
                        self.color_idx=(self.color_idx+1)%len(PLAYER_COLORS)
                    safe = float(WALL_THICKNESS + BALL_RADIUS + 2)
                    self.peers[uid]={"name":pkt.get("name","?"),"color_idx":cidx,
                        "x":pkt.get("x", safe),"y":pkt.get("y", safe),
                        "won":pkt.get("won",False),"last_seen":time.time()}
            except (socket.timeout, json.JSONDecodeError, OSError): pass

    def _reap_loop(self):
        while self._running:
            now=time.time()
            with self.lock:
                dead=[u for u,p in self.peers.items() if now-p["last_seen"]>PEER_TIMEOUT]
                for u in dead: del self.peers[u]
            time.sleep(1.0)

    def get_peers(self): 
        with self.lock: return dict(self.peers)
    def stop(self): self._running=False; self.sock.close()


# ── Draw helpers ───────────────────────────────────────────────────────────────
def draw_ball(surf, color, x, y, radius, glow_cache):
    gr = radius * 3
    if color not in glow_cache:
        gs = pygame.Surface((gr*2, gr*2), pygame.SRCALPHA)
        for i in range(gr, 0, -1):
            pygame.draw.circle(gs, (*color, int(65*(i/gr)**2)), (gr,gr), i)
        glow_cache[color] = gs
    surf.blit(glow_cache[color], (int(x)-gr, int(y)-gr))
    pygame.draw.circle(surf, color, (int(x), int(y)), radius)
    hi = tuple(min(255, c+130) for c in color)
    pygame.draw.circle(surf, hi, (int(x)-radius//3, int(y)-radius//3), radius//3)


def draw_trajectories(surf, bx, by, vx, vy, wall_rects, radius):
    """
    For each of the 4 tilt directions, simulate the ball's path if that kick
    fires next, and draw fading dots along the predicted trajectory.
    Stops simulating when the ball hits a wall or runs out of steps.
    """
    MAG = TILT_IMPULSE_MAG
    directions = [
        ((0,  MAG), (180, 80,  80,  220)),   # down  — warm red
        ((0, -MAG), (80,  180, 255, 220)),   # up    — cool blue
        (( MAG, 0), (80,  220, 120, 220)),   # right — green
        ((-MAG, 0), (220, 180, 60,  220)),   # left  — amber
    ]
    STEPS       = 35
    DOT_SPACING = 3      # simulate every N frames, draw a dot
    MIN_ALPHA   = 18
    MAX_ALPHA   = 160
    DOT_R       = max(2, radius // 5)

    for (kick_x, kick_y), rgba in directions:
        sx, sy = bx, by
        svx = (vx + kick_x) * FRICTION_ACTIVE
        svy = (vy + kick_y) * FRICTION_ACTIVE

        for step in range(STEPS):
            # advance
            sx += svx
            br = pygame.Rect(sx-radius, sy-radius, radius*2, radius*2)
            for wr in wall_rects:
                if br.colliderect(wr):
                    sx = wr.left-radius if svx>0 else wr.right+radius
                    svx *= -0.3
                    break

            sy += svy
            br = pygame.Rect(sx-radius, sy-radius, radius*2, radius*2)
            hit_wall = False
            for wr in wall_rects:
                if br.colliderect(wr):
                    sy = wr.top-radius if svy>0 else wr.bottom+radius
                    svy *= -0.3
                    hit_wall = True
                    break

            svx *= FRICTION_ACTIVE
            svy *= FRICTION_ACTIVE

            if step % DOT_SPACING == 0:
                alpha = int(MAX_ALPHA * (1 - step / STEPS) ** 1.6)
                alpha = max(MIN_ALPHA, alpha)
                r, g, b, _ = rgba
                dot = pygame.Surface((DOT_R*2, DOT_R*2), pygame.SRCALPHA)
                pygame.draw.circle(dot, (r, g, b, alpha), (DOT_R, DOT_R), DOT_R)
                surf.blit(dot, (int(sx)-DOT_R, int(sy)-DOT_R))

            # stop if barely moving
            if abs(svx) + abs(svy) < 0.15:
                break


def draw_avatar_label(surf, font, text, cx, cy, color, avatar, radius, ball_rect=None):
    """Draw circular avatar (or coloured dot) + username above the ball.
    Avatar size = radius * 3.  Fades to 50% opacity when ball_rect overlaps it."""
    AV  = radius * 3          # avatar diameter scales with ball
    gap = radius + 4
    total_h = AV + 3 + font.size("A")[1]
    ay  = int(cy) - gap - total_h
    ax  = int(cx) - AV // 2

    # Bounding rect of avatar for intersection test
    av_rect = pygame.Rect(ax, ay, AV, AV)
    alpha   = 128 if (ball_rect and av_rect.colliderect(ball_rect)) else 255

    # Draw onto a temp surface so we can set alpha
    tmp = pygame.Surface((AV, AV), pygame.SRCALPHA)

    if avatar is not None:
        av_scaled = pygame.transform.smoothscale(avatar, (AV, AV))
        # coloured ring — draw onto tmp
        pygame.draw.circle(tmp, (*color, alpha), (AV//2, AV//2), AV//2+2)
        tmp.blit(av_scaled, (0, 0))
        # Re-apply alpha mask so the whole thing is semi-transparent
        tmp.set_alpha(alpha)
        surf.blit(tmp, (ax, ay))
    else:
        pygame.draw.circle(tmp, (*color, alpha), (AV//2, AV//2), AV//2)
        hi = tuple(min(255, c+100) for c in color)
        pygame.draw.circle(tmp, (*hi, alpha), (AV//2-AV//8, AV//2-AV//8), AV//8)
        tmp.set_alpha(alpha)
        surf.blit(tmp, (ax, ay))

    # Username below avatar
    ty = ay + AV + 3
    sh = font.render(text, True, (0, 0, 0))
    tx = font.render(text, True, color)
    tx_r = tx.get_rect(centerx=int(cx), top=ty)
    surf.blit(sh, (tx_r.x+1, tx_r.y+1))
    surf.blit(tx, tx_r)


# ── HUD toolbar ───────────────────────────────────────────────────────────────
def draw_hud(surf, font, W, H, gxd, gyd, fh, fv, host_str, peers, port,
             mouse_pos, clicked):
    """Bottom toolbar with text + ⚙ Config button. Returns True if config clicked."""
    bar_h = 34
    bar_y = H - bar_h
    pygame.draw.rect(surf, (10, 12, 22), (0, bar_y, W, bar_h))
    pygame.draw.line(surf, (40, 55, 100), (0, bar_y), (W, bar_y))

    txt = (f"  vel {gxd:+.2f},{gyd:+.2f}   "
           f"flipH:{'ON' if fh else 'off'}  flipV:{'ON' if fv else 'off'}   "
           f"[{host_str}] peers:{peers}  port:{port}   "
           f"[R]=new maze   [ESC]=quit")
    t = font.render(txt, True, (70, 95, 150))
    surf.blit(t, (0, bar_y + (bar_h - font.size("A")[1])//2))

    # ⚙ Config button on right
    btn_w, btn_h = 90, 26
    btn_rect = pygame.Rect(W - btn_w - 8, bar_y + (bar_h-btn_h)//2, btn_w, btn_h)
    hover = btn_rect.collidepoint(mouse_pos)
    col   = (70, 100, 180) if hover else (40, 55, 90)
    pygame.draw.rect(surf, col, btn_rect, border_radius=6)
    pygame.draw.rect(surf, (90, 120, 200), btn_rect, 1, border_radius=6)
    bt = font.render("⚙ Config", True, (200, 215, 255))
    surf.blit(bt, bt.get_rect(center=btn_rect.center))

    return clicked and btn_rect.collidepoint(mouse_pos)


# ── Calibration overlay ────────────────────────────────────────────────────────
def pill_toggle_draw(surf, font, label, state, lx, y, pill_x):
    surf.blit(font.render(label, True, (185,205,235)), (lx, y+6))
    pw, ph = 80, 32
    col = (55,195,115) if state else (50,50,78)
    pygame.draw.rect(surf, col, (pill_x, y, pw, ph), border_radius=16)
    pygame.draw.rect(surf, (110,140,200), (pill_x, y, pw, ph), 1, border_radius=16)
    kx = pill_x+pw-26 if state else pill_x+6
    pygame.draw.circle(surf, (240,248,255), (kx+10, y+16), 12)
    vt = font.render("ON" if state else "OFF", True,
                     (210,255,210) if state else (105,110,130))
    surf.blit(vt, (pill_x+(10 if state else 22), y+8))


def draw_calibration(surf, W, H, fonts, fh, fv, rot_locked, rot_supported,
                     mouse_pos, clicked):
    """
    Returns (toggle_h_clicked, toggle_v_clicked, lock_clicked, close_clicked).
    Caller applies the state changes — this function is pure draw + hit-test.
    """
    font_big, font, font_sm = fonts
    ov = pygame.Surface((W,H), pygame.SRCALPHA)
    ov.fill((0,0,0,220)); surf.blit(ov,(0,0))

    pw = 500; ph = 420 if rot_supported else 350
    px = (W-pw)//2; py = (H-ph)//2

    pygame.draw.rect(surf, (12,16,30), (px,py,pw,ph), border_radius=16)
    pygame.draw.rect(surf, (55,78,155), (px,py,pw,ph), 2, border_radius=16)

    t = font_big.render("⚙  Configuration", True, (145,185,255))
    surf.blit(t, t.get_rect(centerx=W//2, top=py+18))

    # ✕ Close button — top-right corner
    close_rect = pygame.Rect(px+pw-46, py+10, 36, 36)
    close_hover = close_rect.collidepoint(mouse_pos)
    pygame.draw.rect(surf, (180,50,50) if close_hover else (70,35,45), close_rect, border_radius=8)
    pygame.draw.rect(surf, (200,80,80), close_rect, 1, border_radius=8)
    xt = font.render("✕", True, (255,210,210))
    surf.blit(xt, xt.get_rect(center=close_rect.center))

    pygame.draw.line(surf,(38,52,100),(px+20,py+66),(px+pw-20,py+66))

    cy = py+84
    lx = px+30

    # Each toggle row is a full-width clickable area, not just the pill
    row_h = 52
    toggle_h_rect = pygame.Rect(px+8, cy,        pw-16, row_h)
    toggle_v_rect = pygame.Rect(px+8, cy+row_h+6, pw-16, row_h)

    def draw_toggle_row(rect, label, state):
        row_hover = rect.collidepoint(mouse_pos)
        if row_hover:
            pygame.draw.rect(surf, (25,32,58), rect, border_radius=10)
        surf.blit(font.render(label, True, (185,205,235)), (rect.x+22, rect.y+14))
        # pill
        pill_x = rect.right - 105
        pill_y = rect.y + 11
        pw2, ph2 = 88, 32
        col = (55,195,115) if state else (50,50,78)
        pygame.draw.rect(surf, col, (pill_x, pill_y, pw2, ph2), border_radius=16)
        pygame.draw.rect(surf, (110,145,205), (pill_x, pill_y, pw2, ph2), 1, border_radius=16)
        kx = pill_x + pw2 - 28 if state else pill_x + 6
        pygame.draw.circle(surf, (240,248,255), (kx+11, pill_y+16), 13)
        vt = font.render("ON" if state else "OFF", True,
                         (210,255,210) if state else (105,110,130))
        surf.blit(vt, (pill_x + (12 if state else 26), pill_y+9))

    draw_toggle_row(toggle_h_rect, "Flip Horizontal  [H]", fh)
    draw_toggle_row(toggle_v_rect, "Flip Vertical    [V]", fv)

    lock_btn_rect = None
    if rot_supported:
        sep_y = cy + row_h*2 + 18
        pygame.draw.line(surf,(35,48,95),(px+20,sep_y),(px+pw-20,sep_y))
        label = "🔒 Rotation Locked" if rot_locked else "🔓 Lock Screen Rotation"
        lock_btn_rect = pygame.Rect(px+pw//2-155, sep_y+14, 310, 46)
        hover = lock_btn_rect.collidepoint(mouse_pos)
        col = (80,125,205) if rot_locked else ((55,72,115) if hover else (32,42,72))
        pygame.draw.rect(surf, col, lock_btn_rect, border_radius=10)
        pygame.draw.rect(surf, (100,135,210), lock_btn_rect, 1, border_radius=10)
        lt = font.render(label, True, (220,230,255))
        surf.blit(lt, lt.get_rect(center=lock_btn_rect.center))

    hint_y = py+ph-75
    pygame.draw.line(surf,(35,48,95),(px+20,hint_y-10),(px+pw-20,hint_y-10))
    for i, line in enumerate([
        "Tap a row to toggle   |   H / V keys also work",
        "ESC — close   |   Arrow keys unaffected by flip",
    ]):
        s = font_sm.render(line, True, (65,88,145))
        surf.blit(s, s.get_rect(centerx=W//2, top=hint_y+i*28))

    # Hit-test — only fire on actual click, mutually exclusive
    if clicked:
        if close_rect.collidepoint(mouse_pos):
            return False, False, False, True    # close
        if toggle_h_rect.collidepoint(mouse_pos):
            return True,  False, False, False   # flip H
        if toggle_v_rect.collidepoint(mouse_pos):
            return False, True,  False, False   # flip V
        if lock_btn_rect and lock_btn_rect.collidepoint(mouse_pos):
            return False, False, True,  False   # lock
    return False, False, False, False


# ── Username prompt ────────────────────────────────────────────────────────────
def username_prompt(screen, clock, font_big, font):
    name=""; cursor=True; ct=time.time()
    while True:
        clock.tick(30)
        for ev in pygame.event.get():
            if ev.type==pygame.QUIT: pygame.quit(); sys.exit()
            if ev.type==pygame.KEYDOWN:
                if ev.key==pygame.K_RETURN and name.strip(): return name.strip()[:16]
                elif ev.key==pygame.K_BACKSPACE: name=name[:-1]
                elif ev.unicode and len(name)<16 and ev.unicode.isprintable():
                    name+=ev.unicode
        if time.time()-ct>0.5: cursor=not cursor; ct=time.time()
        W,H=screen.get_size()
        screen.fill((8,8,14))
        pw,ph=460,220; px,py=(W-pw)//2,(H-ph)//2
        pygame.draw.rect(screen,(12,16,30),(px,py,pw,ph),border_radius=16)
        pygame.draw.rect(screen,(55,78,150),(px,py,pw,ph),2,border_radius=16)
        t1=font_big.render("🔴  Ball Maze", True,(215,70,70))
        screen.blit(t1, t1.get_rect(centerx=W//2, top=py+18))
        t2=font.render("Enter your username:", True,(145,170,220))
        screen.blit(t2, t2.get_rect(centerx=W//2, top=py+72))
        box=pygame.Rect(px+45,py+108,pw-90,40)
        pygame.draw.rect(screen,(22,28,48),box,border_radius=8)
        pygame.draw.rect(screen,(65,98,178),box,1,border_radius=8)
        disp=name+("|" if cursor else " ")
        nt=font.render(disp, True,(220,232,255))
        screen.blit(nt, nt.get_rect(midleft=(box.x+12,box.centery)))
        hint=font.render("Press ENTER to play", True,(55,75,128))
        screen.blit(hint, hint.get_rect(centerx=W//2, top=py+165))
        pygame.display.flip()


# ── Main ───────────────────────────────────────────────────────────────────────
def main():
    global flip_horiz, flip_vert

    pygame.init()
    screen = pygame.display.set_mode((1100, 820), pygame.RESIZABLE)
    pygame.display.set_caption("🔴 Ball Maze — LAN Multiplayer")
    clock    = pygame.time.Clock()
    font_big = pygame.font.SysFont("monospace", 28, bold=True)
    font     = pygame.font.SysFont("monospace", 20)
    font_sm  = pygame.font.SysFont("monospace", 16)
    font_lbl = pygame.font.SysFont("monospace", 16, bold=True)
    fonts    = (font_big, font, font_sm)

    cli_name = sys.argv[1] if len(sys.argv)>1 else ""
    if not cli_name:
        cli_name = username_prompt(screen, clock, font_big, font)

    # Load avatar after pygame.init so image module is ready
    avatar = load_avatar(size=36)

    net = NetManager(cli_name, random.randint(0, len(PLAYER_COLORS)-1))
    my_color = PLAYER_COLORS[net.color_idx]

    deadline = time.time()+1.0
    while net.maze_seed is None and time.time()<deadline:
        time.sleep(0.05)

    maze_seed  = net.maze_seed or random.randint(0,2**31)
    maze_walls = generate_maze(COLS, ROWS, seed=maze_seed)

    rot_supported = detect_rotation_support()
    rot_locked    = False

    threading.Thread(target=sensor_reader, daemon=True).start()

    glow_cache = {}

    HUD_H = 34   # toolbar height at bottom

    def compute_offsets():
        w, h = screen.get_size()
        ox = (w - COLS*CELL_W) // 2
        oy = (h - HUD_H - ROWS*CELL_H) // 2
        return ox, oy

    # Ball position stored as maze-relative (independent of window size / OX,OY shifts).
    # Converted to screen coords each frame: screen_x = OX + ball_rx, etc.
    SAFE = float(WALL_THICKNESS + BALL_RADIUS + 2)
    ball_rx = SAFE   # maze-relative X
    ball_ry = SAFE   # maze-relative Y
    vx = vy = 0.0
    won = False; win_time = 0.0
    show_cal = False

    key_vecs  = {pygame.K_LEFT:(-1,0), pygame.K_RIGHT:(1,0),
                 pygame.K_UP:(0,-1),   pygame.K_DOWN:(0,1)}
    keys_held = set()
    net_timer = 0.0

    def reset():
        nonlocal ball_rx,ball_ry,vx,vy,won,maze_walls,maze_seed
        maze_seed=random.randint(0,2**31)
        net.maze_seed=maze_seed
        maze_walls=generate_maze(COLS,ROWS,seed=maze_seed)
        ball_rx=SAFE; ball_ry=SAFE
        vx=vy=0.0; won=False

    # ─── game loop ─────────────────────────────────────────────────────────────
    while True:
        dt = clock.tick(60)/1000.0
        net_timer += dt

        W, H    = screen.get_size()
        OX, OY  = compute_offsets()
        clicked = False
        mouse_pos = pygame.mouse.get_pos()

        for event in pygame.event.get():
            if event.type == pygame.QUIT:
                net.stop(); pygame.quit(); sys.exit()
            if event.type == pygame.MOUSEBUTTONDOWN and event.button==1:
                clicked = True
            if event.type == pygame.KEYDOWN:
                k = event.key
                if k==pygame.K_ESCAPE:
                    if show_cal: show_cal=False
                    else: net.stop(); pygame.quit(); sys.exit()
                elif k==pygame.K_c:
                    if not show_cal: show_cal=True  # C opens, ESC/✕ closes
                elif k==pygame.K_r and not show_cal: reset()
                elif k==pygame.K_h:
                    with sensor_lock: flip_horiz=not flip_horiz
                elif k==pygame.K_v:
                    with sensor_lock: flip_vert=not flip_vert
                elif k in key_vecs: keys_held.add(k)
            if event.type==pygame.KEYUP:
                keys_held.discard(event.key)

        # Derive absolute screen position from maze-relative + live offsets
        bx = OX + ball_rx
        by = OY + ball_ry

        # wall rects & goal live every frame
        wall_rects = maze_to_rects(maze_walls, CELL_W, CELL_H, OX, OY, COLS, ROWS)
        goal_x = OX + (COLS-1)*CELL_W + CELL_W//2
        goal_y = OY + (ROWS-1)*CELL_H + CELL_H//2

        # arrow key impulses this frame
        akx=aky=0.0
        for k in keys_held:
            dx,dy=key_vecs[k]; akx+=dx*TILT_IMPULSE_MAG*0.4; aky+=dy*TILT_IMPULSE_MAG*0.4

        # physics — drain sensor impulses, apply friction, collide
        if not won:
            with sensor_lock:
                ix     = _sensor["ix"];  _sensor["ix"] = 0.0
                iy     = _sensor["iy"];  _sensor["iy"] = 0.0
                active = _sensor["active"]

            friction = FRICTION_ACTIVE if (active or keys_held) else FRICTION_NEUTRAL
            vx = (vx + ix + akx) * friction
            vy = (vy + iy + aky) * friction

            bx+=vx
            br=pygame.Rect(bx-BALL_RADIUS,by-BALL_RADIUS,BALL_RADIUS*2,BALL_RADIUS*2)
            for wr in wall_rects:
                if br.colliderect(wr):
                    bx=wr.left-BALL_RADIUS if vx>0 else wr.right+BALL_RADIUS
                    vx*=-0.3
                    br=pygame.Rect(bx-BALL_RADIUS,by-BALL_RADIUS,BALL_RADIUS*2,BALL_RADIUS*2)

            by+=vy
            br=pygame.Rect(bx-BALL_RADIUS,by-BALL_RADIUS,BALL_RADIUS*2,BALL_RADIUS*2)
            for wr in wall_rects:
                if br.colliderect(wr):
                    by=wr.top-BALL_RADIUS if vy>0 else wr.bottom+BALL_RADIUS
                    vy*=-0.3
                    br=pygame.Rect(bx-BALL_RADIUS,by-BALL_RADIUS,BALL_RADIUS*2,BALL_RADIUS*2)

            # write back to maze-relative
            ball_rx = bx - OX
            ball_ry = by - OY

            if math.hypot(bx-goal_x, by-goal_y) < GOAL_R+BALL_RADIUS-4:
                won=True; win_time=time.time()

        # net broadcast
        if net_timer>=NET_TICK:
            net_timer=0.0
            my_color=PLAYER_COLORS[net.color_idx]
            net.send_state(ball_rx, ball_ry, won)

        # ── draw ───────────────────────────────────────────────────────────────
        screen.fill((8,8,13))

        # grid
        for c in range(COLS+1):
            pygame.draw.line(screen,(16,19,27),(OX+c*CELL_W,OY),(OX+c*CELL_W,OY+ROWS*CELL_H))
        for r in range(ROWS+1):
            pygame.draw.line(screen,(16,19,27),(OX,OY+r*CELL_H),(OX+COLS*CELL_W,OY+r*CELL_H))

        # goal
        pulse=0.6+0.4*math.sin(time.time()*4)
        pygame.draw.circle(screen,(int(50*pulse),int(220*pulse),int(100*pulse)),(goal_x,goal_y),GOAL_R)
        pygame.draw.circle(screen,(200,255,220),(goal_x,goal_y),GOAL_R//2)

        # walls
        for wr in wall_rects:
            pygame.draw.rect(screen,(42,52,80),wr)
            pygame.draw.rect(screen,(65,85,130),wr,1)

        # trajectory previews (drawn under the ball)
        if not won:
            draw_trajectories(screen, bx, by, vx, vy, wall_rects, BALL_RADIUS)

        # remote peers
        peers = net.get_peers()
        safe_min = float(WALL_THICKNESS + BALL_RADIUS + 2)
        safe_max_x = float(COLS*CELL_W - WALL_THICKNESS - BALL_RADIUS - 2)
        safe_max_y = float(ROWS*CELL_H - WALL_THICKNESS - BALL_RADIUS - 2)
        local_ball_rect = pygame.Rect(int(bx)-BALL_RADIUS, int(by)-BALL_RADIUS,
                                      BALL_RADIUS*2, BALL_RADIUS*2)
        for uid,p in peers.items():
            pc=PLAYER_COLORS[p["color_idx"]%len(PLAYER_COLORS)]
            rx = max(safe_min, min(safe_max_x, p["x"]))
            ry = max(safe_min, min(safe_max_y, p["y"]))
            px_=OX+rx; py_=OY+ry
            pbr = pygame.Rect(int(px_)-BALL_RADIUS, int(py_)-BALL_RADIUS,
                              BALL_RADIUS*2, BALL_RADIUS*2)
            draw_ball(screen,pc,px_,py_,BALL_RADIUS,glow_cache)
            draw_avatar_label(screen,font_lbl,p["name"],px_,py_,pc,None,BALL_RADIUS,ball_rect=pbr)
            if p["won"]:
                wt=font_lbl.render("🏆",True,(255,220,50))
                screen.blit(wt,wt.get_rect(centerx=int(px_),top=int(py_)+BALL_RADIUS+4))

        # local ball
        draw_ball(screen, my_color, bx, by, BALL_RADIUS, glow_cache)
        draw_avatar_label(screen, font_lbl, cli_name, bx, by, my_color, avatar,
                          BALL_RADIUS, ball_rect=local_ball_rect)

        # HUD toolbar
        with sensor_lock: fh_cur,fv_cur=flip_horiz,flip_vert
        host_str="HOST" if net.is_host else "CLIENT"
        config_clicked = draw_hud(screen, font_sm, W, H,
                                   vx, vy, fh_cur, fv_cur,
                                   host_str, len(peers), LAN_PORT,
                                   mouse_pos, clicked)
        if config_clicked and not show_cal:
            show_cal = True

        # win overlay
        if won:
            elapsed=time.time()-win_time
            ov=pygame.Surface((W,H),pygame.SRCALPHA)
            ov.fill((0,200,100,int(min(180,elapsed*300))))
            screen.blit(ov,(0,0))
            txt=font_big.render("YOU WIN! 🏆  Press R for a new maze",True,(255,255,255))
            screen.blit(txt,txt.get_rect(center=(W//2,H//2)))

        # calibration overlay — handle all actions, never let a toggle close the menu
        if show_cal:
            # Read flip state fresh each frame (may have been updated by keypress above)
            with sensor_lock: fh_draw,fv_draw = flip_horiz,flip_vert
            tog_h, tog_v, lock_clicked, close_clicked = draw_calibration(
                screen, W, H, fonts,
                fh_draw, fv_draw, rot_locked, rot_supported, mouse_pos, clicked)
            if tog_h:
                with sensor_lock: flip_horiz = not flip_horiz
            if tog_v:
                with sensor_lock: flip_vert  = not flip_vert
            if lock_clicked:
                rot_locked = not rot_locked
                set_rotation_locked(rot_locked)
            if close_clicked:
                show_cal = False

        pygame.display.flip()


if __name__=="__main__":
    main()
