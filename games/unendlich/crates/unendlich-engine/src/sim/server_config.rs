//! `server_config` — parses the headless server's command-line arguments into a validated config.
//! Pure (no I/O) so it is fully testable; the `unendlich-server` bin calls [`ServerConfig::parse`]
//! on `std::env::args()` and then stands up the world. Kept deliberately small and explicit (no
//! arg-parsing crate) so the deployable surface is obvious for cloud configs.

#[derive(Debug, Clone, PartialEq)]
pub struct ServerConfig {
    /// World name to host.
    pub name: String,
    /// World seed.
    pub seed: u64,
    /// UDP/TCP port for the host to bind.
    pub port: u16,
    /// Simulation ticks per second.
    pub tick_hz: u32,
    /// Chunk view radius (in chunks) the server keeps resident around active players.
    pub view_distance: u32,
    /// Optional bind address (default 0.0.0.0 for cloud).
    pub bind: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            name: "world".to_string(),
            seed: 0,
            port: 9877,
            tick_hz: 20,
            view_distance: 8,
            bind: "0.0.0.0".to_string(),
        }
    }
}

/// Parse result: either a config, a request to print help, or an error message.
#[derive(Debug, Clone, PartialEq)]
pub enum ParseOutcome {
    Config(ServerConfig),
    Help,
    Error(String),
}

pub const HELP: &str = "\
unendlich-server — headless world host

USAGE:
  unendlich-server [OPTIONS]

OPTIONS:
  --name <STR>            world name (default: world)
  --seed <U64>           world seed (default: 0)
  --port <U16>           bind port (default: 9877)
  --bind <ADDR>          bind address (default: 0.0.0.0)
  --tick-hz <U32>        simulation ticks per second (default: 20)
  --view-distance <U32>  resident chunk radius (default: 8)
  -h, --help             print this help
";

impl ServerConfig {
    /// Parse args (excluding argv[0]). Recognizes `--key value` and `--key=value`.
    pub fn parse<I, S>(args: I) -> ParseOutcome
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut cfg = ServerConfig::default();
        let mut it = args.into_iter().map(|s| s.as_ref().to_string()).peekable();
        while let Some(arg) = it.next() {
            // support --key=value
            let (key, inline_val) = match arg.split_once('=') {
                Some((k, v)) => (k.to_string(), Some(v.to_string())),
                None => (arg.clone(), None),
            };
            // value getter: inline first, else the next arg
            let mut take_val = |key: &str| -> Result<String, String> {
                if let Some(v) = inline_val.clone() {
                    return Ok(v);
                }
                it.next().ok_or_else(|| format!("missing value for {key}"))
            };
            match key.as_str() {
                "-h" | "--help" => return ParseOutcome::Help,
                "--name" => match take_val("--name") {
                    Ok(v) => cfg.name = v,
                    Err(e) => return ParseOutcome::Error(e),
                },
                "--seed" => match take_val("--seed").and_then(parse_u64) {
                    Ok(v) => cfg.seed = v,
                    Err(e) => return ParseOutcome::Error(e),
                },
                "--port" => match take_val("--port").and_then(parse_u16) {
                    Ok(v) => cfg.port = v,
                    Err(e) => return ParseOutcome::Error(e),
                },
                "--bind" => match take_val("--bind") {
                    Ok(v) => cfg.bind = v,
                    Err(e) => return ParseOutcome::Error(e),
                },
                "--tick-hz" => match take_val("--tick-hz").and_then(parse_u32_min1) {
                    Ok(v) => cfg.tick_hz = v,
                    Err(e) => return ParseOutcome::Error(e),
                },
                "--view-distance" => match take_val("--view-distance").and_then(parse_u32_min1) {
                    Ok(v) => cfg.view_distance = v,
                    Err(e) => return ParseOutcome::Error(e),
                },
                other => return ParseOutcome::Error(format!("unknown argument: {other}")),
            }
        }
        ParseOutcome::Config(cfg)
    }
}

fn parse_u64(s: String) -> Result<u64, String> {
    s.parse::<u64>().map_err(|_| format!("invalid u64: {s}"))
}
fn parse_u16(s: String) -> Result<u16, String> {
    s.parse::<u16>().map_err(|_| format!("invalid port: {s}"))
}
fn parse_u32_min1(s: String) -> Result<u32, String> {
    let v = s.parse::<u32>().map_err(|_| format!("invalid u32: {s}"))?;
    if v == 0 {
        return Err("value must be >= 1".to_string());
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> ParseOutcome {
        ServerConfig::parse(args.iter().copied())
    }

    #[test]
    fn defaults_when_empty() {
        match parse(&[]) {
            ParseOutcome::Config(c) => assert_eq!(c, ServerConfig::default()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parses_space_form() {
        match parse(&["--name", "atlas", "--seed", "42", "--port", "5000"]) {
            ParseOutcome::Config(c) => {
                assert_eq!(c.name, "atlas");
                assert_eq!(c.seed, 42);
                assert_eq!(c.port, 5000);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parses_equals_form() {
        match parse(&["--seed=99", "--tick-hz=30", "--bind=127.0.0.1"]) {
            ParseOutcome::Config(c) => {
                assert_eq!(c.seed, 99);
                assert_eq!(c.tick_hz, 30);
                assert_eq!(c.bind, "127.0.0.1");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn help_flag() {
        assert_eq!(parse(&["--help"]), ParseOutcome::Help);
        assert_eq!(parse(&["-h"]), ParseOutcome::Help);
    }

    #[test]
    fn unknown_arg_errors() {
        match parse(&["--frobnicate"]) {
            ParseOutcome::Error(e) => assert!(e.contains("unknown")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn invalid_number_errors() {
        match parse(&["--port", "notaport"]) {
            ParseOutcome::Error(e) => assert!(e.contains("invalid")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn missing_value_errors() {
        match parse(&["--seed"]) {
            ParseOutcome::Error(e) => assert!(e.contains("missing value")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn zero_tick_hz_rejected() {
        match parse(&["--tick-hz", "0"]) {
            ParseOutcome::Error(e) => assert!(e.contains(">= 1")),
            other => panic!("{other:?}"),
        }
    }
}
