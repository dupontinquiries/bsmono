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
    last_error: Option<String>,
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
            last_error: None,
        }
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
    pub fn schedule_repaint(&self) -> bool {
        match self.deadline {
            Some(at) => {
                let now = Instant::now();
                let after = at.saturating_duration_since(now);
                self.ctx.request_repaint_after(after);
                true
            }
            None => false,
        }
    }
}

impl Presenter for EguiPresenter {
    fn submit(&mut self, frame: Frame) -> core_traits::Result<Option<RecycleToken>> {
        let (w, h) = (frame.width as usize, frame.height as usize);

        match vidkit_core::frame_to_rgba8(&frame) {
            Ok(rgba) => {
                let image = ColorImage::from_rgba_unmultiplied([w, h], &rgba);
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
                self.last_error = None;
            }
            Err(e) => {
                // Drop the picture, keep the buffer. Counting this as a drop
                // rather than an error means a single unsupported frame — a
                // 10-bit one arriving mid-stream, say — costs a frame and not
                // the session.
                self.drops += 1;
                self.last_error = Some(e.to_string());
            }
        }

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

    fn frame(token: Option<u32>, layout: PlaneLayout) -> Frame {
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
            pts: Duration::ZERO,
            duration: None,
            recycle: token.map(RecycleToken),
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

        // P010 is 10-bit; the CPU converter refuses it rather than misreading.
        let returned = p.submit(frame(Some(2), PlaneLayout::P010)).unwrap();
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
    #[test]
    fn a_past_deadline_schedules_immediately() {
        let ctx = ctx();
        let mut p = EguiPresenter::new(&ctx);
        p.set_deadline(Some(Instant::now() - Duration::from_secs(5)));
        assert!(p.schedule_repaint());
    }
}
