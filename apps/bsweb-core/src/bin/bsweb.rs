//! bsweb — friendly management of a blackstar parts web.

use anyhow::{bail, Result};
use bsweb::{matches, Codec, Part, Web};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "bsweb",
    about = "Inspect and edit a blackstar parts web (.bin / .bin.gz / .bin.zst)",
    long_about = "A parts web stores geometry once and references it many times, with the \
                  annotations that say what each piece is. Most commands read only the JSON \
                  index (637 KB on the real catalog) and never touch the 7.3 MB payload.",
    version
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum Out {
    /// Aligned columns for reading.
    Table,
    /// One id per line, for pipes.
    Ids,
    /// Full records.
    Json,
}

#[derive(Subcommand)]
enum Cmd {
    /// List parts, with filters.
    Ls {
        web: PathBuf,
        /// Part kind, glob ok: roadwheel, *wheel*
        #[arg(long)]
        name: Option<String>,
        /// Source model, glob ok: car-*
        #[arg(short, long)]
        from: Option<String>,
        /// Catalog: vehiclecat, greeblecat, …
        #[arg(short, long)]
        catalog: Option<String>,
        /// Only parts that recurred at least this often.
        #[arg(long)]
        min_instances: Option<u32>,
        /// Only parts whose cluster fit was this tight or better.
        #[arg(long)]
        max_residual: Option<f64>,
        /// Only parts sharing geometry with another part.
        #[arg(long)]
        shared: bool,
        #[arg(short, long, value_enum, default_value_t = Out::Table)]
        output: Out,
        #[arg(short = 'n', long, default_value_t = 40)]
        limit: usize,
    },
    /// Everything known about one part.
    Show { web: PathBuf, id: String },
    /// Counts, sizes, dedup ratio, the annotation coverage.
    Stat { web: PathBuf },
    /// Structural check. Exit 1 if anything is wrong.
    Verify { web: PathBuf },
    /// What each part kind is, and how many there are.
    Kinds { web: PathBuf, #[arg(short = 'n', long, default_value_t = 30)] limit: usize },
    /// Remove parts by id or by filter. Geometry is kept until `gc`.
    Rm {
        web: PathBuf,
        /// Ids to remove. Globs allowed.
        ids: Vec<String>,
        /// Remove by part kind instead.
        #[arg(long)]
        name: Option<String>,
        /// Show what would go, write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Write here instead of in place.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Drop unreferenced geometry and renumber.
    Gc { web: PathBuf, #[arg(short, long)] out: Option<PathBuf> },
    /// Copy parts from one web into another.
    Merge {
        /// The web to add to.
        web: PathBuf,
        /// The web to take from.
        source: PathBuf,
        /// Only parts matching this kind.
        #[arg(long)]
        name: Option<String>,
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Write shell completions to stdout.
    ///
    /// bash:  bsweb completions bash  > /etc/bash_completion.d/bsweb
    /// zsh:   bsweb completions zsh   > ~/.zfunc/_bsweb
    /// fish:  bsweb completions fish  > ~/.config/fish/completions/bsweb.fish
    Completions { shell: Shell },
}

fn human(n: u64) -> String {
    const U: [&str; 5] = ["B", "K", "M", "G", "T"];
    let (mut v, mut i) = (n as f64, 0);
    while v >= 1024.0 && i < 4 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{n} B") } else { format!("{v:.1} {}", U[i]) }
}

fn pick(p: &Part, name: &Option<String>, from: &Option<String>, catalog: &Option<String>,
        min_inst: Option<u32>, max_res: Option<f64>) -> bool {
    if let Some(n) = name {
        if !p.name.as_deref().map(|s| matches(n, s)).unwrap_or(false) { return false; }
    }
    if let Some(f) = from {
        if !p.from.as_deref().map(|s| matches(f, s)).unwrap_or(false) { return false; }
    }
    if let Some(c) = catalog {
        if p.catalog.as_deref() != Some(c.as_str()) { return false; }
    }
    if let Some(m) = min_inst {
        if p.instances.unwrap_or(0) < m { return false; }
    }
    if let Some(r) = max_res {
        if p.residual_mean.unwrap_or(f64::MAX) > r { return false; }
    }
    true
}

fn save(w: &Web, src: &PathBuf, out: &Option<PathBuf>) -> Result<()> {
    let dest = out.clone().unwrap_or_else(|| src.clone());
    let codec = out.as_ref().map(|p| Codec::for_path(p)).unwrap_or(w.codec);
    let n = w.write(&dest, codec)?;
    println!("wrote {} ({})", dest.display(), human(n));
    Ok(())
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "bsweb", &mut std::io::stdout());
        }

        Cmd::Ls { web, name, from, catalog, min_instances, max_residual, shared, output, limit } => {
            let w = Web::open_index_only(&web)?;
            let rc = w.refcounts();
            let sel: Vec<&Part> = w.index.parts.iter()
                .filter(|p| pick(p, &name, &from, &catalog, min_instances, max_residual))
                .filter(|p| !shared || rc.get(p.geom).copied().unwrap_or(0) > 1)
                .collect();
            match output {
                Out::Ids => for p in sel.iter().take(limit) { println!("{}", Web::key(p)); },
                Out::Json => println!("{}", serde_json::to_string_pretty(&sel)?),
                Out::Table => {
                    println!("{:<34} {:<14} {:>6} {:>5} {:>8}  {}",
                             "id", "kind", "tris", "inst", "residual", "from");
                    for p in sel.iter().take(limit) {
                        println!("{:<34} {:<14} {:>6} {:>5} {:>8}  {}",
                                 Web::key(p).chars().take(34).collect::<String>(),
                                 p.name.as_deref().unwrap_or("-"),
                                 p.nt,
                                 p.instances.map(|i| i.to_string()).unwrap_or("-".into()),
                                 p.residual_mean.map(|r| format!("{r:.4}")).unwrap_or("-".into()),
                                 p.from.as_deref().unwrap_or("-"));
                    }
                }
            }
            if sel.len() > limit {
                eprintln!("\n{} of {} shown ( -n {} for more )", limit, sel.len(), sel.len());
            } else {
                eprintln!("\n{} part(s)", sel.len());
            }
        }

        Cmd::Show { web, id } => {
            let w = Web::open_index_only(&web)?;
            let all = w.find_all(&id);
            if all.is_empty() { bail!("no part {id}"); }
            if all.len() > 1 {
                eprintln!("'{id}' is ambiguous — it exists in {} catalogs with different \
                           geometry. Use one of:", all.len());
                for p in &all {
                    eprintln!("   {}   ({} tris)", Web::key(p), p.nt);
                }
                std::process::exit(2);
            }
            let p = all[0];
            println!("{}", serde_json::to_string_pretty(p)?);
            let rc = w.refcounts();
            let n = rc.get(p.geom).copied().unwrap_or(0);
            if n > 1 {
                let sharers: Vec<&str> = w.index.parts.iter()
                    .filter(|q| q.geom == p.geom && Web::key(q) != Web::key(p))
                    .map(|q| q.id.as_str()).collect();
                println!("\ngeometry shared with {} other part(s): {}", n - 1, sharers.join(", "));
            }
        }

        Cmd::Stat { web } => {
            let w = Web::open(&web)?;
            let pl = w.payload()?.len();
            let rc = w.refcounts();
            let np = w.index.parts.len();
            let ng = w.index.geom.len();
            let annotated = w.index.parts.iter().filter(|p| p.name.is_some()).count();
            let tris: usize = w.index.parts.iter().map(|p| p.nt).sum();
            println!("codec        : {:?}", w.codec);
            println!("index version: {}", w.index.version);
            println!("parts        : {np}");
            println!("unique geom  : {ng}   ({} folded)", np.saturating_sub(ng));
            println!("annotated    : {annotated} / {np}");
            println!("triangles    : {tris}");
            println!("payload      : {}", human(pl as u64));
            println!("on disk      : {}", human(std::fs::metadata(&web)?.len()));
            println!("shared blobs : {}", rc.iter().filter(|c| **c > 1).count());
            println!("orphan blobs : {}", rc.iter().filter(|c| **c == 0).count());
        }

        Cmd::Verify { web } => {
            let w = Web::open(&web)?;
            let errs = w.verify();
            if errs.is_empty() {
                println!("ok: {} parts, {} geom blobs, payload consistent",
                         w.index.parts.len(), w.index.geom.len());
            } else {
                for e in &errs { println!("FAIL {e}"); }
                std::process::exit(1);
            }
        }

        Cmd::Kinds { web, limit } => {
            let w = Web::open_index_only(&web)?;
            let mut m = std::collections::BTreeMap::<&str, (usize, usize)>::new();
            for p in &w.index.parts {
                let e = m.entry(p.name.as_deref().unwrap_or("(unannotated)")).or_default();
                e.0 += 1;
                e.1 += p.instances.unwrap_or(0) as usize;
            }
            let mut v: Vec<_> = m.into_iter().collect();
            v.sort_by_key(|(_, (c, _))| std::cmp::Reverse(*c));
            println!("{:<22} {:>7} {:>10}", "kind", "parts", "instances");
            for (k, (c, i)) in v.into_iter().take(limit) {
                println!("{k:<22} {c:>7} {i:>10}");
            }
        }

        Cmd::Rm { web, ids, name, dry_run, out } => {
            let mut w = Web::open(&web)?;
            let mut hit: HashSet<String> = HashSet::new();
            for p in &w.index.parts {
                if ids.iter().any(|g| matches(g, &p.id) || matches(g, &Web::key(p)))
                    || name.as_deref().map(|n| p.name.as_deref().map(|s| matches(n, s)).unwrap_or(false)).unwrap_or(false)
                {
                    hit.insert(Web::key(p));
                }
            }
            if hit.is_empty() { bail!("nothing matched"); }
            if dry_run {
                println!("would remove {} part(s):", hit.len());
                let mut v: Vec<_> = hit.iter().collect();
                v.sort();
                for id in v.iter().take(40) { println!("  {id}"); }
                if v.len() > 40 { println!("  … and {} more", v.len() - 40); }
                return Ok(());
            }
            let n = w.remove(&hit);
            let orphans = w.refcounts().iter().filter(|c| **c == 0).count();
            println!("removed {n} part(s); {orphans} geom blob(s) now orphaned (run `gc` to reclaim)");
            save(&w, &web, &out)?;
        }

        Cmd::Gc { web, out } => {
            let mut w = Web::open(&web)?;
            let (dropped, freed) = w.gc()?;
            println!("dropped {dropped} geom blob(s), freed {} of payload", human(freed as u64));
            save(&w, &web, &out)?;
        }

        Cmd::Merge { web, source, name, out } => {
            let mut dst = Web::open(&web)?;
            let src = Web::open(&source)?;
            let offs = src.offsets();
            let pl = src.payload()?;
            let (mut added, mut folded, mut skipped) = (0, 0, 0);
            for p in &src.index.parts {
                if let Some(n) = &name {
                    if !p.name.as_deref().map(|s| matches(n, s)).unwrap_or(false) { continue; }
                }
                if dst.find(&Web::key(p)).is_some() { skipped += 1; continue; }
                let g = src.index.geom[p.geom];
                let o = offs[p.geom];
                let shared = dst.add(p.clone(), &pl[o..o + g.v], &pl[o + g.v..o + g.v + g.i])?;
                added += 1;
                if shared { folded += 1; }
            }
            println!("added {added} part(s) ({folded} folded onto existing geometry), {skipped} skipped as duplicate ids");
            save(&dst, &web, &out)?;
        }
    }
    Ok(())
}
