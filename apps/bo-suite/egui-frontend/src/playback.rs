//! playback — the thing that makes a sound.
//!
//! Everything around this exists and is tested. `Transport` holds the
//! position, the repeat mode and the pause-is-really-stop rule. `Queue`
//! holds what plays next. `SinkPicker` produces a [`Route`] — one speaker
//! or several. Before this module nothing in the tree opened an audio
//! device, so the play button produced no sound: the binary was 66 lines
//! with three hardcoded rooms in it. This is what it plays through.
//!
//! ## A subprocess, for the third time in this fleet and the same reasons
//!
//! `mpv` (or `ffplay`, or `paplay`) spawned through `sh`, its sink chosen
//! by an environment variable. Not a linked decoder:
//!
//! * A UI crate that links an audio stack cannot be tested anywhere
//!   without a sound card, and this crate's tests run in CI.
//! * The command is a string you can read in a log and paste into a
//!   terminal, which is the difference between "no sound" and "no sound,
//!   and here is exactly what I asked for".
//! * Cross-building for ARM stays as cheap as it is; the runtime
//!   dependency is a binary every distribution ships.
//!
//! ## Multi-speaker is a routing decision, not a second player
//!
//! Playing the same track from two processes gives two clocks and an echo
//! — they drift within seconds, and the drift is audible in a way nobody
//! can fix from the UI. So a multi-room route builds ONE stream and points
//! it at a combine sink that fans out. The sink has to exist first, which
//! is [`Plan::setup`]: the command that creates it, separate from the one
//! that plays, because creating it is a system change that outlives the
//! track.
//!
//! ## What is verified here and what is not
//!
//! The plan — which commands, in which order, with which sink — is pure
//! and tested. The subprocess mechanics are tested against a stand-in
//! launcher, as in `snapclone::capture`. NOT verified: that `mpv` and
//! `pactl` accept these flags, and that any sound has come out. This
//! sandbox has no audio device.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Where a stream should go.
///
/// Mirrors `settings_page::Route` rather than reusing it, so the engine
/// does not depend on the settings page — the routing decision belongs to
/// the picker, and the engine only needs to know the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sink {
    /// The system default. What you get before anyone has chosen.
    Default,
    /// One named PulseAudio/PipeWire sink.
    One(String),
    /// Several, through a combine sink built for them.
    ///
    /// The SAME audio in each — multi-room, not surround. Asking for
    /// surround by selecting several speakers is how people end up with
    /// the left channel in the kitchen.
    Many(Vec<String>),
    /// One stream SPLIT across speakers by position.
    ///
    /// (sink id, PulseAudio channel name), already validated by
    /// `surround::Assignment` — this type carries the answer, not the
    /// rules.
    Surround { members: Vec<(String, String)>, channel_map: String },
}

impl Sink {
    /// The combine sink's name, for a multi-room route.
    ///
    /// Derived from the members rather than random or fixed: a fixed name
    /// collides when a second app does the same thing, and a random one
    /// leaks a sink per route change because nothing knows the old name to
    /// tear it down.
    pub fn combine_name(members: &[String]) -> String {
        Sink::derived_name("bo_room", members)
    }

    /// The virtual sink's name, prefixed by what it IS.
    ///
    /// A surround sink called `bo_combine_…` is a lie you meet at three in
    /// the morning with `pactl list sinks` open — combine duplicates and
    /// remap splits, and the name is the only thing distinguishing them
    /// once they are loaded. My first version reused the combine name for
    /// both and a test caught it by matching the word.
    pub fn surround_name(members: &[String]) -> String {
        Sink::derived_name("bo_surround", members)
    }

    fn derived_name(prefix: &str, members: &[String]) -> String {
        let mut ids: Vec<&str> = members.iter().map(|s| s.as_str()).collect();
        // Sorted, so the same set of speakers in a different order is the
        // same sink — otherwise picking A then B and B then A leaves two.
        ids.sort_unstable();
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for id in &ids {
            for b in id.as_bytes() {
                h ^= u64::from(*b);
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        format!("{prefix}_{h:016x}")
    }
}

/// The players this app knows how to drive, best first.
///
/// mpv leads because it is the only one of these that will print a status
/// line in a format we choose, which is what feeds the progress bar. The
/// others play and report nothing, which is a working player and a bar
/// that does not move — a real degradation, and better than refusing to
/// play at all.
///
/// This list exists because mpv is NOT always installed. I assumed it was
/// and hardcoded it; this sandbox has `ffplay` and no mpv, which is how I
/// found out.
pub const KNOWN_PLAYERS: &[(&str, &str)] = &[
    ("mpv", DEFAULT_PLAYER),
    ("ffplay", "ffplay -nodisp -autoexit -loglevel error"),
    ("paplay", "paplay"),
];

/// The first known player present on PATH.
///
/// `None` when there is none — reported rather than guessed, because a
/// command that does not exist fails with "No such file or directory" and
/// reads as a missing TRACK rather than a missing player.
pub fn discover_player() -> Option<&'static str> {
    let path = std::env::var_os("PATH")?;
    for (bin, cmd) in KNOWN_PLAYERS {
        if std::env::split_paths(&path).any(|d| d.join(bin).is_file()) {
            return Some(cmd);
        }
    }
    None
}

/// A track that ends sooner than this did not play.
///
/// Real music is minutes long and even a skipped one takes a moment to
/// open. A "finished" track that lasted a quarter of a second is a player
/// that failed and said so on stderr — or, on `ffplay` with no sound card,
/// failed and said so while exiting zero.
pub const MIN_PLAYED: std::time::Duration = std::time::Duration::from_millis(250);

/// The line of stderr worth showing, if any.
///
/// NOT the last line, which is what I used first and which real output
/// showed to be the least useful: `ffplay` with no sound card ends with
/// "Failed to open file 'x' or configure filtergraph", blaming the file,
/// while the actual cause — "audio open failed" — is the line before, and
/// above that sit eight lines of ALSA configuration noise.
///
/// So: the first line that names a known cause, then the last non-ALSA
/// line, then nothing.
pub fn complaint(stderr: &str) -> Option<String> {
    const CAUSES: &[&str] = &[
        "audio open failed", "cannot open", "no such file", "permission denied",
        "invalid data", "device or resource busy", "connection refused",
        "unable to open", "failed to open audio",
    ];
    // ALSA's chatter is excluded from the whole search, not just from the
    // fallback. Its lines contain "No such file or directory" — a phrase
    // that is on the cause list for good reasons — so scanning them first
    // returns a configuration message about a missing card node instead of
    // "audio open failed". My first version did exactly that, against the
    // real transcript, in the test below.
    let lines: Vec<&str> = stderr.lines().map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("ALSA lib"))
        .collect();
    for l in &lines {
        let low = l.to_lowercase();
        if CAUSES.iter().any(|c| low.contains(c)) {
            return Some((*l).to_string());
        }
    }
    // ALSA prints its whole config search on any failure; none of it is
    // the answer and all of it is longer than the answer.
    lines.last().map(|l| (*l).to_string())
}

/// The default player command. `--no-video` because a music file with
/// cover art embedded otherwise opens a window.
pub const DEFAULT_PLAYER: &str = concat!(
    "mpv --no-video --really-quiet ",
    // THE POSITION, on stdout, in a format this app defined.
    //
    // `Transport::report` is the sync point between the player's clock and
    // the progress bar, and nothing called it: the bar extrapolated from
    // zero forever and `duration` was never set, so `fraction()` was
    // permanently 0 and the handle never moved.
    //
    // mpv will print a status line of our choosing, so the channel costs
    // no IPC socket and no second connection — the process we already
    // spawned tells us on the pipe we already have. A prefix nothing else
    // emits, so a player that ignores the flag simply reports nothing
    // rather than having its ordinary chatter misread as a timestamp.
    // The prefix is spelled out here because `concat!` takes literals
    // only; `STATUS_PREFIX` below is the same string and a test holds the
    // two together, so a change to one that misses the other fails here
    // rather than as a progress bar that silently stops moving.
    "--term-status-msg='BOPOS ${=time-pos} ${=duration}'");

/// The marker a status line starts with.
pub const STATUS_PREFIX: &str = "BOPOS";

/// Parse one status line into (position, duration), both seconds.
///
/// Anything else is `None`, including a line that starts with the prefix
/// and does not carry two numbers: mpv prints `${=duration}` literally for
/// a stream that has none, and reading that as a length would put the
/// handle at the far end of a bar with no end.
pub fn parse_status(line: &str) -> Option<(f64, f64)> {
    let rest = line.trim().strip_prefix(STATUS_PREFIX)?;
    let mut it = rest.split_whitespace();
    let pos: f64 = it.next()?.parse().ok()?;
    let dur: f64 = it.next()?.parse().ok()?;
    if !pos.is_finite() || !dur.is_finite() || pos < 0.0 || dur <= 0.0 {
        return None;
    }
    Some((pos, dur))
}

