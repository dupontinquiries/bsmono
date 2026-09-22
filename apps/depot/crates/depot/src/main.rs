//! `depot` — one binary. `depot serve` is the server; everything else is the
//! CLI that operators and scripts use on either side of the air gap.

mod checks;
mod clientcfg;
mod terminal;
mod tutorial;
mod web;

use cli_kit::Cli;
use depot_core::config::default_root;
use depot_core::pack::Select;
use depot_core::{sig, Depot};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

const USAGE: &str = "\
depot — an offline mirror of cargo crates, Fedora RPMs, a Nix binary cache and
Rust toolchains, moved between networks as signed, deduplicated packs.

USAGE
  depot <command> [options]        --root DIR or $DEPOT_ROOT selects the depot
                                   (default: /var/lib/depot as root, else
                                   ~/.local/share/depot)

SET UP
  init                             create the root, depot.toml, a signing key
  serve [--bind ADDR] [--quiet]    run the web server + UI (default 0.0.0.0:8000)
  status [--json]                  what this depot holds
  admin-token [--new]              show / create the token that enables HTTP writes
  keygen                           create keys/depot.key if missing
  trust NAME (HEX|FILE)            accept packs signed by this public key
  client-config --server URL [--crates] [--rpm ID]... [--nix] [--toolchain] [--all]
                                   print the config a device needs

INGEST (online side, or anywhere you have the files)
  ingest crate FILE|DIR...         .crate files
  ingest rpm --repo ID FILE|DIR... .rpm files into repo ID
  ingest nix DIR                   a `nix copy --to file://DIR` directory
  ingest toolchain DIR|FILE...     rust-install/ from pack-rust.sh, or tarballs
  rpm gpg-key --repo ID FILE       attach the repo's RPM-GPG-KEY (ASCII armored)
  rpm describe --repo ID TEXT      label a repo in the UI
  yank NAME VERS [--undo]          mark a crate version yanked in the index

SYNC (sneakernet or HTTP)
  have [--out FILE]                list every object hash here (give it to pack)
  pack --out FILE [--have FILE] [--all] [--crates] [--rpm ID|*]... [--nix]
       [--toolchains] [--unsigned]  write a pack of what the receiver lacks
  unpack FILE [--allow-unsigned]   read a pack: verify signature + hashes, merge
  inspect FILE                     show a pack's manifest without unpacking

MAINTENANCE
  rebuild                          regenerate pub/ from the catalog
  verify                           re-hash every object
  gc [--delete]                    list (or delete) objects nothing references
  manual | decisions               print the manual / the design decisions
  tutorial [NAME]                  list the step-by-step tutorials, or print one
  niftyplot FILE|-                 render a .niftyplot diagram (or stdin) to SVG on stdout
";

fn main() -> ExitCode {
    let cli = Cli::from_env();
    if cli.wants_help() && cli.subcommand().is_none() {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("depot: {e}");
            ExitCode::from(1)
        }
    }
}

fn root(cli: &Cli) -> PathBuf {
    cli.opt("root").map(PathBuf::from).unwrap_or_else(default_root)
}

fn open(cli: &Cli) -> Result<Depot, String> {
    Depot::open(&root(cli)).map_err(|e| e.to_string())
}

