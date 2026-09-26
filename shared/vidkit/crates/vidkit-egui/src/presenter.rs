//! `Presenter` on an egui texture.
//!
//! The portable path: convert on the CPU, upload as an ordinary texture, let
//! egui composite it. Works on the glow backend every app in this tree already
//! uses, at the cost of the conversion and the upload. The zero-copy route —
//! import the decoder's DMA-BUF and convert in `vidkit-vk`'s shader — needs
//! the wgpu backend and `wgpu_hal` access and is a different presenter.
//!
//! # Repaint scheduling
//!
//! This does **not** drive a repaint loop. `next_deadline` reports when the
//! next queued frame is due and the shell passes it to
//! `Context::request_repaint_after`. On a 90 Hz panel playing 24 fps that is 24
//! wakeups a second instead of 90, which on a phone is most of the battery
//! available here.

use egui::{ColorImage, Context, TextureHandle, TextureOptions, Vec2};
use std::time::Instant;
use vidkit_core::frame::{Frame, RecycleToken};
use vidkit_core::traits::{self as core_traits, Presenter};

/// Holds one frame as an egui texture.
pub struct EguiPresenter {
    ctx: Context,
    texture: Option<TextureHandle>,
    /// Size of the frame currently in `texture`.
    frame_size: Vec2,
    /// The token of the frame the texture was built from.
    ///
    /// Held until the next `submit` replaces it, mirroring what a real
    /// presenter must do: the compositor may still be reading the current
    /// frame, so releasing it at submit time would let the decoder write into
    /// memory being sampled. Here the pixels have already been copied into an
    /// egui texture and the token could be returned immediately — it is held
    /// anyway so that swapping in the zero-copy presenter does not change the
    /// buffer accounting the player depends on.
    held: Option<RecycleToken>,
    deadline: Option<Instant>,
    drops: u32,
    uploads: u64,
    /// Reused conversion target. See `planes_to_rgba8_into` for why.
    scratch: Vec<u8>,
    /// PTS of the frame currently in the texture, so a re-submit of the same
    /// frame costs nothing.
    shown_pts: Option<std::time::Duration>,
    uploaded_bytes: u64,
    skipped_uploads: u64,
    last_error: Option<String>,
    /// Damage rendering (vidkit_core::damage): the tile tree and the
    /// persistent RGBA canvas it updates. None = whole-frame conversion.
    damage: Option<(vidkit_core::damage::TileTree, Vec<u8>)>,
    /// Conversions a frame may spend, as a fraction of a whole frame. 1.0
    /// never degrades (only unchanged tiles are skipped).
    pub damage_budget: f32,
    /// The last frame's plan, for profiling.
    pub last_plan: Option<vidkit_core::damage::Plan>,
}

impl EguiPresenter {
    pub fn new(ctx: &Context) -> Self {
        Self {
            ctx: ctx.clone(),
            texture: None,
            frame_size: Vec2::ZERO,
            held: None,
            deadline: None,
            drops: 0,
            uploads: 0,
            scratch: Vec::new(),
            shown_pts: None,
            uploaded_bytes: 0,
            skipped_uploads: 0,
            last_error: None,
            damage: None,
            damage_budget: 1.0,
            last_plan: None,
        }
    }

    pub fn damage_rendering(&self) -> bool {
        self.damage.is_some()
    }

    /// Turn damage rendering on or off.
    pub fn set_damage_rendering(&mut self, on: bool) {
        if on != self.damage.is_some() {
            self.damage = None;
            if on {
                // Built lazily at the first frame, when the size is known.
                self.damage = Some((vidkit_core::damage::TileTree::new(0, 0), Vec::new()));
            }
        }
    }

