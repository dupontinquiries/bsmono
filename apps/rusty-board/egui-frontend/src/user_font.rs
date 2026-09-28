//! user_font — the `look.font` setting: find the face, load it, install it.
//!
//! The value is either a path to a font file or an installed family name,
//! which fontconfig resolves (`fc-match`, the same answer every other app on
//! the device gets for that name). Empty is egui's built-in face.
//!
//! Installed through `uikit_egui::fonts::set_primary_font`, which keeps the
//! built-in faces and the symbol fallbacks BEHIND the chosen one: a text face
//! with no `⇧` or `→` still draws them.
//!
//! The file is checked for a font signature before egui sees it, because
//! epaint panics on bytes it cannot parse, and a keyboard that crashes on
//! start because of one setting cannot be used to change it back.

use std::path::{Path, PathBuf};

pub const KEY: &str = "look.font";

/// An installed family, for the companion's picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Family {
    pub name: String,
    pub file: PathBuf,
}

/// Where the face for `spec` lives. `Ok(None)`: the built-in face.
pub fn resolve(spec: &str) -> Result<Option<PathBuf>, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Ok(None);
    }
    let p = Path::new(spec);
    if p.is_absolute() || spec.starts_with('~') {
        let p = match spec.strip_prefix("~/") {
            Some(rest) => std::env::var_os("HOME").map(|h| PathBuf::from(h).join(rest)).ok_or("no HOME")?,
            None => p.to_path_buf(),
        };
        return if p.is_file() { Ok(Some(p)) } else { Err(format!("{}: no such file", p.display())) };
    }
    let out = std::process::Command::new("fc-match")
        .args(["-f", "%{file}\n%{family}", spec])
        .output()
        .map_err(|e| format!("fc-match: {e} (install fontconfig, or give a file path)"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    let file = lines.next().unwrap_or("").trim();
    let family = lines.next().unwrap_or("");
    if file.is_empty() {
        return Err(format!("no font matches {spec:?}"));
    }
    // fc-match ALWAYS answers, falling back to the default sans for a name
    // it does not know. Say so rather than silently showing DejaVu.
    if !family.split(',').any(|f| f.trim().eq_ignore_ascii_case(spec)) {
        return Err(format!("no installed font named {spec:?} (fontconfig offered {})", family.split(',').next().unwrap_or("?")));
    }
    Ok(Some(PathBuf::from(file)))
}

/// Whether `bytes` start like a font egui can parse: TrueType, OpenType/CFF
/// or a collection. WOFF/WOFF2 are web wrappers egui cannot read.
pub fn looks_like_font(bytes: &[u8]) -> bool {
    matches!(bytes.get(..4), Some([0, 1, 0, 0]) | Some(b"OTTO") | Some(b"true") | Some(b"ttcf"))
}

/// The face's bytes, checked. `Ok(None)`: the built-in face.
pub fn load(spec: &str) -> Result<Option<Vec<u8>>, String> {
    let Some(path) = resolve(spec)? else { return Ok(None) };
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !looks_like_font(&bytes) {
        return Err(format!("{}: not a TrueType/OpenType font", path.display()));
    }
    Ok(Some(bytes))
}

/// Install `spec` into `ctx`. On error the fonts are left as they were.
pub fn apply(ctx: &egui::Context, spec: &str) -> Result<(), String> {
    let face = load(spec)?;
    uikit_egui::fonts::set_primary_font(ctx, face.map(|b| ("rb-user-font", b)));
    Ok(())
}

/// Installed families, sorted, one entry per family (its regular face when
/// there is one). Empty when fontconfig is missing.
pub fn installed_families() -> Vec<Family> {
    let Ok(out) = std::process::Command::new("fc-list")
        .args([":", "-f", "%{family[0]}\t%{style[0]}\t%{file}\n"])
        .output()
    else {
        return Vec::new();
    };
    parse_fc_list(&String::from_utf8_lossy(&out.stdout))
}

pub fn parse_fc_list(text: &str) -> Vec<Family> {
    let mut best: std::collections::BTreeMap<String, (bool, PathBuf)> = Default::default();
    for line in text.lines() {
        let mut f = line.split('\t');
        let (Some(name), Some(style), Some(file)) = (f.next(), f.next(), f.next()) else { continue };
        let name = name.trim();
        if name.is_empty() || !(file.ends_with(".ttf") || file.ends_with(".otf") || file.ends_with(".ttc")) {
            continue;
        }
        let regular = matches!(style.trim(), "Regular" | "Book" | "Roman" | "Normal");
        let e = best.entry(name.to_string()).or_insert((regular, PathBuf::from(file)));
        if regular && !e.0 {
            *e = (true, PathBuf::from(file));
        }
    }
    best.into_iter().map(|(name, (_, file))| Family { name, file }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_the_builtin_face() {
        assert_eq!(resolve("  ").unwrap(), None);
        assert_eq!(load("").unwrap(), None);
    }

    #[test]
    fn a_missing_file_is_an_error_not_a_panic() {
        assert!(resolve("/nope/none.ttf").is_err());
    }

    #[test]
    fn only_font_signatures_are_accepted() {
        assert!(looks_like_font(&[0, 1, 0, 0, 9]));
        assert!(looks_like_font(b"OTTO...."));
        assert!(!looks_like_font(b"wOFF...."), "woff is not readable by egui");
        assert!(!looks_like_font(b"<html>"));
        let dir = std::env::temp_dir().join(format!("rb-font-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bad = dir.join("bad.ttf");
        std::fs::write(&bad, b"not a font").unwrap();
        assert!(load(bad.to_str().unwrap()).unwrap_err().contains("not a TrueType"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fc_list_keeps_one_face_per_family_preferring_regular() {
        let t = "Inter\tBold\t/f/Inter-Bold.ttf\nInter\tRegular\t/f/Inter-Regular.ttf\nX\tRegular\t/f/x.pcf.gz\n\tRegular\t/f/y.ttf\n";
        let f = parse_fc_list(t);
        assert_eq!(f, vec![Family { name: "Inter".into(), file: "/f/Inter-Regular.ttf".into() }]);
    }

    /// A real installed face loads and installs without panicking egui.
    #[test]
    fn an_installed_face_installs() {
        let fams = installed_families();
        let Some(f) = fams.first() else { return }; // no fontconfig here
        let ctx = egui::Context::default();
        apply(&ctx, f.file.to_str().unwrap()).unwrap();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| ui.label("Hello ⇧ →"));
        });
    }
}
