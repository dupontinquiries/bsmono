//! palace — the zettelkasten as a place you walk through.
//!
//! `graph_view` draws the link graph as rings around a focus, top-down.
//! This is the same topology stood up: nodes placed on the floor by ring
//! distance from the focus, a first-person camera you move with the
//! movement keys, and a modal key layer — helix's shape — for the things
//! a graph wants that a walk does not give you: jump to the next node,
//! find one by its first letter, open it, fork a child off it, tag it.
//!
//! Three layers, each a seam:
//!
//! * [`Keys`] — pure. Movement keys come from `settings_kit::MovementLayout`
//!   (WASD, `.oeu` for Dvorak, ESDF, arrows), the same setting the games
//!   read (`games/soldier`). Motions are helix-style: `n`/`p` next and
//!   previous node, `/<char>` find by first letter (helix's search key),
//!   `g` then `g`/`e`/`h` for first/last/home(focus), `Enter` open, `F`
//!   fork, `t` tag (then type, `Enter` to commit, `Esc` to cancel), `b`
//!   back to the previous jump. The motion keys are chosen outside every
//!   layout's movement set — the test `motion_keys_are_outside_every_
//!   movement_set` holds that: helix's `f` and `u` were the first draft,
//!   and ESDF's `f` and Dvorak's `u` are why they are not the keys.
//! * [`WalkCam`] — pure. Position, yaw, pitch; a perspective projection.
//! * [`scene`] — pure. Where each node stands, from the topology's rings.
//! * [`palace_ui`] — the only egui: reads keys, moves the camera, paints.
//!
//! Depth is painted, not simulated: nearer nodes are larger and brighter,
//! edges fade with distance, and labels only draw within reading range.
//! There is no collision and no gravity — this is a diagram you stand in,
//! not a game; the game keys are borrowed because hands know them.

use egui::{Color32, Pos2, Rect, Stroke, Ui};
use scene_kit::spatial::Vec3;
use settings_kit::MovementLayout;

use crate::graph_view::nifty::Topology;

// ── keys ───────────────────────────────────────────────────────────────────

/// The four movement keys for a layout, as lower-case chars. Arrows are
/// handled by the egui layer (they are not chars); this returns `None`.
pub fn movement_keys(layout: MovementLayout) -> Option<[char; 4]> {
    // forward, left, back, right — the inverted T by physical position.
    match layout {
        MovementLayout::Wasd => Some(['w', 'a', 's', 'd']),
        MovementLayout::Dvorak => Some(['.', 'o', 'e', 'u']),
        MovementLayout::Esdf => Some(['e', 's', 'd', 'f']),
        MovementLayout::Arrows => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Focus the next / previous node in ring order.
    Next,
    Prev,
    /// Focus the nearest node whose label starts with this letter.
    Find(char),
    First,
    Last,
    /// Back to the focus node (the room's centre).
    Home,
    /// Open the focused node — the host decides what "open" is (the note).
    Open,
    /// Make a child note linked from the focused one.
    Fork,
    /// Add this tag to the focused node.
    Tag(String),
    /// Undo the last jump.
    Undo,
    /// Leave the palace.
    Exit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Walk,
    /// `g` was pressed; the next key completes it.
    Goto,
    /// `f` was pressed; the next key is the letter to find.
    Find,
    /// Typing a tag.
    Tag(String),
}

#[derive(Debug, Clone)]
pub struct Keys {
    pub layout: MovementLayout,
    pub mode: Mode,
}

/// One frame's worth of input, already reduced to chars.
#[derive(Debug, Default, Clone)]
pub struct Input<'a> {
    /// Keys currently held (movement reads these).
    pub held: &'a [char],
    /// Keys pressed this frame, in order (motions read these).
    pub typed: &'a [char],
    pub enter: bool,
    pub escape: bool,
    pub backspace: bool,
    /// Arrow keys held, for the `Arrows` layout: forward, left, back, right.
    pub arrows: [bool; 4],
}

/// What a frame of keys means.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Step {
    /// −1..1 each: forward/back, right/left.
    pub forward: f32,
    pub strafe: f32,
    pub actions: Vec<Action>,
}

