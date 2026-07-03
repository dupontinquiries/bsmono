use fao_v1;
use std::time::Instant;
use std::io::{Read, Write};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    
    // Subcommand dispatch.
    if args.len() >= 2 && args[1] == "library" {
        library_subcommand(&args[2..]);
        return;
    }
    if args.len() >= 2 && args[1] == "export" {
        export_subcommand(&args[2..]);
        return;
    }
    
    if args.len() < 4 {
        eprintln!("FAO CLI v1.0");
        eprintln!("");
        eprintln!("Usage:");
        eprintln!("  fao --pipe W H [-q N] [-dz N] [-cb N] [--no-recon]");
        eprintln!("    Pipe-mode encoder. Reads RGB frames from stdin, writes encoded frames to stdout.");
        eprintln!("");
        eprintln!("  fao library <subcommand> [args...]");
        eprintln!("    Manage a persistent FAO library (multi-video web sharing).");
        eprintln!("    Subcommands: init, list, info, verify");
        eprintln!("");
        eprintln!("  fao export <in.fao> <out.mkv> [--codec av1|h264] [--crf N] [--library PATH]");
        eprintln!("    Re-encode a .fao file to a portable format (Matroska container).");
        eprintln!("    Decodes frames through the FAO pipeline and pipes RGB to ffmpeg.");
        std::process::exit(1);
    }
    
    let (mut q, mut dz, mut cb) = (0.4f32, 1.0f32, 15i32);
    let (mut pw, mut ph, mut recon) = (0usize, 0usize, true);
    let mut pipe = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--pipe" => { 
                pipe = true; 
                pw = args.get(i+1).and_then(|s| s.parse().ok()).unwrap_or(0); 
                ph = args.get(i+2).and_then(|s| s.parse().ok()).unwrap_or(0); 
                i += 2; 
            }
            "--no-recon" => { recon = false; }
            "-q" => { q = args.get(i+1).and_then(|s| s.parse().ok()).unwrap_or(0.4); i += 1; }
            "-dz" => { dz = args.get(i+1).and_then(|s| s.parse().ok()).unwrap_or(1.0); i += 1; }
            "-cb" => { cb = args.get(i+1).and_then(|s| s.parse().ok()).unwrap_or(15); i += 1; }
            _ => {}
        }
        i += 1;
    }

    if !pipe {
        eprintln!("Error: --pipe mode required (file mode requires image crate, not included)");
        std::process::exit(1);
    }
    
    if pw == 0 || ph == 0 {
        eprintln!("Error: invalid dimensions {}x{}", pw, ph);
        std::process::exit(1);
    }

    let n = pw * ph * 3;
    let mut data = vec![0u8; n];
    std::io::stdin().read_exact(&mut data).expect("read");
    let t0 = Instant::now();
    
    if recon {
        let (rgb, tb, psnr) = fao_v1::encode_iframe_recon(&data, pw, ph, q, dz, cb);
        let el = t0.elapsed();
        std::io::stdout().write_all(&rgb).expect("write");
        std::io::stdout().flush().unwrap();
        let eb = (tb + 7) / 8;
        let rb = (pw * ph * 3) as u64;
        eprintln!(
            r#"{{"psnr":{:.2},"ratio":{:.2},"bpp":{:.3},"enc_bytes":{},"raw_bytes":{},"blocks":{},"time_ms":{:.1}}}"#,
            psnr,
            rb as f64 / eb as f64,
            tb as f64 / (pw * ph) as f64,
            eb,
            rb,
            (pw / 8) * (ph / 8),
            el.as_secs_f64() * 1000.0
        );
    } else {
        let n = pw * ph;
        let jqf = (5.0 + q * 93.0).round() as i32;
        let lq = fao_v1::build_qtable(&fao_v1::LUMA_Q, jqf);
        let cq = fao_v1::build_qtable(&fao_v1::CHROMA_Q, (jqf - cb).max(5));
        let mut y = vec![0.0f32; n];
        let mut cba = vec![0.0f32; n];
        let mut cr = vec![0.0f32; n];
        fao_v1::rgb_to_planes(&data, pw, ph, &mut y, &mut cba, &mut cr);
        let tb = fao_v1::encode_iframe(&y, &cba, &cr, pw, ph, &lq, &cq, dz, None, None, None);
        let el = t0.elapsed();
        let eb = (tb + 7) / 8;
        let rb = (pw * ph * 3) as u64;
        eprintln!(
            r#"{{"ratio":{:.2},"bpp":{:.3},"enc_bytes":{},"time_ms":{:.1}}}"#,
            rb as f64 / eb as f64,
            tb as f64 / (pw * ph) as f64,
            eb,
            el.as_secs_f64() * 1000.0
        );
    }
}


