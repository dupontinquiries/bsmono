#!/usr/bin/env python3
"""Mutation testing for vidkit.

A green test suite proves nothing on its own. This injects one realistic bug at
a time, runs the tests, and reports which bugs nobody noticed. A surviving
mutant is a piece of behaviour that is asserted nowhere.

Each mutation is a hand-written plausible defect rather than a random operator
flip, because the interesting question is not "can the suite detect noise" but
"would it catch the mistake a tired person actually makes at 1am".

    python3 tools/mutate.py            # run all
    python3 tools/mutate.py caps       # run mutants whose id contains 'caps'
"""

import os
import subprocess
import sys
import pathlib

# The Vulkan tests skip when there is no device, and a skipped test kills no
# mutant. Point at Mesa's lavapipe if nothing else is configured, so the shader
# mutants below are actually executed rather than quietly surviving.
LVP_ICD = "/usr/share/vulkan/icd.d/lvp_icd.json"

# This tree wants rustc >= 1.91 (shared/uikit's lockfile is v4, which cargo
# 1.75 cannot parse) and egui 0.34 declares MSRV 1.92 while building fine on
# 1.91. Prefer a packaged 1.91 when the default toolchain is older, so the
# sweep does not report every mutant as "did not compile".
RUST_1_91_BIN = "/usr/lib/rust-1.91/bin"

ROOT = pathlib.Path(__file__).resolve().parent.parent

