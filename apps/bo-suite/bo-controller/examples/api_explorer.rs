// examples/api_explorer.rs
//
// Standalone probe tool for a Bang & Olufsen device's HTTP API. Hits a
// list of candidate ASE/BeoZone/Mozart endpoints and reports which ones
// respond, with status codes and a short body preview. Useful for
// mapping exactly what your specific product/firmware exposes before
// wiring a feature into the main app.
//
// Run:
//   cargo run --example api_explorer
//   cargo run --example api_explorer -- http://192.168.2.12
//   cargo run --example api_explorer -- http://192.168.2.12 8080
//
// Args:
//   [1] base URL   (default http://192.168.2.12)
//   [2] ASE port   (default none; appended as :PORT if given)

use reqwest::Client;
use std::time::Duration;

#[derive(Clone, Copy)]
enum Method {
    Get,
}

struct Probe {
    method: Method,
    path: &'static str,
    note: &'static str,
}

const PROBES: &[Probe] = &[
    // Device / identity
    Probe { method: Method::Get, path: "/BeoDevice",                                   note: "device identity + firmware" },
    Probe { method: Method::Get, path: "/api/getData?path=BeoSound%3A%2Fvolume&roles=value", note: "ASE volume (getData)" },

    // Zone / sound
    Probe { method: Method::Get, path: "/BeoZone/Zone/Sound/Volume/Speaker/Level",     note: "Mozart volume level" },
    Probe { method: Method::Get, path: "/BeoZone/Zone/Sound/Volume/Speaker/Muted",     note: "Mozart mute" },
    Probe { method: Method::Get, path: "/BeoZone/Zone/Sound/Mode",                     note: "active sound mode" },
    Probe { method: Method::Get, path: "/BeoZone/Zone/Sound/Mode/List",                note: "sound mode list" },
    Probe { method: Method::Get, path: "/BeoZone/Zone/Sound/Tone",                     note: "bass/treble tone" },
    Probe { method: Method::Get, path: "/BeoZone/Zone/Sound/Equalizer",                note: "per-band EQ" },

    // Sources / playback
    Probe { method: Method::Get, path: "/BeoZone/Zone/Sources",                        note: "source list" },
    Probe { method: Method::Get, path: "/BeoZone/Zone/ActiveSources",                  note: "active source" },
    Probe { method: Method::Get, path: "/BeoZone/Zone/Music",                          note: "now-playing metadata" },
    Probe { method: Method::Get, path: "/BeoZone/Zone/Sound/PlayQueue",                note: "play queue" },

    // Power / timers
    Probe { method: Method::Get, path: "/BeoDevice/powerManagement/standby",           note: "standby state" },
    Probe { method: Method::Get, path: "/BeoDevice/timer",                             note: "sleep timer" },

    // Notifications (long-poll)
    Probe { method: Method::Get, path: "/BeoNotify/Notifications",                     note: "event stream (will hang — Ctrl-C)" },

    // Beolink
    Probe { method: Method::Get, path: "/BeoZone/Zone/Sources/peers",                  note: "Beolink peers" },
];

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let base = args.get(1).cloned().unwrap_or_else(|| "http://192.168.2.12".to_string());
    let base = match args.get(2) {
        Some(port) => format!("{}:{}", base, port),
        None => base,
    };

    println!("Probing {} ...\n", base);

    let client = Client::builder()
        .timeout(Duration::from_secs(4))
        .build()
        .expect("client build");

    for probe in PROBES {
        // Skip the long-poll notifications endpoint by default — it never
        // returns. Uncomment to test it (Ctrl-C to stop).
        if probe.path.contains("/BeoNotify/") {
            println!("  [skip] {:<48} {}", probe.path, "(long-poll; edit example to test)");
            continue;
        }

        let url = format!("{}{}", base, probe.path);
        let result = match probe.method {
            Method::Get => client.get(&url).send().await,
        };

        match result {
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                let preview = body.chars().take(80).collect::<String>().replace('\n', " ");
                let whitelisted = if body.contains("not whitelisted") { " [NOT WHITELISTED]" } else { "" };
                let mark = if status.is_success() && whitelisted.is_empty() { "✓" } else { "·" };
                println!("  {} {:<48} {} {}{}", mark, probe.path, status.as_u16(), preview, whitelisted);
                let _ = probe.note;
            }
            Err(e) => {
                let kind = if e.is_timeout() { "timeout" } else { "error" };
                println!("  ✗ {:<48} {} ({})", probe.path, kind, probe.note);
            }
        }
    }

    println!("\nLegend: ✓ supported   · responded but unusable / not whitelisted   ✗ no response");
}
