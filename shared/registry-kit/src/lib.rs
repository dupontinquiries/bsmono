//! # registry-kit
//!
//! Link-time registries: declare a registry of entries, then register entries
//! from anywhere in the crate (or any module) with **no central list to edit**.
//! Built on [`linkme`] distributed slices; this crate is the ergonomic facade,
//! so apps depend on `registry-kit` (one place to manage the pattern and the
//! `linkme` version) instead of `linkme` directly.
//!
//! This generalizes the autoloader used in unendlich's `sim/service_registry`
//! (and the other `*_registry*` modules), where a `ServiceEntry` slice collects
//! entries declared next to each service. Migrating those onto this kit is a
//! near-mechanical swap (see below).
//!
//! ## Use
//! ```
//! use registry_kit::{registry, entry, collect};
//!
//! #[derive(Debug)]
//! pub struct Service { pub name: &'static str }
//!
//! // 1. declare the registry (a slice of your entry type)
//! registry!(pub SERVICES: Service);
//!
//! // 2. register entries from anywhere — co-locate them with their feature
//! #[entry(SERVICES)] static THIRST:   Service = Service { name: "thirst" };
//! #[entry(SERVICES)] static TREMBLE:  Service = Service { name: "tremble" };
//!
//! // 3. read them back (order is unspecified)
//! let names: Vec<_> = collect(&SERVICES).into_iter().map(|s| s.name).collect();
//! assert!(names.contains(&"thirst") && names.contains(&"tremble"));
//! assert_eq!(SERVICES.len(), 2);
//! ```
//!
//! ## Migrating an existing linkme registry
//! ```text
//! - use linkme::distributed_slice;                  + use registry_kit::{registry, entry};
//! - #[distributed_slice]                            + registry!(pub SERVICES: ServiceEntry);
//! - pub static SERVICES: [ServiceEntry] = [..];
//! - #[distributed_slice(SERVICES)]                  + #[entry(SERVICES)]
//!   static THIRST: ServiceEntry = ...;                static THIRST: ServiceEntry = ...;
//! ```

/// Register an entry into a registry. Re-export of [`linkme::distributed_slice`]
/// so registration sites depend only on this crate:
/// `#[registry_kit::entry(MY_REGISTRY)] static X: Entry = ...;`
pub use linkme::distributed_slice as entry;

#[doc(hidden)]
pub use linkme;

/// Declare a registry — a distributed slice of `EntryType`.
///
/// `registry!(pub SERVICES: ServiceEntry);`
/// Attributes (docs, `cfg`, …) pass through:
/// `registry!(#[doc = "all services"] pub SERVICES: ServiceEntry);`
#[macro_export]
macro_rules! registry {
    ($(#[$m:meta])* $vis:vis $NAME:ident : $Entry:ty) => {
        $(#[$m])*
        #[$crate::linkme::distributed_slice]
        $vis static $NAME: [$Entry] = [..];
    };
}

/// Collect references to every registered entry. Convenience over iterating the
/// slice directly (`for e in &REGISTRY { … }` also works).
pub fn collect<T>(reg: &'static [T]) -> Vec<&'static T> {
    reg.iter().collect()
}
