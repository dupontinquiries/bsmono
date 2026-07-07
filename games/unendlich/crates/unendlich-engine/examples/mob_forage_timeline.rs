//! examples/mob_forage_timeline.rs — WID r102-M1-mobs headless proof.
//!
//! Runs `sim::mob_forage_memory::MobForager` through a scripted scenario and
//! renders a behaviour timeline to a PNG: per-tick mode (Forage/Flee/Rest) as a
//! colour strip, plus hunger / fatigue / threat / food-memory traces. No bin, no
//! GPU — a dependency-free CPU rasteriser + PNG writer (pattern copied from
//! examples/sim_collage.rs).
//!
//! Run: cargo run --example mob_forage_timeline --features gpu
//! Output: /mnt/user-data/outputs/mob_forage_timeline.png
//!
//! Scenario (≈30s @ 60fps): start fed+safe (Rest) → hunger climbs, food appears
//! out of reach then leaves view (Forage on remembered place) → predator
//! approaches (Flee overrides) → predator leaves, creature eats (hunger falls,
//! back to Rest). The strip should read Rest → Forage → Flee → Forage/Rest.

use unendlich_engine::sim::fauna_mind::WorldSense;
use unendlich_engine::sim::mob_forage_memory::{ForageSense, MobForager, MobMode, SpeciesParams};

const W: usize = 900;
const H: usize = 360;
const DT: f32 = 1.0 / 60.0;
const SECS: f32 = 30.0;

fn main() {
    let steps = (SECS / DT) as usize;
    let mut m = MobForager::new([0.0, 64.0, 0.0], 1.0, 0.5, SpeciesParams::grazer());
    m.hunger = 0.0;
    m.fatigue = 0.2;

    let mut modes: Vec<MobMode> = Vec::with_capacity(steps);
    let mut hunger = Vec::with_capacity(steps);
    let mut fatigue = Vec::with_capacity(steps);
    let mut threat = Vec::with_capacity(steps);
    let mut foodmem = Vec::with_capacity(steps);

    for i in 0..steps {
        let t = i as f32 * DT;
        // ── script the senses over time ──
        let mut sense = WorldSense::neutral();
        let mut forage = ForageSense::none();

        // food visible 6–9s (then leaves view; the creature should remember it)
        if (6.0..9.0).contains(&t) {
            forage = ForageSense { food_visible: true, food_pos: [18.0, 64.0, 4.0], eating: false };
        }
        // predator close 14–20s → threat → Flee
        if (14.0..20.0).contains(&t) {
            sense = WorldSense {
                player_pos: [2.0, 64.0, 0.0],
                player_yaw: 0.0,
                noise: 1.0,
                has_los: true,
                roar_power: 0.0,
                roar_dist: 1.0e6,
                roar_range: 1.0,
                herd_centroid: [0.0, 0.0],
                nearest_neighbor: [1.0e6, 1.0e6],
            };
        }
        // reaches food and eats 24–30s → hunger falls
        if t >= 24.0 {
            forage = ForageSense { food_visible: true, food_pos: [0.0, 64.0, 0.0], eating: true };
        }

        let out = m.step(&sense, &forage, DT);
        modes.push(out.mode);
        hunger.push(out.hunger);
        fatigue.push(out.fatigue);
        threat.push(out.threat);
        foodmem.push(if m.recalled_food().is_some() { 1.0 } else { 0.0 });
    }

    // ── render ──
    let mut buf = vec![16u8; W * H * 4];
    for px in buf.chunks_mut(4) {
        px[3] = 255;
    }
    let plot_top = 70usize;
    let plot_h = 250usize;

    // mode colour strip across the top
    for x in 0..W {
        let i = x * steps / W;
        let c = match modes[i.min(steps - 1)] {
            MobMode::Rest => [70, 110, 170],
            MobMode::Forage => [80, 170, 90],
            MobMode::Flee => [200, 70, 60],
        };
        for y in 10..50 {
            let o = (y * W + x) * 4;
            buf[o] = c[0];
            buf[o + 1] = c[1];
            buf[o + 2] = c[2];
        }
    }

    // four traces
    let trace = |buf: &mut [u8], data: &[f32], col: [u8; 3]| {
        for x in 0..W {
            let i = (x * steps / W).min(steps - 1);
            let v = data[i].clamp(0.0, 1.0);
            let y = plot_top + plot_h - (v * plot_h as f32) as usize;
            let y = y.min(plot_top + plot_h - 1);
            for yy in y.saturating_sub(1)..=(y + 1).min(H - 1) {
                let o = (yy * W + x) * 4;
                buf[o] = col[0];
                buf[o + 1] = col[1];
                buf[o + 2] = col[2];
            }
        }
    };
    trace(&mut buf, &hunger, [230, 180, 60]); // hunger — amber
    trace(&mut buf, &fatigue, [150, 120, 220]); // fatigue — violet
    trace(&mut buf, &threat, [220, 80, 80]); // threat — red
    trace(&mut buf, &foodmem, [80, 200, 120]); // food memory present — green (0/1)

    write_png("/mnt/user-data/outputs/mob_forage_timeline.png", W, H, &buf).expect("png");
    // numeric summary to stdout (the reviewer can read this without the image)
    let count = |want: MobMode| modes.iter().filter(|&&x| x == want).count();
    println!("mob_forage_timeline: {steps} ticks over {SECS}s");
    println!(
        "  modes: Rest={} Forage={} Flee={}",
        count(MobMode::Rest),
        count(MobMode::Forage),
        count(MobMode::Flee)
    );
    println!("  food remembered after sighting (tick 600): {}", foodmem.get(600).copied().unwrap_or(0.0));
    println!("  wrote mob_forage_timeline.png ({W}x{H})");
}