fn run(cli: &Cli) -> Result<(), String> {
    let value_opts = ["root", "bind", "repo", "out", "have", "server", "rpm"];
    let pos = cli.positionals_with_values(&value_opts);
    let Some(cmd) = pos.first().copied() else {
        print!("{USAGE}");
        return Ok(());
    };
    let args: Vec<&str> = pos[1..].to_vec();
    match cmd {
        "init" => {
            let r = root(cli);
            let d = Depot::init(&r).map_err(|e| e.to_string())?;
            let st = d.status().map_err(|e| e.to_string())?;
            println!("initialised {}", r.display());
            println!("  config      {}/depot.toml", r.display());
            println!("  signing key {}", st.signing_key.unwrap_or_default());
            println!("  writes      disabled until `depot admin-token --new`");
            println!("next: `depot serve`, then open http://<this host>:8000/");
            Ok(())
        }
        "serve" => serve(cli),
        "status" => {
            let d = open(cli)?;
            let st = d.status().map_err(|e| e.to_string())?;
            if cli.flag("json") {
                println!("{}", serde_json::to_string_pretty(&st).unwrap());
            } else {
                println!("root        {}", st.root);
                println!("bind        {}  (public_url: {:?})", st.bind, d.config.server.public_url);
                println!("objects     {}  ({})", st.objects, human(st.bytes));
                println!("crates      {} names, {} versions", st.counts.crates, st.counts.crate_versions);
                println!("rpm         {} repos, {} packages", st.counts.rpm_repos, st.counts.rpm_packages);
                for (id, r) in &d.catalog.rpm {
                    println!("              {id}: {} packages{}{}", r.packages.len(), if r.gpg_key.is_some() { ", gpg key" } else { ", NO gpg key" }, if r.description.is_empty() { String::new() } else { format!(" — {}", r.description) });
                }
                println!("nix         {} store paths", st.counts.nix_paths);
                println!("toolchains  {} files", st.counts.toolchain_files);
                println!("signing key {}", st.signing_key.as_deref().unwrap_or("(none — depot keygen)"));
                println!("trusted     {}", if st.trusted.is_empty() { "(none — depot trust NAME HEX)".to_string() } else { st.trusted.join(", ") });
                println!("http writes {}", if st.writes_enabled { "enabled (admin token present)" } else { "disabled" });
            }
            Ok(())
        }
        "admin-token" => {
            let d = open(cli)?;
            let p = d.root.join(&d.config.server.admin_token_file);
            if cli.flag("new") {
                if p.exists() {
                    return Err(format!("{} exists; delete it to rotate", p.display()));
                }
                let tok = net_kit::auth::generate_key();
                write_private(&p, &tok)?;
                println!("{tok}");
                eprintln!("written to {} (0600). HTTP writes are now enabled; pass it as Authorization: Bearer <token>.", p.display());
            } else {
                match std::fs::read_to_string(&p) {
                    Ok(t) => println!("{}", t.trim()),
                    Err(_) => println!("(no token — HTTP writes disabled; `depot admin-token --new` to enable)"),
                }
            }
            Ok(())
        }
        "keygen" => {
            let r = root(cli);
            let pk = sig::keygen(&r).map_err(|e| e.to_string())?;
            println!("{pk}");
            Ok(())
        }
        "trust" => {
            let r = root(cli);
            let (name, key) = match args.as_slice() {
                [n, k] => (*n, *k),
                _ => return Err("usage: depot trust NAME (HEX|FILE)".into()),
            };
            let hex = if Path::new(key).is_file() { std::fs::read_to_string(key).map_err(|e| e.to_string())?.trim().to_string() } else { key.to_string() };
            sig::trust(&r, name, &hex).map_err(|e| e.to_string())?;
            println!("trusting {name} ({}…)", &hex[..16.min(hex.len())]);
            Ok(())
        }
        "client-config" => {
            let d = open(cli)?;
            let server = cli.require("server")?;
            let want = want_from_cli(cli, &d);
            for s in clientcfg::snippets(&d, server, &want) {
                println!("# ── {} → {}", s.kind, s.path);
                print!("{}", s.contents);
                println!("# then: {}\n", s.after);
            }
            Ok(())
        }
        "ingest" => ingest(cli, &args),
        "rpm" => {
            let mut d = open(cli)?;
            let repo = cli.require("repo")?;
            match args.as_slice() {
                ["gpg-key", f] => {
                    let i = d.set_rpm_gpg_key(repo, Path::new(f)).map_err(|e| e.to_string())?;
                    println!("{}: gpg key {} ({})", repo, &i.hash[..12], human(i.size));
                }
                ["describe", rest @ ..] => {
                    d.set_rpm_description(repo, &rest.join(" ")).map_err(|e| e.to_string())?;
                    println!("{repo}: described");
                }
                _ => return Err("usage: depot rpm (gpg-key FILE | describe TEXT) --repo ID".into()),
            }
            Ok(())
        }
        "yank" => {
            let mut d = open(cli)?;
            let (name, vers) = match args.as_slice() {
                [n, v] => (*n, *v),
                _ => return Err("usage: depot yank NAME VERS [--undo]".into()),
            };
            d.yank_crate(name, vers, !cli.flag("undo")).map_err(|e| e.to_string())?;
            println!("{name}@{vers}: yanked={}", !cli.flag("undo"));
            Ok(())
        }
        "have" => {
            let d = open(cli)?;
            let h = d.have().map_err(|e| e.to_string())?;
            let text = h.join("\n") + "\n";
            match cli.opt("out") {
                Some(p) => {
                    std::fs::write(p, &text).map_err(|e| e.to_string())?;
                    eprintln!("{} hashes → {p}", h.len());
                }
                None => print!("{text}"),
            }
            Ok(())
        }
        "pack" => {
            let d = open(cli)?;
            let out = cli.require("out")?;
            let have: HashSet<String> = match cli.opt("have") {
                Some(p) => std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?.lines().map(|l| l.trim().to_string()).filter(|l| depot_core::is_sha256_hex(l)).collect(),
                None => HashSet::new(),
            };
            let mut sel = Select::default();
            if cli.flag("all") {
                sel = Select::all();
            }
            if cli.flag("crates") {
                sel.crates = true;
            }
            if cli.flag("nix") {
                sel.nix = true;
            }
            if cli.flag("toolchains") {
                sel.toolchains = true;
            }
            if cli.flag("alpine") {
                sel.alpine = true;
            }
            let repos = cli.opt_all("rpm");
            if repos.iter().any(|r| *r == "*") {
                sel.rpm = Some(Vec::new());
            } else if !repos.is_empty() {
                sel.rpm = Some(repos.iter().map(|s| s.to_string()).collect());
            }
            if !sel.crates && sel.rpm.is_none() && !sel.nix && !sel.toolchains && !sel.alpine {
                return Err("nothing selected: --all, or any of --crates --rpm ID --nix --toolchains --alpine".into());
            }
            let rep = d.pack(&sel, &have, Path::new(out), cli.flag("unsigned")).map_err(|e| e.to_string())?;
            let size = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
            println!("{out}: {} objects, {} of artifacts, {} on disk; {} skipped (receiver has them){}", rep.objects, human(rep.bytes), human(size), rep.skipped_have, if cli.flag("unsigned") { "; UNSIGNED" } else { "" });
            Ok(())
        }
        "unpack" => {
            let mut d = open(cli)?;
            let f = args.first().ok_or("usage: depot unpack FILE [--allow-unsigned]")?;
            let rep = d.unpack(Path::new(f), cli.flag("allow-unsigned")).map_err(|e| e.to_string())?;
            println!("{f}: signed by {}", rep.signer.as_deref().unwrap_or("NOBODY (allowed by --allow-unsigned)"));
            println!("  objects: {} new, {} already here ({})", rep.objects_new, rep.objects_present, human(rep.bytes));
            println!("  catalog: {} crate versions, {} rpms, {} nix paths, {} toolchain files, {} alpine repos new", rep.crates_new, rep.rpm_new, rep.nix_new, rep.toolchain_new, rep.alpine_new);
            Ok(())
        }
        "inspect" => {
            let f = args.first().ok_or("usage: depot inspect FILE")?;
            let m = depot_core::pack::inspect(Path::new(f)).map_err(|e| e.to_string())?;
            let c = m.catalog.counts();
            println!("format {}  created {}  signer {}", m.format, m.created, m.signer.as_deref().unwrap_or("(unsigned)"));
            println!("objects {} ({})", m.objects.len(), human(m.objects.iter().map(|o| o.size).sum()));
            println!("catalog: {} crates/{} versions, {} rpm repos/{} packages, {} nix, {} toolchain", c.crates, c.crate_versions, c.rpm_repos, c.rpm_packages, c.nix_paths, c.toolchain_files);
            Ok(())
        }
        "rebuild" => {
            let d = open(cli)?;
            d.rebuild_all().map_err(|e| e.to_string())?;
            println!("rebuilt {}", d.store.pub_dir().display());
            Ok(())
        }
        "verify" => {
            let d = open(cli)?;
            let bad = d.store.verify().map_err(|e| e.to_string())?;
            if bad.is_empty() {
                println!("ok: every object hashes to its name");
                Ok(())
            } else {
                for h in &bad {
                    println!("CORRUPT {h}");
                }
                Err(format!("{} corrupt object(s)", bad.len()))
            }
        }
        "gc" => {
            let d = open(cli)?;
            let orphans = d.orphans().map_err(|e| e.to_string())?;
            let mut bytes = 0;
            for h in &orphans {
                bytes += d.store.size_of(h).unwrap_or(0);
                if cli.flag("delete") {
                    d.store.remove(h).map_err(|e| e.to_string())?;
                }
            }
            println!("{} unreferenced object(s), {}{}", orphans.len(), human(bytes), if cli.flag("delete") { " — deleted" } else { " (pass --delete to remove)" });
            Ok(())
        }
        "manual" => {
            print!("{}", web::MANUAL_MD);
            Ok(())
        }
        "decisions" => {
            print!("{}", web::DECISIONS_MD);
            Ok(())
        }
        "niftyplot" => {
            let src = match args.first() {
                Some(&"-") | None => {
                    let mut s = String::new();
                    std::io::Read::read_to_string(&mut std::io::stdin(), &mut s).map_err(|e| e.to_string())?;
                    s
                }
                Some(f) => std::fs::read_to_string(f).map_err(|e| format!("{f}: {e}"))?,
            };
            print!("{}", nifty_core::svg::render_source(&src));
            Ok(())
        }
        "tutorial" => {
            match args.first() {
                None => {
                    for t in tutorial::all() {
                        println!("{:<16} {}  ({} steps)", t.slug, t.title, t.steps.len());
                    }
                    println!("\ndepot tutorial NAME   — or open http://<depot>:8000/#tutorials for the step view");
                }
                Some(slug) => {
                    let src = tutorial::find(slug).ok_or_else(|| format!("no tutorial {slug:?}; `depot tutorial` lists them"))?;
                    print!("{}", src.text);
                }
            }
            Ok(())
        }
        other => Err(format!("unknown command {other:?}\n{USAGE}")),
    }
}

