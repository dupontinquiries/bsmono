"""
Paperbound unified input — mouse, touch, and game controllers.

Goals:
  • Mobile touch: pygame emits FINGERDOWN/FINGERMOTION/FINGERUP with normalized
    (0..1) coords. We convert those to canvas pixels and feed the same code path
    as the mouse, so the whole UI is touch-native with zero per-button work.
  • Controllers: a cursor you drive with the left stick + A/cross to click. This
    powers couch play and, with two pads, local 1v1 on a single screen.
  • Local 1v1 hotseat: two independent "players", each an input source bound to
    a team, with their own selection + a cursor.

The module is pygame-only (no game import) so it stays testable in isolation.
"""
import pygame

# a logical "click" or "point" event the game consumes, in canvas pixels
class PointerEvent:
    __slots__ = ("x", "y", "kind", "player", "source")
    def __init__(self, x, y, kind, player=0, source="mouse"):
        self.x, self.y = x, y
        self.kind = kind            # "down" | "move" | "up"
        self.player = player        # which local player (0/1) this belongs to
        self.source = source        # "mouse" | "touch" | "pad"


class Cursor:
    """An on-screen cursor a controller can drive. Also used to show touch/mouse."""
    def __init__(self, x, y, color):
        self.x, self.y = x, y
        self.color = color
        self.visible = False
        self.pressed = False


class InputHub:
    """Owns pad state + cursors and translates raw pygame events into
    PointerEvents in canvas space. `to_canvas` maps window px → canvas px."""
    def __init__(self, to_canvas, W, H, players=1):
        self.to_canvas = to_canvas
        self.W, self.H = W, H
        self.players = players
        self.pads = []
        self.cursors = [Cursor(W * 0.5, H * 0.7, (36, 128, 118)),
                        Cursor(W * 0.5, H * 0.3, (150, 36, 42))]
        self.touch_active = False
        self._prev_btn = {}          # (pad_index) -> bool A pressed last frame
        self.refresh_pads()

    def refresh_pads(self):
        pygame.joystick.init()
        self.pads = []
        for i in range(pygame.joystick.get_count()):
            j = pygame.joystick.Joystick(i)
            j.init()
            self.pads.append(j)

    def pad_count(self):
        return len(self.pads)

    def process(self, events, dt):
        """Return a list of PointerEvents for this frame."""
        out = []
        for e in events:
            if e.type == pygame.MOUSEBUTTONDOWN:
                x, y = self.to_canvas(e.pos)
                out.append(PointerEvent(x, y, "down", 0, "mouse"))
            elif e.type == pygame.MOUSEMOTION:
                x, y = self.to_canvas(e.pos)
                out.append(PointerEvent(x, y, "move", 0, "mouse"))
            elif e.type == pygame.MOUSEBUTTONUP:
                x, y = self.to_canvas(e.pos)
                out.append(PointerEvent(x, y, "up", 0, "mouse"))
            # ── touch: coords are normalized 0..1 over the *window* ──
            elif e.type == pygame.FINGERDOWN:
                self.touch_active = True
                x, y = self.to_canvas((e.x * self._win_w(), e.y * self._win_h()))
                out.append(PointerEvent(x, y, "down", 0, "touch"))
            elif e.type == pygame.FINGERMOTION:
                x, y = self.to_canvas((e.x * self._win_w(), e.y * self._win_h()))
                out.append(PointerEvent(x, y, "move", 0, "touch"))
            elif e.type == pygame.FINGERUP:
                x, y = self.to_canvas((e.x * self._win_w(), e.y * self._win_h()))
                out.append(PointerEvent(x, y, "up", 0, "touch"))
            elif e.type == pygame.JOYDEVICEADDED or e.type == pygame.JOYDEVICEREMOVED:
                self.refresh_pads()

        # ── controllers: move each pad's cursor, emit down/up on A/cross ──
        for pi, pad in enumerate(self.pads[:self.players]):
            cur = self.cursors[pi]
            try:
                ax = pad.get_axis(0); ay = pad.get_axis(1)
            except Exception:
                ax = ay = 0.0
            dead = 0.18
            if abs(ax) < dead: ax = 0.0
            if abs(ay) < dead: ay = 0.0
            speed = 720 * dt
            cur.x = max(0, min(self.W, cur.x + ax * speed))
            cur.y = max(0, min(self.H, cur.y + ay * speed))
            if ax or ay:
                cur.visible = True
                out.append(PointerEvent(cur.x, cur.y, "move", pi, "pad"))
            # button 0 is A/cross on most pads
            try:
                a_now = pad.get_button(0) == 1
            except Exception:
                a_now = False
            was = self._prev_btn.get(pi, False)
            if a_now and not was:
                out.append(PointerEvent(cur.x, cur.y, "down", pi, "pad"))
                cur.pressed = True
            elif (not a_now) and was:
                out.append(PointerEvent(cur.x, cur.y, "up", pi, "pad"))
                cur.pressed = False
            self._prev_btn[pi] = a_now
        return out

    def _win_w(self):
        return pygame.display.get_surface().get_width() if pygame.display.get_surface() else self.W
    def _win_h(self):
        return pygame.display.get_surface().get_height() if pygame.display.get_surface() else self.H

    def draw_cursors(self, screen):
        for pi, cur in enumerate(self.cursors[:self.players]):
            if not cur.visible:
                continue
            r = 16 if cur.pressed else 12
            pygame.draw.circle(screen, cur.color, (int(cur.x), int(cur.y)), r, 3)
            pygame.draw.circle(screen, (240, 230, 210), (int(cur.x), int(cur.y)), 3)


if __name__ == "__main__":
    # headless sanity: construct the hub, feed a fake touch event, check mapping
    import os
    os.environ["SDL_VIDEODRIVER"] = "dummy"
    pygame.init()
    pygame.display.set_mode((1280, 800))
    hub = InputHub(lambda p: (p[0], p[1]), 1280, 800, players=2)
    class FakeFinger:
        type = pygame.FINGERDOWN; x = 0.5; y = 0.5
    evs = hub.process([FakeFinger()], 1/60)
    assert evs and evs[0].kind == "down" and evs[0].source == "touch"
    print(f"✅ inputs.py: touch mapped to canvas ({evs[0].x:.0f},{evs[0].y:.0f}), "
          f"pads={hub.pad_count()}")
