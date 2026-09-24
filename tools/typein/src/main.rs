//! The shell around the two pure modules: find the layout, build the script,
//! hand it to dotool.

use std::io::Write;
use std::process::{Command, Stdio};
use typein::{keyspec, layout::{self, Layout}};

const USAGE: &str = "\
typein — type text and press keys under any keyboard layout

  typein warp enter              type \"warp\", press Enter
  typein git status enter        text and keys interleave freely
  typein C-a C-v                 ctrl+a then ctrl+v
  typein C-{x c}                 ctrl+x then ctrl+c, one prefix, two keys
  typein M-{1 2 3}               alt+1 alt+2 alt+3
  typein -- enter tab            the literal words, not the keys
  typein --dry warp enter        print the dotool script, send nothing
  typein --check                 which layout, from where, and can it type

Modifiers: C- ctrl, M- alt, A- alt, S- shift, s- super (the case of S
matters, as in Emacs). They stack: C-M-a. ctrl+c style also works.
Env: TYPEIN_LAYOUT, TYPEIN_VARIANT override detection; DOTOOL names the
binary.
";

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.trim().is_empty())
}

fn detect() -> Layout {
    if let Some(l) = env("TYPEIN_LAYOUT") {
        return Layout { layout: layout::first(&l), variant: layout::first(&env("TYPEIN_VARIANT").unwrap_or_default()), source: "TYPEIN_LAYOUT".into() };
    }
    if let Some(l) = env("DOTOOL_XKB_LAYOUT") {
        return Layout { layout: layout::first(&l), variant: layout::first(&env("DOTOOL_XKB_VARIANT").unwrap_or_default()), source: "DOTOOL_XKB_LAYOUT".into() };
    }
    let cfg = env("XDG_CONFIG_HOME").map(std::path::PathBuf::from).or_else(|| env("HOME").map(|h| std::path::PathBuf::from(h).join(".config")));
    if let Some(dir) = cfg {
        let p = dir.join("cosmic/com.system76.CosmicComp/v1/xkb_config");
        if let Some(l) = std::fs::read_to_string(&p).ok().and_then(|t| layout::from_cosmic_ron(&t)) {
            return l;
        }
    }
    if let Some(l) = env("XKB_DEFAULT_LAYOUT") {
        return Layout { layout: layout::first(&l), variant: layout::first(&env("XKB_DEFAULT_VARIANT").unwrap_or_default()), source: "XKB_DEFAULT_LAYOUT".into() };
    }
    if let Some(l) = Command::new("localectl").arg("status").output().ok().filter(|o| o.status.success()).and_then(|o| layout::from_localectl(&String::from_utf8_lossy(&o.stdout))) {
        return l;
    }
    if let Some(l) = std::fs::read_to_string("/etc/default/keyboard").ok().and_then(|t| layout::from_etc_default_keyboard(&t)) {
        return l;
    }
    Layout { layout: "us".into(), variant: String::new(), source: "fallback (nothing else said)".into() }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut literal = false;
    let (mut dry, mut check) = (false, false);
    while let Some(a) = args.first().cloned() {
        match a.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return;
            }
            "--dry" => {
                dry = true;
                args.remove(0);
            }
            "--check" => {
                check = true;
                args.remove(0);
            }
            "--" => {
                args.remove(0);
                literal = true;
                break;
            }
            _ => break,
        }
    }

    let l = detect();
    let bin = env("DOTOOL").unwrap_or_else(|| "dotool".into());

    if check {
        println!("layout   {}", l.layout);
        println!("variant  {}", if l.variant.is_empty() { "(none)" } else { &l.variant });
        println!("source   {}", l.source);
        match which(&bin) {
            Some(p) => println!("dotool   {p}"),
            None => println!("dotool   NOT FOUND — apk add dotool"),
        }
        let uinput = std::fs::OpenOptions::new().write(true).open("/dev/uinput");
        println!("uinput   {}", if uinput.is_ok() { "writable".into() } else { format!("not writable ({}) — run under sudo or join the input group", uinput.unwrap_err().kind()) });
        return;
    }

    if args.is_empty() {
        eprint!("{USAGE}");
        std::process::exit(1);
    }

    let steps = match keyspec::parse(&args, literal) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("typein: {e}");
            std::process::exit(1);
        }
    };
    let script = keyspec::to_dotool(&steps);

    if dry {
        print!("{script}");
        return;
    }

    let mut child = match Command::new(&bin)
        .env("DOTOOL_XKB_LAYOUT", &l.layout)
        .env("DOTOOL_XKB_VARIANT", &l.variant)
        .stdin(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("typein: cannot run {bin}: {e}");
            std::process::exit(1);
        }
    };
    if let Some(mut si) = child.stdin.take() {
        let _ = si.write_all(script.as_bytes());
    }
    let st = child.wait().ok().and_then(|s| s.code()).unwrap_or(1);
    std::process::exit(st);
}

fn which(bin: &str) -> Option<String> {
    if bin.contains('/') {
        return std::path::Path::new(bin).exists().then(|| bin.to_string());
    }
    env("PATH")?.split(':').map(|d| std::path::Path::new(d).join(bin)).find(|p| p.exists()).map(|p| p.display().to_string())
}
