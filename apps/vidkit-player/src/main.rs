//! vidkit-player — the runnable shell over `shared/vidkit`.
//!
//! This exists because the kit had no way to run. Everything was libraries plus
//! one feature-gated `example`, so `cargo run` did nothing, nothing installed,
//! and gitmind's catalogue correctly refused to offer a build button for a
//! `role = "shared-lib"` entry that ships no binary. An app nobody can start is
//! indistinguishable from an app that does not work.
//!
//! ```sh
//! vidkit-player            # built-in test pattern, no file needed
//! vidkit-player clip.mp4   # a local MP4 or WebM
//! ```
//!
//! There is deliberately no decoder yet: nothing in this tree decodes H.264,
//! VP9 or AV1. Opening a real file therefore demuxes it, reports what it found,
//! and shows the test pattern in the video surface. That is an honest partial
//! result — the container, codec, resolution and duration on screen are read
//! from the file — and it beats a black window that cannot say why.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;
use uikit_egui::{adaptive::Adaptive, skin::Skin, PaletteColors};
use vidkit_core::io::MemorySource;
use vidkit_core::traits::{MediaSource, Presenter};
use vidkit_egui::{bottom_bar, next_fit, EguiPresenter, Fit, Transport, VideoView};

mod pattern;

/// What we managed to learn about the file the user named.
struct Opened {
    label: String,
    duration: Option<Duration>,
}

/// Demux a local file far enough to describe it.
///
/// Container is sniffed rather than taken from the extension: a `.mp4` holding
/// WebM is rare and a file with no extension is not, and the magic is the file
/// whereas the name is a guess.
fn open_local(path: &PathBuf) -> Result<Opened, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let head = bytes.get(..16).unwrap_or(&bytes).to_vec();

    let is_webm = head.len() >= 4 && head[0..4] == [0x1A, 0x45, 0xDF, 0xA3];
    let is_mp4 = head.len() >= 8
        && matches!(
            &head[4..8],
            b"ftyp" | b"styp" | b"moov" | b"free" | b"skip" | b"mdat"
        );

    let (kind, profile, dims, dur, packets) = if is_webm {
        let mut d = vidkit_webm::WebmDemuxer::open(MemorySource(bytes))
            .map_err(|e| format!("webm: {e}"))?;
        let (p, dims, dur) = (d.video_profile(), d.dimensions(), d.duration());
        let mut n = 0;
        while matches!(d.next_packet(), Ok(Some(_))) {
            n += 1;
        }
        ("WebM", p, dims, dur, n)
    } else if is_mp4 {
        let mut d =
            vidkit_mp4::Mp4Demuxer::open(MemorySource(bytes)).map_err(|e| format!("mp4: {e}"))?;
        let (p, dims, dur) = (d.video_profile(), d.dimensions(), d.duration());
        let mut n = 0;
        while matches!(d.next_packet(), Ok(Some(_))) {
            n += 1;
        }
        ("MP4", p, dims, dur, n)
    } else {
        return Err(format!(
            "{}: not an MP4 or WebM file (first bytes {:02x?})",
            path.display(),
            &head[..head.len().min(8)]
        ));
    };

    let codec = profile
        .map(|p| format!("{} {}-bit", p.codec, p.bit_depth))
        .unwrap_or_else(|| "unknown codec".into());

    let _ = packets;
    Ok(Opened {
        label: format!(
            "{} · {kind} · {codec} · {}x{} · {packets} packets",
            path.file_name().unwrap_or_default().to_string_lossy(),
            dims.0,
            dims.1
        ),
        duration: dur,
    })
}

struct Player {
    presenter: EguiPresenter,
    view: VideoView,
    transport: Transport,
    colors: PaletteColors,
    skin: Skin,
    started: Instant,
    /// `Err` is shown rather than swallowed: a player that opens nothing and
    /// says nothing is the failure this whole binary exists to avoid.
    opened: Option<Result<Opened, String>>,
}

impl Player {
    fn new(cc: &eframe::CreationContext<'_>, file: Option<PathBuf>) -> Self {
        let colors = vidkit_egui::install(&cc.egui_ctx, snapclone_theme::palette_by_id("dark"));
        let opened = file.map(|p| open_local(&p));

        let status = match &opened {
            Some(Ok(_)) => "NV12 · CPU convert".to_string(),
            Some(Err(_)) => "not playing".to_string(),
            None => "test pattern · NV12 · CPU convert".into(),
        };
        let duration = match &opened {
            Some(Ok(o)) => o.duration,
            _ => Some(Duration::from_secs(212)),
        };

        Self {
            presenter: EguiPresenter::new(&cc.egui_ctx),
            view: VideoView::new().with_fit(Fit::Contain),
            transport: Transport {
                duration,
                playing: true,
                status,
                ..Default::default()
            },
            colors,
            skin: Skin::carved(),
            started: Instant::now(),
            opened,
        }
    }

    /// Feed the presenter. Until a decoder exists this is the test pattern —
    /// stated on screen rather than implied.
    fn produce(&mut self) {
        let t = self.started.elapsed().as_secs_f32();
        let frame = pattern::bars(t * 0.25);
        let _ = self.presenter.submit(frame);
        self.presenter
            .set_deadline(Some(Instant::now() + Duration::from_millis(33)));
    }
}

impl eframe::App for Player {
    fn ui(&mut self, _ui: &mut egui::Ui, _f: &mut eframe::Frame) {}

    #[allow(deprecated)]
    fn update(&mut self, ctx: &egui::Context, _f: &mut eframe::Frame) {
        if self.transport.playing {
            let t = self.started.elapsed().as_secs_f64() * self.transport.speed;
            if let Some(d) = self.transport.duration {
                self.transport.position = Duration::from_secs_f64(t % d.as_secs_f64().max(1.0));
            }
            self.produce();
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

        // What is playing, always visible.
        //
        // This used to live only in the bar's status field, which the adaptive
        // layout drops on anything narrower than a desktop -- so at 960 points
        // the app opened a file, parsed it, and said nothing about it. "What am
        // I playing" is the primary question a player answers; it does not
        // belong in the first field to be dropped for space.
        egui::TopBottomPanel::top("what").show(ctx, |ui| match &self.opened {
            Some(Err(e)) => {
                ui.colored_label(egui::Color32::from_rgb(220, 120, 120), e);
            }
            Some(Ok(o)) => {
                ui.horizontal_wrapped(|ui| {
                    ui.small(&o.label);
                    ui.separator();
                    // Said plainly rather than left to be inferred from a
                    // picture that does not match the file's dimensions.
                    ui.small("no decoder yet — showing the test pattern");
                });
            }
            None => {
                ui.small("test pattern — pass a .mp4 or .webm to open one");
            }
        });

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
    let mut args = std::env::args().skip(1);
    let first = args.next();

    if matches!(first.as_deref(), Some("-h") | Some("--help")) {
        println!("vidkit-player [FILE]\n");
        println!("  FILE   a local .mp4 or .webm; omit for the built-in test pattern");
        println!("\nGestures: drag to scrub (rate rises with height), left half");
        println!("vertical for volume, right half for speed, right edge to rotate,");
        println!("pinch to zoom, tap to play/pause.");
        return Ok(());
    }

    let file = first.map(PathBuf::from);

    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            // An app id, or Wayland shows the binary name and no icon.
            .with_app_id("vidkit-player")
            .with_inner_size([960.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "vidkit-player",
        opts,
        Box::new(|cc| Ok(Box::new(Player::new(cc, file)))),
    )
}
