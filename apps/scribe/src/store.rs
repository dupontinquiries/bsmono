//! store -- the settings file and saving a script safely. Plain `key = value` text, readable anywhere.

use crate::style::{self, Options, Paper};
use std::path::{Path, PathBuf};

/// Where settings live: `$XDG_CONFIG_HOME/scribe/settings.conf`, else `~/.config/scribe/settings.conf`.
pub fn settings_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("scribe").join("settings.conf"))
}

/// What the editor remembers.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub options: Options,
    /// "type", "page" or "both".
    pub view: String,
}

impl Default for Settings {
    fn default() -> Settings { Settings { options: Options::default(), view: "both".into() } }
}

impl Settings {
    pub fn to_text(&self) -> String {
        let o = &self.options;
        format!("style = {}\nfont = {}\nsize = {}\npaper = {}\nscene_numbers = {}\npage_numbers = {}\ntitle_page = {}\ncenter_all = {}\nview = {}\n",
            o.style.name, o.font.clone().unwrap_or_default(), o.size_pt.map(|s| s.to_string()).unwrap_or_default(),
            o.paper.css().to_ascii_lowercase(), o.scene_numbers, o.page_numbers, o.title_page, o.center_all, self.view)
    }

    /// Anything missing or unreadable keeps its default; nothing here can fail.
    pub fn parse(text: &str) -> Settings {
        let mut s = Settings::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "style" => if let Some(st) = style::find(v) { s.options.style = st },
                "font" => s.options.font = style::sanitize_font(v),
                "size" => s.options.size_pt = v.parse::<f32>().ok().filter(|x| x.is_finite()),
                "paper" => if let Some(p) = Paper::parse(v) { s.options.paper = p },
                "scene_numbers" => s.options.scene_numbers = v == "true",
                "page_numbers" => s.options.page_numbers = v != "false",
                "title_page" => s.options.title_page = v != "false",
                "center_all" => s.options.center_all = v == "true",
                "view" if ["type", "page", "both"].contains(&v) => s.view = v.to_string(),
                _ => {}
            }
        }
        s
    }

    pub fn load() -> Settings {
        settings_path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| Settings::parse(&t)).unwrap_or_default()
    }

    pub fn save(&self) {
        if let Some(p) = settings_path() { let _ = write_atomic(&p, &self.to_text()); }
    }
}

/// Write beside the target and rename into place: a crash or a full disk leaves the old file, never a torn one.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
    let tmp = path.with_extension(format!("{}.scribe-tmp", path.extension().and_then(|e| e.to_str()).unwrap_or("txt")));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_bad_values_keep_defaults() {
        let mut s = Settings::default();
        s.options.style = style::find("noir").unwrap();
        s.options.font = Some("Georgia, serif".into());
        s.options.size_pt = Some(13.5);
        s.options.paper = Paper::A4;
        s.options.scene_numbers = true;
        s.options.page_numbers = false;
        s.options.center_all = true;
        s.view = "page".into();
        assert_eq!(Settings::parse(&s.to_text()), s);
        let bad = Settings::parse("style = nope\nfont = x;}\nsize = NaN\npaper = legal\nview = sideways\ngarbage\nscene_numbers = maybe");
        assert_eq!(bad, Settings::default());
        assert_eq!(Settings::parse(""), Settings::default());
    }

    #[test]
    fn a_save_is_atomic_and_leaves_no_temp_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("nested").join("film.fountain");
        write_atomic(&p, "one").unwrap();
        write_atomic(&p, "two").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "two");
        let names: Vec<_> = std::fs::read_dir(p.parent().unwrap()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["film.fountain".to_string()], "no temp file left behind");
    }
}