// ─────────────────────────────────────────────────────────────────────
// Library subcommand
// ─────────────────────────────────────────────────────────────────────

fn library_subcommand(args: &[String]) {
    if args.is_empty() {
        eprintln!("FAO library — multi-video web sharing");
        eprintln!("");
        eprintln!("Subcommands:");
        eprintln!("  init <path>          Create a new empty library at <path>");
        eprintln!("  list <path>          List all webs in the library");
        eprintln!("  info <path>          Print library summary (size, web count)");
        eprintln!("  verify <path>        Check that every manifest entry has a readable blob");
        std::process::exit(1);
    }
    
    use std::path::Path;
    
    match args[0].as_str() {
        "init" => {
            let p = match args.get(1) {
                Some(p) => Path::new(p),
                None => { eprintln!("error: missing <path>"); std::process::exit(1); }
            };
            match fao_v1::library::Library::init(p) {
                Ok(()) => println!("library initialized at {:?}", p),
                Err(e) => { eprintln!("error: {}", e); std::process::exit(1); }
            }
        }
        "list" => {
            let p = match args.get(1) {
                Some(p) => Path::new(p),
                None => { eprintln!("error: missing <path>"); std::process::exit(1); }
            };
            let lib = match fao_v1::library::Library::open(p) {
                Ok(l) => l,
                Err(e) => { eprintln!("error: {}", e); std::process::exit(1); }
            };
            println!("{:<32} {:>10} {:>10} {:>5}  {:<40}", 
                "uuid", "verts", "tris", "refs", "source_video");
            for stub in lib.list() {
                println!("{:<32} {:>10} {:>10} {:>5}  {:<40}",
                    stub.uuid, stub.vertex_count, stub.triangle_count, stub.ref_count,
                    truncate(&stub.source_video, 40));
            }
        }
        "info" => {
            let p = match args.get(1) {
                Some(p) => Path::new(p),
                None => { eprintln!("error: missing <path>"); std::process::exit(1); }
            };
            let lib = match fao_v1::library::Library::open(p) {
                Ok(l) => l,
                Err(e) => { eprintln!("error: {}", e); std::process::exit(1); }
            };
            let total = lib.total_disk_bytes();
            let total_verts: u64 = lib.manifest.webs.iter().map(|w| w.vertex_count as u64).sum();
            let total_tris: u64 = lib.manifest.webs.iter().map(|w| w.triangle_count as u64).sum();
            println!("FAO library at {:?}", p);
            println!("  version:        {}", lib.manifest.version);
            println!("  webs:           {}", lib.manifest.web_count);
            println!("  total vertices: {}", total_verts);
            println!("  total triangles:{}", total_tris);
            println!("  on-disk size:   {:.2} MiB", total as f64 / 1_048_576.0);
        }
        "verify" => {
            let p = match args.get(1) {
                Some(p) => Path::new(p),
                None => { eprintln!("error: missing <path>"); std::process::exit(1); }
            };
            let lib = match fao_v1::library::Library::open(p) {
                Ok(l) => l,
                Err(e) => { eprintln!("error: {}", e); std::process::exit(1); }
            };
            let mut ok = 0usize;
            let mut bad = 0usize;
            for stub in lib.list() {
                match lib.load_web(&stub.uuid) {
                    Ok(_) => ok += 1,
                    Err(e) => { eprintln!("  BAD {}: {}", stub.uuid, e); bad += 1; }
                }
            }
            println!("verified: {} ok, {} bad", ok, bad);
            if bad > 0 { std::process::exit(2); }
        }
        other => {
            eprintln!("error: unknown library subcommand '{}'", other);
            std::process::exit(1);
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max { s.to_string() }
    else { format!("...{}", &s[s.len().saturating_sub(max - 3)..]) }
}

// ═══════════════════════════════════════════════════════════════════
// fao export — re-encode .fao to portable format (AV1/H.264 in Matroska)
// ═══════════════════════════════════════════════════════════════════

fn export_subcommand(args: &[String]) {
    use std::path::Path;
    use std::process::{Command, Stdio};
    
    if args.len() < 2 {
        eprintln!("FAO export — re-encode .fao to portable format");
        eprintln!("");
        eprintln!("Usage:");
        eprintln!("  fao export <in.fao> <out.mkv> [--codec av1|h264] [--crf N] [--library PATH]");
        eprintln!("");
        eprintln!("Options:");
        eprintln!("  --codec   Target codec. 'av1' (default, libsvtav1) or 'h264' (libx264).");
        eprintln!("  --crf     Constant Rate Factor (quality). Default: 28 for av1, 23 for h264.");
        eprintln!("            Lower = higher quality + larger file. Reasonable range 18..40.");
        eprintln!("  --library Path to a FAO library to attach during decode. Required if the .fao");
        eprintln!("            contains V8LR (library reference) chunks; otherwise optional.");
        eprintln!("");
        eprintln!("This decodes each .fao frame through the FAO pipeline (mesh rasterizer for v8,");
        eprintln!("DCT pipeline for v2.1) and pipes raw RGB to ffmpeg, which writes the result as");
        eprintln!("a Matroska (.mkv) container. ffmpeg must be on $PATH.");
        std::process::exit(1);
    }
    
    let in_path = Path::new(&args[0]);
    let out_path = Path::new(&args[1]);
    let mut codec = "av1".to_string();
    let mut crf: Option<u32> = None;
    let mut library_path: Option<String> = None;
    
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--codec" => {
                i += 1;
                if i >= args.len() { eprintln!("--codec needs a value"); std::process::exit(1); }
                codec = args[i].clone();
                if codec != "av1" && codec != "h264" {
                    eprintln!("--codec must be 'av1' or 'h264' (got '{}')", codec);
                    std::process::exit(1);
                }
            }
            "--crf" => {
                i += 1;
                if i >= args.len() { eprintln!("--crf needs a value"); std::process::exit(1); }
                match args[i].parse::<u32>() {
                    Ok(n) if n <= 63 => crf = Some(n),
                    _ => { eprintln!("--crf must be 0..63 (got '{}')", args[i]); std::process::exit(1); }
                }
            }
            "--library" => {
                i += 1;
                if i >= args.len() { eprintln!("--library needs a path"); std::process::exit(1); }
                library_path = Some(args[i].clone());
            }
            other => {
                eprintln!("unknown option '{}'", other);
                std::process::exit(1);
            }
        }
        i += 1;
    }
    
    let effective_crf = crf.unwrap_or(if codec == "av1" { 28 } else { 23 });
    
    // ── Open the .fao ──
    eprintln!("Reading {}", in_path.display());
    let data = match std::fs::read(in_path) {
        Ok(d) => d,
        Err(e) => { eprintln!("error: failed to read {}: {}", in_path.display(), e); std::process::exit(1); }
    };
    let header = match fao_v1::read_fao_header(&data) {
        Some(h) => h,
        None => { eprintln!("error: not a valid .fao file (bad header)"); std::process::exit(1); }
    };
    let index = fao_v1::read_fao_index(&data, header.frame_count as usize);
    if index.is_empty() {
        eprintln!("error: .fao file has no frames in its index");
        std::process::exit(1);
    }
    let w = header.width as usize;
    let h = header.height as usize;
    let rgb_sz = w * h * 3;
    let fps = if header.fps_den > 0 {
        header.fps_num as f64 / header.fps_den as f64
    } else { 30.0 };
    
    eprintln!("  width: {}, height: {}, fps: {:.2}, frames: {}", w, h, fps, index.len());
    
    // ── V8 decoder setup (mesh trailer + optional library) ──
    // 
    // Scan the tail of the file for V8MX and V8LR magic bytes. Same 
    // approach as fao_app::read_v8_trailer: scan up to the last 512MB. 
    // V8MX trailer carries the web list (meshes + transforms); V8LR 
    // carries library references.
    let mut v8_decoder: Option<fao_v1::v8_reconstruction_codec::V8Decoder> = None;
    let scan_start = data.len().saturating_sub(512 * 1024 * 1024);
    let mut mx_pos: Option<usize> = None;
    let mut lr_pos: Option<usize> = None;
    if data.len() >= 8 {
        for i in scan_start..data.len().saturating_sub(4) {
            if &data[i..i+4] == b"V8MX" && mx_pos.is_none() { mx_pos = Some(i); }
            else if &data[i..i+4] == b"V8LR" && lr_pos.is_none() { lr_pos = Some(i); }
            if mx_pos.is_some() && lr_pos.is_some() { break; }
        }
    }
    if let Some(pos) = mx_pos {
        if let Some(webs) = fao_v1::v8_reconstruction_codec::V8Decoder::parse_v8mx(&data[pos..]) {
            let mut dec = fao_v1::v8_reconstruction_codec::V8Decoder::new(w as u32, h as u32);
            dec.webs = webs;
            eprintln!("  V8MX: {} webs decoded from trailer at offset {}", dec.webs.len(), pos);
            
            // V8LR refs
            if let Some(lr_off) = lr_pos {
                if let Some(refs) = fao_v1::v8_reconstruction_codec::read_v8lr_trailer(&data[lr_off..]) {
                    eprintln!("  V8LR: {} library references in trailer", refs.len());
                    dec.library_refs = refs;
                }
            }
            
            // Attach library if specified or required
            if let Some(libp) = &library_path {
                match fao_v1::library::Library::open(Path::new(libp)) {
                    Ok(lib) => {
                        eprintln!("  library attached: {} ({} webs)", libp, lib.manifest.web_count);
                        dec.attach_library(std::sync::Arc::new(std::sync::RwLock::new(lib)));
                    }
                    Err(e) => {
                        eprintln!("warning: failed to open library at {}: {}", libp, e);
                        eprintln!("  V8LR-referenced chunks will render as gray frames.");
                    }
                }
            } else if !dec.library_refs.is_empty() {
                eprintln!("warning: .fao has V8LR refs but no --library was provided.");
                eprintln!("  Those frames will render as gray. Pass --library PATH to fix.");
            }
            
            v8_decoder = Some(dec);
        }
    } else {
        eprintln!("  no V8MX trailer found — file has no v8 mesh data");
        eprintln!("  (v8/Z frames will fail to render; other codecs work normally)");
    }
    
    // ── Launch ffmpeg with rawvideo RGB input ──
    let mut ffmpeg_cmd = Command::new("ffmpeg");
    ffmpeg_cmd
        .args(["-y", "-hide_banner", "-loglevel", "warning"])
        .args(["-f", "rawvideo", "-pixel_format", "rgb24"])
        .args(["-video_size", &format!("{}x{}", w, h)])
        .args(["-framerate", &format!("{}", fps)])
        .args(["-i", "pipe:0"]);
    
    // Output codec selection
    match codec.as_str() {
        "av1" => {
            ffmpeg_cmd.args(["-c:v", "libsvtav1", "-crf", &effective_crf.to_string(), "-preset", "6"]);
        }
        "h264" => {
            ffmpeg_cmd.args(["-c:v", "libx264", "-crf", &effective_crf.to_string(), "-preset", "medium"]);
        }
        _ => unreachable!(),
    }
    ffmpeg_cmd.args(["-pix_fmt", "yuv420p"]);
    ffmpeg_cmd.arg(out_path);
    ffmpeg_cmd.stdin(Stdio::piped());
    
    eprintln!("Spawning ffmpeg: {} → {} ({}, crf={})",
        in_path.display(), out_path.display(), codec, effective_crf);
    let mut child = match ffmpeg_cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: failed to spawn ffmpeg: {}", e);
            eprintln!("  is ffmpeg installed and on $PATH?");
            std::process::exit(1);
        }
    };
    let mut stdin = child.stdin.take().expect("stdin piped");
    
    // ── Iterate frames, decode, pipe RGB ──
    let t_start = Instant::now();
    let mut prev_rgb: Vec<u8> = vec![64u8; rgb_sz];  // gray for skip-first-frame
    let mut n_decoded = 0usize;
    let mut n_skip = 0usize;
    let mut n_v8 = 0usize;
    let mut n_v2 = 0usize;
    let mut n_failed = 0usize;
    
    for (idx, entry) in index.iter().enumerate() {
        let rgb = decode_one_frame(&data, entry, idx, w, h, v8_decoder.as_ref(), &prev_rgb);
        let (rgb_out, kind) = match rgb {
            Some((r, k)) => (r, k),
            None => {
                n_failed += 1;
                (prev_rgb.clone(), "fail")
            }
        };
        match kind {
            "skip" => n_skip += 1,
            "v8"   => n_v8 += 1,
            "v2"   => n_v2 += 1,
            _      => {}
        }
        if let Err(e) = stdin.write_all(&rgb_out) {
            eprintln!("error: ffmpeg stdin write failed at frame {}: {}", idx, e);
            break;
        }
        prev_rgb = rgb_out;
        n_decoded += 1;
        if n_decoded % 60 == 0 || n_decoded == 1 {
            let elapsed = t_start.elapsed().as_secs_f64();
            let fps_out = n_decoded as f64 / elapsed.max(0.001);
            eprintln!("  frame {} / {} ({:.1} fps decode, {:.1}% done)",
                n_decoded, index.len(), fps_out,
                100.0 * n_decoded as f64 / index.len() as f64);
        }
    }
    
    drop(stdin);  // signal EOF to ffmpeg
    let status = match child.wait() {
        Ok(s) => s,
        Err(e) => { eprintln!("error: ffmpeg wait failed: {}", e); std::process::exit(1); }
    };
    
    let elapsed = t_start.elapsed().as_secs_f64();
    eprintln!("");
    eprintln!("Decoded {} frames in {:.1}s ({:.1} fps)", n_decoded, elapsed, n_decoded as f64 / elapsed.max(0.001));
    eprintln!("  v8 (mesh):     {} frames", n_v8);
    eprintln!("  v2.1 (DCT):    {} frames", n_v2);
    eprintln!("  skip (dup):    {} frames", n_skip);
    if n_failed > 0 {
        eprintln!("  failed:        {} frames (rendered as previous-frame holds)", n_failed);
    }
    
    if !status.success() {
        eprintln!("ffmpeg exited with non-zero status: {:?}", status.code());
        std::process::exit(1);
    }
    
    let out_size = std::fs::metadata(out_path).map(|m| m.len()).unwrap_or(0);
    let in_size = data.len() as u64;
    eprintln!("");
    eprintln!("Wrote {} ({:.1} MB)", out_path.display(), out_size as f64 / 1_048_576.0);
    eprintln!("  input  .fao: {:.1} MB", in_size as f64 / 1_048_576.0);
    eprintln!("  output:      {:.1} MB ({:.2}x)", out_size as f64 / 1_048_576.0, out_size as f64 / in_size.max(1) as f64);
}