    /// Damage path: convert and upload only what the plan says. Returns
    /// false if the frame is not a form it handles (the caller falls back).
    fn show_damaged(&mut self, frame: &Frame) -> bool {
        use vidkit_core::damage::{convert_region, TileTree};
        use vidkit_core::frame::{FrameData, PlaneLayout};
        let FrameData::Cpu(planes) = &frame.data else { return false };
        let bps = match frame.layout {
            PlaneLayout::Nv12 => 1,
            PlaneLayout::P010 => 2,
            _ => return false,
        };
        let (w, h) = (frame.width, frame.height);
        let Some((tree, canvas)) = self.damage.as_mut() else { return false };
        let fresh = tree.width != w || tree.height != h || self.texture.is_none();
        if fresh {
            *tree = TileTree::new(w, h);
            *canvas = vec![0u8; (w * h * 4) as usize];
        }
        let hashes = tree.hash_frame(planes, bps);
        let budget = ((w as f64 * h as f64) * self.damage_budget.clamp(0.01, 1.0) as f64) as u64;
        let plan = tree.plan(&hashes, budget);
        for op in &plan.ops {
            if convert_region(planes, frame.layout, frame.color, w, *op, canvas).is_err() {
                self.drops += 1;
                return true;
            }
        }
        let full = |canvas: &[u8]| ColorImage::from_rgba_unmultiplied([w as usize, h as usize], canvas);
        match &mut self.texture {
            Some(t) if !fresh => {
                for op in &plan.ops {
                    let mut rect = Vec::with_capacity((op.w * op.h * 4) as usize);
                    for y in op.y..op.y + op.h {
                        let a = ((y * w + op.x) * 4) as usize;
                        rect.extend_from_slice(&canvas[a..a + (op.w * 4) as usize]);
                    }
                    let img = ColorImage::from_rgba_unmultiplied([op.w as usize, op.h as usize], &rect);
                    t.set_partial([op.x as usize, op.y as usize], img, TextureOptions::LINEAR);
                    self.uploaded_bytes += (op.w * op.h * 4) as u64;
                }
            }
            Some(t) => {
                t.set(full(canvas), TextureOptions::LINEAR);
                self.uploaded_bytes += (w * h * 4) as u64;
            }
            None => {
                self.texture = Some(self.ctx.load_texture("vidkit-video", full(canvas), TextureOptions::LINEAR));
                self.uploaded_bytes += (w * h * 4) as u64;
            }
        }
        if !plan.ops.is_empty() || fresh {
            self.uploads += 1;
        }
        self.frame_size = Vec2::new(w as f32, h as f32);
        self.shown_pts = Some(frame.pts);
        self.last_plan = Some(plan);
        self.last_error = None;
        true
    }

    pub fn texture(&self) -> Option<&TextureHandle> {
        self.texture.as_ref()
    }

    pub fn frame_size(&self) -> Vec2 {
        self.frame_size
    }

    pub fn has_frame(&self) -> bool {
        self.texture.is_some()
    }

    /// Frames uploaded since construction, for a stats overlay.
    pub fn upload_count(&self) -> u64 {
        self.uploads
    }

    /// Bytes pushed to the GPU since construction.
    ///
    /// Worth showing in a stats overlay, because it is the number this path
    /// costs: RGBA8 is 4 bytes a pixel where the decoder produced 1.5, so a
    /// 1080p60 stream uploads about 500 MB/s here against 187 MB/s for the
    /// planes it came from. Uploading NV12 and converting in a shader is the
    /// structural fix and needs a paint callback; this counter is what would
    /// show it working.
    pub fn uploaded_bytes(&self) -> u64 {
        self.uploaded_bytes
    }

    /// Submissions that cost nothing because the frame was already on screen.
    pub fn skipped_uploads(&self) -> u64 {
        self.skipped_uploads
    }

    /// The last conversion failure, if any.
    ///
    /// A frame that cannot be converted is dropped rather than propagated as an
    /// error: one unsupported frame in a stream should not tear down playback,
    /// and the shell can surface this string in a corner instead.
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Tell the presenter when the next frame is due, so `next_deadline` has
    /// something to report. The player knows this; the presenter does not.
    pub fn set_deadline(&mut self, at: Option<Instant>) {
        self.deadline = at;
    }

    /// Ask egui to wake up in time for the next frame.
    ///
    /// Call once per UI frame. Returns whether a repaint was scheduled, so a
    /// shell can tell "playing" from "idle" without duplicating the logic.
    pub fn schedule_repaint(&mut self) -> bool {
        match self.deadline {
            Some(at) => {
                let now = Instant::now();
                // A deadline far in the past means nobody set a new one --
                // playback stopped and the caller forgot to say so. Without
                // this, `saturating_duration_since` returns zero, egui is asked
                // to repaint IMMEDIATELY, and a paused player redraws at full
                // rate forever showing the same still frame. On a phone that is
                // the whole screen and GPU awake for nothing, and it is
                // invisible on a desktop.
                //
                // No legitimate deadline survives this long: the player sets a
                // new one every frame it produces.
                if now.saturating_duration_since(at) > STALE_DEADLINE {
                    self.deadline = None;
                    return false;
                }
                let after = at.saturating_duration_since(now);
                self.ctx.request_repaint_after(after);
                true
            }
            None => false,
        }
    }

