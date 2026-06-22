// gitmind-sim/src/main.rs
//
// Honest measurement harness. For each scenario it versions a large file
// many times and records, per version:
//   - gitmind chunk-store size on disk (cumulative, deduped + zstd)
//   - gitmind new bytes stored THIS version (the incremental cost)
//   - git .git size as LOOSE objects (what git costs before `gc`)
//   - git .git size PACKED (what git costs after `repack -adf` + `gc`)
//
// The point isn't to rig a win. Against fully-packed git on a single
// small-edit file, git's delta compression is genuinely good and the
// sizes land close. The story the numbers tell is:
//   1. gitmind reaches pack-level efficiency at WRITE time — no `gc`
//      step — so it tracks git-packed while git-loose balloons.
//   2. On incompressible media with shared regions, chunk dedup keeps
//      incremental cost low.
// Output: sim_out/metrics.json for the plotter.

use gitmind_core::store::ChunkStore;
use std::fs;
use std::io::Cursor;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

// Deterministic pseudo-random fill (xorshift64*) so runs are reproducible
// and chunk boundaries are stable across versions.
struct Rng(u64);
impl Rng {
    fn fill(&mut self, buf: &mut [u8]) {
        for b in buf.iter_mut() {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            *b = (x.wrapping_mul(0x2545F4914F6CDD1D) >> 33) as u8;
        }
    }
}

fn dir_size(p: &Path) -> u64 {
    let mut total = 0;
    if let Ok(rd) = fs::read_dir(p) {
        for e in rd.flatten() {
            let path = e.path();
            match fs::symlink_metadata(&path) {
                Ok(m) if m.is_dir() => total += dir_size(&path),
                Ok(m) => total += m.len(),
                Err(_) => {}
            }
        }
    }
    total
}

fn git(dir: &Path, args: &[&str]) {
    let st = Command::new("git").arg("-C").arg(dir).args(args).status().unwrap();
    assert!(st.success(), "git {:?}", args);
}

struct Row {
    version: usize,
    gm_cumulative: u64,
    gm_new_bytes: u64,
    git_loose: u64,
    git_packed: u64,
}

/// Version a `size`-byte file `n` times. `mutate` decides how each new
/// version differs from the previous bytes.
fn scenario(
    name: &str,
    size: usize,
    n: usize,
    mut mutate: impl FnMut(&mut Vec<u8>, usize, &mut Rng),
    work: &Path,
) -> Vec<Row> {
    let store_dir = work.join(format!("{name}_store"));
    fs::create_dir_all(&store_dir).unwrap();
    let store = ChunkStore::open(store_dir.join("store.redb")).unwrap();

    let git_dir = work.join(format!("{name}_git"));
    fs::create_dir_all(&git_dir).unwrap();
    git(&git_dir, &["init", "-q"]);
    git(&git_dir, &["config", "user.email", "s@s"]);
    git(&git_dir, &["config", "user.name", "s"]);

    let mut rng = Rng(0x9E3779B97F4A7C15);
    let mut bytes = vec![0u8; size];
    rng.fill(&mut bytes);

    let mut rows = Vec::new();
    for v in 0..n {
        if v > 0 {
            mutate(&mut bytes, v, &mut rng);
        }
        // gitmind ingest
        let before = store_file_size(&store_dir);
        let rep = store.ingest("asset.bin", Cursor::new(bytes.clone())).unwrap();
        let gm_cumulative = store_file_size(&store_dir);
        let gm_new_bytes = rep.new_bytes_stored;
        let _ = before;

        // git: write + commit this version (loose), measure, then pack.
        fs::write(git_dir.join("asset.bin"), &bytes).unwrap();
        git(&git_dir, &["add", "asset.bin"]);
        git(&git_dir, &["commit", "-q", "-m", &format!("v{v}")]);
        let git_loose = dir_size(&git_dir.join(".git"));
        git(&git_dir, &["repack", "-adfq"]);
        git(&git_dir, &["gc", "-q", "--prune=now"]);
        let git_packed = dir_size(&git_dir.join(".git"));

        rows.push(Row { version: v, gm_cumulative, gm_new_bytes, git_loose, git_packed });
    }
    rows
}

