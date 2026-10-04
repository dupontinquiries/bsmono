//! `XDG_DATA_DIRS=/path/a:/path/b XDG_CONFIG_HOME=/tmp/x cargo run --example search -- private`
//! Prints what the launcher would show for a query over the application directories named by `XDG_DATA_DIRS`:
//! the quickest way to see why a search finds (or misses) something, using a copy of a phone's `.desktop` files.
use hatch_core::{Config, Launcher};

fn main() {
    let query = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
    let mut l = Launcher::new(Config::default());
    println!("{} entries visible with no query", l.result_count());
    l.set_query(&query);
    println!("query {query:?}: {} result(s)", l.result_count());
    for (i, e) in l.results().take(15).enumerate() {
        println!("  {:>2}. {:<44} id={} kind={:?}", i + 1, e.name, e.id, e.kind);
    }
}
