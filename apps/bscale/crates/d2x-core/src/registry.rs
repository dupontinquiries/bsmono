//! The models folder as a catalogue: what is there, what each one costs, and
//! whether it actually loads and produces sane output.
//!
//! Shaped after logscope's picker (`apps/logscope/crates/logscope-egui/src/model.rs`),
//! including the lesson it paid for: **selection is keyed by path, not by
//! index**. A rescan rebuilds the list, so adding one file shifts every index
//! after it and a stored index silently names a different model — a bounds
//! check does not help, because the stale index is still in range. `key()` is
//! what a UI or a config file should persist.
//!
//! Verification matters more here than in a chat app: a model file that loads
//! but was converted wrongly produces a picture, just a bad one, and the
//! failure appears hours into a job. So each entry gets a cheap forward pass
//! whose result is checked for finiteness and for grey-in/grey-out — a
//! scrambled weight order fails both.

use crate::frame::Frame;
use crate::net::Network;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    /// Real-ESRGAN RRDBNet (the photo models).
    Rrdb,
    /// SRVGGNetCompact (`realesr-animevideov3`).
    Srvgg,
    /// waifu2x `upconv_7`.
    Upconv7,
}

impl Arch {
    pub fn label(self) -> &'static str {
        match self {
            Arch::Rrdb => "RRDBNet",
            Arch::Srvgg => "SRVGG",
            Arch::Upconv7 => "upconv_7",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Verify {
    /// Loaded, forward pass finite, grey stayed grey.
    Ok,
    /// Loaded and ran, but something looks off — usable with care.
    Warn(String),
    /// Would not load, or produced garbage.
    Failed(String),
}

impl Verify {
    pub fn usable(&self) -> bool {
        !matches!(self, Verify::Failed(_))
    }
    pub fn badge(&self) -> &'static str {
        match self {
            Verify::Ok => "ok",
            Verify::Warn(_) => "warn",
            Verify::Failed(_) => "failed",
        }
    }
    pub fn detail(&self) -> Option<&str> {
        match self {
            Verify::Ok => None,
            Verify::Warn(m) | Verify::Failed(m) => Some(m),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub path: PathBuf,
    pub name: String,
    pub arch: Arch,
    pub scale: u32,
    pub unshuffle: u32,
    pub shuffle: u32,
    pub ops: usize,
    pub macs_per_px: f64,
    pub params: u64,
    pub bytes: u64,
    pub status: Verify,
}

impl ModelInfo {
    /// Stable identity across rescans — see the module note.
    pub fn key(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }

    /// Seconds of pure network time for a full 1080p frame at `gmac_per_s`
    /// (from `bscale bench`), for putting a cost on a ladder rung.
    pub fn secs_per_1080p(&self, gmac_per_s: f64) -> f64 {
        self.macs_per_px * 1920.0 * 1080.0 / (gmac_per_s * 1e9)
    }

    /// One-line summary in the shape logscope's picker rows use:
    /// (name, subtitle, badge).
    pub fn row(&self) -> (String, String, String) {
        let sub = format!(
            "{} x{} · {:.2} MMAC/px · {:.1}M params · {:.1} MB",
            self.arch.label(),
            self.scale,
            self.macs_per_px / 1e6,
            self.params as f64 / 1e6,
            self.bytes as f64 / (1024.0 * 1024.0)
        );
        (self.name.clone(), sub, self.status.badge().to_string())
    }
}

fn arch_of(net: &Network) -> Arch {
    if net.exact {
        Arch::Upconv7
    } else if net.unshuffle >= 2 || net.ops.len() > 100 {
        Arch::Rrdb
    } else {
        Arch::Srvgg
    }
}

/// Run a small forward pass and judge the result.
///
/// Two checks, both of which a mis-converted model fails: the output must be
/// finite everywhere, and a flat mid-grey input must come out near mid-grey.
/// The second is the sharper one — a scrambled weight or channel order still
/// produces finite numbers, but not grey.
pub fn verify(net: &Network) -> Verify {
    // Big enough that every layer has real spatial extent after the valid
    // convolutions, small enough to stay cheap on the CPU.
    let side = 8 + 2 * net.pre_pad;
    let mut grey = Frame::new(side, side);
    grey.data.iter_mut().for_each(|b| *b = 128);
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| net.upscale_cpu(&grey, side)));
    let out = match out {
        Ok(f) => f,
        Err(_) => return Verify::Failed("forward pass panicked".into()),
    };
    if out.width != side * net.scale || out.height != side * net.scale {
        return Verify::Failed(format!("output {}x{} is not {}x the input", out.width, out.height, net.scale));
    }
    // Ignore a border of one output block: same-pad networks legitimately
    // darken at the frame edge, where the padding is not real image.
    let m = (net.pre_pad * net.scale).min(side * net.scale / 4);
    let inner = out.crop(m, m, out.width - 2 * m, out.height - 2 * m);
    let mean = inner.data.iter().map(|&b| b as f64).sum::<f64>() / inner.data.len() as f64;
    let spread = inner.data.iter().map(|&b| (b as f64 - mean).abs()).fold(0.0, f64::max);
    if !mean.is_finite() {
        return Verify::Failed("output is not finite".into());
    }
    if (mean - 128.0).abs() > 24.0 {
        return Verify::Failed(format!("flat grey in gave mean {mean:.0}, expected ~128 — weights are probably mis-ordered"));
    }
    if spread > 24.0 {
        return Verify::Warn(format!("flat grey in varied by up to {spread:.0} levels"));
    }
    if (mean - 128.0).abs() > 6.0 {
        return Verify::Warn(format!("flat grey in gave mean {mean:.0}"));
    }
    Verify::Ok
}

/// Load and describe one model file. `deep` runs [`verify`]; without it the
/// entry is classified from shapes alone, which is fast enough to scan a
/// folder of 67 MB files on a UI thread.
pub fn inspect(path: &Path, deep: bool) -> ModelInfo {
    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let fail = |m: String| ModelInfo {
        path: path.to_path_buf(),
        name: name.clone(),
        arch: Arch::Srvgg,
        scale: 0,
        unshuffle: 1,
        shuffle: 1,
        ops: 0,
        macs_per_px: 0.0,
        params: 0,
        bytes,
        status: Verify::Failed(m),
    };
    let net = match Network::load(path) {
        Ok(n) => n,
        Err(e) => return fail(format!("{e:#}")),
    };
    let params: u64 = net
        .ops
        .iter()
        .map(|op| match op {
            crate::net::Op::Conv(c) => (c.weight.len() + c.bias.len()) as u64,
            crate::net::Op::Deconv(d) => (d.weight.len() + d.bias.len()) as u64,
            _ => 0,
        })
        .sum();
    let status = if deep { verify(&net) } else { Verify::Ok };
    ModelInfo {
        path: path.to_path_buf(),
        name,
        arch: arch_of(&net),
        scale: net.scale,
        unshuffle: net.unshuffle,
        shuffle: net.shuffle,
        ops: net.ops.len(),
        macs_per_px: net.macs_per_input_pixel(),
        params,
        bytes,
        status,
    }
}

/// Every model file under `dir` (recursive, one level of symlink-free walk),
/// cheapest first — so `[0]` is the cascade rung and `last()` the quality one.
pub fn scan(dir: &Path, deep: bool) -> Vec<ModelInfo> {
    let mut files = Vec::new();
    collect(dir, 0, &mut files);
    files.sort();
    let mut out: Vec<ModelInfo> = files.iter().map(|p| inspect(p, deep)).collect();
    // Unusable entries sink to the bottom but are kept: a folder with a broken
    // file should say so, not silently omit it.
    out.sort_by(|a, b| a.status.usable().cmp(&b.status.usable()).reverse().then(a.macs_per_px.partial_cmp(&b.macs_per_px).unwrap_or(std::cmp::Ordering::Equal)));
    out
}

fn collect(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    if depth > 3 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, depth + 1, out);
        } else if matches!(p.extension().and_then(|x| x.to_str()), Some("safetensors") | Some("json")) {
            // A .json here is only a model if it parses as one; `inspect`
            // decides, and a stray config file shows up as `failed` rather
            // than being hidden.
            out.push(p);
        }
    }
}

