//! Proves the core claim: entries registered in *different modules* all show up
//! in the registry with no central list — the autoloader property.
use registry_kit::{collect, entry, registry};

#[derive(Debug, PartialEq)]
pub struct Plugin {
    pub name: &'static str,
    pub order: u8,
}

registry!(pub PLUGINS: Plugin);

// registered here, at the top level
#[entry(PLUGINS)]
static ROOT: Plugin = Plugin { name: "root", order: 0 };

mod alpha {
    use super::{entry, Plugin, PLUGINS};
    #[entry(PLUGINS)]
    static A: Plugin = Plugin { name: "alpha", order: 1 };
}

mod beta {
    use super::{entry, Plugin, PLUGINS};
    #[entry(PLUGINS)]
    static B1: Plugin = Plugin { name: "beta-1", order: 2 };
    #[entry(PLUGINS)]
    static B2: Plugin = Plugin { name: "beta-2", order: 2 };
}

#[test]
fn all_modules_register() {
    let mut names: Vec<&'static str> = collect(&PLUGINS).into_iter().map(|p| p.name).collect();
    names.sort_unstable();
    assert_eq!(names, ["alpha", "beta-1", "beta-2", "root"]);
    assert_eq!(PLUGINS.len(), 4);
}