    /// The next frame's deadline, if one is set.
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// Playback stopped; stop asking for repaints.
    ///
    /// The explicit call is the correct fix and the staleness guard above is
    /// the backstop, because "forgot to clear the deadline" costs battery
    /// silently and is not visible in any screenshot.
    pub fn idle(&mut self) {
        self.deadline = None;
    }
}

/// How far past its deadline a frame can be before the schedule is treated as
/// abandoned rather than late. Generous: a stalled decoder should still get a
/// repaint when it recovers.
const STALE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(1);

impl EguiPresenter {
    /// Put a frame on screen without taking it.
    ///
    /// [`Presenter::submit`] consumes the frame, which suits a decoder with
    /// recycle tokens but throws away the planes of one that pools its own
    /// buffers (the ffmpeg path, which gets its planes back after this).
    /// Returns whether an upload happened.
    pub fn show(&mut self, frame: &Frame) -> bool {
        let (w, h) = (frame.width as usize, frame.height as usize);

        // A frame already on screen is a no-op. The player re-submits when a
        // repaint happens for some other reason -- a resize, a gesture, a
        // hover -- and converting and re-uploading the same pixels for that is
        // pure waste, which on a paused phone is the difference between idle
        // and warm.
        if self.shown_pts == Some(frame.pts) && self.texture.is_some() {
            self.skipped_uploads += 1;
            return false;
        }

        if self.damage.is_some() && self.show_damaged(frame) {
            return true;
        }

        match vidkit_core::frame_to_rgba8_into(frame, &mut self.scratch) {
            Ok(()) => {
                let image = ColorImage::from_rgba_unmultiplied([w, h], &self.scratch);
                match &mut self.texture {
                    // Reusing the handle keeps the same texture id, so a widget
                    // that cached it does not end up painting a stale frame.
                    Some(t) => t.set(image, TextureOptions::LINEAR),
                    None => {
                        self.texture = Some(self.ctx.load_texture(
                            "vidkit-video",
                            image,
                            TextureOptions::LINEAR,
                        ));
                    }
                }
                self.frame_size = Vec2::new(w as f32, h as f32);
                self.uploads += 1;
                self.uploaded_bytes += (w * h * 4) as u64;
                self.shown_pts = Some(frame.pts);
                self.last_error = None;
                true
            }
            Err(e) => {
                // Drop the picture, keep the buffer. Counting this as a drop
                // rather than an error means a single unsupported frame — a
                // 10-bit one arriving mid-stream, say — costs a frame and not
                // the session.
                self.drops += 1;
                self.last_error = Some(e.to_string());
                false
            }
        }
    }
}

impl Presenter for EguiPresenter {
    fn submit(&mut self, frame: Frame) -> core_traits::Result<Option<RecycleToken>> {
        self.show(&frame);
        let mut frame = frame;
        let previous = self.held.take();
        self.held = frame.recycle.take();
        Ok(previous)
    }

    fn release_all(&mut self) -> Vec<RecycleToken> {
        self.held.take().into_iter().collect()
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.deadline
    }

