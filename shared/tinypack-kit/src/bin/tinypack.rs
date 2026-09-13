//! tinypack — the visible loop over tinypack-kit.
//!
//! ```text
//! tinypack text encode  [--greedy] [FILE]            stdin/FILE -> stdout (bytes)
//! tinypack text decode  [FILE]                       bytes -> text
//! tinypack text stat    [FILE]                       bits/byte, vs raw deflate-ish baselines
//! tinypack text fit     --budget N [FILE]            how much of FILE fits in N bytes
//! tinypack image fit    --budget N [--color] [--ladder 64,48,32] IN.ppm [-o OUT.tp]
//! tinypack image encode --q Q IN.pgm|ppm -o OUT.tp
//! tinypack image decode IN.tp -o OUT.pgm|ppm
//! tinypack budgets      [FILE]                       the tag table, for FILE's text
//! ```
//! Images are PGM (P5) / PPM (P6); with `--features png`, `.png` works too.

use std::io::{Read, Write};
use std::process::exit;
use tinypack_kit::image::{self, Image};
use tinypack_kit::{text, Budget};

fn die(msg: &str) -> ! {
    eprintln!("tinypack: {msg}");
    exit(2)
}

fn read_all(path: Option<&str>) -> Vec<u8> {
    let mut v = Vec::new();
    match path {
        Some(p) => std::fs::read(p).unwrap_or_else(|e| die(&format!("{p}: {e}"))),
        None => {
            std::io::stdin().read_to_end(&mut v).unwrap_or_else(|e| die(&e.to_string()));
            v
        }
    }
}

struct Args {
    pos: Vec<String>,
    flags: Vec<(String, Option<String>)>,
}

impl Args {
    fn parse(argv: &[String]) -> Args {
        let mut a = Args { pos: vec![], flags: vec![] };
        let mut i = 0;
        while i < argv.len() {
            let s = &argv[i];
            if let Some(name) = s.strip_prefix("--") {
                let takes_value = matches!(name, "budget" | "ladder" | "q" | "o" | "out" | "size");
                if takes_value {
                    i += 1;
                    let v = argv.get(i).cloned().unwrap_or_else(|| die(&format!("--{name} needs a value")));
                    a.flags.push((name.to_string(), Some(v)));
                } else {
                    a.flags.push((name.to_string(), None));
                }
            } else if s == "-o" {
                i += 1;
                a.flags.push(("o".into(), argv.get(i).cloned()));
            } else {
                a.pos.push(s.clone());
            }
            i += 1;
        }
        a
    }
    fn flag(&self, n: &str) -> bool {
        self.flags.iter().any(|(k, _)| k == n)
    }
    fn value(&self, n: &str) -> Option<&str> {
        self.flags.iter().find(|(k, _)| k == n).and_then(|(_, v)| v.as_deref())
    }
    fn num(&self, n: &str) -> Option<usize> {
        self.value(n).map(|v| v.parse().unwrap_or_else(|_| die(&format!("--{n}: not a number: {v}"))))
    }
}

// ── image I/O ────────────────────────────────────────────────────────────────

fn read_image(path: &str) -> Image {
    let bytes = std::fs::read(path).unwrap_or_else(|e| die(&format!("{path}: {e}")));
    if bytes.starts_with(b"P5") || bytes.starts_with(b"P6") {
        return parse_pnm(&bytes).unwrap_or_else(|| die(&format!("{path}: bad PNM")));
    }
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        return read_png(&bytes).unwrap_or_else(|e| die(&format!("{path}: {e}")));
    }
    die(&format!("{path}: not a PGM/PPM (P5/P6){}", if cfg!(feature = "png") { " or PNG" } else { "; build with --features png for PNG" }))
}

fn parse_pnm(b: &[u8]) -> Option<Image> {
    // P5/P6 <ws> W <ws> H <ws> MAXVAL <single ws> data ; comments start with #
    let channels = if b.starts_with(b"P5") { 1 } else { 3 };
    let mut pos = 2;
    let mut fields = Vec::new();
    while fields.len() < 3 {
        while pos < b.len() && (b[pos].is_ascii_whitespace() || b[pos] == b'#') {
            if b[pos] == b'#' {
                while pos < b.len() && b[pos] != b'\n' {
                    pos += 1;
                }
            } else {
                pos += 1;
            }
        }
        let start = pos;
        while pos < b.len() && b[pos].is_ascii_digit() {
            pos += 1;
        }
        fields.push(std::str::from_utf8(&b[start..pos]).ok()?.parse::<u32>().ok()?);
    }
    pos += 1; // single whitespace
    let (w, h, maxv) = (fields[0], fields[1], fields[2]);
    if maxv != 255 {
        return None;
    }
    let n = (w * h * channels) as usize;
    let data = b.get(pos..pos + n)?.to_vec();
    Some(Image { width: w, height: h, channels, data })
}

