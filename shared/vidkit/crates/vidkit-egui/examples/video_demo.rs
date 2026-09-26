//! A synthetic clip through the real pipeline, so the surface can be looked at.
//!
//! `docs/agents/HEADLESS_GUI_TESTING.md` is the reason this exists: the
//! letterbox arithmetic passes its own unit tests and that says nothing about
//! what reaches the screen. This generates NV12 frames, pushes them through the
//! real `EguiPresenter` (so the CPU converter and the texture upload are the
//! ones under test, not a stand-in) and draws them with `VideoView`.
//!
//! ```sh
//! cargo run -p vidkit-egui --features demo --example video_demo --ignore-rust-version
//! ```
//!
//! Headless:
//!
//! ```sh
//! export DISPLAY=:99 LIBGL_ALWAYS_SOFTWARE=1
//! Xvfb :99 -screen 0 1280x800x24 & sleep 3; openbox & sleep 2
//! ./target/debug/examples/video_demo & sleep 9
//! import -window root /tmp/shot.png
//! ```

use std::time::{Duration, Instant};

use eframe::egui;
use vidkit_core::frame::{ColorInfo, CpuPlanes, Frame, FrameData, PlaneLayout};
use vidkit_core::traits::Presenter;
use vidkit_egui::{bottom_bar, next_fit, EguiPresenter, Fit, Transport, VideoView};
use uikit_egui::{adaptive::Adaptive, skin::Skin, PaletteColors};

const W: usize = 640;
const H: usize = 360;

/// SMPTE-ish colour bars in NV12, with a moving vertical sweep so successive
/// frames differ and a stuck presenter is visible rather than plausible.
///
/// Bar values are BT.709 limited-range Y/Cb/Cr, so the CPU converter's range
/// expansion and matrix are both exercised: if either is wrong the bars come
/// out washed or hue-shifted, which is obvious on sight and invisible to a
/// compile.
fn nv12_bars(phase: f32) -> Frame {
    // (Y, Cb, Cr) for white, yellow, cyan, green, magenta, red, blue, black.
    const BARS: [(u8, u8, u8); 8] = [
        (235, 128, 128),
        (219, 16, 138),
        (188, 154, 16),
        (173, 42, 26),
        (78, 214, 230),
        (63, 102, 240),
        (32, 240, 118),
        (16, 128, 128),
    ];

    let mut y = vec![0u8; W * H];
    let (cw, ch) = (W / 2, H / 2);
    let mut uv = vec![128u8; cw * ch * 2];

    let sweep = ((phase.fract() * W as f32) as usize).min(W - 1);

    for row in 0..H {
        for col in 0..W {
            let bar = BARS[(col * BARS.len()) / W];
            // A brighter column marching left to right.
            let lift = if col.abs_diff(sweep) < 3 { 40u8 } else { 0 };
            y[row * W + col] = bar.0.saturating_add(lift);
        }
    }
    for row in 0..ch {
        for col in 0..cw {
            let bar = BARS[((col * 2) * BARS.len()) / W];
            uv[(row * cw + col) * 2] = bar.1;
            uv[(row * cw + col) * 2 + 1] = bar.2;
        }
    }

    Frame {
        data: FrameData::Cpu(CpuPlanes {
            data: vec![y, uv],
            strides: vec![W as u32, (cw * 2) as u32],
        }),
        width: W as u32,
        height: H as u32,
        layout: PlaneLayout::Nv12,
        color: ColorInfo::default(),
        pts: Duration::ZERO,
        duration: Some(Duration::from_millis(33)),
        recycle: None,
        grain: None,
    }
}

struct Demo {
    presenter: EguiPresenter,
    view: VideoView,
    transport: Transport,
    started: Instant,
    colors: PaletteColors,
    skin: Skin,
}

impl Demo {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let colors = vidkit_egui::install(&cc.egui_ctx, snapclone_theme::palette_by_id("dark"));
        Self {
            presenter: EguiPresenter::new(&cc.egui_ctx),
            view: {
                let mut v = VideoView::new().with_fit(Fit::Contain);
                // Tinted, not black. Black is the right default for a real
                // player and wrong for a demo whose test pattern ENDS in a
                // black bar: against a black letterbox the picture looks 7/8
                // its actual width, which is exactly the wrong conclusion I
                // drew from the first screenshot before reading the numbers.
                v.bar = egui::Color32::from_rgb(28, 24, 38);
                v
            },
            transport: Transport {
                duration: Some(Duration::from_secs(212)),
                playing: true,
                status: "NV12 640x360 · CPU convert".into(),
                ..Default::default()
            },
            started: Instant::now(),
            colors,
            skin: Skin::carved(),
        }
    }
}

impl eframe::App for Demo {
    // eframe 0.34 requires both; the house examples do the same.
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}

    #[allow(deprecated)]
    fn update(&mut self, ctx: &egui::Context, _f: &mut eframe::Frame) {
        let t = self.started.elapsed().as_secs_f32();

        if self.transport.playing {
            self.transport.position = Duration::from_secs_f32(t % 212.0);
            let _ = self.presenter.submit(nv12_bars(t * 0.25));
            // 30 fps, scheduled rather than free-running -- the whole point of
            // next_deadline.
            self.presenter
                .set_deadline(Some(Instant::now() + Duration::from_millis(33)));
        }
        self.presenter.schedule_repaint();

        egui::TopBottomPanel::bottom("transport").show(ctx, |ui| {
            let adaptive = Adaptive::detect(ui.ctx());
            let act = bottom_bar(
                ui,
                &mut self.transport,
                self.view.fit,
                &self.colors,
                &self.skin,
                &adaptive,
            );
            if act.toggle_play {
                self.transport.playing = !self.transport.playing;
            }
            if let Some(to) = act.seek_to {
                self.transport.position = to;
            }
            if act.rotate {
                self.transport.rotation = vidkit_egui::apply_rotation(self.transport.rotation, 1);
            }
            if act.cycle_fit {
                self.view.fit = next_fit(self.view.fit);
            }
            if act.reset_view {
                self.transport.zoom = 1.0;
                self.transport.rotation = 0;
            }
        });

        if !Adaptive::detect(ctx).is_compact() {
        egui::TopBottomPanel::bottom("hints").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.small("drag ↔ scrub (higher = faster)");
                ui.separator();
                ui.small("left ↕ volume");
                ui.separator();
                ui.small("right ↕ speed");
                ui.separator();
                ui.small("right edge ↕ rotate");
                ui.separator();
                ui.small("pinch: 5% zoom, far = fit/fill");
                ui.separator();
                ui.small("tap: play/pause");
            });
        });
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            let (_r, action) = self.view.show(ui, &self.presenter, &mut self.transport);
            if action.toggle_play {
                self.transport.playing = !self.transport.playing;
            }
            if let Some(to) = action.seek_to {
                self.transport.position = to;
            }
        });
    }
}

fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([900.0, 620.0]),
        ..Default::default()
    };
    eframe::run_native(
        "vidkit-video-demo",
        opts,
        Box::new(|cc| Ok(Box::new(Demo::new(cc)))),
    )
}