    fn take_drop_count(&mut self) -> u32 {
        std::mem::take(&mut self.drops)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use vidkit_core::frame::{ColorInfo, CpuPlanes, FrameData, PlaneLayout};

    fn ctx() -> Context {
        let ctx = Context::default();
        // A texture upload needs a frame in flight for the texture allocator.
        ctx.begin_pass(egui::RawInput::default());
        ctx
    }

    /// Distinct timestamps by default: real frames have them, and a helper
    /// that hands out pts=0 forever hides the redundant-upload skip.
    fn frame(token: Option<u32>, layout: PlaneLayout) -> Frame {
        frame_at(token, layout, Duration::from_millis(NEXT_PTS.fetch_add(33, Ordering::SeqCst)))
    }

    static NEXT_PTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    use std::sync::atomic::Ordering;

    fn frame_at(token: Option<u32>, layout: PlaneLayout, pts: Duration) -> Frame {
        let planes = match layout {
            PlaneLayout::Nv12 => CpuPlanes {
                data: vec![vec![126u8; 4], vec![128, 128]],
                strides: vec![2, 2],
            },
            _ => CpuPlanes {
                data: vec![vec![126u8; 4], vec![128], vec![128]],
                strides: vec![2, 1, 1],
            },
        };
        Frame {
            data: FrameData::Cpu(planes),
            width: 2,
            height: 2,
            layout,
            color: ColorInfo::default(),
            pts,
            duration: None,
            recycle: token.map(RecycleToken),
            grain: None,
        }
    }

    #[test]
    fn a_submitted_frame_becomes_a_texture() {
        let ctx = ctx();
        let mut p = EguiPresenter::new(&ctx);
        assert!(!p.has_frame());
        p.submit(frame(None, PlaneLayout::Nv12)).unwrap();
        assert!(p.has_frame());
        assert_eq!(p.frame_size(), Vec2::new(2.0, 2.0));
        assert_eq!(p.upload_count(), 1);
    }

    /// The buffer accounting the player depends on: submit returns the
    /// PREVIOUS frame's token, not this one's.
    #[test]
    fn submit_returns_the_previous_token_not_the_current_one() {
        let ctx = ctx();
        let mut p = EguiPresenter::new(&ctx);

        assert_eq!(p.submit(frame(Some(7), PlaneLayout::Nv12)).unwrap(), None);
        assert_eq!(
            p.submit(frame(Some(8), PlaneLayout::Nv12)).unwrap(),
            Some(RecycleToken(7))
        );
        // The newest is still held.
        assert_eq!(p.release_all(), vec![RecycleToken(8)]);
        assert!(p.release_all().is_empty(), "release_all must not repeat");
    }

    #[test]
    fn a_frame_with_no_token_yields_none() {
        let ctx = ctx();
        let mut p = EguiPresenter::new(&ctx);
        p.submit(frame(None, PlaneLayout::Nv12)).unwrap();
        assert_eq!(p.submit(frame(None, PlaneLayout::Nv12)).unwrap(), None);
    }

    /// An unconvertible frame must cost a frame, not the session -- and must
    /// still give its buffer back, or the pool bleeds out.
    #[test]
    fn an_unconvertible_frame_is_dropped_but_its_buffer_is_kept() {
        let ctx = ctx();
        let mut p = EguiPresenter::new(&ctx);
        p.submit(frame(Some(1), PlaneLayout::Nv12)).unwrap();

        // A frame whose plane list is short for its layout. This used to be a
        // P010 frame, on the grounds that the converter refused 10-bit -- it
        // now handles it, so that stopped being an unconvertible case and the
        // test was asserting against a frame that converts fine.
        let mut broken = frame(Some(2), PlaneLayout::I420);
        if let FrameData::Cpu(planes) = &mut broken.data {
            planes.data.truncate(1);
        }
        let returned = p.submit(broken).unwrap();
        assert_eq!(returned, Some(RecycleToken(1)), "buffer must still return");
        assert_eq!(p.take_drop_count(), 1);
        assert_eq!(p.take_drop_count(), 0, "drop count must reset when read");
        assert!(p.last_error().is_some(), "the reason should be reportable");

        // ...and the next good frame recovers.
        p.submit(frame(Some(3), PlaneLayout::Nv12)).unwrap();
        assert!(p.last_error().is_none());
        assert_eq!(p.upload_count(), 2);
    }

    #[test]
    fn nv12_and_i420_frames_both_upload() {
        let ctx = ctx();
        let mut p = EguiPresenter::new(&ctx);
        p.submit(frame(None, PlaneLayout::Nv12)).unwrap();
        p.submit(frame(None, PlaneLayout::I420)).unwrap();
        assert_eq!(p.upload_count(), 2);
    }

    /// The texture handle is reused across submits. A fresh handle each frame
    /// changes the texture id, so a widget that cached one paints a stale
    /// frame -- and it churns egui's texture allocator once per frame.
    #[test]
    fn the_texture_handle_is_reused_across_frames() {
        let ctx = ctx();
        let mut p = EguiPresenter::new(&ctx);
        p.submit(frame(None, PlaneLayout::Nv12)).unwrap();
        let first = p.texture().unwrap().id();
        p.submit(frame(None, PlaneLayout::Nv12)).unwrap();
        assert_eq!(p.texture().unwrap().id(), first);
    }

    /// Re-submitting the frame already on screen must cost nothing. The
    /// player re-submits on any repaint -- a resize, a hover, a gesture -- and
    /// converting and uploading the same pixels for that is pure waste.
    #[test]
    fn resubmitting_the_same_frame_uploads_nothing() {
        let ctx = ctx();
        let mut p = EguiPresenter::new(&ctx);
        let pts = Duration::from_millis(500);

        p.submit(frame_at(None, PlaneLayout::Nv12, pts)).unwrap();
        let after_first = p.uploaded_bytes();
        assert!(after_first > 0, "the first frame must actually upload");

        for _ in 0..10 {
            p.submit(frame_at(None, PlaneLayout::Nv12, pts)).unwrap();
        }
        assert_eq!(p.uploaded_bytes(), after_first, "re-uploaded an unchanged frame");
        assert_eq!(p.upload_count(), 1);
        assert_eq!(p.skipped_uploads(), 10);

        // ...but a NEW frame still uploads.
        p.submit(frame_at(None, PlaneLayout::Nv12, Duration::from_millis(533)))
            .unwrap();
        assert!(p.uploaded_bytes() > after_first, "a new frame was skipped");
    }

    /// A skipped upload must still return the previous buffer, or the pool
    /// bleeds out on any paused surface.
    #[test]
    fn a_skipped_upload_still_recycles() {
        let ctx = ctx();
        let mut p = EguiPresenter::new(&ctx);
        let pts = Duration::from_millis(500);
        p.submit(frame_at(Some(1), PlaneLayout::Nv12, pts)).unwrap();
        let returned = p.submit(frame_at(Some(2), PlaneLayout::Nv12, pts)).unwrap();
        assert_eq!(returned, Some(RecycleToken(1)), "skipped path leaked a buffer");
        assert_eq!(p.skipped_uploads(), 1);
    }

    /// The measured cost of this path, so a change to it is visible rather
    /// than argued about: RGBA8 is four bytes a pixel.
    #[test]
    fn uploaded_bytes_tracks_the_real_cost() {
        let ctx = ctx();
        let mut p = EguiPresenter::new(&ctx);
        p.submit(frame(None, PlaneLayout::Nv12)).unwrap();
        assert_eq!(p.uploaded_bytes(), 2 * 2 * 4);
    }

    #[test]
    fn no_deadline_means_no_repaint_is_scheduled() {
        let ctx = ctx();
        let mut p = EguiPresenter::new(&ctx);
        assert!(!p.schedule_repaint(), "idle must not request a repaint");
        p.set_deadline(Some(Instant::now() + Duration::from_millis(40)));
        assert!(p.schedule_repaint());
        assert!(p.next_deadline().is_some());
    }

    /// A deadline already in the past must still schedule -- saturating to zero
    /// rather than underflowing into a very long sleep.
    /// The battery bug: a paused player must stop asking for repaints. Without
    /// this it redraws at full rate forever, showing the same still frame.
    #[test]
    fn an_idle_presenter_schedules_nothing() {
        let ctx = ctx();
        let mut p = EguiPresenter::new(&ctx);
        p.set_deadline(Some(Instant::now() + Duration::from_millis(40)));
        assert!(p.schedule_repaint());

        p.idle();
        assert!(!p.schedule_repaint(), "a paused presenter kept waking the GPU");
        assert!(p.next_deadline().is_none());
    }

    /// The backstop for a caller that forgets `idle()`. A deadline nobody has
    /// refreshed for a second is abandoned, not merely late.
    #[test]
    fn an_abandoned_deadline_stops_scheduling() {
        let ctx = ctx();
        let mut p = EguiPresenter::new(&ctx);
        p.set_deadline(Some(Instant::now() - Duration::from_secs(5)));
        assert!(
            !p.schedule_repaint(),
            "a five-second-old deadline still requested a repaint"
        );
        assert!(p.next_deadline().is_none(), "the stale deadline was not cleared");
    }

    #[test]
    fn a_past_deadline_schedules_immediately() {
        let ctx = ctx();
        let mut p = EguiPresenter::new(&ctx);
        // Recently past -- a late frame, which must still repaint.
        p.set_deadline(Some(Instant::now() - Duration::from_millis(50)));
        assert!(p.schedule_repaint(), "a late frame must still be drawn");
    }
}