impl Keys {
    pub fn new(layout: MovementLayout) -> Keys { Keys { layout, mode: Mode::Walk } }

    pub fn step(&mut self, input: &Input<'_>) -> Step {
        let mut out = Step::default();
        // Movement from held keys — only while walking, so typing a tag
        // that contains an `e` does not also walk you backwards.
        if self.mode == Mode::Walk {
            let mut fwd = 0.0f32;
            let mut strafe = 0.0f32;
            if let Some([f, l, b, r]) = movement_keys(self.layout) {
                if input.held.contains(&f) { fwd += 1.0; }
                if input.held.contains(&b) { fwd -= 1.0; }
                if input.held.contains(&r) { strafe += 1.0; }
                if input.held.contains(&l) { strafe -= 1.0; }
            }
            if input.arrows[0] { fwd += 1.0; }
            if input.arrows[2] { fwd -= 1.0; }
            if input.arrows[3] { strafe += 1.0; }
            if input.arrows[1] { strafe -= 1.0; }
            out.forward = fwd.clamp(-1.0, 1.0);
            out.strafe = strafe.clamp(-1.0, 1.0);
        }

        if input.escape {
            match self.mode {
                Mode::Walk => out.actions.push(Action::Exit),
                _ => self.mode = Mode::Walk,
            }
            return out;
        }
        if input.enter {
            match std::mem::replace(&mut self.mode, Mode::Walk) {
                Mode::Tag(t) if !t.trim().is_empty() => out.actions.push(Action::Tag(t.trim().to_string())),
                Mode::Tag(_) => {}
                _ => out.actions.push(Action::Open),
            }
            return out;
        }
        if input.backspace {
            if let Mode::Tag(t) = &mut self.mode { t.pop(); }
            return out;
        }
        let movement_arr = movement_keys(self.layout);
        let movement: &[char] = movement_arr.as_ref().map(|k| &k[..]).unwrap_or(&[]);
        for &ch in input.typed {
            match &mut self.mode {
                Mode::Tag(t) => t.push(ch),
                Mode::Goto => {
                    self.mode = Mode::Walk;
                    match ch {
                        'g' => out.actions.push(Action::First),
                        'e' => out.actions.push(Action::Last),
                        'h' => out.actions.push(Action::Home),
                        _ => {}
                    }
                }
                Mode::Find => {
                    self.mode = Mode::Walk;
                    if ch.is_alphanumeric() { out.actions.push(Action::Find(ch.to_ascii_lowercase())); }
                }
                Mode::Walk => {
                    if movement.contains(&ch) { continue; } // held movement, not a motion (case-exact: `F` is fork)
                    match ch {
                        'n' => out.actions.push(Action::Next),
                        'p' => out.actions.push(Action::Prev),
                        '/' => self.mode = Mode::Find,
                        'g' => self.mode = Mode::Goto,
                        't' => self.mode = Mode::Tag(String::new()),
                        'F' => out.actions.push(Action::Fork),
                        'b' => out.actions.push(Action::Undo),
                        _ => {}
                    }
                }
            }
        }
        out
    }

    /// The one-line mode hint for the corner of the view.
    pub fn hint(&self) -> String {
        let mv = movement_keys(self.layout).map(|k| format!("{}{}{}{}", k[0], k[1], k[2], k[3])).unwrap_or_else(|| "arrows".into());
        match &self.mode {
            Mode::Walk => format!("{mv} move · n/p next/prev · /‹letter› find · gg/ge/gh first/last/home · ⏎ open · F fork · t tag · b back · esc leave"),
            Mode::Goto => "g: g first · e last · h home".into(),
            Mode::Find => "/: type the first letter of a node".into(),
            Mode::Tag(t) => format!("tag: {t}▏  (⏎ to add, esc to cancel)"),
        }
    }
}

// ── camera ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct WalkCam {
    pub pos: Vec3,
    /// Radians, 0 = looking down −z... here +z is "into the room".
    pub yaw: f32,
    pub pitch: f32,
    /// Focal length in screen-heights: 1.0 ≈ a 53° vertical field.
    pub focal: f32,
}