/// Decode one .fao frame to RGB. Returns (rgb, kind) where kind is one of
/// "skip", "v8", "v2", "raw", "fail". Caller passes prev_rgb so skip frames 
/// can repeat. The decoder argument may be None if the file has no v8 
/// trailer (older .fao without v8 webs).
fn decode_one_frame(
    data: &[u8],
    entry: &fao_v1::FaoFrameEntry,
    idx: usize,
    w: usize,
    h: usize,
    v8_dec: Option<&fao_v1::v8_reconstruction_codec::V8Decoder>,
    prev_rgb: &[u8],
) -> Option<(Vec<u8>, &'static str)> {
    let (payload, _hdr) = fao_v1::read_fao_frame(data, entry)?;
    let rgb_sz = w * h * 3;
    let yuv_sz = w * h * 3 / 2;
    
    // Tier-based shortcuts.
    // Skip-tier (0 bytes payload) → hold previous frame.
    if payload.is_empty() {
        return Some((prev_rgb.to_vec(), "skip"));
    }
    
    // Inspect marker byte.
    match payload[0] {
        b'Z' => {
            // v7/v8 reconstruction frame — needs the decoder.
            if let Some(dec) = v8_dec {
                if let Some(rgb) = dec.render_frame_rgb(&payload, idx as u32) {
                    return Some((rgb, "v8"));
                }
            }
            // No decoder or render failed: hold previous frame.
            Some((prev_rgb.to_vec(), "fail"))
        }
        b'V' if payload.len() >= 2 && payload[1] == b'8' => {
            // Legacy "V8..." frame markers (rare; supplanted by 'Z').
            if let Some(dec) = v8_dec {
                if let Some(rgb) = dec.render_frame_rgb(&payload, idx as u32) {
                    return Some((rgb, "v8"));
                }
            }
            Some((prev_rgb.to_vec(), "fail"))
        }
        b'D' => {
            // DCT keyframe (v2 family). Decompress YUV → RGB.
            fao_v1::decompress_frame_dct(&payload, w, h)
                .map(|yuv| (yuv420p_to_rgb(&yuv, w, h), "v2"))
        }
        b'P' => {
            fao_v1::decompress_frame_yuv_split(&payload, w, h)
                .map(|yuv| (yuv420p_to_rgb(&yuv, w, h), "v2"))
        }
        b'L' => {
            fao_v1::decompress_frame_lossy_chroma(&payload, w, h)
                .map(|yuv| (yuv420p_to_rgb(&yuv, w, h), "v2"))
        }
        b'T' => {
            // T-frame needs an anchor — we don't track those here; emit prev.
            // (A future enhancement would walk the index back to the latest 
            //  K/D/P frame, decode it, then apply the T residual.)
            Some((prev_rgb.to_vec(), "v2"))
        }
        _ => {
            // Try YUV420p raw, then RGB raw, then generic zstd.
            if payload.len() == yuv_sz {
                Some((yuv420p_to_rgb(&payload, w, h), "raw"))
            } else if payload.len() == rgb_sz {
                Some((payload.clone(), "raw"))
            } else if let Some(yuv) = fao_v1::decompress_frame(&payload, yuv_sz) {
                Some((yuv420p_to_rgb(&yuv, w, h), "raw"))
            } else if let Some(rgb) = fao_v1::decompress_frame(&payload, rgb_sz) {
                Some((rgb, "raw"))
            } else {
                None
            }
        }
    }
}

