//! MENU FX — time, as data.
//!
//! `menu.rs` is deliberately timeless: rows, a cursor, a stack, and every
//! change lands the same frame it was asked for. That is the right core
//! and the wrong feel. Destiny's menus taught a generation what "smooth"
//! means — the cursor is a creature with momentum, lists CASCADE in, and
//! commitment is a held ring, not a click — and this game's own fiction
//! adds the second half: nothing here fades, because everything here is
//! a MACHINE. Panels deploy. Plates unfold and overshoot and settle,
//! the way a transforming hull does (`ship.rs` scales its fold time by
//! hull length for exactly this reason).
//!
//! So this module is the missing dimension, kept headless: pure state
//! machines driven by `tick(dt)` and sampled by whoever draws. Nothing
//! in here knows a vertex, a texture or a row label — `hud.rs` samples
//! it, the preview harness (`menusim.rs`) samples it into text frames,
//! and the tests sample it into assertions. One clock, three consumers,
//! zero disagreements.
//!
//! The pieces:
//! * `ease` — the curve vocabulary. `back_out` is the Transformers one:
//!   it overshoots and SEATS, like a panel latching home.
//! * `Deploy` — a staggered per-row entrance/exit. Each row slides in
//!   along its own axis with an unfold, offset by its index: the
//!   Destiny cascade, parameterised per station so the bridge sweeps
//!   wide while the armory snaps like a weapons rack.
//! * `Cursor` — a damped spring toward the selected row, with a squash
//!   term read off its own velocity. The selection bar is ALIVE.
//! * `Hold` — hold-to-confirm with grace: releasing early decays the
//!   ring rather than zeroing it, which is the difference between a
//!   timer and a feel.
//! * `Transition` — the push/pop choreography between two screens:
//!   the outgoing retracts while the incoming deploys, on one clock.

// ------------------------------------------------------------- eases ---

/// The curve vocabulary. All take and return 0..1 (inputs are clamped),
/// so they compose and none of them can fling a panel off screen.
pub mod ease {
    /// fast start, gentle landing — the workhorse for anything arriving
    pub fn smooth_out(t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t)
    }
    /// gentle start, fast finish — for things leaving
    pub fn smooth_in(t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        t * t * t
    }
    /// slow-fast-slow — camera-ish drifts
    pub fn smooth(t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }
    /// overshoot and seat: passes 1.0, comes back, latches. THE
    /// mechanical ease — a plate that arrives without overshoot looks
    /// painted on; one that arrives with it looks like it locked.
    pub fn back_out(t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        let c1 = 1.70158;
        let c3 = c1 + 1.0;
        1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
    }
    /// how far past 1.0 `back_out` reaches at its peak — the draw side
    /// can budget for it instead of clipping the latch
    pub const BACK_OVERSHOOT: f32 = 0.10;
}

// ------------------------------------------------------------ deploy ---

/// How a station's rows arrive. The numbers are the station's character.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeployStyle {
    /// seconds between one row starting and the next — the cascade
    pub stagger: f32,
    /// seconds one row takes to seat
    pub per_row: f32,
    /// slide distance at t=0, in row-heights; sign is direction
    pub slide: f32,
    /// 0 = slide only, 1 = full unfold (scale-y from folded)
    pub unfold: f32,
}

impl DeployStyle {
    /// the default: a measured sweep
    pub const fn glide() -> DeployStyle {
        DeployStyle { stagger: 0.045, per_row: 0.34, slide: 1.6, unfold: 0.35 }
    }
    /// weapons-rack snap: short throw, hard latch, tight cascade
    pub const fn rack() -> DeployStyle {
        DeployStyle { stagger: 0.030, per_row: 0.22, slide: 0.9, unfold: 0.75 }
    }
    /// bridge sweep: long throw, unhurried, barely unfolds
    pub const fn sweep() -> DeployStyle {
        DeployStyle { stagger: 0.060, per_row: 0.45, slide: 2.6, unfold: 0.15 }
    }
    /// cargo cascade: rows drop like crates, full unfold
    pub const fn stack() -> DeployStyle {
        DeployStyle { stagger: 0.022, per_row: 0.26, slide: 1.1, unfold: 1.0 }
    }

    /// When the whole list is seated.
    pub fn total(&self, rows: usize) -> f32 {
        self.per_row + self.stagger * rows.saturating_sub(1) as f32
    }
}

/// One row's pose at a moment: offset in row-heights along the slide
/// axis, alpha, and vertical unfold scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowPose {
    pub offset: f32,
    pub alpha: f32,
    pub unfold: f32,
}