/// The cheap and expensive rungs of a ladder among usable models of the same
/// output scale, which is what the cascade needs (tiers must agree on scale).
pub fn ladder(models: &[ModelInfo], scale: u32) -> Option<(&ModelInfo, &ModelInfo)> {
    let mut same: Vec<&ModelInfo> = models.iter().filter(|m| m.status.usable() && m.scale == scale).collect();
    if same.len() < 2 {
        return None;
    }
    same.sort_by(|a, b| a.macs_per_px.partial_cmp(&b.macs_per_px).unwrap_or(std::cmp::Ordering::Equal));
    Some((same[0], same[same.len() - 1]))
}

/// Restore a selection by identity after a rescan; falls back to the cheapest
/// usable model. Index-based restoration is the bug logscope documented.
pub fn select_by_key<'a>(models: &'a [ModelInfo], key: Option<&str>) -> Option<&'a ModelInfo> {
    // Restore only a USABLE model. A scan keeps broken entries on purpose (a
    // folder with a bad file should say so), so the saved key can name one --
    // e.g. after pointing the models dir at ~/models, where the only .json was
    // another program's config -- and restoring it handed the job a file that
    // then failed at "model json: expected a top-level array of layers".
    if let Some(k) = key {
        if let Some(m) = models.iter().find(|m| m.key() == k && m.status.usable()) {
            return Some(m);
        }
    }
    models.iter().find(|m| m.status.usable())
}