# (id, relative path, find, replace)
MUTANTS = [
    # --- capability matching -------------------------------------------
    ("caps/prefer-worst-path", "crates/vidkit-core/src/caps.rs",
     ".min_by_key(|c| c.cost)", ".max_by_key(|c| c.cost)"),
    ("caps/bitdepth-comparison", "crates/vidkit-core/src/caps.rs",
     "if p.bit_depth > self.max_bit_depth {", "if p.bit_depth < self.max_bit_depth {"),
    ("caps/ignore-fps-budget", "crates/vidkit-core/src/caps.rs",
     "(fps as u64) <= budget", "true"),
    ("caps/ignore-max-size", "crates/vidkit-core/src/caps.rs",
     "if w > self.max_width || h > self.max_height {", "if false {"),

    # --- format selection ----------------------------------------------
    ("select/cost-ordering-reversed", "crates/vidkit-ytdlp/src/select.rs",
     "other\n            .cost\n            .cmp(&self.cost)", "self.cost\n            .cmp(&other.cost)"),
    ("select/prefer-higher-bitrate", "crates/vidkit-ytdlp/src/select.rs",
     ".then(other.bitrate_bps.cmp(&self.bitrate_bps))", ".then(self.bitrate_bps.cmp(&other.bitrate_bps))"),
    ("select/no-fps-clamp", "crates/vidkit-ytdlp/src/select.rs",
     "effective_fps: fps.min(b.display_max_hz),", "effective_fps: fps,"),
    ("select/no-resolution-clamp", "crates/vidkit-ytdlp/src/select.rs",
     "let effective_pixels = (sl.min(dl) as u64) * (ss.min(ds) as u64);",
     "let effective_pixels = (sl as u64) * (ss as u64);"),
    ("select/ignore-bitrate-cap", "crates/vidkit-ytdlp/src/select.rs",
     "if bps > cap {", "if false {"),

    # --- sync policy ----------------------------------------------------
    ("clock/never-drop-late", "crates/vidkit-core/src/clock.rs",
     "if behind > self.late_limit {", "if false {"),
    ("clock/no-slack", "crates/vidkit-core/src/clock.rs",
     "slack: interval / 2,", "slack: Duration::ZERO,"),
    ("clock/rate-rewrites-history", "crates/vidkit-core/src/clock.rs",
     "self.banked += self.elapsed_media();\n        self.origin = Instant::now();\n        self.rate = rate.clamp(0.25, 4.0);",
     "self.rate = rate.clamp(0.25, 4.0);"),

    # --- frame queue ----------------------------------------------------
    ("queue/leak-discarded-buffers", "crates/vidkit-core/src/queue.rs",
     "self.reclaimed.push(t);", "let _ = t;"),
    ("queue/drop-newest-under-pressure", "crates/vidkit-core/src/queue.rs",
     "if let Some(old) = self.frames.pop_front() {\n                    self.discard(old);\n                }\n                self.dropped_pressure += 1;",
     "self.dropped_pressure += 1;\n                return Some(frame);\n                #[allow(unreachable_code)]"),
    ("queue/flush-forgets-restart", "crates/vidkit-core/src/queue.rs",
     "        self.started = false;\n    }\n}", "        self.started = true;\n    }\n}"),
    ("queue/drain-one-late-frame-per-poll", "crates/vidkit-core/src/queue.rs",
     "                    self.dropped_late += 1;\n                    // ...and re-examine the next one.",
     "                    self.dropped_late += 1;\n                    return Poll::Starved;"),

    # --- player ---------------------------------------------------------
    ("player/drop-refused-packet", "crates/vidkit-core/src/player.rs",
     "                Ok(false) => {\n                    self.pending = Some(pkt);", "                Ok(false) => {\n                    drop(pkt);"),
    ("player/format-change-keeps-queue", "crates/vidkit-core/src/player.rs",
     "                    self.queue.flush();\n                    self.reclaim_queue()?;\n                    let released = self.presenter.release_all();",
     "                    let released = self.presenter.release_all();"),
    ("player/format-change-strands-presenter-frame", "crates/vidkit-core/src/player.rs",
     "                    let released = self.presenter.release_all();\n                    self.give_back(released)?;\n                    return Ok(Some(Event::FormatChanged { width, height }));",
     "                    return Ok(Some(Event::FormatChanged { width, height }));"),
    ("player/starved-asks-for-a-sleep", "crates/vidkit-core/src/player.rs",
     "Ok(Event::Idle { wake_in: None })", "Ok(Event::Idle { wake_in: Some(Duration::from_millis(16)) })"),
    ("player/never-recycle-presented", "crates/vidkit-core/src/player.rs",
     "                if let Some(t) = released {\n                    self.decoder.recycle(t)?;\n                }", ""),
    ("player/seek-forgets-buffers", "crates/vidkit-core/src/player.rs",
     "        let released = self.presenter.release_all();\n        self.give_back(released)?;\n        self.queue.flush();\n        self.reclaim_queue()?;",
     "        self.queue.flush();"),
    ("player/unbounded-packet-loop", "crates/vidkit-core/src/player.rs",
     "for _ in 0..self.budget.packets_per_tick {", "for _ in 0..u32::MAX {"),
    ("player/strand-overflow-frame", "crates/vidkit-core/src/player.rs",
     "                        if let Some(t) = returned.recycle {\n                            self.decoder.recycle(t)?;\n                        }", "                        drop(returned);"),

    # --- V4L2 state machine ---------------------------------------------
    ("v4l2/no-generation-guard", "crates/vidkit-v4l2/src/stateful.rs",
     "        if gen != self.generation {\n            return Ok(());\n        }", "        let _ = gen;"),
    ("v4l2/streamoff-output-on-resolution-change", "crates/vidkit-v4l2/src/stateful.rs",
     "        self.api.stream_off(Queue::Capture)?;\n        // REQBUFS(0) frees",
     "        self.api.stream_off(Queue::Output)?;\n        self.api.stream_off(Queue::Capture)?;\n        // REQBUFS(0) frees"),
    ("v4l2/no-capture-headroom", "crates/vidkit-v4l2/src/stateful.rs",
     "let want = min + CAPTURE_HEADROOM;", "let want = min;"),
    ("v4l2/streamon-capture-before-queueing", "crates/vidkit-v4l2/src/stateful.rs",
     "        for i in 0..got {\n            self.api.queue_capture(i)?;\n        }\n        self.api.stream_on(Queue::Capture)?;",
     "        self.api.stream_on(Queue::Capture)?;\n        for i in 0..got {\n            self.api.queue_capture(i)?;\n        }"),
    ("v4l2/setup-capture-before-source-change", "crates/vidkit-v4l2/src/stateful.rs",
     "        self.api.stream_on(Queue::Output)?;\n        self.state = State::AwaitingSourceChange;",
     "        self.api.stream_on(Queue::Output)?;\n        self.configure_capture()?;\n        self.state = State::AwaitingSourceChange;"),
    ("v4l2/errored-buffer-not-requeued", "crates/vidkit-v4l2/src/stateful.rs",
     "            // Errored or empty: straight back into the pool.\n            self.api.queue_capture(index)?;", ""),
    ("v4l2/flush-skips-requeue", "crates/vidkit-v4l2/src/stateful.rs",
     "        for i in 0..self.capture_count {\n            self.api.queue_capture(i)?;\n        }\n\n        self.api.stream_on(Queue::Output)?;",
     "        self.api.stream_on(Queue::Output)?;"),
    ("v4l2/accepts-packets-while-draining", "crates/vidkit-v4l2/src/stateful.rs",
     "            other => return Err(DecodeError::BadState(other)),\n        }\n        // Reclaim consumed input",
     "            _ => {}\n        }\n        // Reclaim consumed input"),

    # --- audio clock -----------------------------------------------------
    ("audio/latency-sign", "crates/vidkit-core/src/audio_clock.rs",
     "self.frames_written.saturating_sub(self.latency_frames)", "self.frames_written.saturating_add(self.latency_frames)"),
    ("audio/underrun-ignored", "crates/vidkit-core/src/audio_clock.rs",
     "self.frames_written = self.frames_written.saturating_sub(frames);", "let _ = frames;"),
    ("audio/resume-replays-frames", "crates/vidkit-core/src/audio_clock.rs",
     "            self.base = self.paused_at;\n            self.frames_written = 0;\n            self.latency_frames = 0;\n            self.paused = false;",
     "            self.base = self.paused_at;\n            self.paused = false;"),
    ("audio/seek-keeps-stale-frames", "crates/vidkit-core/src/audio_clock.rs",
     "        self.frames_written = 0;\n        self.latency_frames = 0;\n    }", "    }"),
    ("audio/rate-applied-twice", "crates/vidkit-core/src/audio_clock.rs",
     "        self.base + self.frames_to_duration(self.audible_frames())\n    }\n\n    fn set_rate",
     "        self.base + self.frames_to_duration((self.audible_frames() as f64 * self.rate) as u64)\n    }\n\n    fn set_rate"),

    # --- yt-dlp resolution ------------------------------------------------
    ("resolve/expiry-never-parsed", "crates/vidkit-ytdlp/src/resolve.rs",
     "        if k == \"expire\" {", "        if false {"),
    ("resolve/no-refresh-margin", "crates/vidkit-ytdlp/src/resolve.rs",
     "Some(exp) => now + self.refresh_margin >= exp,", "Some(exp) => now >= exp,"),
    ("resolve/refresh-reselects-always", "crates/vidkit-ytdlp/src/resolve.rs",
     "let wanted = self.current.as_ref().map(|r| r.video_format_id.clone());",
     "let wanted: Option<String> = None;"),
    ("resolve/latest-expiry-wins", "crates/vidkit-ytdlp/src/resolve.rs",
     "        (Some(x), Some(y)) => Some(x.min(y)),\n        (x, y) => x.or(y),\n    };\n\n    Some(Resolved {",
     "        (Some(x), Some(y)) => Some(x.max(y)),\n        (x, y) => x.or(y),\n    };\n\n    Some(Resolved {"),
    ("resolve/no-flag-guard", "crates/vidkit-ytdlp/src/resolve.rs",
     "    a.push(\"--\".into());\n", ""),
    ("resolve/live-accepted", "crates/vidkit-ytdlp/src/resolve.rs",
     "if probe.is_live == Some(true) {", "if false {"),
    ("resolve/drops-http-headers", "crates/vidkit-ytdlp/src/resolve.rs",
     "        headers: v.http_headers.clone(),\n        width: v.width.unwrap_or(0),\n        height: v.height.unwrap_or(0),\n        fps: v.fps_or_guess(),\n        expires_at,",
     "        headers: Default::default(),\n        width: v.width.unwrap_or(0),\n        height: v.height.unwrap_or(0),\n        fps: v.fps_or_guess(),\n        expires_at,"),

    # --- mp4 demux ---------------------------------------------------------
    ("mp4/stsc-first-chunk-zero-based", "crates/vidkit-mp4/src/moov.rs",
     "            let base = *chunk_offsets\n                .get(chunk as usize - 1)",
     "            let base = *chunk_offsets\n                .get(chunk as usize)"),
    ("mp4/stsc-last-entry-not-open-ended", "crates/vidkit-mp4/src/moov.rs",
     "            None => chunk_offsets.len() as u32,", "            None => first_chunk,"),
    ("mp4/samples-contiguous-across-chunks", "crates/vidkit-mp4/src/moov.rs",
     "            let mut running = base;", "            let mut running = if offsets.is_empty() { base } else { *offsets.last().unwrap() + sizes[sample - 1] as u64 };"),
    ("mp4/ignore-ctts", "crates/vidkit-mp4/src/moov.rs",
     "        Some(c) => sample_ctts(&parse_ctts(c)?, n),", "        Some(_) => vec![0i32; n],"),
    ("mp4/negative-ctts-wraps", "crates/vidkit-mp4/src/moov.rs",
     "                d.saturating_sub((-cts[i]) as u64)", "                d.wrapping_sub((-cts[i]) as u64)"),
    ("mp4/stss-zero-based", "crates/vidkit-mp4/src/moov.rs",
     "s.contains(&(i as u32 + 1))", "s.contains(&(i as u32))"),
    ("mp4/no-stss-means-no-sync", "crates/vidkit-mp4/src/moov.rs",
     ".unwrap_or(true),", ".unwrap_or(false),"),
    ("mp4/tkhd-width-offset", "crates/vidkit-mp4/src/moov.rs",
     "        (be_u32(&tk, 12)?, 76)", "        (be_u32(&tk, 12)?, 80)"),
    ("mp4/seek-ignores-sync", "crates/vidkit-mp4/src/demux.rs",
     "self.cursor = self.track.sync_sample_at_or_before(ticks);",
     "self.cursor = self.track.samples.iter().position(|s| s.dts >= ticks).unwrap_or(0);"),
    ("mp4/no-parameter-sets-after-seek", "crates/vidkit-mp4/src/demux.rs",
     "        self.needs_parameter_sets = true;\n        Ok(())", "        Ok(())"),
    ("mp4/never-send-parameter-sets", "crates/vidkit-mp4/src/demux.rs",
     "let with_ps = s.is_sync && self.needs_parameter_sets;", "let with_ps = false;"),
    ("mp4/skip-annexb-conversion", "crates/vidkit-mp4/src/demux.rs",
     "                let out = to_annex_b(&raw, cfg, with_ps).map_err(io)?;",
     "                let _ = cfg; let out = raw.clone();"),
    ("mp4/avcc-unmasked-length-size", "crates/vidkit-mp4/src/annexb.rs",
     "let length_size = ((data[4] & 0x03) + 1) as usize;", "let length_size = (data[4] as usize).wrapping_add(1) & 0xFF;"),
    ("mp4/avcc-sps-count-unmasked", "crates/vidkit-mp4/src/annexb.rs",
     "let num_sps = (data[at] & 0x1F) as usize;", "let num_sps = data[at] as usize;"),
    ("mp4/zero-nal-accepted", "crates/vidkit-mp4/src/annexb.rs",
     "        if len == 0 {\n            return Err(Error::Malformed(format!(\"zero-length NAL at {at}\")));\n        }", "        if len == 0 { at += 1; continue; }"),
    ("mp4/nal-overrun-clamped", "crates/vidkit-mp4/src/annexb.rs",
     "        if end > sample.len() {", "        if false {"),
    ("mp4/box-size-zero-loops", "crates/vidkit-mp4/src/boxes.rs",
     "        if h.end() <= self.cursor {", "        if false {"),
    ("mp4/box-past-limit-allowed", "crates/vidkit-mp4/src/boxes.rs",
     "    if end > limit {", "    if false {"),

    # --- gesture routing ---------------------------------------------------
    ("gest/zone-midline-flipped", "crates/vidkit-egui/src/gestures.rs",
     "    if at.x >= surface.center().x {", "    if at.x < surface.center().x {"),
    ("gest/no-axis-lock", "crates/vidkit-egui/src/gestures.rs",
     "            if a.total.length() < cfg.axis_lock_px {\n                return out;\n            }", ""),
    ("gest/axis-choice-inverted", "crates/vidkit-egui/src/gestures.rs",
     "            a.axis = Some(if a.total.x.abs() >= a.total.y.abs() {",
     "            a.axis = Some(if a.total.x.abs() < a.total.y.abs() {"),
    ("gest/zone-recomputed-mid-drag", "crates/vidkit-egui/src/gestures.rs",
     "            (Some(DragAxis::Vertical), zone) => {",
     "            (Some(DragAxis::Vertical), _) => {\n                let zone = zone_at(surface, to, &cfg);"),
    ("gest/scrub-speed-inverted", "crates/vidkit-egui/src/gestures.rs",
     "    let from_bottom = ((surface.max.y - at.y) / h).clamp(0.0, 1.0) as f64;",
     "    let from_bottom = ((at.y - surface.min.y) / h).clamp(0.0, 1.0) as f64;"),
    ("gest/scrub-height-ignored", "crates/vidkit-egui/src/gestures.rs",
     "    cfg.scrub_span_at_bottom + (cfg.scrub_span_at_top - cfg.scrub_span_at_bottom) * from_bottom",
     "    let _ = from_bottom; cfg.scrub_span_at_bottom"),
    ("gest/detent-residue-dropped", "crates/vidkit-egui/src/gestures.rs",
     "                a.detent_residue += delta.y;\n                let mut emit",
     "                a.detent_residue = delta.y;\n                let mut emit"),
    ("gest/volume-direction-flipped", "crates/vidkit-egui/src/gestures.rs",
     "                while a.detent_residue <= -cfg.detent_px {\n                    a.detent_residue += cfg.detent_px;\n                    emit(1, &mut out);",
     "                while a.detent_residue <= -cfg.detent_px {\n                    a.detent_residue += cfg.detent_px;\n                    emit(-1, &mut out);"),
    ("gest/rotate-too-sensitive", "crates/vidkit-egui/src/gestures.rs",
     "                while a.detent_residue >= cfg.rotate_px {", "                while a.detent_residue >= 1.0 {"),
    ("gest/edge-strip-swallows-horizontal", "crates/vidkit-egui/src/gestures.rs",
     "            (Some(DragAxis::Horizontal), _) => {",
     "            (Some(DragAxis::Horizontal), Zone::RightEdge) => {}\n            (Some(DragAxis::Horizontal), _) => {"),
    ("gest/drag-also-taps", "crates/vidkit-egui/src/gestures.rs",
     "        if was_drag {\n            Vec::new()\n        } else {\n            vec![Intent::TogglePlay]\n        }",
     "        let _ = was_drag;\n        vec![Intent::TogglePlay]"),
    ("gest/no-scrub-commit", "crates/vidkit-egui/src/gestures.rs",
     "                out.push(Intent::ScrubCommit(a.scrub_seconds));", ""),
    ("gest/speed-not-quantised", "crates/vidkit-egui/src/gestures.rs",
     "    let stepped = (current * 20.0).round() as i32 + steps;\n    (stepped as f64 / 20.0).clamp(0.25, 4.0)",
     "    (current + steps as f64 * 0.05).clamp(0.25, 4.0)"),
    ("gest/volume-unclamped", "crates/vidkit-egui/src/gestures.rs",
     "    (stepped as f32 / 20.0).clamp(0.0, 1.0)", "    stepped as f32 / 20.0"),
    ("gest/rotation-no-wrap", "crates/vidkit-egui/src/gestures.rs",
     "    (current + quarter_turns).rem_euclid(4)", "    current + quarter_turns"),
    ("gest/pinch-never-locks", "crates/vidkit-egui/src/gestures.rs",
     "        if self.pinch_scale >= self.cfg.lock_zoom_in {", "        if false {"),

    # --- view / bar ---------------------------------------------------------
    ("view/lock-keeps-stale-zoom", "crates/vidkit-egui/src/view.rs",
     "            Intent::LockView(lock) => {\n                t.zoom = 1.0;",
     "            Intent::LockView(lock) => {"),
    ("view/scrub-accumulator-not-reset", "crates/vidkit-egui/src/view.rs",
     "                let target = offset_position(t.position, t.duration, s);\n                self.scrub_seconds = 0.0;",
     "                let target = offset_position(t.position, t.duration, s);"),
    ("view/scrub-not-clamped", "crates/vidkit-egui/src/view.rs",
     "    Duration::from_secs_f64(target.clamp(0.0, hi.max(0.0)))",
     "    Duration::from_secs_f64(target.max(0.0))"),
    ("view/widget-changes-playback-itself", "crates/vidkit-egui/src/view.rs",
     "            Intent::TogglePlay => {\n                action.toggle_play = true;",
     "            Intent::TogglePlay => {\n                action.toggle_play = true;\n                t.playing = !t.playing;"),
    ("view/rotation-ignored-by-fit", "crates/vidkit-egui/src/view.rs",
     "    if quarter.rem_euclid(4) % 2 == 1 {\n        Vec2::new(raw.y, raw.x)\n    } else {\n        raw\n    }",
     "    let _ = quarter; raw"),
    ("bar/phone-layout-same-as-desktop", "crates/vidkit-egui/src/bar.rs",
     "            DeviceClass::Phone => Self {\n                // Dropping two words is worth a whole row here.\n                word_labels: false,",
     "            DeviceClass::Phone => Self {\n                word_labels: true,"),
    ("bar/fit-cycle-incomplete", "crates/vidkit-egui/src/bar.rs",
     "        Fit::Cover => Fit::Stretch,", "        Fit::Cover => Fit::Contain,"),
    ("bar/percent-truncates", "crates/vidkit-egui/src/bar.rs",
     '    format!("{}%", (v * 100.0).round() as i64)', '    format!("{}%", (v * 100.0) as i64)'),
    ("fit/rotation-aspect-lost", "crates/vidkit-egui/src/fit.rs",
     "    let display_w = frame_size.x * par.ratio();", "    let display_w = frame_size.x;"),
    ("presenter/token-released-immediately", "crates/vidkit-egui/src/presenter.rs",
     "        let previous = self.held.take();\n        self.held = frame.recycle.take();\n        Ok(previous)",
     "        Ok(frame.recycle.take())"),
    ("presenter/failed-frame-drops-buffer", "crates/vidkit-egui/src/presenter.rs",
     "                self.drops += 1;\n                self.last_error = Some(e.to_string());",
     "                self.drops += 1;\n                self.last_error = Some(e.to_string());\n                return Ok(None);"),

    # --- convert shader (needs a Vulkan device; lavapipe is enough) --------
    ("shader/cb-cr-swapped", "crates/vidkit-vk/shaders/convert.wgsl",
     "        cb = c.r;\n        cr = c.g;", "        cb = c.g;\n        cr = c.r;"),
    ("shader/no-range-expansion", "crates/vidkit-vk/shaders/convert.wgsl",
     "        yv = (yv - 16.0 / 255.0) * (255.0 / 219.0);", "        yv = yv;"),
    ("shader/chroma-not-centred", "crates/vidkit-vk/shaders/convert.wgsl",
     "        u  = (u  - 128.0 / 255.0) * (255.0 / 224.0);", "        u  = u * (255.0 / 224.0);"),
    ("shader/always-bt601", "crates/vidkit-vk/shaders/convert.wgsl",
     "    switch id {\n        case 0u: { // BT.601", "    switch 0u {\n        case 0u: { // BT.601"),
    ("shader/transparent-alpha", "crates/vidkit-vk/shaders/convert.wgsl",
     "vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0)",
     "vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 0.0)"),
    ("shader/no-clamp", "crates/vidkit-vk/shaders/convert.wgsl",
     "clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0))", "rgb"),
    ("shader/bounds-check-dropped", "crates/vidkit-vk/shaders/convert.wgsl",
     "    if gid.x >= p.width || gid.y >= p.height {\n        return;\n    }", ""),
    ("shader/plain-reinhard-tonemap", "crates/vidkit-vk/shaders/convert.wgsl",
     "    let knee = 0.8;\n    let v = x / max(peak, 1.0);\n    if v <= knee {\n        return v;\n    }",
     "    let knee = 0.8;\n    let v = x / max(peak, 1.0);\n    return v / (1.0 + v);\n    if v <= knee {\n        return v;\n    }"),
    ("shader/nv12-treated-as-planar", "crates/vidkit-vk/shaders/convert.wgsl",
     "    if p.plane_count == 2u {", "    if false {"),

    # --- webm demux --------------------------------------------------------
    ("webm/size-keeps-marker-bit", "crates/vidkit-webm/src/ebml.rs",
     "let mut v = (b[0] as u64) & ((1u64 << (8 - len)) - 1);", "let mut v = b[0] as u64;"),
    ("webm/id-strips-marker-bit", "crates/vidkit-webm/src/ebml.rs",
     "    let mut v = 0u32;\n    for byte in b.iter().take(len) {\n        v = (v << 8) | *byte as u32;\n    }",
     "    let mut v = (b[0] & ((1u8 << (8 - len)) - 1)) as u32;\n    for byte in b.iter().take(len).skip(1) {\n        v = (v << 8) | *byte as u32;\n    }"),
    ("webm/unknown-size-read-as-value", "crates/vidkit-webm/src/ebml.rs",
     "    Ok((if all_ones { None } else { Some(v) }, len))", "    Ok((Some(v), len))"),
    ("webm/zero-first-byte-accepted", "crates/vidkit-webm/src/ebml.rs",
     "    if first == 0 {\n        return None;\n    }", "    if first == 0 {\n        return Some(8);\n    }"),
    ("webm/no-sign-extension", "crates/vidkit-webm/src/ebml.rs",
     "    Ok(((raw << shift) as i64) >> shift)", "    Ok(raw as i64)"),
    ("webm/element-past-parent-allowed", "crates/vidkit-webm/src/ebml.rs",
     "        if end > self.limit {", "        if false {"),
    ("webm/non-container-unknown-size-allowed", "crates/vidkit-webm/src/ebml.rs",
     "                if !is_container(h.id) {", "                if false {"),
    ("webm/wrong-timestamp-scale-default", "crates/vidkit-webm/src/segment.rs",
     "pub const DEFAULT_TIMESTAMP_SCALE_NS: u64 = 1_000_000;", "pub const DEFAULT_TIMESTAMP_SCALE_NS: u64 = 1_000;"),
    ("webm/ignore-timestamp-scale", "crates/vidkit-webm/src/segment.rs",
     "    ticks.saturating_mul(scale_ns)", "    ticks.saturating_mul(1_000_000)"),
    ("webm/negative-block-ts-unsigned", "crates/vidkit-webm/src/segment.rs",
     "        cluster_ts.saturating_sub((-(rel as i32)) as u64)", "        cluster_ts.saturating_add(rel as u16 as u64)"),
    ("webm/ignore-track-filter", "crates/vidkit-webm/src/segment.rs",
     "                if track == track_number {\n                    // Lacing packs", "                if true {\n                    // Lacing packs"),
    ("webm/lacing-silently-accepted", "crates/vidkit-webm/src/segment.rs",
     "                    if flags & 0x06 != 0 {", "                    if false {"),
    ("webm/simpleblock-keyframe-flag-wrong-bit", "crates/vidkit-webm/src/segment.rs",
     "                        is_key: flags & 0x80 != 0,", "                        is_key: flags & 0x08 != 0,"),
    ("webm/blockgroup-always-keyframe", "crates/vidkit-webm/src/segment.rs",
     "                            is_key: !has_reference,", "                            is_key: true,"),
    ("webm/track-type-not-checked", "crates/vidkit-webm/src/segment.rs",
     "        if track_type != 1 {\n            continue;\n        }", "        let _ = track_type;"),
    ("webm/codec-id-nul-not-trimmed", "crates/vidkit-webm/src/segment.rs",
     "                        .trim_end_matches('\\0')\n                        .to_string();", "                        .to_string();"),
    ("webm/cluster-child-set-too-wide", "crates/vidkit-webm/src/segment.rs",
     "        if !is_cluster_child(h.id) {\n            return Ok(cursor);\n        }", "        if false { return Ok(cursor); }"),
    ("webm/seek-ignores-keyframes", "crates/vidkit-webm/src/lib.rs",
     "        self.cursor = self.keyframe_at_or_before(to.as_nanos() as u64);",
     "        self.cursor = self.blocks.iter().position(|b| b.timestamp_ns >= to.as_nanos() as u64).unwrap_or(0);"),

    # --- codec parsing -----------------------------------------------------
    ("codec/av1-bitdepth-default-10", "crates/vidkit-core/src/codec.rs",
     "p.parse::<u8>().ok()).unwrap_or(8);\n            // Field 5 is monochrome",
     "p.parse::<u8>().ok()).unwrap_or(10);\n            // Field 5 is monochrome"),
    ("select/no-source-pixel-penalty", "crates/vidkit-ytdlp/src/select.rs",
     ".then(other.source_pixels.cmp(&self.source_pixels))", ""),
    ("codec/audio-accepted-as-video", "crates/vidkit-core/src/codec.rs",
     "        _ => None,\n    }\n}", "        _ => Some(VideoProfile { codec: Codec::H264, profile: None, bit_depth: 8, chroma: Chroma::Yuv420, level: None }),\n    }\n}"),
]