impl Default for WalkCam {
    fn default() -> Self {
        WalkCam { pos: Vec3 { x: 0.0, y: 1.6, z: -6.0 }, yaw: 0.0, pitch: -0.1, focal: 1.2 }
    }
}

impl WalkCam {
    pub fn forward(&self) -> Vec3 {
        Vec3 { x: self.yaw.sin(), y: 0.0, z: self.yaw.cos() }
    }
    pub fn right(&self) -> Vec3 {
        Vec3 { x: self.yaw.cos(), y: 0.0, z: -self.yaw.sin() }
    }

    pub fn walk(&mut self, forward: f32, strafe: f32, dist: f32) {
        let f = self.forward();
        let r = self.right();
        self.pos.x += (f.x * forward + r.x * strafe) * dist;
        self.pos.z += (f.z * forward + r.z * strafe) * dist;
    }

    pub fn turn(&mut self, dyaw: f32, dpitch: f32) {
        self.yaw += dyaw;
        self.pitch = (self.pitch + dpitch).clamp(-1.2, 1.2);
    }

    /// Face a point (yaw only), for jumps.
    pub fn look_at(&mut self, p: Vec3) {
        self.yaw = (p.x - self.pos.x).atan2(p.z - self.pos.z);
    }

    /// World → screen. `None` behind the camera. Returns (pos, depth).
    pub fn project(&self, p: Vec3, rect: Rect) -> Option<(Pos2, f32)> {
        let d = Vec3 { x: p.x - self.pos.x, y: p.y - self.pos.y, z: p.z - self.pos.z };
        // yaw about y
        let (sy, cy) = self.yaw.sin_cos();
        let x1 = d.x * cy - d.z * sy;
        let z1 = d.x * sy + d.z * cy;
        // pitch about x
        let (sp, cp) = self.pitch.sin_cos();
        let y2 = d.y * cp - z1 * sp;
        let z2 = d.y * sp + z1 * cp;
        if z2 < 0.05 { return None; }
        let f = self.focal * rect.height();
        let sx = rect.center().x + x1 / z2 * f;
        let sy_ = rect.center().y - y2 / z2 * f;
        Some((Pos2::new(sx, sy_), z2))
    }
}

// ── scene ──────────────────────────────────────────────────────────────────

/// Where each node stands: the focus at the origin, ring `r` at radius
/// `r * spread`, nodes spaced evenly around it, and a slight rise per ring
/// so the far rings are visible over the near ones.
pub mod scene {
    use super::*;

    pub fn positions(topo: &Topology, focus: &str, depth: usize, spread: f32) -> Vec<(String, usize, Vec3)> {
        let rings = topo.rings(focus, depth);
        let mut per_ring: std::collections::BTreeMap<usize, Vec<String>> = Default::default();
        for (id, r) in rings { per_ring.entry(r).or_default().push(id); }
        let mut out = Vec::new();
        for (r, ids) in per_ring {
            let n = ids.len().max(1) as f32;
            for (i, id) in ids.into_iter().enumerate() {
                let p = if r == 0 { Vec3 { x: 0.0, y: 0.0, z: 0.0 } } else {
                    let a = (i as f32 / n) * std::f32::consts::TAU + r as f32 * 0.3;
                    let rad = r as f32 * spread;
                    Vec3 { x: a.cos() * rad, y: r as f32 * 0.15, z: a.sin() * rad }
                };
                out.push((id, r, p));
            }
        }
        out
    }
}

// ── the view ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Palace {
    pub cam: WalkCam,
    pub keys: Keys,
    /// The node the motions act on (not the room centre, which is the
    /// graph view's `focus`).
    pub cursor: Option<String>,
    jumps: Vec<String>,
    /// Actions the host has not consumed yet: Open, Fork, Tag.
    pub pending: Vec<Action>,
    pub spread: f32,
}

impl Palace {
    pub fn new(layout: MovementLayout) -> Palace {
        Palace { cam: WalkCam::default(), keys: Keys::new(layout), cursor: None, jumps: Vec::new(), pending: Vec::new(), spread: 3.0 }
    }

