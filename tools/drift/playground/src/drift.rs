//! Drift — the two-sided boundary journal (MVP).
//!
//! The whole idea in one paragraph: a *boundary* is any seam where two parts
//! of a system change independently (Rust <-> QML, service <-> service,
//! caller <-> callee across a dynamic edge). Each side makes a claim. The
//! producer claims "I serviced intent X." The consumer claims "I attempted
//! intent X." Drift records both and, at the end, diffs the two sets. The
//! difference localizes every wiring break with no guessing:
//!
//!   attempted ∩ serviced  -> healthy boundary
//!   attempted − serviced  -> BROKEN WIRE  (consumer called, nobody answered)
//!   serviced  − attempted -> DEAD SURFACE (producer ready, nobody called it)
//!
//! Crucially: with the `drift-trace` feature OFF, the macros expand to
//! nothing. The `#[cfg]` is *inside* the expansion, so the recording calls
//! (and this module's machinery) are stripped before name resolution. The
//! normal build never pays for any of it.

#[cfg(feature = "drift-trace")]
mod inner {
    use std::collections::BTreeSet;
    use std::sync::{Mutex, OnceLock};

    #[derive(Default)]
    struct Journal {
        attempted: BTreeSet<String>, // consumer side
        serviced: BTreeSet<String>,  // producer side
    }

    fn journal() -> &'static Mutex<Journal> {
        static J: OnceLock<Mutex<Journal>> = OnceLock::new();
        J.get_or_init(|| Mutex::new(Journal::default()))
    }

    pub fn record_attempt(name: &str) {
        journal().lock().unwrap().attempted.insert(name.to_string());
    }

    pub fn record_service(name: &str) {
        journal().lock().unwrap().serviced.insert(name.to_string());
    }

    pub fn report() {
        let j = journal().lock().unwrap();
        eprintln!("\n──────── drift journal ────────");

        for n in j.attempted.intersection(&j.serviced) {
            eprintln!("  ✓  {n}: consumer called, producer serviced");
        }
        for n in j.attempted.difference(&j.serviced) {
            eprintln!(
                "  ✗  {n}: consumer attempted, NO producer serviced it\n         → BROKEN WIRE (typo / renamed method / null context / wrong object)"
            );
        }
        for n in j.serviced.difference(&j.attempted) {
            eprintln!(
                "  ⚠  {n}: producer serviced, NO consumer attempted it\n         → DEAD SURFACE (unwired button / removed caller)"
            );
        }
        eprintln!("───────────────────────────────\n");
    }
}

// Stable names the macros call. Present only when the feature is on; when off,
// the macro lines that reference them are cfg-stripped, so nothing dangles.
#[cfg(feature = "drift-trace")]
pub use inner::{record_attempt as __attempt, record_service as __service, report as __report};

/// Producer side. Wraps the body of a boundary-crossing function and records
/// that the intent was serviced. Returns the body's value unchanged.
#[macro_export]
macro_rules! intent {
    ($name:expr, $body:block) => {{
        #[cfg(feature = "drift-trace")]
        $crate::drift::__service($name);
        $body
    }};
}

/// Consumer side. Declares "I am about to invoke intent `$name` across the
/// boundary." Place it right before the call site.
#[macro_export]
macro_rules! attempt {
    ($name:expr) => {{
        #[cfg(feature = "drift-trace")]
        $crate::drift::__attempt($name);
    }};
}

/// Print the reconciliation. No-op when the feature is off.
#[macro_export]
macro_rules! drift_report {
    () => {{
        #[cfg(feature = "drift-trace")]
        $crate::drift::__report();
    }};
}