# Mutants that are *expected* to survive, with the reason. A survivor here is
# a deliberate decision, not a gap. Anything not on this list that survives is
# untested behaviour and should either get a test or get deleted.
EXPECTED_SURVIVORS = {
    "shader/bounds-check-dropped":
        "survives on lavapipe: the out-of-range invocations from a non-multiple "
        "dispatch write nothing observable, either because naga emits its own "
        "bounds check or because the driver discards the stores. Which of the "
        "two is unverified, and neither is guaranteed on turnip or AGX, so the "
        "explicit guard stays",
    "mp4/box-size-zero-loops":
        "unreachable given the size checks in read_box_header (every encoding "
        "yields total >= 8, and the size==0 form is bounded by limit); kept as "
        "a hang guard because a future size encoding could break that",
    "player/strand-overflow-frame":
        "branch is unreachable today (is_full is checked immediately before the "
        "push); kept because push's contract allows it and a second producer "
        "would make it live",
}


def test_env():
    env = dict(os.environ)
    if "VK_ICD_FILENAMES" not in env and pathlib.Path(LVP_ICD).exists():
        env["VK_ICD_FILENAMES"] = LVP_ICD
    if pathlib.Path(RUST_1_91_BIN).is_dir():
        env["PATH"] = RUST_1_91_BIN + os.pathsep + env.get("PATH", "")
    # One crate at a time: parallel rustc on this box is killed silently.
    env.setdefault("CARGO_BUILD_JOBS", "1")
    return env


