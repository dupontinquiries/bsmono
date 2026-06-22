use unendlich_shared::run_summary::RunBlock;

fn main() {
    let path = "/tmp/unendlich_version_demo.txt";
    let _ = std::fs::remove_file(path);

    // STAGE 1: simulate two pre-marker (v0.24.2-c.2) runs written
    // directly to the file (as a pre-c.3 binary would have).
    std::fs::write(path, "\
==== RUN START 2026-05-11T13:00:00Z ====
version: 0.24.2-c.2
duration: 30.0s
cmdline: pre-c.3 unendlich-world
env:
  UNENDLICH_RD = 8
---- summaries ----
GPU summary stuff
==== RUN END 2026-05-11T13:00:30Z ====

==== RUN START 2026-05-11T13:01:00Z ====
version: 0.24.2-c.2
duration: 25.0s
cmdline: pre-c.3 unendlich-world
env:
  UNENDLICH_RD = 6
---- summaries ----
more GPU summary stuff
==== RUN END 2026-05-11T13:01:25Z ====

").expect("write pre-marker file");

    // STAGE 2: first c.3 run.
    let block1 = RunBlock {
        version: "0.24.2-c.3",
        uuid: "r-1111111111111111",
        duration_secs: 30.0,
        cmdline: "unendlich-world (c.3 binary)",
        env_flags: "UNENDLICH_RD = 8\nUNENDLICH_SHADOW_CASCADES = 1",
        body: "first c.3 GPU summary",
    };
    block1.append(path).expect("append c.3 #1");

    // STAGE 3: second c.3 run (no extra markers expected).
    let block2 = RunBlock {
        version: "0.24.2-c.3",
        uuid: "r-2222222222222222",
        duration_secs: 25.0,
        cmdline: "unendlich-world (c.3 binary)",
        env_flags: "UNENDLICH_RD = 8\nUNENDLICH_SHADOW_CASCADES = 4",
        body: "second c.3 GPU summary",
    };
    block2.append(path).expect("append c.3 #2");

    let contents = std::fs::read_to_string(path).unwrap();
    println!("{}", contents);
}