    /// Apply a motion to the cursor over the current placement. Pure, so
    /// the jump order is a test.
    pub fn apply(&mut self, action: Action, placed: &[(String, usize, Vec3)]) {
        let ids: Vec<&String> = placed.iter().map(|(id, _, _)| id).collect();
        if ids.is_empty() { return; }
        let cur = self.cursor.as_ref().and_then(|c| ids.iter().position(|i| *i == c));
        let mut jump_to = |this: &mut Palace, i: usize| {
            if let Some(c) = this.cursor.take() { this.jumps.push(c); }
            this.cursor = Some(ids[i].clone());
            if let Some((_, _, p)) = placed.iter().find(|(id, _, _)| id == ids[i]) { this.cam.look_at(*p); }
        };
        match action {
            Action::Next => jump_to(self, cur.map(|i| (i + 1) % ids.len()).unwrap_or(0)),
            Action::Prev => jump_to(self, cur.map(|i| (i + ids.len() - 1) % ids.len()).unwrap_or(ids.len() - 1)),
            Action::First => jump_to(self, 0),
            Action::Last => jump_to(self, ids.len() - 1),
            Action::Home => jump_to(self, placed.iter().position(|(_, r, _)| *r == 0).unwrap_or(0)),
            Action::Find(ch) => {
                let start = cur.map(|i| i + 1).unwrap_or(0);
                let hit = (0..ids.len()).map(|k| (start + k) % ids.len())
                    .find(|&i| ids[i].chars().next().map(|c| c.to_ascii_lowercase()) == Some(ch));
                if let Some(i) = hit { jump_to(self, i); }
            }
            Action::Undo => {
                if let Some(prev) = self.jumps.pop() {
                    self.cursor = Some(prev.clone());
                    if let Some((_, _, p)) = placed.iter().find(|(id, _, _)| *id == prev) { self.cam.look_at(*p); }
                }
            }
            other => self.pending.push(other),
        }
    }
}

/// Paint the palace and read its keys. Returns the node to OPEN when the
/// user asks (the same contract as `graph_ui`'s return); Fork and Tag
/// wait in `palace.pending` for the host.
/// The characters a frame's events typed.
///
/// Pulled out of `palace_ui` so it can be tested: the bug it exists for is invisible from
/// outside — the palace's response to a letter is movement, and asserting on movement would
/// test the walker rather than the reading.
///
/// AN ON-SCREEN KEYBOARD COMMITS ITS LETTERS. Plasma's virtual keyboard and Maliit are
/// Wayland text-input clients: they commit a string for ordinary characters and send real key
/// events only for the editing keys. Reading `Event::Text` alone drops every letter while
/// backspace and enter keep working — the same split that made the terminal look like it had
/// a broken keymap, and this surface had it too.
///
/// Preedit is NOT typed: it is what an IME shows while composing, and taking it would walk
/// the palace on every intermediate guess and then again on the committed word.
pub fn typed_chars(events: &[egui::Event]) -> Vec<char> {
    let mut typed = Vec::new();
    for ev in events {
        match ev {
            egui::Event::Text(t) => typed.extend(t.chars()),
            egui::Event::Ime(egui::ImeEvent::Commit(t)) => typed.extend(t.chars()),
            _ => {}
        }
    }
    typed
}