/// Standalone YUV420p (planar) → RGB conversion. BT.601 limited-range.
/// Mirrors TranscodeState::yuv_to_rgb in fao_app.
fn yuv420p_to_rgb(yuv: &[u8], w: usize, h: usize) -> Vec<u8> {
    let y_sz = w * h;
    let uv_w = w / 2;
    let uv_h = h / 2;
    let uv_sz = uv_w * uv_h;
    let mut rgb = vec![0u8; w * h * 3];
    if yuv.len() < y_sz + 2 * uv_sz {
        return rgb;
    }
    for py in 0..h {
        for px in 0..w {
            let yv = yuv[py * w + px] as f32;
            let uv_off = (py / 2) * uv_w + (px / 2);
            let cu = yuv[y_sz + uv_off] as f32;
            let cv = yuv[y_sz + uv_sz + uv_off] as f32;
            let yn = (yv - 16.0) * (255.0 / 219.0);
            let un = cu - 128.0;
            let vn = cv - 128.0;
            let r = (yn + 1.402 * vn).clamp(0.0, 255.0) as u8;
            let g = (yn - 0.344136 * un - 0.714136 * vn).clamp(0.0, 255.0) as u8;
            let b = (yn + 1.772 * un).clamp(0.0, 255.0) as u8;
            let oi = (py * w + px) * 3;
            rgb[oi]     = r;
            rgb[oi + 1] = g;
            rgb[oi + 2] = b;
        }
    }
    rgb
}