// ── dependency-free PNG writer (pattern from examples/sim_collage.rs) ──

fn write_png(path: &str, w: usize, h: usize, rgba: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    let mut raw = Vec::with_capacity(h * (1 + w * 4));
    for y in 0..h {
        raw.push(0);
        raw.extend_from_slice(&rgba[y * w * 4..(y + 1) * w * 4]);
    }
    chunk(&mut out, b"IDAT", &zlib_store(&raw));
    chunk(&mut out, b"IEND", &[]);
    std::fs::File::create(path)?.write_all(&out)
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = 0xffff_ffffu32;
    for b in kind.iter().chain(data.iter()) {
        crc ^= *b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    out.extend_from_slice(&(crc ^ 0xffff_ffff).to_be_bytes());
}

fn zlib_store(data: &[u8]) -> Vec<u8> {
    let mut z = vec![0x78, 0x01];
    let mut i = 0;
    while i < data.len() {
        let n = (data.len() - i).min(0xffff);
        let fin = i + n >= data.len();
        z.push(if fin { 1 } else { 0 });
        z.extend_from_slice(&(n as u16).to_le_bytes());
        z.extend_from_slice(&(!(n as u16)).to_le_bytes());
        z.extend_from_slice(&data[i..i + n]);
        i += n;
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    z
}
//! examples/mob_forage_timeline.rs — WID r102-M1-mobs headless proof.
//!
//! Runs `sim::mob_forage_memory::MobForager` through a scripted scenario and
//! renders a behaviour timeline to a PNG: per-tick mode (Forage/Flee/Rest) as a
//! colour strip, plus hunger / fatigue / threat / food-memory traces. No bin, no
//! GPU — a dependency-free CPU rasteriser + PNG writer (pattern copied from
//! examples/sim_collage.rs).
//!
//! Run: cargo run --example mob_forage_timeline --features gpu
//! Output: /mnt/user-data/outputs/mob_forage_timeline.png
//!
//! Scenario (≈30s @ 60fps): start fed+safe (Rest) → hunger climbs, food appears
//! out of reach then leaves view (Forage on remembered place) → predator
//! approaches (Flee overrides) → predator leaves, creature eats (hunger falls,
//! back to Rest). The strip should read Rest → Forage → Flee → Forage/Rest.

use unendlich_engine::sim::fauna_mind::WorldSense;
use unendlich_engine::sim::mob_forage_memory::{ForageSense, MobForager, MobMode, SpeciesParams};

const W: usize = 900;
const H: usize = 360;
const DT: f32 = 1.0 / 60.0;
const SECS: f32 = 30.0;

fn main() {
    let steps = (SECS / DT) as usize;
    let mut m = MobForager::new([0.0, 64.0, 0.0], 1.0, 0.5, SpeciesParams::grazer());
    m.hunger = 0.0;
    m.fatigue = 0.2;

    let mut modes: Vec<MobMode> = Vec::with_capacity(steps);
    let mut hunger = Vec::with_capacity(steps);
    let mut fatigue = Vec::with_capacity(steps);
    let mut threat = Vec::with_capacity(steps);
    let mut foodmem = Vec::with_capacity(steps);

    for i in 0..steps {
        let t = i as f32 * DT;
        // ── script the senses over time ──
        let mut sense = WorldSense::neutral();
        let mut forage = ForageSense::none();

        // food visible 6–9s (then leaves view; the creature should remember it)
        if (6.0..9.0).contains(&t) {
            forage = ForageSense { food_visible: true, food_pos: [18.0, 64.0, 4.0], eating: false };
        }
        // predator close 14–20s → threat → Flee
        if (14.0..20.0).contains(&t) {
            sense = WorldSense {
                player_pos: [2.0, 64.0, 0.0],
                player_yaw: 0.0,
                noise: 1.0,
                has_los: true,
                roar_power: 0.0,
                roar_dist: 1.0e6,
                roar_range: 1.0,
                herd_centroid: [0.0, 0.0],
                nearest_neighbor: [1.0e6, 1.0e6],
            };
        }
        // reaches food and eats 24–30s → hunger falls
        if t >= 24.0 {
            forage = ForageSense { food_visible: true, food_pos: [0.0, 64.0, 0.0], eating: true };
        }

        let out = m.step(&sense, &forage, DT);
        modes.push(out.mode);
        hunger.push(out.hunger);
        fatigue.push(out.fatigue);
        threat.push(out.threat);
        foodmem.push(if m.recalled_food().is_some() { 1.0 } else { 0.0 });
    }

    // ── render ──
    let mut buf = vec![16u8; W * H * 4];
    for px in buf.chunks_mut(4) {
        px[3] = 255;
    }
    let plot_top = 70usize;
    let plot_h = 250usize;

    // mode colour strip across the top
    for x in 0..W {
        let i = x * steps / W;
        let c = match modes[i.min(steps - 1)] {
            MobMode::Rest => [70, 110, 170],
            MobMode::Forage => [80, 170, 90],
            MobMode::Flee => [200, 70, 60],
        };
        for y in 10..50 {
            let o = (y * W + x) * 4;
            buf[o] = c[0];
            buf[o + 1] = c[1];
            buf[o + 2] = c[2];
        }
    }

    // four traces
    let trace = |buf: &mut [u8], data: &[f32], col: [u8; 3]| {
        for x in 0..W {
            let i = (x * steps / W).min(steps - 1);
            let v = data[i].clamp(0.0, 1.0);
            let y = plot_top + plot_h - (v * plot_h as f32) as usize;
            let y = y.min(plot_top + plot_h - 1);
            for yy in y.saturating_sub(1)..=(y + 1).min(H - 1) {
                let o = (yy * W + x) * 4;
                buf[o] = col[0];
                buf[o + 1] = col[1];
                buf[o + 2] = col[2];
            }
        }
    };
    trace(&mut buf, &hunger, [230, 180, 60]); // hunger — amber
    trace(&mut buf, &fatigue, [150, 120, 220]); // fatigue — violet
    trace(&mut buf, &threat, [220, 80, 80]); // threat — red
    trace(&mut buf, &foodmem, [80, 200, 120]); // food memory present — green (0/1)

    write_png("/mnt/user-data/outputs/mob_forage_timeline.png", W, H, &buf).expect("png");
    // numeric summary to stdout (the reviewer can read this without the image)
    let count = |want: MobMode| modes.iter().filter(|&&x| x == want).count();
    println!("mob_forage_timeline: {steps} ticks over {SECS}s");
    println!(
        "  modes: Rest={} Forage={} Flee={}",
        count(MobMode::Rest),
        count(MobMode::Forage),
        count(MobMode::Flee)
    );
    println!("  food remembered after sighting (tick 600): {}", foodmem.get(600).copied().unwrap_or(0.0));
    println!("  wrote mob_forage_timeline.png ({W}x{H})");
}

// ── dependency-free PNG writer (pattern from examples/sim_collage.rs) ──

fn write_png(path: &str, w: usize, h: usize, rgba: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    let mut raw = Vec::with_capacity(h * (1 + w * 4));
    for y in 0..h {
        raw.push(0);
        raw.extend_from_slice(&rgba[y * w * 4..(y + 1) * w * 4]);
    }
    chunk(&mut out, b"IDAT", &zlib_store(&raw));
    chunk(&mut out, b"IEND", &[]);
    std::fs::File::create(path)?.write_all(&out)
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = 0xffff_ffffu32;
    for b in kind.iter().chain(data.iter()) {
        crc ^= *b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    out.extend_from_slice(&(crc ^ 0xffff_ffff).to_be_bytes());
}

fn zlib_store(data: &[u8]) -> Vec<u8> {
    let mut z = vec![0x78, 0x01];
    let mut i = 0;
    while i < data.len() {
        let n = (data.len() - i).min(0xffff);
        let fin = i + n >= data.len();
        z.push(if fin { 1 } else { 0 });
        z.extend_from_slice(&(n as u16).to_le_bytes());
        z.extend_from_slice(&(!(n as u16)).to_le_bytes());
        z.extend_from_slice(&data[i..i + n]);
        i += n;
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    z
}