pub fn palace_ui(ui: &mut Ui, palace: &mut Palace, topo: &Topology, focus: &str, depth: usize,
                 colors: &uikit_egui::PaletteColors) -> Option<String> {
    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, colors.bg_base);
    let dt = ui.input(|i| i.stable_dt).min(0.1);

    // ASK FOR A KEYBOARD. This is a typing surface with no `TextEdit` in it, and egui asks
    // the platform for IME only on behalf of a focused TextEdit — so on a phone the on-screen
    // keyboard had no reason to appear here at all, and its commits had nowhere to go.
    //
    // The whole pane as the area, its bottom edge as the cursor rect: there is no caret to
    // point at, and a compositor that puts its keyboard away from the bottom of a full-screen
    // pane has nowhere better to put it.
    crate::kbd_focus::request_ime(
        ui.ctx(),
        rect,
        egui::Rect::from_min_size(rect.left_bottom(), egui::vec2(1.0, 1.0)),
    );

    // Keys. Chars come from text events (layout-aware); held movement from
    // keys_down, which egui exposes as physical Keys — mapped back to the
    // layout's chars by name for letters, and by the two punctuation keys
    // Dvorak's set needs.
    let (held, typed, enter, escape, backspace, arrows) = ui.input(|i| {
        let mut held = Vec::new();
        for k in &i.keys_down {
            if let Some(ch) = key_char(*k) { held.push(ch); }
        }
        let typed = typed_chars(&i.events);
        (held, typed,
         i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape), i.key_pressed(egui::Key::Backspace),
         [i.key_down(egui::Key::ArrowUp), i.key_down(egui::Key::ArrowLeft), i.key_down(egui::Key::ArrowDown), i.key_down(egui::Key::ArrowRight)])
    });
    let step = palace.keys.step(&Input { held: &held, typed: &typed, enter, escape, backspace, arrows });
    palace.cam.walk(step.forward, step.strafe, 4.0 * dt);
    if resp.dragged() {
        let d = resp.drag_delta();
        palace.cam.turn(d.x * 0.005, -d.y * 0.005);
    }

    let placed = scene::positions(topo, focus, depth, palace.spread);
    if palace.cursor.is_none() { palace.cursor = Some(focus.to_string()); }
    let mut open: Option<String> = None;
    for a in step.actions {
        match a {
            Action::Open => open = palace.cursor.clone(),
            Action::Exit => palace.pending.push(Action::Exit),
            other => palace.apply(other, &placed),
        }
    }

    // Project everything once.
    let projected: Vec<(usize, Pos2, f32)> = placed.iter().enumerate()
        .filter_map(|(i, (_, _, pos))| palace.cam.project(*pos, rect).map(|(s, d)| (i, s, d)))
        .collect();
    let index: std::collections::HashMap<&str, usize> = placed.iter().enumerate().map(|(i, (id, _, _))| (id.as_str(), i)).collect();
    let fade = |d: f32| ((1.0 - d / 30.0).clamp(0.15, 1.0) * 255.0) as u8;

    // Floor edges first (so nodes paint over them).
    for (i, s, d) in &projected {
        let id = &placed[*i].0;
        for nb in topo.neighbors(id) {
            if let Some(&j) = index.get(nb.as_str()) {
                if j < *i { continue; } // each edge once
                if let Some((_, s2, d2)) = projected.iter().find(|(k, _, _)| *k == j) {
                    let a = fade(d.max(*d2));
                    p.line_segment([*s, *s2], Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(colors.border.r(), colors.border.g(), colors.border.b(), a)));
                }
            }
        }
    }
    // Nodes, far to near.
    let mut order = projected.clone();
    order.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    for (i, s, d) in &order {
        let (id, ring, _) = &placed[*i];
        let r = (rect.height() * 0.05 / d).clamp(2.0, 28.0);
        let is_cursor = palace.cursor.as_deref() == Some(id.as_str());
        let base = if *ring == 0 { colors.accent } else { colors.bg_raised };
        let col = Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), fade(*d));
        p.circle_filled(*s, r, col);
        if is_cursor {
            p.circle_stroke(*s, r + 3.0, Stroke::new(2.0_f32, colors.glow));
        }
        if *d < 18.0 {
            let font = egui::FontId::proportional((rect.height() * 0.03 / d * 4.0).clamp(9.0, 18.0));
            p.text(Pos2::new(s.x, s.y - r - 4.0), egui::Align2::CENTER_BOTTOM, id, font,
                   if is_cursor { colors.text } else { colors.text_weak });
        }
        // click to put the cursor there
        if resp.clicked() && ui.input(|inp| inp.pointer.interact_pos()).is_some_and(|pp| pp.distance(*s) <= r + 4.0) {
            palace.cursor = Some(id.clone());
        }
    }
    // The mode line.
    p.text(Pos2::new(rect.left() + 8.0, rect.bottom() - 6.0), egui::Align2::LEFT_BOTTOM, palace.keys.hint(),
           egui::FontId::monospace(11.0), colors.text_weak);
    open
}