impl RowPose {
    pub const SEATED: RowPose = RowPose { offset: 0.0, alpha: 1.0, unfold: 1.0 };
}

/// The cascade clock. `t` runs 0..total and the direction decides
/// whether rows are arriving or retracting; retraction reverses the
/// stagger so the LAST row leaves first, which is how a machine folds.
#[derive(Clone, Copy, Debug)]
pub struct Deploy {
    pub style: DeployStyle,
    t: f32,
    entering: bool,
}

impl Deploy {
    pub fn enter(style: DeployStyle) -> Deploy {
        Deploy { style, t: 0.0, entering: true }
    }
    pub fn exit(style: DeployStyle) -> Deploy {
        Deploy { style, t: 0.0, entering: false }
    }
    pub fn tick(&mut self, dt: f32) {
        self.t += dt.max(0.0);
    }
    pub fn done(&self, rows: usize) -> bool {
        self.t >= self.style.total(rows)
    }
    /// Sample row `i` of `rows`. Entering rows run top-first; exiting
    /// rows run bottom-first and drive their pose backwards.
    pub fn row(&self, i: usize, rows: usize) -> RowPose {
        let order = if self.entering { i } else { rows.saturating_sub(1).saturating_sub(i) };
        let local = ((self.t - self.style.stagger * order as f32) / self.style.per_row).clamp(0.0, 1.0);
        let k = if self.entering { ease::back_out(local) } else { 1.0 - ease::smooth_in(local) };
        RowPose {
            offset: self.style.slide * (1.0 - k),
            alpha: if self.entering { ease::smooth_out(local) } else { 1.0 - ease::smooth_in(local) },
            unfold: 1.0 - self.style.unfold * (1.0 - k),
        }
    }
}

// ------------------------------------------------------------ cursor ---

/// The selection bar as a damped spring. `target` is a row index; the
/// bar's `y` chases it in row units, and `squash` reads the speed so the
/// draw side can stretch the bar along its travel — the Destiny cursor
/// in two numbers.
#[derive(Clone, Copy, Debug)]
pub struct Cursor {
    pub y: f32,
    v: f32,
    pub target: f32,
}

/// Spring constants: stiff enough to arrive inside ~150 ms over one
/// row, damped just under critical so it settles with one soft kiss of
/// overshoot rather than a wobble.
const CURSOR_K: f32 = 320.0;
const CURSOR_D: f32 = 30.0;

impl Cursor {
    pub fn at(row: usize) -> Cursor {
        Cursor { y: row as f32, v: 0.0, target: row as f32 }
    }
    pub fn aim(&mut self, row: usize) {
        self.target = row as f32;
    }
    pub fn tick(&mut self, dt: f32) {
        // fixed substeps: a spring integrated at a spiky dt explodes,
        // and a menu must never be the thing that explodes
        let mut left = dt.clamp(0.0, 0.1);
        const H: f32 = 1.0 / 240.0;
        while left > 0.0 {
            let h = left.min(H);
            let a = CURSOR_K * (self.target - self.y) - CURSOR_D * self.v;
            self.v += a * h;
            self.y += self.v * h;
            left -= h;
        }
    }
    /// 0 at rest, rising with travel speed; capped so a long jump does
    /// not draw the bar as a line
    pub fn squash(&self) -> f32 {
        (self.v.abs() * 0.06).min(0.45)
    }
    pub fn settled(&self) -> bool {
        (self.y - self.target).abs() < 0.01 && self.v.abs() < 0.05
    }
}

// -------------------------------------------------------------- hold ---

/// Hold-to-confirm. Destiny's ring: commitment is a duration, and
/// letting go early DECAYS the ring at twice fill speed instead of
/// zeroing it — an almost-committed hold resumed quickly still lands,
/// a tap never does.
#[derive(Clone, Copy, Debug, Default)]
pub struct Hold {
    /// 0..1
    pub ring: f32,
    held: bool,
}

/// Seconds of continuous hold to confirm.
pub const HOLD_SEC: f32 = 0.55;

impl Hold {
    pub fn press(&mut self) {
        self.held = true;
    }
    pub fn release(&mut self) {
        self.held = false;
    }
    /// Returns true ONCE, on the tick the ring closes.
    pub fn tick(&mut self, dt: f32) -> bool {
        if self.held {
            let was = self.ring;
            self.ring = (self.ring + dt / HOLD_SEC).min(1.0);
            if was < 1.0 && self.ring >= 1.0 {
                return true;
            }
        } else {
            self.ring = (self.ring - dt * 2.0 / HOLD_SEC).max(0.0);
        }
        false
    }
}