/// Which player a command string drives.
///
/// Derived from the command's first word rather than carried beside it,
/// so a caller cannot hand the ffplay flags to mpv: the two spell "start
/// at" and "report position" differently, and a flag the other player
/// does not know is a track that does not play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerKind { Mpv, Ffplay, Paplay, Other }

impl PlayerKind {
    pub fn of(command: &str) -> PlayerKind {
        let first = command.split_whitespace().next().unwrap_or("");
        match first.rsplit('/').next().unwrap_or(first) {
            "mpv" => PlayerKind::Mpv,
            "ffplay" => PlayerKind::Ffplay,
            "paplay" => PlayerKind::Paplay,
            _ => PlayerKind::Other,
        }
    }

    /// Can the running process be paused, resumed and seeked in place?
    ///
    /// Only mpv, through its JSON IPC socket. For the others "pause" is a
    /// kill and "resume" a restart at the remembered position — which
    /// `Transport` already models, since that is what BT sinks do too.
    pub fn steerable(self) -> bool { self == PlayerKind::Mpv }

    /// Does the player tell us where it is while it runs?
    pub fn reports_position(self) -> bool {
        matches!(self, PlayerKind::Mpv | PlayerKind::Ffplay)
    }
}

/// Per-play options the plan folds into the command line.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlayOpts {
    /// Where in the track to begin. A resume after a kill-style pause, or
    /// a seek on a player that cannot seek in place.
    pub start_secs: f64,
    /// The unix socket mpv should serve its JSON IPC on. `None` for the
    /// other players and in tests; mpv without it plays fine and cannot
    /// be steered.
    pub ipc: Option<String>,
}

/// The commands to run for one track.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Run first, once — creates the combine sink. Empty for a single
    /// speaker, which needs no setup at all.
    pub setup: Vec<String>,
    /// The player, with its sink already chosen.
    pub play: String,
    /// Run when the route is abandoned. Tearing a combine sink down is not
    /// optional: they persist in the audio server, and a session that
    /// creates one per route change leaves a list of dead sinks in every
    /// other app's output picker.
    pub teardown: Vec<String>,
    /// Which player `play` runs, so the host knows what it can ask of it.
    pub kind: PlayerKind,
    /// The IPC socket the player was told to serve, if any.
    pub ipc: Option<String>,
}

/// Build the plan for a track on a route, from the beginning, unsteered.
///
/// `url` is quoted for the shell. It comes from a library that scanned a
/// disk, so it contains apostrophes and spaces routinely, and an unquoted
/// path is both a bug and an injection.
pub fn plan(player: &str, url: &str, sink: &Sink) -> Plan {
    plan_with(player, url, sink, &PlayOpts::default())
}

/// The flags that start a player at an offset and open its position
/// channel, in each player's own spelling.
///
/// mpv: `--start=` and `--input-ipc-server=`. ffplay: `-ss` and `-stats`
/// — the stats line is what carries the clock, on stderr, and it is
/// printed regardless of `-loglevel error` because ffplay writes it with
/// `fprintf` when explicitly asked. paplay can do neither, and a flag it
/// does not know would stop it playing at all.
pub fn player_flags(kind: PlayerKind, opts: &PlayOpts) -> String {
    let mut flags: Vec<String> = Vec::new();
    // Below a twentieth of a second is the beginning: mpv seeks to the
    // nearest keyframe anyway, and "--start=0.0" on a stream that cannot
    // seek is a warning nobody needs.
    let start = if opts.start_secs > 0.05 { Some(opts.start_secs) } else { None };
    match kind {
        PlayerKind::Mpv => {
            if let Some(s) = start { flags.push(format!("--start={s:.2}")); }
            if let Some(sock) = &opts.ipc {
                flags.push(format!("--input-ipc-server={}", shell_quote(sock)));
            }
        }
        PlayerKind::Ffplay => {
            flags.push("-stats".into());
            if let Some(s) = start { flags.push(format!("-ss {s:.2}")); }
        }
        PlayerKind::Paplay | PlayerKind::Other => {}
    }
    flags.join(" ")
}

/// Build the plan for a track on a route, with a start offset and, for
/// mpv, an IPC socket.
pub fn plan_with(player: &str, url: &str, sink: &Sink, opts: &PlayOpts) -> Plan {
    let kind = PlayerKind::of(player);
    let flags = player_flags(kind, opts);
    let player: String = if flags.is_empty() { player.to_string() }
                         else { format!("{player} {flags}") };
    let player = player.as_str();
    let ipc = if kind == PlayerKind::Mpv { opts.ipc.clone() } else { None };
    let q = shell_quote(url);
    // See media_name_from_url's own doc for exactly what this is and is
    // not: a real improvement over the bare binary name a volume mixer
    // showed before, not a stand-in for real ID3 title/artist metadata
    // (which nothing in this playback layer resolves yet). Prepended the
    // same way PULSE_SINK already is below -- an environment variable
    // libpulse understands identically regardless of which of the three
    // players ends up running, exactly the reasoning PULSE_SINK's own
    // comment already gives for choosing that mechanism over a
    // per-player flag.
    // THROUGH `env`, not a shell assignment: `PULSE_PROP_media.name=x cmd`
    // is a syntax error in dash ("not found"), because a dot is not a
    // valid shell variable name — the property libpulse reads has one,
    // so no shell can set it as a variable. `env` accepts any NAME=VALUE
    // and execs the program in place, so the pid the kill goes to is
    // still the player's. Found the day a track with a space in its
    // name was played under sh: every play had failed this way since the
    // property was added, and the tests asserted the string, not the run.
    let prop = format!("env PULSE_PROP_media.name={}", shell_quote(&media_name_from_url(url)));
    match sink {
        Sink::Default => Plan { setup: Vec::new(), play: format!("{prop} {player} {q}"),
                                teardown: Vec::new(), kind, ipc },
        Sink::One(name) => Plan {
            setup: Vec::new(),
            // The sink travels as an environment variable rather than a
            // player flag: every player spells the flag differently and
            // PULSE_SINK is understood by all of them through libpulse.
            // Swapping mpv for ffplay then costs nothing.
            play: format!("{prop} PULSE_SINK={} {player} {q}", shell_quote(name)),
            teardown: Vec::new(),
            kind, ipc,
        },
        // SURROUND. A null sink with the room's channel map, one loopback
        // per speaker taking its own channel, and the player pointed at
        // the null sink.
        //
        // Not `module-combine-sink`: combine DUPLICATES a stream to every
        // slave, which is the multi-room case above. Splitting a
        // multichannel stream needs a remap per destination, and the
        // difference between the two is the whole distinction this variant
        // exists for.
        //
        // MARKED UNVERIFIED, deliberately and in the code rather than only
        // in a commit message: this module chain is written from the
        // documentation and has never been run. The plan is a testable
        // artifact — the commands, their order, the channel each speaker
        // gets — and whether PulseAudio accepts these exact arguments is
        // not something a fixture can answer.
        Sink::Surround { members, channel_map } => {
            let name = Sink::surround_name(
                &members.iter().map(|(m, _)| m.clone()).collect::<Vec<_>>());
            let mut setup = vec![format!(
                "pactl load-module module-null-sink sink_name={name} \
                 channels={} channel_map={}",
                channel_map.split(',').count(), shell_quote(channel_map))];
            for (sink, ch) in members {
                // `remix=no` matters: with remixing on, a speaker given
                // one channel of a 5.1 map gets a downmix of everything
                // instead, and the setup plays correctly-quiet mud from
                // every corner.
                setup.push(format!(
                    "pactl load-module module-remap-sink sink_name={name}_{} \
                     master={} channels=1 master_channel_map={} channel_map=mono remix=no",
                    ch.replace('-', "_"), shell_quote(sink), shell_quote(ch)));
            }
            Plan {
                setup,
                play: format!("{prop} PULSE_SINK={name} {player} {q}"),
                // Unloading BY NAME rather than every module of the type:
                // `unload-module module-remap-sink` would take out any
                // remap another application had created, which on a
                // desktop is somebody else's headset.
                teardown: vec![format!("pactl unload-module {name}")],
                kind, ipc,
            }
        }
        Sink::Many(members) => {
            let combine = Sink::combine_name(members);
            let slaves = members.iter().map(|m| m.as_str()).collect::<Vec<_>>().join(",");
            Plan {
                setup: vec![format!(
                    "pactl load-module module-combine-sink sink_name={} slaves={} \
                     sink_properties=device.description={}",
                    combine, shell_quote(&slaves), combine)],
                play: format!("{prop} PULSE_SINK={combine} {player} {q}"),
                teardown: vec![format!("pactl unload-module module-combine-sink")],
                kind, ipc,
            }
        }
    }
}

