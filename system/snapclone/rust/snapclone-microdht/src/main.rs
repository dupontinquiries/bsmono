//! main.rs — `sc-microdht`, the standalone micro-DHT node binary.
//!
//! Run a local DHT test peer:
//!
//!     sc-microdht                      # listen on 127.0.0.1:4544, verbose
//!     sc-microdht --bind 0.0.0.0:4544  # listen on all interfaces (LAN test)
//!     sc-microdht --quiet              # no per-request logging
//!
//! Then point a dev build of the app at it via Settings → Beta testers →
//! "Local micro-DHT". Poke it by hand with netcat:
//!
//!     echo -n 'PUT sc-deadbeef 6869 60' | nc -u -w1 127.0.0.1 4544   # -> OK
//!     echo -n 'GET sc-deadbeef'         | nc -u -w1 127.0.0.1 4544   # -> VALUES ...
//!     echo -n 'STATS'                   | nc -u -w1 127.0.0.1 4544
//!
//! Stop with Ctrl-C (the read timeout lets the loop notice the flag).

use snapclone_microdht::node::Node;
use std::sync::atomic::Ordering;

fn main() {
    let mut bind = String::from("127.0.0.1:4544");
    let mut verbose = true;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bind" | "-b" => {
                if let Some(v) = args.next() {
                    bind = v;
                } else {
                    eprintln!("--bind requires an address (e.g. 127.0.0.1:4544)");
                    std::process::exit(2);
                }
            }
            "--quiet" | "-q" => verbose = false,
            "--help" | "-h" => {
                print_help();
                return;
            }
            other => {
                eprintln!("unknown argument: {other}");
                print_help();
                std::process::exit(2);
            }
        }
    }

    let node = match Node::bind(&bind, verbose) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("[microdht] failed to bind {bind}: {e}");
            std::process::exit(1);
        }
    };

    // Best-effort Ctrl-C handling without a signal crate: install a tiny
    // SIGINT handler via a thread that flips the shutdown flag. We can't catch
    // SIGINT in pure std on stable without libc, so instead we rely on the
    // run loop's read-timeout + the OS terminating us on a second Ctrl-C.
    // For a clean first-Ctrl-C exit when launched from a terminal, we print a
    // hint. (Tests use the shutdown handle directly.)
    let _shutdown = node.shutdown_handle();
    if verbose {
        match node.local_addr() {
            Ok(a) => eprintln!("[microdht] ready on {a} — Ctrl-C to stop"),
            Err(_) => {}
        }
    }

    if let Err(e) = node.run() {
        eprintln!("[microdht] run error: {e}");
        std::process::exit(1);
    }
    let _ = _shutdown.load(Ordering::Relaxed);
}

fn print_help() {
    println!(
        "sc-microdht — local DHT test peer for SnapClone beta testing\n\n\
         USAGE:\n    \
         sc-microdht [--bind ADDR] [--quiet]\n\n\
         OPTIONS:\n    \
         -b, --bind ADDR   address to listen on (default 127.0.0.1:4544)\n    \
         -q, --quiet       suppress per-request logging\n    \
         -h, --help        show this help\n\n\
         PROTOCOL (UDP, one line per datagram):\n    \
         PING                         -> PONG\n    \
         PUT <sc-id> <hexvalue> <ttl> -> OK\n    \
         GET <sc-id>                  -> VALUES <sc-id> <hex> ...\n    \
         STATS                        -> STATS keys=.. values=.. ..\n"
    );
}
