//! morph_pad — the 2D field beside the library.
//!
//! logscope has its matrix, beatspace its latent pad; this is the sample
//! library's version of the same gesture: load one to four SIMILAR sounds
//! (the classifier's near-neighbours, a converter's variants, four snares
//! from one pack) onto the pad's corners and the puck blends them — or,
//! with one sound loaded, reshapes it (X: spectral tilt, Y: decay). The
//! DSP is `studio_audio`'s (`morph_weights`/`tone_map`/`morph_blend`),
//! pinned headless including neutral-is-identity; this module owns only
//! the surface and the bounce.
//!
//! The pad's output enters the workflow through the library's OWN door:
//! right-click on the field bounces the current sound to
//! `<library>/morphs/` as a WAV, where the tree, the classifier and the
//! rack/clip buttons treat it like any other sample. Double right-click
//! resets the puck — and un-bounces the file the double's first click
//! just wrote, because a reset that leaves a junk bounce behind teaches
//! people to fear the reset.

use std::path::{Path, PathBuf};
use studio_audio::SourceAudio;

/// What library-bound renders are encoded as — a PER-DEVICE choice (the
/// workspace sidecar's), because storage constraints are per device: a
/// Fairphone and a studio desktop editing one project want different
/// answers. The slider's rungs ascend in storage; high-quality Opus is
/// the default because on most systems and most ears it IS the sound,
/// FLAC is the lossless rung for the rest, and WAV exists at the top for
/// whoever insists — never as anyone's default.
///
/// This is `audio_kit::BounceFormat`, re-exported under its old name:
/// beatspace's arrangement projects wanted the identical ladder (same
/// rungs, same tokens, same "never wav" default) and it was lifted out
/// from under this module rather than reinvented. `label`/`token`/
/// `from_token`/`ext`/`ALL` are unchanged, so nothing else in this file
/// or its sidecar format moved.
pub use audio_kit::BounceFormat;

/// The pad's whole state, owned by the shell.
#[derive(Default)]
pub struct PadState {
    /// Up to four (name, audio) anchors, corner order TL, TR, BL, BR.
    pub anchors: Vec<(String, SourceAudio)>,
    pub x: f32,
    pub y: f32,
    /// The last bounce this pad wrote — what a double right-click's reset
    /// removes again (its own first click wrote it a frame ago).
    pub last_bounce: Option<PathBuf>,
}

impl PadState {
    pub fn neutral() -> (f32, f32) { (0.5, 1.0) }
    /// Add an anchor; a full pad replaces the OLDEST — the pad is a
    /// scratch surface, not an archive, and refusing the fifth sound
    /// would make trying sounds feel like filing paperwork.
    pub fn push(&mut self, name: String, audio: SourceAudio) {
        if self.anchors.len() >= 4 { self.anchors.remove(0); }
        self.anchors.push((name, audio));
        if self.anchors.len() == 1 {
            let (x, y) = PadState::neutral();
            self.x = x; self.y = y;
        }
    }
    pub fn blend(&self) -> SourceAudio {
        let refs: Vec<&SourceAudio> = self.anchors.iter().map(|(_, a)| a).collect();
        studio_audio::morph_blend(&refs, self.x, self.y)
    }
}

/// Render the pad's current sound into `<dir>/morphs/` as a dithered WAV,
/// never overwriting — the exporter's unique-name rule. Returns the path,
/// which is also what a reset may need to remove again.
pub fn bounce(dir: &Path, state: &PadState, format: BounceFormat)
              -> Result<PathBuf, String> {
    let a = state.blend();
    if a.frames() == 0 {
        return Err("the pad is empty — add samples from the library's ⋯ menu".into());
    }
    let out_dir = dir.join("morphs");
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("create {out_dir:?}: {e}"))?;
    let stem = if state.anchors.len() == 1 {
        format!("{}-tone", sanitize_stem(&state.anchors[0].0))
    } else {
        format!("morph-{}", state.anchors.iter()
            .map(|(n, _)| sanitize_stem(n)).collect::<Vec<_>>().join("-"))
    };
    // Resolve what will actually be written BEFORE naming the file: a build
    // without Opus used to fall back to FLAC bytes inside a `.opus` file,
    // which the library then failed to decode as Opus.
    let format = match format {
        BounceFormat::Wav | BounceFormat::Flac => format,
        _ if !crate::OPUS_BUILT => BounceFormat::Flac,
        f => f,
    };
    let ext = format.ext();
    let mut path = out_dir.join(format!("{stem}.{ext}"));
    let mut n = 1u32;
    while path.exists() {
        path = out_dir.join(format!("{stem}-{n}.{ext}"));
        n += 1;
    }
    let seed = path.to_string_lossy().len() as u64;
    let bytes = match format {
        BounceFormat::Wav => studio_audio::export::export_wav_dithered_bytes(
            &a.channels[0], 1, 48_000, seed).map_err(|e| format!("wav: {e}"))?,
        BounceFormat::Flac => studio_media::flac::encode_flac(&a, seed)
            .map_err(|e| format!("flac: {e}"))?,
        f => {
            let bitrate = match f { BounceFormat::Opus32 => 32_000,
                                    BounceFormat::Opus64 => 64_000,
                                    BounceFormat::Opus128 => 128_000,
                                    BounceFormat::Opus256 => 256_000,
                                    _ => 192_000 };
            match studio_media::encode_opus(&a, bitrate) {
                Ok(b) => b,
                // A build without the opus feature falls to the lossless
                // rung, not to silence — bigger, never a hole.
                Err(_) => studio_media::flac::encode_flac(&a, seed)
                    .map_err(|e| format!("flac: {e}"))?,
            }
        }
    };
    std::fs::write(&path, bytes).map_err(|e| format!("write {path:?}: {e}"))?;
    Ok(path)
}