/// Why the model `key` names cannot run, or None if it can (or none is
/// chosen). What a job start says instead of failing at the config stage.
pub fn refusal(models: &[ModelInfo], key: Option<&str>) -> Option<String> {
    let k = key?;
    let m = models.iter().find(|m| m.key() == k)?;
    if m.status.usable() {
        return None;
    }
    Some(format!("{} is not a model this can run ({})", m.path.display(), m.status.detail().unwrap_or("unusable")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ~/models as reported: a language model's config.json beside nothing
    /// bscale can run. Kept in the list (as unusable), never restored, and a
    /// start that still names it is refused with the reason.
    #[test]
    fn a_stray_json_is_listed_but_never_restored_or_run() {
        let dir = std::env::temp_dir().join(format!("d2x-stray-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("qwen")).unwrap();
        let stray = dir.join("qwen/config.json");
        std::fs::write(&stray, r#"{"model_type":"qwen3","hidden_size":4096}"#).unwrap();
        let mut found = scan(&dir, true);
        assert_eq!(found.len(), 1);
        assert!(!found[0].status.usable(), "listed as unusable, not hidden");
        let key = found[0].key();
        assert!(select_by_key(&found, Some(&key)).is_none(), "an unusable model is never restored");
        let why = refusal(&found, Some(&key)).expect("a start naming it is refused");
        assert!(why.contains("config.json"), "{why}");
        // Beside real models, the saved key falls back to a usable one.
        found.extend(scan(&models_dir(), true));
        let pick = select_by_key(&found, Some(&key)).expect("a usable model");
        assert!(pick.status.usable() && pick.key() != key);
        assert!(refusal(&found, Some(&pick.key())).is_none());
        assert!(refusal(&found, None).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn models_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models")
    }

    #[test]
    fn scans_and_verifies_the_vendored_models() {
        let dir = models_dir();
        if !dir.exists() {
            return;
        }
        let found = scan(&dir, true);
        assert!(!found.is_empty(), "no models found in {}", dir.display());
        for m in &found {
            let (name, sub, badge) = m.row();
            eprintln!("{badge:>6}  {name:34} {sub}");
            if let Some(d) = m.status.detail() {
                eprintln!("        {d}");
            }
        }
        // The vendored compact model and the waifu2x JSON must both verify.
        let srvgg = found.iter().find(|m| m.name.contains("animevideov3")).expect("vendored compact model");
        assert_eq!(srvgg.status, Verify::Ok, "{:?}", srvgg.status);
        assert_eq!((srvgg.arch, srvgg.scale), (Arch::Srvgg, 4));
        assert!(srvgg.params > 400_000 && srvgg.params < 1_500_000, "{} params", srvgg.params);
        if let Some(w) = found.iter().find(|m| m.name.contains("upconv_7")) {
            assert_eq!(w.status, Verify::Ok, "{:?}", w.status);
            assert_eq!((w.arch, w.scale), (Arch::Upconv7, 2));
        }
        // Cheapest first among usable entries.
        let usable: Vec<&ModelInfo> = found.iter().filter(|m| m.status.usable()).collect();
        for w in usable.windows(2) {
            assert!(w[0].macs_per_px <= w[1].macs_per_px, "not cost-ordered");
        }
    }

    #[test]
    fn a_corrupt_model_is_reported_not_hidden() {
        let dir = std::env::temp_dir().join(format!("d2x-reg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("junk.safetensors"), b"not a model at all").unwrap();
        std::fs::write(dir.join("notes.json"), b"{\"hello\": 1}").unwrap();
        let found = scan(&dir, true);
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|m| matches!(m.status, Verify::Failed(_))), "{found:?}");
        assert!(select_by_key(&found, None).is_none(), "must not offer a broken model");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A weight order that a bad conversion would produce must fail
    /// verification, not merely look different.
    #[test]
    fn verify_catches_scrambled_weights() {
        let good = Network::upconv7(&serde_json::from_str(&std::fs::read_to_string(models_dir().join("upconv_7_art_scale2.0x_model.json")).unwrap_or_default()).unwrap_or(serde_json::Value::Null), "x".into());
        let Ok(good) = good else { return };
        assert_eq!(verify(&good), Verify::Ok);
        let mut bad = good.clone();
        if let crate::net::Op::Conv(c) = &mut bad.ops[0] {
            c.weight.reverse();
        }
        assert!(matches!(verify(&bad), Verify::Failed(_) | Verify::Warn(_)), "scrambled weights passed verification");
    }

    #[test]
    fn ladder_picks_cheap_and_dear_at_one_scale() {
        let a = |name: &str, macs: f64, scale: u32| ModelInfo {
            path: PathBuf::from(name),
            name: name.into(),
            arch: Arch::Rrdb,
            scale,
            unshuffle: 1,
            shuffle: 1,
            ops: 1,
            macs_per_px: macs,
            params: 0,
            bytes: 0,
            status: Verify::Ok,
        };
        // MAC per input pixel, so the RRDB x2 figure is 4.48e6, not 4.48.
        let ms = vec![a("mid", 2.0e6, 2), a("cheap", 0.5e6, 2), a("dear", 9.0e6, 2), a("other", 0.1e6, 4)];
        let (c, d) = ladder(&ms, 2).unwrap();
        assert_eq!((c.name.as_str(), d.name.as_str()), ("cheap", "dear"));
        assert!(ladder(&ms, 4).is_none(), "one model is not a ladder");
        // 9 MMAC/px over a 1080p frame at 1000 GMAC/s ≈ 18.7 s.
        assert!((d.secs_per_1080p(1000.0) - 18.66).abs() < 0.1, "{}", d.secs_per_1080p(1000.0));
    }
}