fn write_image(path: &str, img: &Image) {
    let bytes = if path.ends_with(".png") {
        write_png(img).unwrap_or_else(|e| die(&e))
    } else {
        let mut v = format!("P{} {} {} 255\n", if img.channels == 1 { 5 } else { 6 }, img.width, img.height).into_bytes();
        v.extend_from_slice(&img.data);
        v
    };
    std::fs::write(path, bytes).unwrap_or_else(|e| die(&format!("{path}: {e}")));
}

#[cfg(feature = "png")]
fn read_png(bytes: &[u8]) -> Result<Image, String> {
    let dec = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut r = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let (w, h) = (info.width, info.height);
    let px = &buf[..info.buffer_size()];
    if info.bit_depth != png::BitDepth::Eight {
        return Err("only 8-bit PNG".into());
    }
    let (channels, data) = match info.color_type {
        png::ColorType::Grayscale => (1, px.to_vec()),
        png::ColorType::Rgb => (3, px.to_vec()),
        png::ColorType::GrayscaleAlpha => (1, px.chunks_exact(2).map(|c| c[0]).collect()),
        png::ColorType::Rgba => (3, px.chunks_exact(4).flat_map(|c| [c[0], c[1], c[2]]).collect()),
        _ => return Err("unsupported PNG colour type".into()),
    };
    Ok(Image { width: w, height: h, channels, data })
}

#[cfg(not(feature = "png"))]
fn read_png(_: &[u8]) -> Result<Image, String> {
    Err("PNG support is behind --features png".into())
}

#[cfg(feature = "png")]
fn write_png(img: &Image) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, img.width, img.height);
        enc.set_color(if img.channels == 1 { png::ColorType::Grayscale } else { png::ColorType::Rgb });
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(|e| e.to_string())?;
        w.write_image_data(&img.data).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

#[cfg(not(feature = "png"))]
fn write_png(_: &Image) -> Result<Vec<u8>, String> {
    Err("PNG support is behind --features png".into())
}

// ── commands ─────────────────────────────────────────────────────────────────

fn cmd_text(a: &Args) {
    let sub = a.pos.get(1).map(String::as_str).unwrap_or("");
    let file = a.pos.get(2).map(String::as_str);
    let how = if a.flag("greedy") { text::Parse::Greedy } else { text::Parse::Optimal };
    match sub {
        "encode" => {
            let input = read_all(file);
            std::io::stdout().write_all(&text::encode(&input, how)).unwrap();
        }
        "decode" => {
            let input = read_all(file);
            let out = text::decode(&input).unwrap_or_else(|e| die(&e.to_string()));
            std::io::stdout().write_all(&out).unwrap();
        }
        "stat" => {
            let input = read_all(file);
            let lines: Vec<&[u8]> = if a.flag("lines") {
                input.split(|&b| b == b'\n').filter(|l| !l.is_empty()).collect()
            } else {
                vec![&input[..]]
            };
            let (mut raw, mut g, mut o) = (0usize, 0usize, 0usize);
            for l in &lines {
                raw += l.len();
                g += text::encode(l, text::Parse::Greedy).len();
                o += text::encode(l, text::Parse::Optimal).len();
            }
            println!("{} unit(s), {raw} B raw", lines.len());
            println!("  greedy  {g:>7} B  {:.2} bits/byte  ratio {:.2}x", g as f64 * 8.0 / raw as f64, raw as f64 / g as f64);
            println!("  optimal {o:>7} B  {:.2} bits/byte  ratio {:.2}x", o as f64 * 8.0 / raw as f64, raw as f64 / o as f64);
        }
        "fit" => {
            let budget = a.num("budget").unwrap_or_else(|| die("text fit needs --budget N"));
            let input = read_all(file);
            let f = text::fit(&input, budget);
            let chars = std::str::from_utf8(&input[..f.bytes_in]).map(|s| s.chars().count()).unwrap_or(f.bytes_in);
            eprintln!(
                "{} of {} bytes ({chars} chars) fit in {budget} bytes -> {} bytes on the wire",
                f.bytes_in,
                input.len(),
                f.wire.len()
            );
            if let Some(o) = a.value("o") {
                std::fs::write(o, &f.wire).unwrap_or_else(|e| die(&e.to_string()));
            } else {
                std::io::stdout().write_all(&input[..f.bytes_in]).unwrap();
            }
        }
        _ => die("text: encode | decode | stat [--lines] | fit --budget N"),
    }
}