fn ingest(cli: &Cli, args: &[&str]) -> Result<(), String> {
    let mut d = open(cli)?;
    let (kind, files) = match args {
        [k, rest @ ..] if !rest.is_empty() => (*k, rest),
        _ => return Err("usage: depot ingest (crate|rpm --repo ID|nix|toolchain|alpine) PATH...".into()),
    };
    match kind {
        "crate" => {
            for f in expand(files, "crate") {
                match d.ingest_crate(&f) {
                    Ok(i) => println!("{} {} ({}){}", i.kind, i.id, human(i.size), if i.fresh { "" } else { " [already had it]" }),
                    Err(e) => eprintln!("SKIP {}: {e}", f.display()),
                }
            }
        }
        "rpm" => {
            let repo = cli.require("repo")?;
            let list = expand(files, "rpm");
            let mut n = 0;
            for f in &list {
                match d.ingest_rpm(repo, f, true) {
                    Ok(i) => {
                        n += 1;
                        println!("{} {} ({}){}", i.kind, i.id, human(i.size), if i.fresh { "" } else { " [already had it]" });
                    }
                    Err(e) => eprintln!("SKIP {}: {e}", f.display()),
                }
            }
            d.rebuild_rpm_repo(repo).map_err(|e| e.to_string())?;
            println!("{repo}: {n} package(s) ingested, repodata rebuilt");
        }
        "nix" => {
            for f in files {
                let rep = d.ingest_nix_dir(Path::new(f)).map_err(|e| e.to_string())?;
                println!("{f}: {} store paths ({} new NARs)", rep.paths, rep.new_nars);
                for e in rep.errors {
                    eprintln!("SKIP {e}");
                }
            }
        }
        // A tree laid out like an Alpine mirror: <branch>/<repo>/<arch>/
        // with APKINDEX.tar.gz and any subset of its .apk files. Served at
        // /alpine/... byte-for-byte (clients keep verifying Alpine's keys).
        "alpine" | "apk" => {
            for f in files {
                let rep = d.ingest_alpine_dir(Path::new(f)).map_err(|e| e.to_string())?;
                println!("{f}: {} repo(s), {} package(s) ({} new), {} named by an index but not ingested",
                    rep.repos, rep.packages, rep.new_packages, rep.not_ingested);
                for e in rep.errors {
                    eprintln!("SKIP {e}");
                }
            }
        }
        "toolchain" => {
            for f in files {
                let (items, errors) = d.ingest_toolchain(Path::new(f)).map_err(|e| e.to_string())?;
                for i in items {
                    println!("{} {} ({})", i.kind, i.id, human(i.size));
                }
                for e in errors {
                    eprintln!("SKIP {e}");
                }
            }
        }
        other => return Err(format!("unknown ingest kind {other:?}: crate | rpm | nix | toolchain")),
    }
    Ok(())
}