def have_vulkan():
    return "VK_ICD_FILENAMES" in test_env()


class OutOfDisk(Exception):
    """The build ran out of disk, which is not a test result."""


def free_mb():
    st = os.statvfs("/tmp")
    return st.f_bavail * st.f_frsize // (1024 * 1024)


def run_tests():
    r = subprocess.run(
        ["cargo", "test", "--quiet", "--ignore-rust-version"],
        cwd=ROOT, capture_output=True, text=True, timeout=900, env=test_env(),
    )
    # A full disk is the failure mode this sandbox actually hits, and cargo
    # reports it as a compile error -- which read as "baseline FAILING, fix the
    # suite" and sent a session looking for a broken test that did not exist.
    # It is worth its own exception: every mutant after it would be
    # misclassified as caught.
    blob = (r.stdout or "") + (r.stderr or "")
    if "No space left on device" in blob or "os error 28" in blob:
        raise OutOfDisk(f"{free_mb()} MB free on /tmp")
    return r.returncode == 0


INFLIGHT = ROOT / ".mutate-inflight"


def restore_inflight():
    """Put back a mutant stranded by a killed run.

    A mutation sweep that is killed mid-mutant -- a timed-out harness call, a
    Ctrl-C, an OOM -- leaves the source mutated, because `finally` never runs.
    That is worse than a failed run: the next command sees a broken tree and
    blames the tests, and in the worst case the mutation gets committed. So the
    original is written to disk BEFORE the edit and replayed on the next start.
    """
    if not INFLIGHT.exists():
        return
    import json

    try:
        rec = json.loads(INFLIGHT.read_text())
        target = ROOT / rec["path"]
        target.write_text(rec["original"])
        print(f"recovered a stranded mutant: restored {rec['path']} "
              f"(from {rec['id']})\n")
    except Exception as e:
        print(f"WARNING: could not replay {INFLIGHT}: {e}")
    finally:
        INFLIGHT.unlink(missing_ok=True)