fn cmd_image(a: &Args) {
    let sub = a.pos.get(1).map(String::as_str).unwrap_or("");
    let file = a.pos.get(2).map(String::as_str).unwrap_or_else(|| die("image: missing input file"));
    match sub {
        "fit" => {
            let budget = a.num("budget").unwrap_or_else(|| die("image fit needs --budget N"));
            let src = read_image(file);
            let color = a.flag("color");
            if color && src.channels != 3 {
                die("--color needs an RGB input");
            }
            let ladder: Vec<u32> = match a.value("ladder") {
                Some(l) => l.split(',').map(|s| s.trim().parse().unwrap_or_else(|_| die("bad --ladder"))).collect(),
                None => image::LADDER.to_vec(),
            };
            let rows = image::fit(&src, budget, &ladder, color);
            if rows.is_empty() {
                die("nothing fits: budget too small for even the smallest rung");
            }
            eprintln!("{}x{}x{} -> budget {budget} B{}", src.width, src.height, src.channels, if color { " (colour)" } else { " (luma)" });
            eprintln!("{:>9} {:>4} {:>6} {:>9} {:>11}", "size", "q", "bytes", "psnr@ref", "psnr@native");
            for r in &rows {
                eprintln!("{:>4}x{:<4} {:>4} {:>6} {:>8.2} {:>10.2}", r.width, r.height, r.q, r.bytes, r.psnr_ref, r.psnr_native);
            }
            let best = &rows[0];
            eprintln!("best: {}x{} q={} {} bytes", best.width, best.height, best.q, best.bytes);
            if let Some(o) = a.value("o") {
                if o.ends_with(".tp") {
                    std::fs::write(o, &best.wire).unwrap_or_else(|e| die(&e.to_string()));
                } else {
                    write_image(o, &image::decode(&best.wire, 1 << 26).unwrap());
                }
            }
        }
        "encode" => {
            let q = a.num("q").unwrap_or(8) as u8;
            let src = read_image(file);
            let src = if a.flag("gray") { src.to_gray() } else { src };
            let src = match a.num("size") {
                Some(long) => {
                    let (w, h) = src.fit_long_edge(long as u32);
                    src.resize_box(w, h)
                }
                None => src,
            };
            let wire = image::encode(&src, q).unwrap_or_else(|e| die(&e.to_string()));
            eprintln!("{}x{}x{} q={q} -> {} bytes", src.width, src.height, src.channels, wire.len());
            let o = a.value("o").unwrap_or_else(|| die("image encode needs -o OUT.tp"));
            std::fs::write(o, &wire).unwrap_or_else(|e| die(&e.to_string()));
        }
        "decode" => {
            let wire = std::fs::read(file).unwrap_or_else(|e| die(&e.to_string()));
            let img = image::decode(&wire, 1 << 26).unwrap_or_else(|e| die(&e.to_string()));
            eprintln!("{}x{}x{}", img.width, img.height, img.channels);
            let o = a.value("o").unwrap_or_else(|| die("image decode needs -o OUT.pgm|ppm|png"));
            write_image(o, &img);
        }
        _ => die("image: fit --budget N [--color] [--ladder ..] IN [-o OUT] | encode --q Q [--size L] [--gray] IN -o OUT.tp | decode IN.tp -o OUT"),
    }
}

fn cmd_budgets(a: &Args) {
    let file = a.pos.get(1).map(String::as_str);
    let input = read_all(file);
    let is_text = std::str::from_utf8(&input).is_ok();
    println!("{:<15} {:>6} {:>9} {:>9}", "tag", "bytes", "fits(B)", "fits(ch)");
    for b in Budget::ALL {
        let f = text::fit(&input, b.bytes());
        let chars = if is_text { std::str::from_utf8(&input[..f.bytes_in]).unwrap().chars().count() } else { f.bytes_in };
        println!("{:<15} {:>6} {:>9} {:>9}", b.name(), b.bytes(), f.bytes_in, chars);
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let a = Args::parse(&argv);
    match a.pos.first().map(String::as_str) {
        Some("text") => cmd_text(&a),
        Some("image") => cmd_image(&a),
        Some("budgets") => cmd_budgets(&a),
        _ => die("usage: tinypack text|image|budgets ... (see the source header)"),
    }
}