/// Files as given, plus every `*.<ext>` directly inside any directory given.
fn expand(args: &[&str], ext: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for a in args {
        let p = PathBuf::from(a);
        if p.is_dir() {
            let mut v: Vec<PathBuf> = std::fs::read_dir(&p).map(|rd| rd.flatten().map(|e| e.path()).filter(|q| q.extension().map(|e| e == ext).unwrap_or(false)).collect()).unwrap_or_default();
            v.sort();
            out.extend(v);
        } else {
            out.push(p);
        }
    }
    out
}

fn want_from_cli(cli: &Cli, d: &Depot) -> clientcfg::Want {
    let all = cli.flag("all");
    let mut rpm: Vec<String> = cli.opt_all("rpm").into_iter().map(String::from).collect();
    if all && rpm.is_empty() {
        rpm = d.catalog.rpm.keys().cloned().collect();
    }
    clientcfg::Want { crates: all || cli.flag("crates"), rpm, nix: all || cli.flag("nix"), toolchain: all || cli.flag("toolchain") }
}

fn serve(cli: &Cli) -> Result<(), String> {
    let r = root(cli);
    let depot = Depot::open(&r).map_err(|e| e.to_string())?;
    let bind = cli.opt("bind").map(String::from).unwrap_or_else(|| depot.config.server.bind.clone());
    let workers = depot.config.server.workers;
    let log = depot.config.server.log && !cli.flag("quiet");
    let terminals = if depot.config.terminal.enabled {
        Some(terminal::Terminals::new(depot.config.terminal.shell.clone(), depot.config.terminal.idle_timeout_s, r.clone()))
    } else {
        None
    };
    let term_on = terminals.is_some();
    let app = web::App {
        pub_dir: depot.store.pub_dir(),
        root: r.clone(),
        public_url: depot.config.server.public_url.clone(),
        token_file: r.join(&depot.config.server.admin_token_file),
        terminals,
        depot: Arc::new(Mutex::new(depot)),
    };
    let writes = app.token_file.is_file();
    let server = depot_httpd::Server::bind(&bind, app).map_err(|e| format!("bind {bind}: {e}"))?.workers(workers).log(log);
    let addr = server.local_addr().map_err(|e| e.to_string())?;
    eprintln!("depot serving {} on http://{addr}/  (HTTP writes {}, terminal {})", r.display(), if writes { "enabled" } else { "disabled" }, if term_on { "ENABLED" } else { "off" });
    // Shutdown: systemd sends SIGTERM (Ctrl-C sends SIGINT). The handler
    // only sets a flag; the accept loop sees it within 100 ms, stops
    // accepting, closes the work channel and joins the workers, so responses
    // in flight complete. Every write is atomic anyway, so even an unclean
    // kill loses nothing; this just avoids cutting a download at 99%.
    let stop = Arc::new(AtomicBool::new(false));
    install_stop_handler(Arc::clone(&stop));
    let r = server.serve(stop).map_err(|e| e.to_string());
    eprintln!("depot: stopped");
    r
}

#[cfg(unix)]
fn install_stop_handler(stop: Arc<AtomicBool>) {
    use std::sync::atomic::Ordering;
    static FLAG: std::sync::OnceLock<Arc<AtomicBool>> = std::sync::OnceLock::new();
    // Async-signal-safe: one atomic store, no allocation, no locks.
    extern "C" fn on_signal(_: libc::c_int) {
        if let Some(f) = FLAG.get() {
            f.store(true, Ordering::Relaxed);
        }
    }
    let _ = FLAG.set(stop);
    // SAFETY: installing a handler that only touches an atomic; the
    // function pointer cast is what signal(2) expects.
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
    }
}

#[cfg(not(unix))]
fn install_stop_handler(_stop: Arc<AtomicBool>) {}

fn write_private(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        f.write_all(contents.as_bytes()).and_then(|_| f.write_all(b"\n")).map_err(|e| e.to_string())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, format!("{contents}\n")).map_err(|e| e.to_string())
    }
}

pub fn human(n: u64) -> String {
    const U: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}