def main():
    pattern = sys.argv[1] if len(sys.argv) > 1 else ""
    restore_inflight()
    selected = [m for m in MUTANTS if pattern in m[0]]

    if not have_vulkan():
        print("warning: no VK_ICD_FILENAMES and no lavapipe; shader mutants "
              "cannot be killed because their tests will skip\n")

    print(f"baseline: ", end="", flush=True)
    try:
        ok = run_tests()
    except OutOfDisk as e:
        print(f"OUT OF DISK ({e}) — free space and rerun; this is not a test failure")
        return 3
    if not ok:
        print("FAILING — fix the suite before mutating")
        return 2
    print(f"green  ({free_mb()} MB free)\n")

    survived, killed, broken = [], [], []

    for mid, rel, find, repl in selected:
        path = ROOT / rel
        original = path.read_text()
        if find not in original:
            broken.append(mid)
            print(f"  ????  {mid}  (pattern not found — mutant is stale)")
            continue
        if original.count(find) != 1:
            broken.append(mid)
            print(f"  ????  {mid}  (pattern matches {original.count(find)}x — ambiguous)")
            continue

        import json
        INFLIGHT.write_text(json.dumps({"id": mid, "path": rel, "original": original}))
        path.write_text(original.replace(find, repl, 1))
        try:
            passed = run_tests()
        except OutOfDisk as e:
            path.write_text(original)
            print(f"\nABORTED at {mid}: out of disk ({e}).")
            print("Every remaining mutant would be reported as killed for the "
                  "wrong reason, so the run stops here.")
            return 3
        finally:
            path.write_text(original)
            INFLIGHT.unlink(missing_ok=True)

        if passed:
            if mid in EXPECTED_SURVIVORS:
                print(f"  (ok)   {mid}  — {EXPECTED_SURVIVORS[mid]}")
            else:
                survived.append(mid)
                print(f"  LIVED  {mid}")
        else:
            killed.append(mid)
            print(f"  killed {mid}")

    total = len(killed) + len(survived)
    print(f"\n{len(killed)}/{total} killed", end="")
    if broken:
        print(f", {len(broken)} stale", end="")
    print()

    if survived:
        print("\nSurvivors — behaviour asserted nowhere:")
        for s in survived:
            print(f"  {s}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