/// The char a held physical key stands for, for movement. Letters map by
/// name; the Dvorak set's `.` is `Period`. egui has no key for `,` or `/`
/// on every platform, so only the keys the four layouts need are mapped.
fn key_char(k: egui::Key) -> Option<char> {
    use egui::Key::*;
    Some(match k {
        A => 'a', D => 'd', E => 'e', F => 'f', O => 'o', S => 's', U => 'u', W => 'w',
        Period => '.',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {

    /// Both typing surfaces in this app read raw events, and both had the same gap: an
    /// on-screen keyboard COMMITS its letters and sends key events only for the editing keys,
    /// so reading `Event::Text` alone drops every letter while backspace and enter work.
    #[test]
    fn an_on_screen_keyboards_letters_are_typed_and_its_guesses_are_not() {
        use egui::{Event, ImeEvent};
        // A physical keyboard: unchanged.
        assert_eq!(typed_chars(&[Event::Text("wa".into())]), vec!['w', 'a']);

        // A virtual keyboard: commits.
        assert_eq!(typed_chars(&[Event::Ime(ImeEvent::Commit("s".into()))]), vec!['s']);

        // Composing text is not input. Taking it would walk the palace on every intermediate
        // guess and then again on the committed word.
        assert!(typed_chars(&[Event::Ime(ImeEvent::Preedit("ni".into()))]).is_empty());
        assert_eq!(
            typed_chars(&[
                Event::Ime(ImeEvent::Preedit("n".into())),
                Event::Ime(ImeEvent::Preedit("ni".into())),
                Event::Ime(ImeEvent::Commit("d".into())),
            ]),
            vec!['d'],
            "the composed character should arrive once"
        );

        // Order is kept across both kinds — a frame can carry either.
        assert_eq!(
            typed_chars(&[Event::Text("a".into()), Event::Ime(ImeEvent::Commit("b".into()))]),
            vec!['a', 'b']
        );
        assert!(typed_chars(&[]).is_empty());
    }
    use super::*;

    fn inp<'a>(held: &'a [char], typed: &'a [char]) -> Input<'a> {
        Input { held, typed, ..Default::default() }
    }

    #[test]
    fn each_layout_walks_with_its_own_keys_and_ignores_the_others() {
        let mut d = Keys::new(MovementLayout::Dvorak);
        let s = d.step(&inp(&['.', 'u'], &[]));
        assert_eq!((s.forward, s.strafe), (1.0, 1.0));
        let s = d.step(&inp(&['w', 'd'], &[]));
        assert_eq!((s.forward, s.strafe), (0.0, 0.0), "qwerty keys do nothing on dvorak");
        let mut q = Keys::new(MovementLayout::Wasd);
        assert_eq!(q.step(&inp(&['s', 'a'], &[])).forward, -1.0);
        assert_eq!(q.step(&inp(&['s', 'a'], &[])).strafe, -1.0);
        let mut ar = Keys::new(MovementLayout::Arrows);
        let s = ar.step(&Input { arrows: [true, false, false, true], ..Default::default() });
        assert_eq!((s.forward, s.strafe), (1.0, 1.0));
    }

    #[test]
    fn motion_keys_are_outside_every_movement_set() {
        for l in [MovementLayout::Wasd, MovementLayout::Dvorak, MovementLayout::Esdf] {
            let keys = movement_keys(l).unwrap();
            for m in ['n', 'p', '/', 'g', 't', 'b'] {
                assert!(!keys.contains(&m), "{l:?} uses {m} for movement");
            }
        }
    }

    #[test]
    fn helix_sequences_resolve_and_typing_a_tag_does_not_walk() {
        let mut k = Keys::new(MovementLayout::Dvorak);
        assert_eq!(k.step(&inp(&[], &['n'])).actions, vec![Action::Next]);
        assert_eq!(k.step(&inp(&[], &['g', 'g'])).actions, vec![Action::First]);
        assert_eq!(k.step(&inp(&[], &['g'])).actions, vec![]);
        assert_eq!(k.step(&inp(&[], &['h'])).actions, vec![Action::Home]);
        assert_eq!(k.step(&inp(&[], &['/', 'R'])).actions, vec![Action::Find('r')]);
        assert_eq!(k.step(&inp(&[], &['F'])).actions, vec![Action::Fork]);
        // tag mode: the e in "idea" must not walk backwards
        k.step(&inp(&[], &['t']));
        let s = k.step(&inp(&['e'], &['i', 'd', 'e', 'a']));
        assert_eq!(s.forward, 0.0);
        assert_eq!(k.mode, Mode::Tag("idea".into()));
        let s = k.step(&Input { enter: true, ..Default::default() });
        assert_eq!(s.actions, vec![Action::Tag("idea".into())]);
        assert_eq!(k.mode, Mode::Walk);
        // escape cancels a tag; a second escape leaves
        k.step(&inp(&[], &['t', 'x']));
        assert!(k.step(&Input { escape: true, ..Default::default() }).actions.is_empty());
        assert_eq!(k.step(&Input { escape: true, ..Default::default() }).actions, vec![Action::Exit]);
        // enter in walk mode opens
        assert_eq!(k.step(&Input { enter: true, ..Default::default() }).actions, vec![Action::Open]);
    }

    #[test]
    fn the_camera_projects_ahead_and_refuses_behind() {
        let cam = WalkCam::default();
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
        let (s, d) = cam.project(Vec3 { x: 0.0, y: 1.6, z: 0.0 }, rect).unwrap();
        assert!((s.x - 400.0).abs() < 1.0, "straight ahead lands on the centre line: {s:?}");
        assert!(d > 5.0);
        assert!(cam.project(Vec3 { x: 0.0, y: 1.6, z: -20.0 }, rect).is_none(), "behind the camera");
        let mut c2 = cam.clone();
        c2.walk(1.0, 0.0, 2.0);
        assert!((c2.pos.z - -4.0).abs() < 1e-4);
        c2.look_at(Vec3 { x: 10.0, y: 0.0, z: -4.0 });
        assert!((c2.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-4, "east is +x");
    }

    #[test]
    fn rings_stand_in_circles_and_the_cursor_walks_them() {
        let ids: Vec<String> = ["home", "alpha", "beta", "gamma"].iter().map(|s| s.to_string()).collect();
        let topo = Topology::new(&ids, &[("home".into(), "alpha".into()), ("home".into(), "beta".into()), ("beta".into(), "gamma".into())]);
        let placed = scene::positions(&topo, "home", 3, 3.0);
        assert_eq!(placed.len(), 4);
        assert_eq!(placed[0].0, "home");
        assert_eq!(placed[0].2, Vec3 { x: 0.0, y: 0.0, z: 0.0 });
        let ring1: Vec<&(String, usize, Vec3)> = placed.iter().filter(|(_, r, _)| *r == 1).collect();
        assert_eq!(ring1.len(), 2);
        for (_, _, p) in &ring1 {
            assert!(((p.x * p.x + p.z * p.z).sqrt() - 3.0).abs() < 1e-3, "ring 1 at radius 3: {p:?}");
        }
        let mut pal = Palace::new(MovementLayout::Wasd);
        pal.cursor = Some("home".into());
        pal.apply(Action::Next, &placed);
        assert_eq!(pal.cursor.as_deref(), Some("alpha"));
        pal.apply(Action::Find('g'), &placed);
        assert_eq!(pal.cursor.as_deref(), Some("gamma"));
        pal.apply(Action::Undo, &placed);
        assert_eq!(pal.cursor.as_deref(), Some("alpha"));
        pal.apply(Action::Home, &placed);
        assert_eq!(pal.cursor.as_deref(), Some("home"));
        pal.apply(Action::Tag("todo".into()), &placed);
        assert_eq!(pal.pending, vec![Action::Tag("todo".into())]);
    }
}
