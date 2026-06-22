//! A miniature todo app whose front/back boundary is deliberately broken in
//! the two ways your QML session agonized over. Run it both ways:
//!
//!   cargo run                          # the "pure" build: bug is INVISIBLE
//!   cargo run --features drift-trace   # the journal localizes the bug
//!
//! Without drift, the delete button just silently does nothing and you get
//! zero signal — exactly the "no logs at all" symptom. With drift, the diff
//! between attempted and serviced intents points straight at the break.

#[macro_use]
mod drift;

// ───────────────────────── backend / producer side ─────────────────────────
mod backend {
    pub fn create_task(title: &str) -> u64 {
        intent!("create_task", {
            println!("[backend] created task: {title}");
            1
        })
    }

    // During a refactor this was renamed delete_task -> remove_task.
    // Types are fine; nothing in Rust complains. But the UI below still
    // talks about "delete_task" across the dynamic boundary.
    pub fn remove_task(id: u64) {
        intent!("remove_task", {
            println!("[backend] removed task {id}");
        })
    }

    // Added during a "just one more feature" moment. No button wired to it.
    pub fn archive_task(id: u64) {
        intent!("archive_task", {
            println!("[backend] archived task {id}");
        })
    }
}

// ──────────────────────── frontend / consumer side ─────────────────────────
// Each handler declares the intent it expects, then calls it. In real life
// these are QML onClicked handlers; here they are plain fns so the demo runs.

fn on_add_clicked() {
    attempt!("create_task");
    backend::create_task("buy milk");
}

fn on_delete_clicked() {
    // BUG: this handler still references the old intent name, and the call
    // was never rewired to remove_task — so the button is a silent no-op.
    attempt!("delete_task");
    // (no backend call reaches a real producer)
}

fn main() {
    on_add_clicked();
    on_delete_clicked();

    // A non-UI caller (think: startup/maintenance job) exercises the producer,
    // so it shows up as serviced — but no button ever attempts it. That is the
    // soft "dead surface" case: the capability runs, yet nothing user-facing
    // reaches it. Note this only appears because the path actually executed;
    // a runtime journal can never report a branch you didn't run. (That gap is
    // what the static reachability lint in v2 would cover.)
    backend::archive_task(1);

    drift_report!();
}