// redb keeps everything in one file; its on-disk length is the store size.
fn store_file_size(store_dir: &Path) -> u64 {
    fs::metadata(store_dir.join("store.redb")).map(|m| m.len()).unwrap_or(0)
}

fn emit_json(out: &Path, scenarios: &[(&str, Vec<Row>)], throughput_mb_s: f64) {
    let mut s = String::from("{\n");
    s.push_str(&format!("  \"throughput_mb_s\": {:.1},\n", throughput_mb_s));
    s.push_str("  \"scenarios\": {\n");
    for (si, (name, rows)) in scenarios.iter().enumerate() {
        s.push_str(&format!("    \"{name}\": [\n"));
        for (ri, r) in rows.iter().enumerate() {
            s.push_str(&format!(
                "      {{\"version\": {}, \"gm_cumulative\": {}, \"gm_new_bytes\": {}, \"git_loose\": {}, \"git_packed\": {}}}{}\n",
                r.version, r.gm_cumulative, r.gm_new_bytes, r.git_loose, r.git_packed,
                if ri + 1 < rows.len() { "," } else { "" }
            ));
        }
        s.push_str(&format!("    ]{}\n", if si + 1 < scenarios.len() { "," } else { "" }));
    }
    s.push_str("  }\n}\n");
    fs::write(out, s).unwrap();
}

fn main() {
    let work = std::env::temp_dir().join("gitmind_sim_work");
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).unwrap();

    let mb = 1024 * 1024;

    // Scenario A: small edit (16 KiB) to a 16 MiB incompressible file,
    // 10 versions. Mimics re-saving a large asset with a tiny change.
    let a = scenario(
        "small_edit_16MiB",
        16 * mb,
        10,
        |bytes, _v, rng| {
            let mid = bytes.len() / 2;
            let mut patch = [0u8; 16 * 1024];
            rng.fill(&mut patch);
            bytes[mid..mid + patch.len()].copy_from_slice(&patch);
        },
        &work,
    );

    // Scenario B: append-only growth (a log/recording getting longer),
    // 10 versions, +1 MiB each. CDC dedups the unchanged prefix; git must
    // store a new (un-deltable across the boundary) object each time.
    let b = scenario(
        "append_1MiB",
        8 * mb,
        10,
        |bytes, _v, rng| {
            let mut tail = vec![0u8; mb];
            rng.fill(&mut tail);
            bytes.extend_from_slice(&tail);
        },
        &work,
    );

    // Throughput: ingest a 64 MiB blob once, measure MB/s end to end.
    let tdir = work.join("tput_store");
    fs::create_dir_all(&tdir).unwrap();
    let tstore = ChunkStore::open(tdir.join("store.redb")).unwrap();
    let mut rng = Rng(1);
    let mut big = vec![0u8; 64 * mb];
    rng.fill(&mut big);
    let t0 = Instant::now();
    tstore.ingest("big.bin", Cursor::new(big)).unwrap();
    let secs = t0.elapsed().as_secs_f64();
    let throughput = (64.0) / secs;

    let out_dir = Path::new("sim_out");
    fs::create_dir_all(out_dir).unwrap();
    emit_json(&out_dir.join("metrics.json"), &[("small_edit_16MiB", a), ("append_1MiB", b)], throughput);

    // Console summary.
    let m: String = fs::read_to_string(out_dir.join("metrics.json")).unwrap();
    println!("ingest throughput: {throughput:.1} MB/s");
    println!("wrote sim_out/metrics.json ({} bytes)", m.len());
    let _ = fs::remove_dir_all(&work);
}
