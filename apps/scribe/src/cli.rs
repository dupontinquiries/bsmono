//! cli -- `scribe export`, `scribe styles`, `scribe info`. Pure: arguments and file contents in, text and
//! files out, so it is tested without a terminal.

use crate::html;
use crate::script;
use crate::style::{self, Options, Paper, STYLES};
use std::path::{Path, PathBuf};

pub const USAGE: &str = "scribe -- type scenes and lines, export a centred page for printing

  scribe export <file> [-o out.html] [--style NAME] [--font \"Georgia, serif\"] [--size PT]
                       [--paper letter|a4] [--numbers] [--center-all] [--no-page-numbers] [--no-title]
  scribe styles
  scribe info <file>

<file> is a Fountain (.fountain) or a quick-typing text/markdown file: `# scene`, then lines,
`NAME: words`, `(direction)`, `===` for a page break.";

pub struct Export {
    pub options: Options,
    pub input: PathBuf,
    pub output: PathBuf,
}

/// Parse the arguments of `export` (after the word itself).
pub fn parse_export(args: &[String]) -> Result<Export, String> {
    let mut o = Options::default();
    let (mut input, mut output) = (None::<PathBuf>, None::<PathBuf>);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |name: &str| it.next().cloned().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "-o" | "--output" => output = Some(PathBuf::from(val("-o")?)),
            "--style" => {
                let v = val("--style")?;
                o.style = style::find(&v).ok_or(format!("no style {v:?}; try: {}", STYLES.iter().map(|s| s.name).collect::<Vec<_>>().join(", ")))?;
            }
            "--font" => {
                let v = val("--font")?;
                style::sanitize_font(&v).ok_or(format!("{v:?} is not a plain font stack (names, commas, quotes only)"))?;
                o.font = Some(v);
            }
            "--size" => o.size_pt = Some(val("--size")?.parse::<f32>().map_err(|_| "--size wants a number of points".to_string())?),
            "--paper" => { let v = val("--paper")?; o.paper = Paper::parse(&v).ok_or(format!("paper is letter or a4, not {v:?}"))?; }
            "--numbers" => o.scene_numbers = true,
            "--center-all" => o.center_all = true,
            "--no-page-numbers" => o.page_numbers = false,
            "--no-title" => o.title_page = false,
            f if f.starts_with('-') => return Err(format!("unknown option {f}")),
            f => { if input.is_some() { return Err(format!("one file at a time (got {f} too)")); } input = Some(PathBuf::from(f)); }
        }
    }
    let input = input.ok_or("export needs a file")?;
    let output = output.unwrap_or_else(|| input.with_extension("html"));
    if output == input { return Err("the output would overwrite the input".into()); }
    Ok(Export { options: o, input, output })
}

/// Read, parse, render, write. Returns a one-line report.
pub fn export(e: &Export) -> Result<String, String> {
    let text = std::fs::read_to_string(&e.input).map_err(|err| format!("{}: {err}", e.input.display()))?;
    let name = e.input.file_name().and_then(|n| n.to_str());
    let s = script::parse_any(name, &text);
    let page = html::render(&s, &e.options);
    std::fs::write(&e.output, page).map_err(|err| format!("{}: {err}", e.output.display()))?;
    Ok(format!("{}: {} scenes, {} lines, {} action, {} words -> {} ({})", e.input.display(), s.scenes(), s.lines(), s.actions(), s.words(), e.output.display(), e.options.style.name))
}

pub fn styles_text() -> String {
    STYLES.iter().map(|s| format!("  {:<11} {}", s.name, s.blurb)).collect::<Vec<_>>().join("\n")
}

pub fn info(path: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let name = path.file_name().and_then(|n| n.to_str());
    let d = script::detect(name, &text);
    let s = script::parse(&text, d);
    Ok(format!("{}: {:?}, {} scenes, {} lines, {} action, {} words{}", path.display(), d, s.scenes(), s.lines(), s.actions(), s.words(),
        s.meta.title.as_ref().map(|t| format!(", titled {t:?}")).unwrap_or_default()))
}

/// The whole command line (without the program name). `Ok` is what to print.
pub fn run(args: &[String]) -> Result<String, String> {
    match args.first().map(String::as_str) {
        Some("export") => export(&parse_export(&args[1..])?),
        Some("styles") => Ok(styles_text()),
        Some("info") => info(Path::new(args.get(1).ok_or("info needs a file")?)),
        Some("-h") | Some("--help") | None => Ok(USAGE.to_string()),
        Some(other) => Err(format!("unknown command {other:?}\n\n{USAGE}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Vec<String> { v.iter().map(|s| s.to_string()).collect() }

    #[test]
    fn export_arguments_are_checked() {
        let e = parse_export(&a(&["s.txt", "--style", "noir", "--paper", "a4", "--numbers", "--size", "13", "--center-all"])).unwrap();
        assert!(e.options.center_all);
        assert_eq!(e.options.style.name, "noir");
        assert_eq!(e.options.paper, Paper::A4);
        assert!(e.options.scene_numbers);
        assert_eq!(e.options.size_pt, Some(13.0));
        assert_eq!(e.output, PathBuf::from("s.html"), "the output defaults beside the input");
        for bad in [vec![], vec!["a", "b"], vec!["a", "--style", "zzz"], vec!["a", "--font", "x;}"], vec!["a", "--paper", "legal"], vec!["a", "--size", "big"], vec!["a", "--nope"], vec!["a", "-o"], vec!["a.html", "-o", "a.html"]] {
            assert!(parse_export(&a(&bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn it_exports_a_file_and_reports_what_it_found() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("film.fountain");
        std::fs::write(&src, "Title: HEAT\nAuthor: A\n\nINT. ROOM - DAY\n\nMARA\nWait.\n\nShe waits.\n").unwrap();
        let out = run(&a(&["export", src.to_str().unwrap(), "--style", "marquee"])).unwrap();
        assert!(out.contains("1 scenes, 1 lines, 1 action"), "{out}");
        let html = std::fs::read_to_string(d.path().join("film.html")).unwrap();
        assert!(html.contains("<h1>HEAT</h1>") && html.contains("INT. ROOM - DAY") && html.contains("Playfair"));
        let info = run(&a(&["info", src.to_str().unwrap()])).unwrap();
        assert!(info.contains("Fountain") && info.contains("HEAT"), "{info}");
        assert!(run(&a(&["export", "/nonexistent/x.txt"])).is_err());
        assert!(run(&a(&["styles"])).unwrap().contains("classic"));
        assert!(run(&a(&[])).unwrap().contains("scribe export"));
        assert!(run(&a(&["frobnicate"])).is_err());
    }
}