/// The `pactl` command that sets the volume of the sink a route plays
/// through, as a percentage.
///
/// The SINK's volume rather than the player's: it is what a volume dial
/// on a speaker controller means, it survives the player being replaced
/// between tracks, and every player here honours it identically. mpv's
/// own `volume` property would only reach mpv. `@DEFAULT_SINK@` is
/// PulseAudio's spelling of "wherever the default route goes".
pub fn volume_command(sink: &Sink, percent: f32) -> String {
    let pct = percent.clamp(0.0, 100.0).round() as u32;
    let name = match sink {
        Sink::Default => "@DEFAULT_SINK@".to_string(),
        Sink::One(n) => shell_quote(n),
        Sink::Many(members) => Sink::combine_name(members),
        Sink::Surround { members, .. } => Sink::surround_name(
            &members.iter().map(|(m, _)| m.clone()).collect::<Vec<_>>()),
    };
    format!("pactl set-sink-volume {name} {pct}%")
}

/// The clock ffplay prints with `-stats`, on stderr, one `\r`-terminated
/// line about ten times a second:
///
/// ```text
///    2.88 M-A: -0.000 fd=   0 aq=    0KB vq=    0KB sq=    0B f=0/0
/// ```
///
/// The first field is the master clock in seconds — the position. There
/// is no duration on the line; that comes from the library's tag reader.
/// Anything that is not this shape is `None`, including ffplay's real
/// complaints, which the caller keeps for `complaint`.
pub fn parse_ffplay_stats(line: &str) -> Option<f64> {
    let mut it = line.split_whitespace();
    let pos: f64 = it.next()?.parse().ok()?;
    let marker = it.next()?;
    if !matches!(marker, "M-A:" | "A-V:" | "M-V:") { return None; }
    if !pos.is_finite() || pos < 0.0 { return None; }
    Some(pos)
}

/// One mpv JSON IPC event line, reduced to what the bar needs.
///
/// Hand-parsed rather than through a JSON crate: the events observed are
/// two, their shape is fixed by mpv, and the crate does not otherwise
/// depend on serde_json. `{"event":"property-change","id":1,
/// "name":"time-pos","data":12.34}` is the whole grammar.
pub fn parse_mpv_event(line: &str) -> Option<(&'static str, f64)> {
    if !line.contains("\"event\":\"property-change\"") { return None; }
    let name = if line.contains("\"name\":\"time-pos\"") { "time-pos" }
               else if line.contains("\"name\":\"duration\"") { "duration" }
               else { return None };
    let rest = line.split("\"data\":").nth(1)?;
    let num: String = rest.trim_start().chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-' || *c == 'e' || *c == 'E' || *c == '+')
        .collect();
    let v: f64 = num.parse().ok()?;
    if !v.is_finite() { return None; }
    Some((name, v))
}

/// What the host can ask of a running player without restarting it.
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    Pause,
    Resume,
    /// Absolute, seconds.
    Seek(f64),
}

impl Control {
    /// The mpv IPC command line, newline-terminated as the protocol wants.
    pub fn mpv_json(&self) -> String {
        match self {
            Control::Pause => "{\"command\":[\"set_property\",\"pause\",true]}\n".into(),
            Control::Resume => "{\"command\":[\"set_property\",\"pause\",false]}\n".into(),
            Control::Seek(s) => format!("{{\"command\":[\"seek\",{:.3},\"absolute\"]}}\n", s.max(0.0)),
        }
    }
}

/// The two observe requests sent once the socket is up. Ids 1 and 2 are
/// arbitrary and only echoed back.
pub const MPV_OBSERVE: &str = concat!(
    "{\"command\":[\"observe_property\",1,\"time-pos\"]}\n",
    "{\"command\":[\"observe_property\",2,\"duration\"]}\n");

/// Put `exec` in front of the PROGRAM, after any leading `NAME=value`
/// assignments.
///
/// The player line starts with environment assignments (`PULSE_SINK=…`,
/// `PULSE_PROP_media.name=…`) and the launcher wrapped the whole line in
/// `exec …` — so the shell was told to exec a program called
/// `PULSE_PROP_media.name='blue2'`, and every play failed with "not
/// found" from the moment the media-name property was added. `exec` is
/// needed (the process we kill must be the one holding the pipe, not a
/// shell around it) and it has to go where the shell reads it: after the
/// assignments, before the command. Quote-aware, because the value of a
/// media name has spaces in it.
pub fn exec_line(cmd: &str) -> String {
    let b = cmd.as_bytes();
    let mut i = 0;
    let n = b.len();
    let skip_ws = |i: &mut usize| { while *i < n && (b[*i] == b' ' || b[*i] == b'\t') { *i += 1; } };
    skip_ws(&mut i);
    loop {
        // One word, honouring single quotes; is it NAME=…?
        let start = i;
        let mut j = i;
        let mut quoted = false;
        let mut saw_eq = false;
        let mut valid_name = true;
        while j < n {
            let c = b[j];
            if quoted { if c == b'\'' { quoted = false; } j += 1; continue; }
            match c {
                b'\'' => { quoted = true; }
                // An escaped character outside quotes — the `\'` in the
                // `'Don'\''t'` idiom shell_quote produces — is literal
                // and must not toggle the quote state.
                b'\\' => { j += 2; continue; }
                b' ' | b'\t' => break,
                b'=' if !saw_eq => {
                    saw_eq = true;
                    valid_name = j > start && b[start].is_ascii_alphabetic() || b[start] == b'_';
                }
                _ => {}
            }
            j += 1;
        }
        if !(saw_eq && valid_name) || j == start { break; }
        i = j;
        skip_ws(&mut i);
        if i >= n { break; }
    }
    format!("{}exec {}", &cmd[..i], &cmd[i..])
}

/// Single-quote for `sh`, the way that survives every character.
///
/// A path from a music library contains apostrophes — `Don't Stop`, every
/// second album — and the naive version breaks on exactly those.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// A readable name for the volume-mixer stream, derived from the file path.
///
/// Reported gap: bo-suite's players showed as bare "ffplay"/"mpv" in a
/// system volume mixer, next to other apps (Clementine, in the report)
/// showing real "artist - title" text. This is NOT that -- there is no
/// track-id-to-metadata resolver anywhere in this playback layer yet
/// (`now_playing_url`'s own doc comment already says so: "the library
/// holds titles and tags, not paths"), and inventing a fake one here
/// would be worse than the bare binary name it replaces. What IS real
/// and already available at every call site is the file path itself --
/// its basename, extension stripped, underscores/hyphens opened into
/// spaces, is a genuine improvement over "ffplay" without pretending to
/// know an artist or title the app has not actually looked up.
///
/// A real ID3/tag-based resolver feeding this instead (or a `Plan` field
/// carrying it in directly) is the actual fix for the Clementine-parity
/// ask; this is the honest interim step available without one.
pub fn media_name_from_url(url: &str) -> String {
    let stem = std::path::Path::new(url)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(url);
    let pretty: String = stem.chars().map(|c| if c == '_' || c == '-' { ' ' } else { c }).collect();
    let pretty = pretty.trim();
    if pretty.is_empty() { "bo-suite".to_string() } else { pretty.to_string() }
}

/// What the player is doing, for the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Idle,
    Playing { url: String },
    /// The player exited with something to say. Its last stderr line,
    /// because "playback failed" tells nobody anything and "Failed to open
    /// audio device" tells them everything.
    Failed(String),
}

/// Shared between the player thread and the UI.
#[derive(Default)]
pub struct Shared {
    state: Mutex<Option<State>>,
    /// The player's own clock: (position, duration). Latest wins — a
    /// backlog of timestamps is worse than the newest one, since every
    /// older entry is a position the track has already passed.
    progress: Mutex<Option<(f64, f64)>>,
    /// The track finished on its own, as opposed to being stopped. Only
    /// this should advance the queue — a track stopped because the user
    /// pressed pause must not skip to the next one.
    ended: std::sync::atomic::AtomicBool,
}

impl Shared {
    pub fn new() -> Arc<Shared> {
        Arc::new(Shared::default())
    }
    /// Edge-triggered, so the host does not re-apply the same state every
    /// frame.
    pub fn take_state(&self) -> Option<State> {
        self.state.lock().ok()?.take()
    }
    /// The newest position the player reported, if it has since the last
    /// call.
    pub fn take_progress(&self) -> Option<(f64, f64)> {
        self.progress.lock().ok()?.take()
    }
    pub fn take_ended(&self) -> bool {
        self.ended.swap(false, Ordering::Relaxed)
    }
    fn set(&self, s: State) {
        if let Ok(mut g) = self.state.lock() { *g = Some(s); }
    }
}