fn sanitize_stem(s: &str) -> String {
    let out: String = s.chars().take(24)
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' }).collect();
    if out.is_empty() { "sound".into() } else { out }
}

/// What the pad asked the shell to do this frame.
pub enum PadAction {
    /// The puck moved and settled: hear the blend.
    Preview,
    /// Right-click: bounce to the library, refresh the tree.
    Bounce,
    /// Double right-click: puck to neutral, and un-bounce the file the
    /// double's first click wrote.
    Reset,
    Remove(usize),
}

/// Draw the field. The hint line is ON the widget — the tooltip lesson,
/// applied at birth this time instead of retrofitted.
pub fn show(ui: &mut egui::Ui, state: &mut PadState,
            c: &uikit_egui::PaletteColors) -> Option<PadAction> {
    let mut action = None;
    if state.anchors.is_empty() {
        ui.colored_label(c.text_weak,
            "empty — send 1–4 similar sounds here from a sample's ⋯ menu");
        return None;
    }
    ui.horizontal_wrapped(|ui| {
        for (i, (name, _)) in state.anchors.iter().enumerate() {
            if ui.small_button(format!("{name} ✕")).clicked() {
                action = Some(PadAction::Remove(i));
            }
        }
    });
    let side = ui.available_width().min(220.0).max(120.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(side, side),
                                              egui::Sense::click_and_drag());
    let p = ui.painter();
    p.rect_filled(rect, 4.0, c.bg_base);
    p.rect_stroke(rect, 4.0, egui::Stroke::new(1.0_f32, c.border), egui::StrokeKind::Inside);
    // Corner labels, so the field says which sound lives where.
    let corners = [rect.left_top(), rect.right_top(), rect.left_bottom(), rect.right_bottom()];
    let aligns = [egui::Align2::LEFT_TOP, egui::Align2::RIGHT_TOP,
                  egui::Align2::LEFT_BOTTOM, egui::Align2::RIGHT_BOTTOM];
    for (i, (name, _)) in state.anchors.iter().enumerate().take(4) {
        p.text(corners[i], aligns[i], sanitize_stem(name),
               egui::FontId::proportional(9.0), c.text_weak);
    }
    if state.anchors.len() == 1 {
        p.text(rect.center_bottom() - egui::vec2(0.0, 2.0), egui::Align2::CENTER_BOTTOM,
               "◀ dark · bright ▶  /  ▲ long · short ▼",
               egui::FontId::proportional(9.0), c.text_weak);
    }
    let puck = egui::pos2(rect.left() + state.x * rect.width(),
                          rect.top() + (1.0 - state.y) * rect.height());
    p.circle_filled(puck, 6.0, c.accent);
    p.circle_stroke(puck, 6.0, egui::Stroke::new(1.5_f32, c.text));

    if let Some(pos) = resp.interact_pointer_pos() {
        if resp.dragged() || resp.clicked() {
            state.x = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            state.y = (1.0 - (pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
        }
    }
    if resp.drag_stopped() || resp.clicked() {
        action = Some(PadAction::Preview);
    }
    if resp.double_clicked_by(egui::PointerButton::Secondary) {
        action = Some(PadAction::Reset);
    } else if resp.secondary_clicked() {
        action = Some(PadAction::Bounce);
    }
    ui.colored_label(c.text_weak,
        "right-click: bounce to library · double right-click: reset");
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32) -> SourceAudio {
        SourceAudio { sample_rate: 48_000, channels: vec![
            (0..24_000).map(|i| (i as f32 / 48_000.0 * hz
                * std::f32::consts::TAU).sin() * 0.4).collect()] }
    }

    #[test]
    fn a_bounce_is_a_real_wav_in_the_librarys_own_door_and_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("studio-morph-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut pad = PadState::default();
        pad.push("Snare 1 (Clean)".into(), tone(300.0));
        pad.push("Snare 4".into(), tone(600.0));
        (pad.x, pad.y) = (0.3, 1.0);
        let first = bounce(&dir, &pad, BounceFormat::Opus192).expect("bounces");
        assert!(first.starts_with(dir.join("morphs")));
        // With Opus: high-quality opus. Without: FLAC, named .flac (never
        // FLAC bytes in a .opus file), still smaller than a wav.
        let want = if crate::OPUS_BUILT { "opus" } else { "flac" };
        assert_eq!(first.extension().unwrap(), want,
                   "the default is compressed, not a phone-filling wav");
        let back = studio_media::decode_bytes(std::fs::read(&first).unwrap(), Some(want))
            .expect("the library can decode its own bounce");
        assert_eq!(back.sample_rate, 48_000);
        assert!(back.frames() >= 20_000);
        let second = bounce(&dir, &pad, BounceFormat::Opus192).expect("bounces again");
        assert_ne!(first, second, "the exporter's unique-name rule holds here too");
        // The lossless rung: a real FLAC, decoded by symphonia, smaller
        // than the WAV rung for the same sound.
        let flac = bounce(&dir, &pad, BounceFormat::Flac).expect("flac bounces");
        assert!(studio_media::decode_bytes(std::fs::read(&flac).unwrap(),
                                           Some("flac")).is_ok());
        let wav = bounce(&dir, &pad, BounceFormat::Wav).expect("wav bounces");
        assert!(std::fs::metadata(&flac).unwrap().len()
                < std::fs::metadata(&wav).unwrap().len(),
                "lossless without the uncompressed bill");
        assert!(bounce(&dir, &PadState::default(), BounceFormat::Opus192).is_err(),
                "an empty pad refuses out loud");
    }

    #[cfg(feature = "audio")] // compares Opus bitrates
    #[test]
    fn voice_bitrate_rungs_bounce_smaller_than_music_ones() {
        let dir = std::env::temp_dir()
            .join(format!("studio-morph-voice-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut pad = PadState::default();
        pad.push("Narration".into(), tone(220.0));
        let thirty_two = bounce(&dir, &pad, BounceFormat::Opus32).expect("bounces at 32k");
        let sixty_four = bounce(&dir, &pad, BounceFormat::Opus64).expect("bounces at 64k");
        let one_two_eight = bounce(&dir, &pad, BounceFormat::Opus128).expect("bounces at 128k");
        // Actual bitstream size, not the requested rate — this is the thing
        // library management cares about, and it's the only way a build
        // without libopus (which falls back to FLAC for every Opus* rung,
        // see `bounce`) would still be caught making all three identical.
        let size = |p: &std::path::Path| std::fs::metadata(p).unwrap().len();
        assert!(size(&thirty_two) < size(&sixty_four),
                "32k rung should be smaller than 64k");
        assert!(size(&sixty_four) < size(&one_two_eight),
                "64k rung should be smaller than 128k");
        assert_eq!(BounceFormat::from_token("opus-32"), BounceFormat::Opus32);
        assert_eq!(BounceFormat::from_token("opus-64"), BounceFormat::Opus64);
        assert_eq!(BounceFormat::ALL.len(), 7, "32k and 64k joined the existing five rungs");
    }

    #[test]
    fn the_pad_holds_four_and_the_fifth_replaces_the_oldest() {
        let mut pad = PadState::default();
        for i in 0..5 {
            pad.push(format!("s{i}"), tone(100.0 + i as f32));
        }
        assert_eq!(pad.anchors.len(), 4);
        assert_eq!(pad.anchors[0].0, "s1", "the scratch surface forgets the oldest, not the newest");
        // One anchor resets the puck to neutral so tone-mapping starts
        // from the identity, never from a stale blend position.
        let mut one = PadState::default();
        one.x = 0.9; one.y = 0.1;
        one.push("kick".into(), tone(60.0));
        assert_eq!((one.x, one.y), PadState::neutral());
    }
}