// -------------------------------------------------------- transition ---

/// Where the choreography is.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Stage {
    /// rows cascading in
    Opening,
    /// everything seated; input feels instant here
    Steady,
    /// rows retracting; when they finish the screen actually changes
    Closing,
    /// nothing on stage
    Shut,
}

/// The one clock a screen's motion runs on. The HOST stays timeless —
/// `menu::Host` still changes state the frame it is asked — and this
/// wraps WHEN to ask it: `close_then` holds the pending action until
/// the retraction finishes, so the stack never changes under a screen
/// that is still on stage.
#[derive(Clone, Debug)]
pub struct Motion {
    pub stage: Stage,
    pub deploy: Deploy,
    pub cursor: Cursor,
    pub hold: Hold,
    rows: usize,
    /// what to do when the current Closing finishes
    pending: Option<Pending>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Pending {
    /// pop one screen (Back)
    Pop,
    /// the caller swaps screens itself; Motion re-opens with this style
    Swap(DeployStyle),
    /// close the whole surface
    Dismiss,
}

impl Motion {
    pub fn shut() -> Motion {
        Motion {
            stage: Stage::Shut,
            deploy: Deploy::enter(DeployStyle::glide()),
            cursor: Cursor::at(0),
            hold: Hold::default(),
            rows: 0,
            pending: None,
        }
    }
    /// A screen takes the stage: rows cascade in with its style.
    pub fn open(&mut self, style: DeployStyle, rows: usize, sel: usize) {
        self.stage = Stage::Opening;
        self.deploy = Deploy::enter(style);
        self.cursor = Cursor::at(sel);
        self.hold = Hold::default();
        self.rows = rows;
        self.pending = None;
    }
    /// Begin the retraction; `then` fires when it completes.
    pub fn close_then(&mut self, then: Pending) {
        if self.stage == Stage::Shut {
            return;
        }
        self.stage = Stage::Closing;
        self.deploy = Deploy::exit(self.deploy.style);
        self.pending = Some(then);
    }
    pub fn rows(&self) -> usize {
        self.rows
    }
    /// A rebuild changed the row count without changing screens: the
    /// stage stays where it is, the clocks keep their time.
    pub fn rows_changed(&mut self, rows: usize) {
        self.rows = rows;
    }
    /// Advance every clock. Returns the pending action exactly once,
    /// on the tick the retraction finishes — the caller mutates the
    /// Host then, and only then.
    pub fn tick(&mut self, dt: f32) -> Option<Pending> {
        self.cursor.tick(dt);
        match self.stage {
            Stage::Opening => {
                self.deploy.tick(dt);
                if self.deploy.done(self.rows) {
                    self.stage = Stage::Steady;
                }
                None
            }
            Stage::Closing => {
                self.deploy.tick(dt);
                if self.deploy.done(self.rows) {
                    self.stage = Stage::Shut;
                    return self.pending.take();
                }
                None
            }
            _ => None,
        }
    }
    /// A row's pose right now. Steady screens are seated by identity,
    /// so the draw path costs nothing once the cascade lands.
    pub fn row(&self, i: usize) -> RowPose {
        match self.stage {
            Stage::Steady => RowPose::SEATED,
            Stage::Shut => RowPose { offset: 0.0, alpha: 0.0, unfold: 0.0 },
            _ => self.deploy.row(i, self.rows),
        }
    }
    /// Whether input should reach the menu: only a seated screen
    /// listens. An opening screen ignores taps the way a deploying
    /// weapons rack should.
    pub fn listening(&self) -> bool {
        self.stage == Stage::Steady
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eases_start_at_zero_and_land_on_one() {
        for f in [ease::smooth_out, ease::smooth_in, ease::smooth, ease::back_out] {
            assert!(f(0.0).abs() < 1e-4);
            assert!((f(1.0) - 1.0).abs() < 1e-4);
            assert!(f(-5.0).is_finite() && f(5.0).is_finite());
        }
        // back_out genuinely overshoots, inside its stated budget
        let peak = (0..100).map(|i| ease::back_out(i as f32 / 100.0)).fold(0.0f32, f32::max);
        assert!(peak > 1.02 && peak <= 1.0 + ease::BACK_OVERSHOOT + 1e-3, "peak {peak}");
    }

    #[test]
    fn a_deploy_cascades_top_first_and_retracts_bottom_first() {
        let style = DeployStyle::rack();
        let mut d = Deploy::enter(style);
        d.tick(style.per_row * 0.6);
        let (a, b) = (d.row(0, 6), d.row(5, 6));
        assert!(a.alpha > b.alpha && a.offset < b.offset, "the cascade runs the wrong way");
        // finish: everything seated exactly
        d.tick(style.total(6));
        assert_eq!(d.row(0, 6), RowPose::SEATED);
        assert_eq!(d.row(5, 6), RowPose::SEATED);
        // exit mirrors: the BOTTOM row leaves first
        let mut x = Deploy::exit(style);
        x.tick(style.per_row * 0.5);
        assert!(x.row(5, 6).alpha < x.row(0, 6).alpha, "the fold runs the wrong way");
    }

    #[test]
    fn the_cursor_is_a_spring_that_arrives_and_settles() {
        let mut c = Cursor::at(0);
        c.aim(4);
        let mut peak = 0.0f32;
        let mut t = 0.0;
        while t < 1.0 {
            c.tick(1.0 / 120.0);
            peak = peak.max(c.y);
            t += 1.0 / 120.0;
        }
        assert!(c.settled(), "still moving after a second: y={} v={}", c.y, c.v);
        assert!(peak > 4.0 && peak < 4.6, "no life ({peak}) or a wobble ({peak})");
        // squash reads speed: mid-flight it is nonzero, at rest zero
        let mut m = Cursor::at(0);
        m.aim(6);
        m.tick(0.04);
        assert!(m.squash() > 0.05);
        assert!(c.squash() < 0.02);
    }

    #[test]
    fn spiky_frame_times_cannot_explode_the_spring() {
        let mut c = Cursor::at(0);
        c.aim(1);
        for _ in 0..40 {
            c.tick(0.1); // 100 ms hitches, back to back
        }
        assert!(c.settled(), "a hitchy frame broke the cursor: y={}", c.y);
    }

    #[test]
    fn a_hold_confirms_once_and_a_tap_never_does() {
        let mut h = Hold::default();
        h.press();
        let mut fired = 0;
        let mut t = 0.0;
        while t < HOLD_SEC * 1.5 {
            if h.tick(1.0 / 60.0) {
                fired += 1;
            }
            t += 1.0 / 60.0;
        }
        assert_eq!(fired, 1, "the ring fired {fired} times");
        // tap: press-release inside a few frames leaves nothing behind
        let mut tap = Hold::default();
        tap.press();
        tap.tick(0.05);
        tap.release();
        for _ in 0..30 {
            assert!(!tap.tick(1.0 / 60.0));
        }
        assert_eq!(tap.ring, 0.0);
        // grace: release at 80%, resume quickly, still lands
        let mut g = Hold::default();
        g.press();
        g.tick(HOLD_SEC * 0.8);
        g.release();
        g.tick(0.05);
        assert!(g.ring > 0.5, "the grace decay is a cliff");
        g.press();
        // a resumed hold finishes in well under half a fresh hold
        let mut t = 0.0;
        let mut landed = false;
        while t < HOLD_SEC * 0.5 {
            if g.tick(1.0 / 60.0) {
                landed = true;
                break;
            }
            t += 1.0 / 60.0;
        }
        assert!(landed, "an almost-held ring took a full fresh hold to finish");
    }

    #[test]
    fn motion_holds_the_pending_action_until_the_fold_finishes() {
        let mut m = Motion::shut();
        m.open(DeployStyle::rack(), 5, 2);
        assert!(!m.listening(), "an opening screen took input");
        // run the cascade home
        let mut guard = 0;
        while m.stage != Stage::Steady {
            assert!(m.tick(1.0 / 60.0).is_none());
            guard += 1;
            assert!(guard < 600, "the deploy never seated");
        }
        assert!(m.listening());
        assert_eq!(m.row(3), RowPose::SEATED);
        // ask it to close: nothing fires until the retraction lands
        m.close_then(Pending::Pop);
        let mut fired = None;
        let mut ticks = 0;
        while fired.is_none() {
            fired = m.tick(1.0 / 60.0);
            ticks += 1;
            assert!(ticks < 600, "the fold never finished");
        }
        assert_eq!(fired, Some(Pending::Pop));
        assert!(ticks > 3, "the fold was instant; that is a cut, not a close");
        assert_eq!(m.stage, Stage::Shut);
        // and it fires exactly once
        assert!(m.tick(1.0 / 60.0).is_none());
    }

    #[test]
    fn every_style_seats_a_long_list_inside_a_second_and_a_half() {
        for s in [DeployStyle::glide(), DeployStyle::rack(), DeployStyle::sweep(), DeployStyle::stack()] {
            assert!(s.total(14) < 1.5, "a 14-row screen takes {:.2}s to open", s.total(14));
            assert!(s.total(1) >= s.per_row);
        }
    }
}
