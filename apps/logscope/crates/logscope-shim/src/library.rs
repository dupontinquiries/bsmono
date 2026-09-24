//! library — the NVMe library and the local cache. `ensure` copies a model in (streaming, so a 40 GB
//! file doesn't sit in memory) and evicts least-recently-used files past the budget first; `touch`
//! marks use. Rotation is just that: the budget plus LRU, nothing cleverer, so you can read the
//! cache directory and know why a file is there.
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Entry { pub name: String, pub bytes: u64, pub path: PathBuf, pub cached: bool }

pub fn scan(dir: &Path) -> Vec<Entry> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x.eq_ignore_ascii_case("gguf")).unwrap_or(false) {
                v.push(Entry { name: p.file_name().unwrap_or_default().to_string_lossy().to_string(), bytes: p.metadata().map(|m| m.len()).unwrap_or(0), path: p, cached: false });
            }
        }
    }
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

pub fn cache_bytes(cache: &Path) -> u64 { scan(cache).iter().map(|e| e.bytes).sum() }

fn last_use(p: &Path) -> SystemTime { p.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH) }

/// Make room for `need` bytes: evict LRU until `cache_bytes + need <= budget`. Never evicts `keep`.
pub fn evict(cache: &Path, budget: u64, need: u64, keep: Option<&str>) -> Vec<String> {
    let mut evicted = Vec::new();
    let mut entries = scan(cache);
    entries.sort_by_key(|e| last_use(&e.path));
    let mut used = entries.iter().map(|e| e.bytes).sum::<u64>();
    for e in entries {
        if used + need <= budget { break; }
        if keep == Some(e.name.as_str()) { continue; }
        if std::fs::remove_file(&e.path).is_ok() { used -= e.bytes; evicted.push(e.name); }
    }
    evicted
}

/// Copy `name` from the library into the cache if it isn't there. Returns the cached path plus
/// what was evicted to make room. A partial copy is written to `.part` and renamed at the end, so a
/// crash mid-copy never leaves a truncated GGUF the server would try to load.
pub fn ensure(library: &Path, cache: &Path, budget: u64, name: &str, on_progress: &mut dyn FnMut(u64, u64)) -> Result<(PathBuf, Vec<String>), String> {
    std::fs::create_dir_all(cache).map_err(|e| e.to_string())?;
    let dst = cache.join(name);
    if dst.is_file() { touch(&dst); return Ok((dst, Vec::new())); }
    let src = library.join(name);
    let bytes = src.metadata().map_err(|_| format!("not in library: {name}"))?.len();
    if bytes > budget { return Err(format!("{name} is {:.1} GiB, over the {:.1} GiB budget", gib(bytes), gib(budget))); }
    let evicted = evict(cache, budget, bytes, Some(name));
    let part = cache.join(format!("{name}.part"));
    {
        use std::io::{Read, Write};
        let mut r = std::fs::File::open(&src).map_err(|e| e.to_string())?;
        let mut w = std::fs::File::create(&part).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 8 << 20];
        let mut done = 0u64;
        loop {
            let n = r.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 { break; }
            w.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            done += n as u64; on_progress(done, bytes);
        }
        w.flush().map_err(|e| e.to_string())?;
    }
    std::fs::rename(&part, &dst).map_err(|e| e.to_string())?;
    touch(&dst);
    Ok((dst, evicted))
}

pub fn touch(p: &Path) { let _ = std::fs::File::options().append(true).open(p).and_then(|f| f.set_modified(SystemTime::now())); }

pub fn gib(b: u64) -> f64 { b as f64 / 1024f64.powi(3) }
