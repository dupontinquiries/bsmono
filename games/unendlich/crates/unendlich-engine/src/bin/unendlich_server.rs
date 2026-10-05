//! `unendlich-server` — headless world host for cloud deployment. Parses CLI args (see --help),
//! stands up a CPU-generated world (no rendering, no window), enforces the world ruleset (exactly
//! one world loaded), and runs a fixed-rate simulation tick loop.
//!
//! SCOPE (r440 → now): r440 brought up the headless world-sim; the networking transport lived
//! inline in the fullgame render loop. That transport is now factored into the reusable
//! `sim::net::lan_host_pump::HostPump` (peer table + admission + star relay, loopback-tested),
//! and THIS bin wires it: the server binds the session socket, announces the world on the LAN
//! discovery beacon, admits joiners (Accept carries the host-assigned entity id), applies their
//! block edits to the authoritative world, relays entities/edits/avatars between peers, and
//! keeps chunks resident around every connected player via `update_with_interest_points`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use unendlich_engine::sim::server_config::{ParseOutcome, ServerConfig, HELP};
use unendlich_engine::world::chunk_manager::ChunkManager;
use unendlich_engine::world::terrain_generator::{Backend, TerrainGenerator};
use unendlich_engine::world::world_list::{WorldEntry, WorldList};

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let cfg = match ServerConfig::parse(&args) {
        ParseOutcome::Help => {
            println!("{HELP}");
            return;
        }
        ParseOutcome::Error(e) => {
            eprintln!("error: {e}\n\n{HELP}");
            std::process::exit(2);
        }
        ParseOutcome::Config(c) => c,
    };

    log::info!(
        "[server] starting world='{}' seed={} bind={}:{} tick={}Hz view_distance={}",
        cfg.name,
        cfg.seed,
        cfg.bind,
        cfg.port,
        cfg.tick_hz,
        cfg.view_distance
    );

    // Enforce the world ruleset: register the world and load it through the WorldList so the
    // no-concurrent rule is the single gate any future "load another world" request hits.
    let mut worlds = WorldList::new();
    let world_id = cfg.seed; // id == seed for now; a real registry would assign ids
    worlds.register(WorldEntry { id: world_id, name: cfg.name.clone(), seed: cfg.seed });
    match worlds.try_load(world_id) {
        Ok(id) => log::info!("[server] world loaded (id={id}); rule={:?}", worlds.rule()),
        Err(e) => {
            eprintln!("error: cannot load world: {e:?}");
            std::process::exit(1);
        }
    }

    // Headless terrain: force CPU (no GPU on a typical server node).
    let generator = match TerrainGenerator::new(cfg.seed, Backend::Cpu, 64) {
        Ok(g) => Arc::new(g),
        Err(e) => {
            eprintln!("error: terrain generator init failed: {e:?}");
            std::process::exit(1);
        }
    };
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    // Persistence: the same directory the fullgame opens for this name and seed (or
    // --world-dir). Without it every edit a player made died with the process.
    let world_dir = cfg.effective_world_dir();
    let mut chunks = ChunkManager::new(generator, workers).with_storage(world_dir.clone());
    log::info!("[server] world ready ({workers} worker threads); edits persist in {}", world_dir.display());

    // ---- Networking: the factored-out reusable host ------------------------
    // Session socket + peer table + star relay. `entity id 0` for the host's
    // roster entry: a headless server has no player entity, and 0 can't
    // collide with either host-assigned ids (>= 1<<32) or legacy port-derived
    // ids (nonzero u16 ports).
    use unendlich_engine::sim::net::join_sync::SyncPlan;
    use unendlich_engine::sim::net::lan_host_pump::{HostPump, PumpEvent, CH_JOINSYNC, CH_WORLDCMD};
    use unendlich_engine::sim::world_authority::WorldCmd;
    use unendlich_engine::world::blocks::BlockId;
    let mut pump = match HostPump::bind(cfg.port, &cfg.name, 0) {
        Ok(p) => {
            log::info!("[server] session socket bound on udp/{}", p.port());
            Some(p)
        }
        Err(e) => {
            log::warn!("[server] session socket bind failed: {e} — running sim-only (no joins)");
            None
        }
    };
    // Access policy: the world's own whitelist.json (the fullgame writes it from its
    // Multiplayer panel; absent means open). The server never changes it.
    let whitelist = unendlich_engine::sim::whitelist::Whitelist::load(&world_dir);
    log::info!(
        "[server] access: {} ({} on allow-list, {})",
        if whitelist.open { "OPEN (anyone may join)" } else { "CLOSED (whitelist only)" },
        whitelist.len(),
        world_dir.join("whitelist.json").display()
    );
    // LAN discovery: announce this world on the multicast beacon so it appears
    // in fullgame Multiplayer lists without needing UNENDLICH_LAN_HOST.
    let beacon = match unendlich_engine::sim::lan_discovery::LanBeacon::bind() {
        Ok(b) => {
            log::info!("[server] discovery beacon bound");
            Some(b)
        }
        Err(e) => {
            log::warn!("[server] beacon bind failed: {e} — direct-connect only (UNENDLICH_LAN_HOST)");
            None
        }
    };
    let mut announcer = unendlich_engine::sim::net::presence_loop::HostAnnouncer::with_period(2000);
    let lan_ip = unendlich_engine::sim::lan_discovery::local_lan_ipv4();
    match lan_ip {
        Some(o) => log::info!("[server] local LAN ip {}.{}.{}.{}", o[0], o[1], o[2], o[3]),
        None => log::warn!("[server] no routable LAN ipv4 — adverts won't carry an address"),
    }
    // Last known position per connected player entity id — the interest points
    // that keep their surroundings resident and ticking.
    let mut player_pos: std::collections::HashMap<u64, (i32, i32, i32)> =
        std::collections::HashMap::new();
    // Join-at-any-time: per-joiner edit catch-up (timebending storage layout,
    // paced a few datagrams per tick so a built world doesn't stall the loop).
    let mut join_syncs: Vec<(std::net::SocketAddrV4, SyncPlan)> = Vec::new();
    let net_start = Instant::now();

    // Fixed-rate tick loop.
    let tick_dt = Duration::from_secs_f64(1.0 / cfg.tick_hz as f64);
    let dt_s = tick_dt.as_secs_f32();
    let mut last_status = Instant::now();
    let mut tick: u64 = 0;

    // Ctrl-C / SIGTERM → the loop ends and the shutdown save runs.
    match install_stop_handler() {
        Ok(()) => log::info!("[server] running — Ctrl-C to stop (edits are saved on the way out)"),
        Err(e) => log::warn!("[server] no signal handler ({e}): a Ctrl-C will not save; use the autosave"),
    }
    let autosave = (cfg.autosave_secs > 0).then(|| Duration::from_secs(cfg.autosave_secs as u64));
    let mut last_autosave = Instant::now();
    while !STOP.load(std::sync::atomic::Ordering::SeqCst) {
        let t0 = Instant::now();
        let now_ms = net_start.elapsed().as_millis() as u64;

        // -- Network drain: admissions, entity intake, edits, relay, prune. --
        if let Some(pump) = pump.as_mut() {
            // The server's world clock: sim ticks over tick_hz, in ms — the
            // joiner adopts it so everyone shares the same time of day.
            let world_time_ms = (tick as f64 / cfg.tick_hz.max(1) as f64 * 1000.0) as u64;
            let events = pump.pump(now_ms, &cfg.name, cfg.seed, world_time_ms, &|name| {
                whitelist.admits(name)
            });
            let mut edits_applied: Vec<(u64, WorldCmd)> = Vec::new();
            for ev in events {
                match ev {
                    PumpEvent::PeerJoined { name, entity_id, rejoined, addr } => {
                        log::info!(
                            "[server] '{name}' joined (entity {entity_id}{}) — {} player(s)",
                            if rejoined { ", rejoined" } else { "" },
                            pump.host.live_peer_count()
                        );
                        // Queue the edit catch-up unless this is a lost-Accept
                        // retry with a plan already draining.
                        if !join_syncs.iter().any(|(a, _)| *a == addr) {
                            let records = chunks.edited_chunk_records();
                            if !records.is_empty() {
                                let plan = SyncPlan::new(&records, 4);
                                log::info!(
                                    "[server] join-sync for '{name}': {} record(s), {} batch(es)",
                                    records.len(),
                                    plan.total_batches()
                                );
                                join_syncs.push((addr, plan));
                            }
                        }
                    }
                    PumpEvent::PeerDenied { name, reason } => {
                        log::info!("[server] join denied for '{name}': {reason:?}");
                    }
                    PumpEvent::PeerLeft { entity_id } => {
                        player_pos.remove(&entity_id);
                        log::info!(
                            "[server] entity {entity_id} timed out — {} player(s)",
                            pump.host.live_peer_count()
                        );
                    }
                    PumpEvent::Entities(states) => {
                        for s in states {
                            player_pos.insert(
                                s.id,
                                (
                                    s.x.to_f32().floor() as i32,
                                    s.y.to_f32().floor() as i32,
                                    s.z.to_f32().floor() as i32,
                                ),
                            );
                        }
                    }
                    PumpEvent::WorldCmds(cmds) => {
                        for (player, cmd) in cmds {
                            let applied = match cmd {
                                WorldCmd::Place { pos, block } => BlockId::try_from(block as u8)
                                    .map(|bid| chunks.set_block(pos.0, pos.1, pos.2, bid))
                                    .unwrap_or(false),
                                WorldCmd::Remove { pos } => {
                                    chunks.set_block(pos.0, pos.1, pos.2, BlockId::Air)
                                }
                                _ => false,
                            };
                            if applied {
                                edits_applied.push((player, cmd));
                            }
                        }
                    }
                    PumpEvent::Avatar { .. } => {} // relayed by the pump; nothing to render here
                }
            }
            // Authoritative echo of applied edits is unnecessary: the pump
            // already star-relayed the raw WORLDCMD datagrams peer→peers, and
            // the server's own state changed above. (`edits_applied` is kept
            // for the status line so operators can see edit throughput.)
            if !edits_applied.is_empty() {
                log::debug!("[server] applied {} edit(s)", edits_applied.len());
            }
            let _ = CH_WORLDCMD; // channel byte re-exported for future host-originated edits
            // Drain join-sync catch-up plans (paced).
            join_syncs.retain_mut(|(addr, plan)| {
                for batch in plan.drain_frame() {
                    let packed = unendlich_engine::sim::net::packet_compress::pack(batch, 48);
                    pump.send_to_peer(CH_JOINSYNC, &packed, *addr);
                }
                !plan.is_done()
            });

            // -- Announce on the discovery beacon (2 s cadence). --
            if let Some(beacon) = &beacon {
                let advert = unendlich_engine::sim::session_mode::WorldAdvert {
                    host_username: cfg.name.clone(),
                    world_name: cfg.name.clone(),
                    players: pump.host.player_count() as u16,
                    max_players: unendlich_engine::sim::net::player_roster::MAX_PLAYERS as u16,
                    last_seen_ms: now_ms,
                    candidates: match lan_ip {
                        Some(ip) => vec![unendlich_engine::sim::lan_discovery::lan_candidate(
                            ip,
                            pump.port(),
                        )],
                        None => Vec::new(),
                    },
                    world_seed: cfg.seed,
                };
                if let Some(refreshed) = announcer.poll(&advert, now_ms) {
                    if let Err(e) = beacon.announce(&refreshed) {
                        log::warn!("[server] announce failed: {e}");
                    }
                }
            }
        }

        // -- World tick: chunks resident around every connected player. --
        if player_pos.is_empty() {
            chunks.update(0, 0, 0);
        } else {
            let points: Vec<(i32, i32, i32)> = player_pos.values().copied().collect();
            chunks.update_with_interest_points(&points);
        }
        chunks.tick_simulation(dt_s);
        tick += 1;

        if last_status.elapsed().as_secs() >= 5 {
            log::info!(
                "[server] tick={tick} chunks_loaded={} players={} ({:.1} tick/s target)",
                chunks.loaded_count(),
                pump.as_ref().map(|p| p.host.player_count()).unwrap_or(1),
                cfg.tick_hz
            );
            last_status = Instant::now();
        }
        if let Some(every) = autosave {
            if last_autosave.elapsed() >= every {
                let saved = chunks.save_all();
                if saved > 0 {
                    log::info!("[server] autosave: {saved} edited chunk(s)");
                }
                last_autosave = Instant::now();
            }
        }

        if let Some(rem) = tick_dt.checked_sub(t0.elapsed()) {
            std::thread::sleep(rem);
        }
    }

    let saved = chunks.save_all();
    log::info!("[server] shutting down after {tick} ticks; {saved} edited chunk(s) saved to {}", world_dir.display());
}

/// Set by the SIGINT/SIGTERM handler; the tick loop ends on it and the shutdown save runs.
/// (A signal handler may only touch an atomic, which is why the flag is a static.)
static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(unix)]
fn install_stop_handler() -> Result<(), String> {
    extern "C" fn on_signal(_sig: libc::c_int) {
        STOP.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    // SAFETY: `on_signal` is an `extern "C" fn(c_int)` that only stores to an atomic, which is
    // async-signal-safe; `signal` is called once, before the loop, from the main thread.
    let handler = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
    for sig in [libc::SIGINT, libc::SIGTERM] {
        if unsafe { libc::signal(sig, handler) } == libc::SIG_ERR {
            return Err(format!("signal({sig}) failed: {}", std::io::Error::last_os_error()));
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn install_stop_handler() -> Result<(), String> {
    Err("no signal handler on this platform".to_string())
}