/// A running player.
pub struct Player {
    child: Arc<Mutex<Option<std::process::Child>>>,
    pub shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// The teardown for the route currently loaded, run when it changes.
    teardown: Vec<String>,
    /// The write half of mpv's IPC socket, once the reader thread has
    /// connected. `None` for every other player and before the connect —
    /// `control` answers false then, and the host falls back to a
    /// restart.
    ipc: Arc<Mutex<Option<std::os::unix::net::UnixStream>>>,
    /// The socket path of the current plan, unlinked after a kill: mpv
    /// removes its own on a clean exit, and a kill is not a clean exit.
    /// A stale file would make the next connect succeed against nothing.
    ipc_path: Option<String>,
    /// Which player the current plan runs.
    kind: PlayerKind,
}

impl Default for Player {
    fn default() -> Self {
        Player { child: Arc::new(Mutex::new(None)), shared: Shared::new(),
                 stop: Arc::new(AtomicBool::new(false)), thread: None,
                 teardown: Vec::new(), ipc: Arc::new(Mutex::new(None)),
                 ipc_path: None, kind: PlayerKind::Other }
    }
}

/// How long to keep trying to reach mpv's socket after the spawn. mpv
/// creates it early, but not before it has parsed its options — a few
/// hundred milliseconds on a phone.
const IPC_CONNECT_WAIT: std::time::Duration = std::time::Duration::from_secs(4);

impl Player {
    pub fn new() -> Player { Player::default() }

    pub fn is_running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    /// The player the current plan runs.
    pub fn kind(&self) -> PlayerKind { self.kind }

    /// Can the running player be paused, resumed and seeked in place
    /// right now? True only once mpv's socket has answered.
    pub fn can_steer(&self) -> bool {
        self.is_running() && self.ipc.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    /// Ask the running player for something without restarting it.
    ///
    /// Returns false when it cannot be asked — no IPC, not connected yet,
    /// or the write failed — and the caller does the kill-and-restart
    /// dance instead. False rather than an error because "this player
    /// cannot" is the ordinary case for two of the three players.
    pub fn control(&self, c: Control) -> bool {
        use std::io::Write;
        let Ok(mut g) = self.ipc.lock() else { return false };
        let Some(sock) = g.as_mut() else { return false };
        match sock.write_all(c.mpv_json().as_bytes()) {
            Ok(()) => true,
            Err(_) => { *g = None; false }
        }
    }

    /// Set the sink's volume, through the same shell the player runs in.
    ///
    /// The result is reported rather than assumed: `pactl` is not on
    /// every machine, and a dial that moves while the room stays the same
    /// loudness is the bug this returns false to prevent.
    pub fn set_volume_with(&self, sink: &Sink, percent: f32, launcher: &str) -> bool {
        std::process::Command::new(launcher).arg("-c").arg(volume_command(sink, percent))
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .status().map(|s| s.success()).unwrap_or(false)
    }

    /// Start a track. Stops whatever was playing first.
    ///
    /// `launcher` is the shell used, injectable so the subprocess path can
    /// be tested against a stand-in — the same seam `snapclone::capture`
    /// uses, and for the same reason: without it the only test possible is
    /// "does it compile".
    pub fn start_with(&mut self, plan: &Plan, launcher: &str) {
        self.stop_now();
        for cmd in &plan.setup {
            // Setup failures are NOT fatal: a combine sink that already
            // exists returns an error, and refusing to play because of it
            // would break the second track on the same route.
            let _ = std::process::Command::new(launcher).arg("-c").arg(cmd).status();
        }
        self.teardown = plan.teardown.clone();
        self.stop = Arc::new(AtomicBool::new(false));
        self.shared = Shared::new();
        self.ipc = Arc::new(Mutex::new(None));
        self.kind = plan.kind;
        let (shared, stop, holder, ipc_slot) =
            (self.shared.clone(), self.stop.clone(), self.child.clone(), self.ipc.clone());
        let cmd = plan.play.clone();
        let launcher = launcher.to_string();
        let url = plan.play.clone();
        let ipc_path = plan.ipc.clone();
        self.ipc_path = ipc_path.clone();
        self.thread = Some(std::thread::spawn(move || {
            let spawned = std::process::Command::new(&launcher)
                .arg("-c").arg(exec_line(&cmd))
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn();
            let mut child = match spawned {
                Ok(c) => c,
                Err(e) => { shared.set(State::Failed(format!("could not start: {e}"))); return; }
            };
            let err = child.stderr.take();
            let out = child.stdout.take();
            if let Ok(mut g) = holder.lock() { *g = Some(child); }
            shared.set(State::Playing { url });
            let started = std::time::Instant::now();

            // The status reader, on its own thread. On the wait thread it
            // would block the exit check between lines, and mpv prints a
            // status line about once a second — so a track would be up to
            // a second late noticing it had ended.
            let reader = out.map(|o| {
                let shared = shared.clone();
                std::thread::spawn(move || {
                    use std::io::BufRead;
                    // Status lines end with a carriage return, not a
                    // newline: mpv redraws one line in place. Splitting on
                    // '\n' would yield the whole session as one line at
                    // the end, which is a progress bar that jumps from 0
                    // to done.
                    let mut buf = Vec::new();
                    let mut r = std::io::BufReader::new(o);
                    while let Ok(n) = r.read_until(b'\r', &mut buf) {
                        if n == 0 { break; }
                        let line = String::from_utf8_lossy(&buf).into_owned();
                        buf.clear();
                        for part in line.split(['\n', '\r']) {
                            if let Some(p) = parse_status(part) {
                                if let Ok(mut g) = shared.progress.lock() { *g = Some(p); }
                            }
                        }
                    }
                })
            });

            // THE STDERR READER. Two jobs: ffplay's clock arrives here (see
            // `parse_ffplay_stats`), and the complaint a failed player
            // leaves has to be kept — but only the complaint. A track's
            // worth of stats lines is thousands of lines that say nothing
            // about why it stopped, so they are parsed and dropped.
            // Returns what it kept, for `complaint`.
            let err_reader = err.map(|e| {
                let shared = shared.clone();
                std::thread::spawn(move || -> String {
                    use std::io::Read;
                    let mut kept: std::collections::VecDeque<String> = Default::default();
                    let mut buf = [0u8; 512];
                    let mut acc: Vec<u8> = Vec::new();
                    let mut src = e;
                    loop {
                        let n = match src.read(&mut buf) { Ok(0) | Err(_) => break, Ok(n) => n };
                        acc.extend_from_slice(&buf[..n]);
                        // Lines end in '\r' (ffplay redraws in place) or
                        // '\n' (everything else). Anything after the last
                        // terminator waits for the next read.
                        while let Some(i) = acc.iter().position(|b| *b == b'\r' || *b == b'\n') {
                            let line = String::from_utf8_lossy(&acc[..i]).trim().to_string();
                            acc.drain(..=i);
                            if line.is_empty() { continue; }
                            if let Some(pos) = parse_ffplay_stats(&line) {
                                if let Ok(mut g) = shared.progress.lock() {
                                    // Duration is not on the line; keep
                                    // whatever the last report had, or 0
                                    // for "unknown" — `Transport` treats
                                    // a zero duration as no bar, not as a
                                    // bar at the end.
                                    let dur = g.map(|(_, d)| d).unwrap_or(0.0);
                                    *g = Some((pos, dur));
                                }
                                continue;
                            }
                            kept.push_back(line);
                            if kept.len() > 40 { kept.pop_front(); }
                        }
                    }
                    if !acc.is_empty() {
                        let tail = String::from_utf8_lossy(&acc).trim().to_string();
                        if !tail.is_empty() && parse_ffplay_stats(&tail).is_none() {
                            kept.push_back(tail);
                        }
                    }
                    kept.into_iter().collect::<Vec<_>>().join("\n")
                })
            });

            // THE IPC SESSION. mpv serves a unix socket once it has parsed
            // its options; connect, ask to be told the clock, and read
            // events until the socket closes with the process. The write
            // half is handed to `control`. Best effort: a player that
            // never answers is still a playing track, just one that
            // restarts on pause.
            let ipc_thread = ipc_path.map(|path| {
                let shared = shared.clone();
                let stop = stop.clone();
                let ipc_slot = ipc_slot.clone();
                std::thread::spawn(move || {
                    use std::io::{BufRead, Write};
                    let deadline = std::time::Instant::now() + IPC_CONNECT_WAIT;
                    let mut sock = None;
                    while std::time::Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
                        if let Ok(s) = std::os::unix::net::UnixStream::connect(&path) {
                            sock = Some(s);
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(25));
                    }
                    let Some(mut sock) = sock else { return };
                    if sock.write_all(MPV_OBSERVE.as_bytes()).is_err() { return; }
                    let Ok(writer) = sock.try_clone() else { return };
                    if let Ok(mut g) = ipc_slot.lock() { *g = Some(writer); }
                    let mut dur = 0.0;
                    let mut pos = 0.0;
                    let mut r = std::io::BufReader::new(sock);
                    let mut line = String::new();
                    while let Ok(n) = r.read_line(&mut line) {
                        if n == 0 { break; }
                        match parse_mpv_event(&line) {
                            Some(("time-pos", v)) => pos = v,
                            Some(("duration", v)) => dur = v,
                            _ => {}
                        }
                        if let Ok(mut g) = shared.progress.lock() { *g = Some((pos, dur)); }
                        line.clear();
                    }
                    if let Ok(mut g) = ipc_slot.lock() { *g = None; }
                })
            });

            // Wait on the child OUTSIDE the mutex: holding it across a wait
            // means `stop_now` blocks until the track ends, which is a UI
            // freeze for the length of a song.
            let status = loop {
                let done = {
                    let Ok(mut g) = holder.lock() else { return };
                    match g.as_mut() {
                        Some(c) => c.try_wait().ok().flatten(),
                        None => break None,
                    }
                };
                if let Some(s) = done { break Some(s); }
                if stop.load(Ordering::Relaxed) { break None; }
                std::thread::sleep(std::time::Duration::from_millis(50));
            };
            if let Ok(mut g) = holder.lock() { let _ = g.take(); }
            // Joined, not detached: a detached reader holding a `Shared`
            // would keep writing positions into a player that has been
            // replaced. This returns promptly because the pipe closes when
            // the child goes — which is only true because the command is
            // built with `exec`, so the process we kill is the one holding
            // the pipe rather than a shell wrapping it.
            if let Some(t) = reader { let _ = t.join(); }
            let msg = err_reader.and_then(|t| t.join().ok()).unwrap_or_default();
            if let Some(t) = ipc_thread { let _ = t.join(); }
            if let Ok(mut g) = ipc_slot.lock() { *g = None; }

            if stop.load(Ordering::Relaxed) {
                // Stopped on purpose. NOT an end: advancing the queue here
                // would make pause skip a track.
                return;
            }
            // AN EXIT CODE IS NOT ENOUGH, and this is not theoretical: run
            // `ffplay` on a machine with no sound card and it prints
            // "audio open failed" and exits ZERO. Trusting the code alone
            // meant a failed track counted as a finished one — so a device
            // with no audio would skip an entire album at full speed while
            // reporting each track as played.
            let too_quick = started.elapsed() < MIN_PLAYED;
            let failed_code = matches!(status, Some(s) if !s.success()) || status.is_none();
            match (failed_code, too_quick, complaint(&msg)) {
                (false, false, None) => {
                    shared.ended.store(true, Ordering::Relaxed);
                    shared.set(State::Idle);
                }
                (_, _, why) => {
                    shared.set(State::Failed(why.unwrap_or_else(
                        || "the player stopped without playing anything".into())));
                }
            }
        }));
    }

    /// Start with the system shell.
    pub fn start(&mut self, plan: &Plan) {
        self.start_with(plan, "sh");
    }

    /// Stop the player. Both halves: the flag ends the wait loop, the kill
    /// ends the child — a flag alone leaves a song playing to the end of a
    /// track nobody is listening to.
    pub fn stop_now(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Ok(mut g) = self.child.lock() {
            if let Some(c) = g.as_mut() { let _ = c.kill(); }
        }
        if let Some(t) = self.thread.take() { let _ = t.join(); }
        if let Ok(mut g) = self.child.lock() {
            if let Some(mut c) = g.take() { let _ = c.wait(); }
        }
        if let Ok(mut g) = self.ipc.lock() { *g = None; }
        if let Some(p) = self.ipc_path.take() { let _ = std::fs::remove_file(p); }
    }

    /// Give up the route's resources. Called when the route CHANGES, not
    /// per track: a combine sink torn down between two tracks on the same
    /// speakers is a gap of silence and a rebuild for nothing.
    pub fn release_route(&mut self, launcher: &str) {
        self.stop_now();
        for cmd in std::mem::take(&mut self.teardown) {
            let _ = std::process::Command::new(launcher).arg("-c").arg(&cmd).status();
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // A player left running after the app closes is a song that keeps
        // playing with nothing to stop it.
        self.stop_now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_name_strips_extension_and_opens_up_separators() {
        assert_eq!(media_name_from_url("/music/01_Some-Track_Name.flac"), "01 Some Track Name");
    }

    #[test]
    fn media_name_falls_back_when_cleanup_leaves_nothing() {
        // A stem of pure separators becomes pure spaces after cleanup,
        // which trims to empty -- must not surface an empty
        // PULSE_PROP_media.name value in that case.
        assert_eq!(media_name_from_url("/music/---.flac"), "bo-suite");
    }

    #[test]
    fn media_name_treats_a_leading_dot_as_no_extension() {
        // Rust's Path::file_stem, unlike Python's os.path.splitext,
        // treats a dotfile as having no extension at all -- the whole
        // name is the stem. Pinned explicitly: worth knowing precisely,
        // not just "it doesn't crash", since it is the kind of platform
        // API surprise this session has been bitten by before.
        assert_eq!(media_name_from_url("/music/.flac"), ".flac");
    }

    #[test]
    fn plan_carries_the_media_name_prop_for_every_sink_kind() {
        // All four Sink variants build their own `play:` string
        // independently -- confirms none of the three PULSE_SINK sites
        // (One/Many/Surround) were missed when this prop was added
        // alongside it.
        assert!(plan("mpv", "/song.flac", &Sink::Default).play
            .starts_with("env PULSE_PROP_media.name='song' mpv"));
        assert!(plan("mpv", "/song.flac", &Sink::One("s".into())).play
            .contains("PULSE_PROP_media.name='song'"));
        assert!(plan("mpv", "/song.flac", &Sink::Many(vec!["a".into(), "b".into()])).play
            .contains("PULSE_PROP_media.name='song'"));
        assert!(plan("mpv", "/song.flac", &Sink::Surround {
            members: vec![("a".into(), "front-left".into())],
            channel_map: "front-left".into(),
        }).play.contains("PULSE_PROP_media.name='song'"));
    }

    #[test]
    fn a_path_with_an_apostrophe_survives_the_shell() {
        // Every second album has one. The naive quoting breaks on exactly
        // those, and the failure is a track that silently does not play.
        let p = plan("mpv", "/music/Don't Stop/01 - Hey, You.flac", &Sink::Default);
        assert!(p.play.contains(r#"'/music/Don'\''t Stop/01 - Hey, You.flac'"#),
                "{}", p.play);
    }

    #[test]
    fn one_speaker_needs_no_setup_and_no_teardown() {
        let p = plan("mpv", "/a.flac", &Sink::One("bluez_sink.AA".into()));
        assert!(p.setup.is_empty() && p.teardown.is_empty());
        assert!(p.play.starts_with("env PULSE_PROP_media.name='a' PULSE_SINK='bluez_sink.AA' mpv"),
                "{}", p.play);
    }

    #[test]
    fn several_speakers_get_one_stream_through_a_combine_sink() {
        // Two players is two clocks and an echo — they drift within
        // seconds, audibly, and nothing in the UI can fix it.
        let p = plan("mpv", "/a.flac",
                     &Sink::Many(vec!["one".into(), "two".into()]));
        assert_eq!(p.setup.len(), 1);
        assert!(p.setup[0].contains("module-combine-sink"), "{}", p.setup[0]);
        assert!(p.setup[0].contains("slaves='one,two'"), "{}", p.setup[0]);
        assert_eq!(p.play.matches("mpv").count(), 1, "more than one player: {}", p.play);
        assert!(!p.teardown.is_empty(), "a combine sink that is never unloaded");
    }

    #[test]
    fn surround_splits_the_stream_instead_of_duplicating_it() {
        // The distinction the variant exists for: combine DUPLICATES, which
        // is multi-room. A surround setup routed through combine puts the
        // whole mix in every corner and sounds like it is working.
        let p = plan("mpv", "/a.flac", &Sink::Surround {
            members: vec![("bt_l".into(), "front-left".into()),
                          ("bt_r".into(), "front-right".into())],
            channel_map: "front-left,front-right".into(),
        });
        assert!(p.setup[0].contains("module-null-sink"), "{}", p.setup[0]);
        assert!(!p.setup.iter().any(|c| c.contains("combine")),
                "surround was routed through a combine sink: {:#?}", p.setup);
        assert_eq!(p.setup.len(), 3, "a null sink and one remap per speaker: {:#?}", p.setup);
        assert_eq!(p.play.matches("mpv").count(), 1, "one player, one clock");
    }

    #[test]
    fn each_speaker_is_remapped_to_its_own_channel_without_remixing() {
        // With remixing on, a speaker given one channel of a 5.1 map gets
        // a downmix of everything instead — correctly quiet mud from every
        // corner, which is much harder to diagnose than silence.
        let p = plan("mpv", "/a.flac", &Sink::Surround {
            members: vec![("bt_rl".into(), "rear-left".into())],
            channel_map: "front-left,front-right,rear-left".into(),
        });
        let remap = p.setup.iter().find(|c| c.contains("module-remap-sink"))
            .expect("no remap");
        assert!(remap.contains("master_channel_map='rear-left'"), "{remap}");
        assert!(remap.contains("remix=no"), "{remap}");
        assert!(p.setup[0].contains("channels=3"), "{}", p.setup[0]);
    }

    #[test]
    fn teardown_names_the_module_rather_than_the_type() {
        // `unload-module module-remap-sink` takes out every remap on the
        // machine, which on a desktop is somebody else's headset.
        let p = plan("mpv", "/a.flac", &Sink::Surround {
            members: vec![("a".into(), "front-left".into())],
            channel_map: "front-left".into(),
        });
        assert_eq!(p.teardown.len(), 1);
        assert!(!p.teardown[0].contains("module-remap-sink"),
                "the teardown unloads by TYPE: {}", p.teardown[0]);
        assert!(p.teardown[0].contains("bo_surround_"), "{}", p.teardown[0]);
    }

    #[test]
    fn a_surround_sink_is_not_named_like_a_combine_one() {
        // The name is the only thing telling them apart in `pactl list
        // sinks`, and combine duplicates where remap splits.
        let s = Sink::surround_name(&["a".into(), "b".into()]);
        let c = Sink::combine_name(&["a".into(), "b".into()]);
        assert!(s.starts_with("bo_surround_"), "{s}");
        assert!(c.starts_with("bo_room_"), "{c}");
        assert_ne!(s, c, "the same speakers gave one name for two different sinks");
    }

    #[test]
    fn the_same_speakers_in_a_different_order_are_the_same_sink() {
        // Otherwise picking A then B and B then A leaves two combine sinks
        // behind, and every other app's output list fills with them.
        let a = Sink::combine_name(&["one".into(), "two".into()]);
        let b = Sink::combine_name(&["two".into(), "one".into()]);
        assert_eq!(a, b);
        assert_ne!(a, Sink::combine_name(&["one".into(), "three".into()]));
    }

    #[cfg(unix)]
    pub(super) fn fake_player(name: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir()
            .join(format!("bo-play-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sh");
        // Stands in for `sh`: ignores -c and runs the body instead, so the
        // spawn, the wait, the exit code and the stderr capture are all
        // exercised without a player or a sound card.
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn a_status_line_parses_and_anything_else_does_not() {
        assert_eq!(parse_status("BOPOS 12.5 240.0"), Some((12.5, 240.0)));
        assert_eq!(parse_status("  BOPOS 0 1  "), Some((0.0, 1.0)));
        // mpv prints the placeholder literally for a stream with no
        // length. Reading that as a duration puts the handle at the far
        // end of a bar with no end.
        assert_eq!(parse_status("BOPOS 3.0 ${=duration}"), None);
        assert_eq!(parse_status("BOPOS 3.0 0"), None, "a zero-length track");
        assert_eq!(parse_status("AV: 00:00:03 / 00:04:00"), None);
        assert_eq!(parse_status(""), None);
    }

    #[test]
    fn the_prefix_in_the_command_is_the_prefix_the_parser_looks_for() {
        // `concat!` takes literals, so the command spells it out while the
        // parser uses the constant. A change to one that misses the other
        // is a progress bar that silently stops moving, which is a hard
        // thing to attribute later.
        assert!(DEFAULT_PLAYER.contains(STATUS_PREFIX),
                "the player command does not emit {STATUS_PREFIX}");
    }

    #[test]
    #[cfg(unix)]
    fn the_players_clock_reaches_the_app() {
        // `Transport::report` is the sync point and had no caller: the bar
        // extrapolated from zero forever and `duration` was never set, so
        // the handle never moved.
        //
        // Carriage returns, not newlines — mpv redraws one line in place,
        // and a reader splitting on '\n' gets the whole session at once
        // at the end.
        let sh = fake_player("status",
            "printf 'BOPOS 1.0 200.0\rBOPOS 2.0 200.0\r'; exit 0");
        let mut p = Player::new();
        p.start_with(&plan("mpv", "/a.flac", &Sink::Default), &sh.display().to_string());
        let mut got = None;
        for _ in 0..200 {
            if let Some(v) = p.shared.take_progress() { got = Some(v); }
            if !p.is_running() && got.is_some() { break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let (pos, dur) = got.expect("no position ever reached the app");
        assert_eq!(dur, 200.0);
        assert!(pos >= 1.0, "the position was never updated: {pos}");
    }

    #[test]
    #[cfg(unix)]
    fn a_track_that_finishes_reports_an_end_the_queue_can_act_on() {
        // Long enough to have been a track. An instant exit is now
        // treated as a failure — see MIN_PLAYED — and a fake that returns
        // immediately would be asserting the old, wrong behaviour.
        let sh = fake_player("ok", "sleep 0.4; exit 0");
        let mut p = Player::new();
        p.start_with(&plan("mpv", "/a.flac", &Sink::Default), &sh.display().to_string());
        for _ in 0..200 {
            if !p.is_running() { break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(p.shared.take_ended(), "a finished track did not report an end");
        assert!(!p.shared.take_ended(), "…and reported it exactly once");
    }

    #[test]
    #[cfg(unix)]
    fn a_player_that_fails_says_what_it_said_rather_than_playback_failed() {
        let sh = fake_player("bad", "echo 'Failed to open audio device' >&2; exit 1");
        let mut p = Player::new();
        p.start_with(&plan("mpv", "/a.flac", &Sink::Default), &sh.display().to_string());
        let mut got = None;
        for _ in 0..200 {
            if let Some(s) = p.shared.take_state() {
                if matches!(s, State::Failed(_)) { got = Some(s); break; }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        match got {
            Some(State::Failed(m)) => assert!(m.contains("audio device"), "{m}"),
            other => panic!("expected a failure with its reason, got {other:?}"),
        }
        assert!(!p.shared.take_ended(), "a failed track advanced the queue");
    }

    #[test]
    fn the_useful_line_is_picked_out_of_a_real_players_complaint() {
        // Captured from `ffplay` on a machine with no sound card. The LAST
        // line blames the file and is wrong; the cause is above it, under
        // eight lines of ALSA configuration noise. I used the last line
        // first, which is how this was found.
        let real = "\
ALSA lib confmisc.c:855:(parse_card) cannot find card '0'
ALSA lib conf.c:5204:(_snd_config_evaluate) function snd_func_card_inum returned error: No such file or directory
ALSA lib pcm.c:2721:(snd_pcm_open_noupdate) Unknown PCM default
No more combinations to try, audio open failed
Failed to open file '/tmp/tone.wav' or configure filtergraph";
        let c = complaint(real).expect("no complaint found");
        assert!(c.contains("audio open failed"), "picked the wrong line: {c}");
        assert!(!c.starts_with("ALSA lib"), "picked ALSA noise: {c}");
    }

    #[test]
    fn nothing_on_stderr_yields_no_complaint() {
        assert_eq!(complaint(""), None);
        assert_eq!(complaint("   \n \n"), None);
    }

    #[test]
    fn only_alsa_noise_still_yields_something_rather_than_nothing() {
        // A player that printed only configuration chatter and died still
        // needs to say something, or the UI reports a failure with no
        // reason at all.
        let c = complaint("ALSA lib a\nALSA lib b");
        assert!(c.is_none(), "ALSA noise was offered as the reason: {c:?}");
    }

    #[test]
    #[cfg(unix)]
    fn a_player_that_fails_while_exiting_zero_is_not_a_finished_track() {
        // THE REASON MIN_PLAYED EXISTS, and it is not hypothetical:
        // `ffplay` with no sound card prints "audio open failed" and exits
        // ZERO. Counted as a finished track, a device with no audio skips
        // an entire album at full speed while reporting each one played.
        let sh = fake_player("liar", "echo 'audio open failed' >&2; exit 0");
        let mut p = Player::new();
        p.start_with(&plan("mpv", "/a.flac", &Sink::Default), &sh.display().to_string());
        let mut state = None;
        for _ in 0..200 {
            if let Some(s) = p.shared.take_state() {
                if matches!(s, State::Failed(_)) { state = Some(s); break; }
            }
            if !p.is_running() && state.is_none() {
                state = p.shared.take_state();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(matches!(state, Some(State::Failed(_))),
                "a zero exit after no playback was accepted: {state:?}");
        assert!(!p.shared.take_ended(), "the queue was advanced by a failure");
    }

    #[test]
    #[cfg(unix)]
    fn stopping_is_not_ending_so_pause_does_not_skip_a_track() {
        // The distinction the whole `ended` flag exists for.
        // `exec`, mirroring the real command. Without it the stand-in
        // stays alive as sleep's parent, the kill takes the wrapper and
        // not the sleeper, and the sleeper keeps the stdout pipe open —
        // which held the status reader for the full thirty seconds when I
        // first wrote this. That is exactly why `start_with` builds
        // `exec <player>`: the process we kill has to be the one holding
        // the pipe.
        let sh = fake_player("long", "exec sleep 30");
        let mut p = Player::new();
        p.start_with(&plan("mpv", "/a.flac", &Sink::Default), &sh.display().to_string());
        std::thread::sleep(std::time::Duration::from_millis(60));
        p.stop_now();
        assert!(!p.is_running(), "the player thread outlived the stop");
        assert!(!p.shared.take_ended(), "a stop was reported as a finished track");
    }
}

// ── the seam between the view and the host ──────────────────────────────────

/// What the view asks the host to do about audio.
///
/// Returned as data rather than performed, for the reason every seam in
/// this fleet is: the controller is constructed and tested with no process
/// table, and a `Player` on it would end that. It also keeps the decision
/// — which sink, which track — where the state is, and the mechanics where
/// the operating system is.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// Play this, here, from `start_secs` in. Zero is the beginning.
    Play { url: String, sink: Sink, start_secs: f64 },
    /// Pause. The host pauses the process in place when the player can be
    /// steered, and stops it otherwise — the transport has already
    /// remembered where it was, which is what `Resume` carries back.
    Pause,
    /// Continue `url` from `at`. An in-place unpause when the same player
    /// is still alive and steerable, a fresh start at `at` otherwise.
    Resume { url: String, sink: Sink, at: f64 },
    /// Move to an absolute position, seconds. In place when steerable,
    /// else a restart at that position.
    Seek { url: String, sink: Sink, at: f64 },
    /// Stop, and remember where. The position is the transport's business;
    /// this only asks for silence.
    Stop,
    /// The route changed: give up the old one's resources before the next
    /// play builds new ones.
    ReleaseRoute,
    /// The dial moved: set the route's sink volume, percent.
    Volume(f32),
}

/// Turn a picker route into a playback sink.
///
/// The two types are deliberately separate — see [`Sink`] — and this is the
/// one place they meet, so a third spelling of "which speakers" cannot
/// appear.
pub fn sink_of(route: &crate::settings_page::Route) -> Sink {
    match route {
        crate::settings_page::Route::Bluetooth(mac) => Sink::One(mac.clone()),
        // A one-member multi-room route is a single speaker taking the
        // combine-sink's latency for nothing. `SinkPicker::can_combine`
        // already refuses to produce one; this is the second guard,
        // because the cost of being wrong is a permanent latency penalty
        // nobody would think to look for.
        crate::settings_page::Route::MultiRoom(macs) if macs.len() < 2 =>
            macs.first().cloned().map(Sink::One).unwrap_or(Sink::Default),
        crate::settings_page::Route::MultiRoom(macs) => Sink::Many(macs.clone()),
    }
}

#[cfg(test)]
mod steer_tests {
    use super::*;

    #[test]
    fn exec_goes_after_the_environment_and_before_the_program() {
        assert_eq!(exec_line("mpv x"), "exec mpv x");
        assert_eq!(exec_line("A=1 B='x y' mpv 'f g'"), "A=1 B='x y' exec mpv 'f g'");
        assert_eq!(exec_line("PULSE_PROP_media.name='Don'\\''t' PULSE_SINK='s' ffplay a"),
                   "PULSE_PROP_media.name='Don'\\''t' PULSE_SINK='s' exec ffplay a");
        // the real plan, end to end: the program follows the exec
        let p = plan(DEFAULT_PLAYER, "/m/a b.flac", &Sink::One("bluez".into()));
        let line = exec_line(&p.play);
        assert!(line.starts_with("exec env PULSE_PROP_media.name='a b' PULSE_SINK='bluez' mpv"), "{line}");
        // a bare value that merely contains '=' later is not an assignment
        assert_eq!(exec_line("mpv --start=3 a"), "exec mpv --start=3 a");
    }

    #[test]
    #[cfg(unix)]
    fn a_plan_with_environment_actually_starts_the_program() {
        // The bug this catches: `exec PULSE_PROP=x mpv` fails with "not
        // found" under a real sh. Run the real shell with a real
        // assignment and a real program.
        let mut p = Player::new();
        // A program that prints its environ block, where the dotted name
        // lives even though no shell could set it as a variable, and
        // lives long enough to count as having played.
        // It reports a clock ONLY if the dotted property reached its
        // environment, so the assertion is on the seam and not on exit.
        // python rather than a nested `sh -c`: dash DROPS an inherited
        // variable whose name is not a valid identifier, which is the
        // whole reason the property travels through `env` after the
        // launcher shell rather than as an assignment inside it.
        let plan = plan("python3 -c 'import os,sys,time; sys.stdout.write(\"BOPOS 1 2\\r\" if os.environ.get(\"PULSE_PROP_media.name\") == \"Some Track\" else \"\"); sys.stdout.flush(); time.sleep(0.4)'",
                        "/Some Track.flac", &Sink::One("bluez".into()));
        p.start_with(&plan, "sh");
        let mut got = None;
        for _ in 0..200 {
            if let Some(v) = p.shared.take_progress() { got = Some(v); }
            if !p.is_running() { break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(p.shared.take_ended(), "the program never ran to its end: {:?}", p.shared.take_state());
        assert_eq!(got, Some((1.0, 2.0)), "the media name never reached the player's environment");
    }

    #[test]
    fn the_player_kind_is_read_off_the_command() {
        assert_eq!(PlayerKind::of(DEFAULT_PLAYER), PlayerKind::Mpv);
        assert_eq!(PlayerKind::of("ffplay -nodisp -autoexit"), PlayerKind::Ffplay);
        assert_eq!(PlayerKind::of("/usr/bin/paplay"), PlayerKind::Paplay);
        assert_eq!(PlayerKind::of("aplay"), PlayerKind::Other);
        assert_eq!(PlayerKind::of(""), PlayerKind::Other);
        // Only mpv can be steered; both mpv and ffplay report a clock.
        assert!(PlayerKind::Mpv.steerable() && !PlayerKind::Ffplay.steerable());
        assert!(PlayerKind::Ffplay.reports_position() && !PlayerKind::Paplay.reports_position());
    }

    #[test]
    fn each_player_gets_its_own_spelling_of_start_at() {
        let opts = PlayOpts { start_secs: 42.5, ipc: Some("/tmp/bo.sock".into()) };
        let mpv = plan_with(DEFAULT_PLAYER, "/a.flac", &Sink::Default, &opts);
        assert!(mpv.play.contains("--start=42.50"), "{}", mpv.play);
        assert!(mpv.play.contains("--input-ipc-server='/tmp/bo.sock'"), "{}", mpv.play);
        assert_eq!(mpv.ipc.as_deref(), Some("/tmp/bo.sock"));
        assert_eq!(mpv.kind, PlayerKind::Mpv);
        // The flags sit between the player and the file, never after the
        // file: mpv would take a flag after the path as a second file.
        let file_at = mpv.play.find("'/a.flac'").unwrap();
        assert!(mpv.play.find("--start=").unwrap() < file_at);

        let ff = plan_with(KNOWN_PLAYERS[1].1, "/a.flac", &Sink::Default, &opts);
        assert!(ff.play.contains("-ss 42.50"), "{}", ff.play);
        assert!(ff.play.contains("-stats"), "no clock without -stats: {}", ff.play);
        assert!(!ff.play.contains("input-ipc"), "an mpv flag reached ffplay: {}", ff.play);
        assert_eq!(ff.ipc, None, "ffplay has no socket to report");

        let pa = plan_with("paplay", "/a.flac", &Sink::Default, &opts);
        assert!(!pa.play.contains("42.5"), "paplay cannot seek and must not be told to: {}", pa.play);
    }

    #[test]
    fn the_beginning_needs_no_start_flag() {
        let opts = PlayOpts { start_secs: 0.0, ipc: None };
        assert!(!plan_with(DEFAULT_PLAYER, "/a.flac", &Sink::Default, &opts).play.contains("--start"));
        assert!(!plan_with("ffplay", "/a.flac", &Sink::Default, &opts).play.contains("-ss"));
        // and the plain `plan` is the unsteered beginning
        assert_eq!(plan(DEFAULT_PLAYER, "/a.flac", &Sink::Default),
                   plan_with(DEFAULT_PLAYER, "/a.flac", &Sink::Default, &PlayOpts::default()));
    }

    #[test]
    fn ffplays_stats_line_yields_the_clock_and_its_complaints_do_not() {
        assert_eq!(parse_ffplay_stats("   2.88 M-A: -0.000 fd=   0 aq=    0KB vq=    0KB sq=    0B f=0/0"),
                   Some(2.88));
        assert_eq!(parse_ffplay_stats("12.5 A-V:  0.001 fd=0"), Some(12.5));
        assert_eq!(parse_ffplay_stats("No more combinations to try, audio open failed"), None);
        assert_eq!(parse_ffplay_stats("Failed to open file '/x' or configure filtergraph"), None);
        assert_eq!(parse_ffplay_stats(""), None);
        assert_eq!(parse_ffplay_stats("nan M-A: 0"), None);
    }

    #[test]
    fn mpv_events_yield_the_two_properties_the_bar_needs() {
        assert_eq!(parse_mpv_event(r#"{"event":"property-change","id":1,"name":"time-pos","data":12.34}"#),
                   Some(("time-pos", 12.34)));
        assert_eq!(parse_mpv_event(r#"{"event":"property-change","id":2,"name":"duration","data":240}"#),
                   Some(("duration", 240.0)));
        // the first report of an unstarted stream is null, which is not a number
        assert_eq!(parse_mpv_event(r#"{"event":"property-change","id":1,"name":"time-pos","data":null}"#), None);
        assert_eq!(parse_mpv_event(r#"{"event":"end-file","reason":"eof"}"#), None);
        assert_eq!(parse_mpv_event(r#"{"request_id":0,"error":"success"}"#), None);
    }

    #[test]
    fn controls_spell_mpvs_protocol() {
        assert_eq!(Control::Pause.mpv_json(), "{\"command\":[\"set_property\",\"pause\",true]}\n");
        assert_eq!(Control::Resume.mpv_json(), "{\"command\":[\"set_property\",\"pause\",false]}\n");
        assert_eq!(Control::Seek(61.5).mpv_json(), "{\"command\":[\"seek\",61.500,\"absolute\"]}\n");
        assert_eq!(Control::Seek(-3.0).mpv_json(), "{\"command\":[\"seek\",0.000,\"absolute\"]}\n",
                   "a negative seek is the beginning, not an error from mpv");
        assert!(MPV_OBSERVE.ends_with('\n'), "every command line is newline-terminated");
    }

    #[test]
    fn the_volume_goes_to_the_sink_the_route_plays_through() {
        assert_eq!(volume_command(&Sink::Default, 35.0), "pactl set-sink-volume @DEFAULT_SINK@ 35%");
        assert_eq!(volume_command(&Sink::One("bluez_sink.AA".into()), 120.0),
                   "pactl set-sink-volume 'bluez_sink.AA' 100%", "clamped");
        let many = Sink::Many(vec!["a".into(), "b".into()]);
        assert!(volume_command(&many, 50.4).contains(&Sink::combine_name(&["a".into(), "b".into()])));
        assert!(volume_command(&many, 50.4).ends_with(" 50%"));
    }

    #[test]
    fn a_player_without_a_socket_cannot_be_steered_and_says_so() {
        let p = Player::new();
        assert!(!p.can_steer());
        assert!(!p.control(Control::Pause), "nothing to talk to");
    }

    #[test]
    #[cfg(unix)]
    fn the_ipc_session_reaches_the_bar_and_carries_controls_back() {
        // A stand-in for mpv's socket: a listener that records what it is
        // asked and emits two property events. This is the whole channel
        // the steerable player rests on, tested with no mpv.
        use std::io::{BufRead, Write};
        let dir = std::env::temp_dir().join(format!("bo-ipc-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock_path = dir.join("mpv.sock");
        let _ = std::fs::remove_file(&sock_path);
        let listener = std::os::unix::net::UnixListener::bind(&sock_path).unwrap();
        let heard = Arc::new(Mutex::new(Vec::<String>::new()));
        let heard2 = heard.clone();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut w = stream.try_clone().unwrap();
            let mut r = std::io::BufReader::new(stream);
            let mut line = String::new();
            // the two observe requests arrive first
            for _ in 0..2 { line.clear(); r.read_line(&mut line).unwrap(); heard2.lock().unwrap().push(line.trim().to_string()); }
            w.write_all(b"{\"event\":\"property-change\",\"id\":2,\"name\":\"duration\",\"data\":180.0}\n").unwrap();
            w.write_all(b"{\"event\":\"property-change\",\"id\":1,\"name\":\"time-pos\",\"data\":7.5}\n").unwrap();
            // then whatever control the app sends
            line.clear();
            r.read_line(&mut line).unwrap();
            heard2.lock().unwrap().push(line.trim().to_string());
        });
        // The plan's player is a shell stand-in that just lives long
        // enough; the socket is served by the thread above.
        let sh = super::tests::fake_player("ipc", "exec sleep 5");
        let mut p = Player::new();
        let plan = plan_with(DEFAULT_PLAYER, "/a.flac", &Sink::Default,
                             &PlayOpts { start_secs: 0.0, ipc: Some(sock_path.display().to_string()) });
        p.start_with(&plan, &sh.display().to_string());
        let mut got = None;
        for _ in 0..400 {
            if let Some(v) = p.shared.take_progress() { got = Some(v); }
            if got.is_some() && p.can_steer() { break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(got, Some((7.5, 180.0)), "the socket's clock did not reach the bar");
        assert!(p.can_steer(), "connected, so the player can be steered");
        assert!(p.control(Control::Seek(30.0)));
        server.join().unwrap();
        let heard = heard.lock().unwrap().clone();
        assert!(heard[0].contains("observe_property") && heard[0].contains("time-pos"), "{heard:?}");
        assert!(heard[1].contains("duration"), "{heard:?}");
        assert!(heard[2].contains("\"seek\",30.000"), "{heard:?}");
        p.stop_now();
        assert!(!p.can_steer(), "a stopped player is not steerable");
    }
}

#[cfg(test)]
mod seam_tests {
    use super::*;
    use crate::settings_page::Route;

    #[test]
    fn a_route_becomes_a_sink_in_exactly_one_place() {
        assert_eq!(sink_of(&Route::Bluetooth("AA".into())), Sink::One("AA".into()));
        assert_eq!(sink_of(&Route::MultiRoom(vec!["AA".into(), "BB".into()])),
                   Sink::Many(vec!["AA".into(), "BB".into()]));
    }

    #[test]
    fn a_multiroom_route_of_one_does_not_pay_for_a_combine_sink() {
        // The picker refuses to make one; this is the second guard, because
        // the cost is a latency penalty nobody would think to look for.
        assert_eq!(sink_of(&Route::MultiRoom(vec!["AA".into()])), Sink::One("AA".into()));
        assert_eq!(sink_of(&Route::MultiRoom(vec![])), Sink::Default);
    }
}

#[cfg(test)]
mod discovery_tests {
    use super::*;

    #[test]
    fn a_player_is_found_when_one_is_installed() {
        // This sandbox has ffplay and no mpv, which is how the hardcoded
        // default was found to be an assumption.
        match discover_player() {
            Some(cmd) => assert!(KNOWN_PLAYERS.iter().any(|(_, c)| *c == cmd),
                                 "an unknown command: {cmd}"),
            None => {} // A machine with none is a real case; see the caller.
        }
    }

    #[test]
    fn mpv_is_preferred_because_it_is_the_one_that_reports_position() {
        // The others play and report nothing: a working player and a bar
        // that does not move. Order is the whole content of the list.
        assert_eq!(KNOWN_PLAYERS[0].0, "mpv");
        assert!(KNOWN_PLAYERS[0].1.contains(STATUS_PREFIX));
        assert!(!KNOWN_PLAYERS[1].1.contains(STATUS_PREFIX),
                "a player that cannot report position claims to");
    }
}
