//! Headless Echelon matches: scripted macro bots for each race fight to the
//! death; every match records its command log and can be re-simulated to
//! prove bit-identical state hashes (`--verify`).

mod png;
mod svg;

use echelon_bots::{bot_step, plan, Bot, CADENCE};

const HASH_EVERY: u32 = 224;
use echelon_data::{DataPack, Kind, MapDef, TilesetDef};
use echelon_net::{Lockstep, NetCfg};
use echelon_sim::map::GameMap;
use echelon_sim::world::Order;
use echelon_sim::{PlayerCmd, World};

struct MatchResult {
    winner: Option<usize>,
    ticks: u32,
    hashes: Vec<(u32, u64)>,
    final_hash: u64,
    log: Vec<(u32, PlayerCmd)>,
    /// per player: (army supply, workers, structures, alloy, flux)
    summary: [(i32, usize, usize, i32, i32); 2],
}

fn run_match(
    pack: DataPack,
    races: [usize; 2],
    seed: u64,
    max_ticks: u32,
    verbose: bool,
    map_name: &str,
    svg_out: Option<(&str, u32)>,
    png_out: Option<(&str, u32)>,
) -> MatchResult {
    let race_names: [String; 2] = [
        pack.races[races[0]].name.clone(),
        pack.races[races[1]].name.clone(),
    ];
    let plans = [plan(&race_names[0]), plan(&race_names[1])];
    let (gmap, ts_name) = load_map(map_name);
    let tileset = load_tileset(&ts_name);
    let mut w = World::new(pack, gmap, races, seed);
    let mut bots = [Bot::default(), Bot::default()];
    let mut log: Vec<(u32, PlayerCmd)> = Vec::new();
    let mut hashes = Vec::new();
    let mut printed = 0usize;
    while w.tick < max_ticks && !w.defeated[0] && !w.defeated[1] {
        let mut cmds: Vec<PlayerCmd> = Vec::new();
        if w.tick % CADENCE == 0 {
            for p in 0..2u8 {
                bot_step(&w, p, &plans[p as usize], &mut bots[p as usize], &mut cmds);
            }
        }
        for c in &cmds {
            log.push((w.tick, c.clone()));
        }
        w.step(&cmds);
        if let Ok(fv) = std::env::var("ECHELON_FILM") {
            let p: Vec<&str> = fv.split(':').collect();
            let from: u32 = p[1].parse().unwrap();
            let n: u32 = p[2].parse().unwrap();
            let every: u32 = p[3].parse().unwrap();
            let hit = w.tick >= from
                && w.tick < from + n * every
                && (w.tick - from).rem_euclid(every) == 0;
            if hit {
                let tsn = load_map(map_name).1;
        let ts = load_tileset(&tsn);
                let path = format!("{}/f{:04}.png", p[0], (w.tick - from) / every);
                let _ = crate::png::render(&w.map, Some(&w), &ts, &path);
            }
        }
        if w.tick % HASH_EVERY == 0 {
            hashes.push((w.tick, w.state_hash()));
        }
        if std::env::var_os("ECHELON_DBG").is_some() && w.tick % (CADENCE * 30) == 0 {
            for p in 0..2u8 {
                let q: usize = w
                    .ents
                    .iter()
                    .filter(|e| e.alive && e.player == p)
                    .map(|e| e.queue.len())
                    .sum();
                println!(
                    "  dbg t{} p{} res={:?} sup={}/{} workers={} army2={} queued={}",
                    w.tick,
                    p,
                    w.res[p as usize],
                    w.used2[p as usize],
                    w.cap2[p as usize],
                    w.workers(p).len(),
                    w.army_supply2(p),
                    q
                );
            }
        }
        if std::env::var_os("ECHELON_DBG2").is_some() && w.tick % 660 == 0 && w.tick > 11000 {
            use echelon_sim::fx;
            let goal = echelon_sim::V2::from_tile(53, 53);
            let (mut near, mut mid, mut far, mut tgt, mut hot) = (0, 0, 0, 0, 0);
            let mut orders = [0usize; 7];
            for &i in w.army(0).iter() {
                let e = &w.ents[i as usize];
                let d2 = fx::dist2(e.pos, goal);
                if d2 <= fx::range2(echelon_sim::Fx::from_int(8)) {
                    near += 1;
                } else if d2 <= fx::range2(echelon_sim::Fx::from_int(25)) {
                    mid += 1;
                } else {
                    far += 1;
                }
                if e.target != 0 {
                    tgt += 1;
                }
                if e.cd > 0 {
                    hot += 1;
                }
                let oi = match &e.order {
                    Order::Idle => 0,
                    Order::Move(_) => 1,
                    Order::AttackMove(_) => 2,
                    Order::HarvestGo(_) => 3,
                    Order::HarvestReturn(_) => 4,
                    Order::BuildGo(_) => 5,
                    Order::Constructing(_) => 6,
                    Order::Hold => 7,
                };
                orders[oi] += 1;
            }
            println!(
                "  probe t{} p0 near={} mid={} far={} target={} cooldown={} orders(idle,move,atk)=({},{},{})",
                w.tick, near, mid, far, tgt, hot, orders[0], orders[1], orders[2]
            );
        }
        if verbose {
            while printed < w.events.len() {
                println!("  {}", w.events[printed]);
                printed += 1;
            }
        }
        if let Some((base, every)) = svg_out {
            if every > 0 && w.tick % every == 0 {
                let t = format!("{} vs {}  t{}", race_names[0], race_names[1], w.tick);
                let path = format!("{}_t{:05}.svg", base.trim_end_matches(".svg"), w.tick);
                let _ = svg::write_svg(&w, &t, &path);
            }
        }
        if let Some((base, every)) = png_out {
            if every > 0 && w.tick % every == 0 {
                let path = format!("{}_t{:05}.png", base.trim_end_matches(".png"), w.tick);
                let _ = png::render(&w.map, Some(&w), &tileset, &path);
            }
        }
    }
    if let Some((base, _)) = png_out {
        let _ = png::render(&w.map, Some(&w), &tileset, base);
    }
    if let Some((base, _)) = svg_out {
        let t = format!(
            "{} vs {}  final t{}",
            race_names[0], race_names[1], w.tick
        );
        let _ = svg::write_svg(&w, &t, base);
    }
    if std::env::var_os("ECHELON_DBG").is_some() {
        for p in 0..2u8 {
            for e in w.ents.iter() {
                if e.alive && e.player == p && e.def != u16::MAX {
                    let d = w.pack.def(e.def);
                    if d.kind == Kind::Structure {
                        println!(
                            "  end p{} {} hp={} pos=({}, {})",
                            p,
                            d.name,
                            e.hp.to_int_floor(),
                            e.pos.x.to_int_floor(),
                            e.pos.y.to_int_floor()
                        );
                    }
                }
            }
        }
    }
    let winner = match (w.defeated[0], w.defeated[1]) {
        (true, false) => Some(1),
        (false, true) => Some(0),
        _ => None,
    };
    if std::env::var("ECHELON_PROBE").is_ok() {
        for (i, e) in w.ents.iter().enumerate() {
            if e.alive
                && e.def != u16::MAX
                && w.pack.def(e.def).kind != echelon_data::Kind::Structure
            {
                println!(
                    "PROBE p{} {} {} at {:?} path {}/{} order {:?}",
                    e.player,
                    if w.pack.def(e.def).kind == echelon_data::Kind::Worker { "w" } else { "u" },
                    i,
                    e.pos.tile(),
                    e.path_i,
                    e.path.len(),
                    e.order
                );
            }
        }
        for (i, n) in w.map.nodes.iter().enumerate() {
            println!(
                "PROBE node {} {:?} at ({},{}) amount {} taken_by {}",
                i, n.kind, n.tx, n.ty, n.amount, n.taken_by
            );
        }
    }
    let mut summary = [(0, 0, 0, 0, 0); 2];
    for pl in 0..2u8 {
        let structs = w
            .ents
            .iter()
            .filter(|e| {
                e.alive
                    && e.player == pl
                    && e.def != u16::MAX
                    && w.pack.def(e.def).kind == echelon_data::Kind::Structure
            })
            .count();
        let gas = w
            .ents
            .iter()
            .filter(|e| {
                e.alive
                    && e.player == pl
                    && e.def != u16::MAX
                    && e.progress == 0
                    && w.pack.def(e.def).gas
            })
            .count();
        summary[pl as usize] = (
            w.army_supply2(pl) / 2,
            w.workers(pl).len(),
            structs * 100 + gas, // packed: structures*100 + gas count
            w.res[pl as usize].0,
            w.res[pl as usize].1,
        );
    }
    MatchResult {
        winner,
        ticks: w.tick,
        hashes,
        final_hash: w.state_hash(),
        log,
        summary,
    }
}

fn replay(pack: DataPack, races: [usize; 2], seed: u64, map_name: &str, log: &[(u32, PlayerCmd)], upto: u32) -> (Vec<(u32, u64)>, u64) {
    let mut w = World::new(pack, load_map(map_name).0, races, seed);
    let mut li = 0usize;
    let mut hashes = Vec::new();
    while w.tick < upto {
        let mut cmds = Vec::new();
        while li < log.len() && log[li].0 == w.tick {
            cmds.push(log[li].1.clone());
            li += 1;
        }
        w.step(&cmds);
        if w.tick % HASH_EVERY == 0 {
            hashes.push((w.tick, w.state_hash()));
        }
    }
    (hashes, w.state_hash())
}

/// Two full clients, each simulating its own World and computing only its
/// own player's bot commands, joined by an in-memory transport running the
/// real lockstep protocol (input delay, turn merge, hash checkpoints).
fn run_lockstep(race1: &str, race2: &str, seed: u64, max_ticks: u32, map_name: &str) {
    let cfg = NetCfg {
        players: 2,
        turn_len: 4,
        delay: 2,
    };
    let pack0 = load_pack();
    let r1 = pack0.race_index(race1).expect("race1");
    let r2 = pack0.race_index(race2).expect("race2");
    println!(
        "== lockstep {} vs {}  (seed {}, turn={}t delay={} turns) ==",
        race1, race2, seed, cfg.turn_len, cfg.delay
    );
    let plans = [plan(race1), plan(race2)];
    let mut worlds = [
        World::new(pack0, load_map(map_name).0, [r1, r2], seed),
        World::new(load_pack(), load_map(map_name).0, [r1, r2], seed),
    ];
    let mut sess = [Lockstep::new(cfg, 0), Lockstep::new(cfg, 1)];
    let mut bots = [Bot::default(), Bot::default()];
    let mut turn = 0u32;
    let mut checkpoints = 0u32;
    let max_turns = max_ticks / cfg.turn_len;
    while turn < max_turns {
        for side in 0..2usize {
            let cmds = if turn % 6 == 0 {
                let mut v = Vec::new();
                bot_step(
                    &worlds[side],
                    side as u8,
                    &plans[side],
                    &mut bots[side],
                    &mut v,
                );
                v
            } else {
                Vec::new()
            };
            sess[side].submit_local(cmds);
        }
        let o0 = sess[0].take_outbox();
        let o1 = sess[1].take_outbox();
        for m in o0 {
            sess[1].recv(m);
        }
        for m in o1 {
            sess[0].recv(m);
        }
        for side in 0..2usize {
            while let Some((_t, merged)) = sess[side].next_turn() {
                worlds[side].step(&merged);
                for _ in 1..cfg.turn_len {
                    worlds[side].step(&[]);
                }
                let tk = worlds[side].tick;
                if tk % 448 == 0 {
                    let h = worlds[side].state_hash();
                    sess[side].report_hash(tk, h);
                    if side == 0 {
                        checkpoints += 1;
                    }
                }
            }
        }
        let o0 = sess[0].take_outbox();
        let o1 = sess[1].take_outbox();
        for m in o0 {
            sess[1].recv(m);
        }
        for m in o1 {
            sess[0].recv(m);
        }
        if let Some(t) = sess[0].desync.or(sess[1].desync) {
            println!("   DESYNC at checkpoint tick {}", t);
            std::process::exit(1);
        }
        if worlds[0].defeated.iter().any(|d| *d) {
            break;
        }
        turn += 1;
    }
    let w = &worlds[0];
    let mins = w.tick / 22 / 60;
    let secs = w.tick / 22 % 60;
    let winner = match (w.defeated[0], w.defeated[1]) {
        (true, false) => Some((1usize, race2)),
        (false, true) => Some((0usize, race1)),
        _ => None,
    };
    match winner {
        Some((pw, name)) => println!(
            "   winner: player {} ({}) at t{} ({}m{:02}s)",
            pw, name, w.tick, mins, secs
        ),
        None => println!("   no winner by t{} ({}m{:02}s)", w.tick, mins, secs),
    }
    let same = worlds[0].state_hash() == worlds[1].state_hash();
    println!(
        "   {} command turns, {} hash checkpoints exchanged, desync: none",
        turn, checkpoints
    );
    println!(
        "   client worlds bit-identical at end: {}",
        if same { "YES" } else { "NO" }
    );
    if !same {
        std::process::exit(1);
    }
}

/// Two PROCESSES, one match: this process plays `side` (0 hosts on `bind`, 1 joins `peer`)
/// with its own bot, the other side's commands come over `echelon_net::udp::UdpLink`, and
/// both run the same lockstep as `run_lockstep` does in memory -- turns, delay, hash
/// checkpoints, desync detection. The host waits for the joiner's first datagram to learn
/// its address. Across two machines this is the first networked Echelon match; on one it is
/// `--lan-host 7777` in one terminal and `--lan-join 127.0.0.1:7777` in another.
fn run_lan(race1: &str, race2: &str, seed: u64, max_ticks: u32, map_name: &str, side: u8, bind: Option<u16>, peer: Option<std::net::SocketAddr>) {
    use echelon_net::udp::UdpLink;
    use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
    use std::time::{Duration, Instant};
    let cfg = NetCfg { players: 2, turn_len: 4, delay: 2 };
    let pack = load_pack();
    let r1 = pack.race_index(race1).expect("race1");
    let r2 = pack.race_index(race2).expect("race2");
    let mut link = match (side, bind, peer) {
        (0, Some(port), _) => {
            let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)).expect("bind the host port");
            sock.set_nonblocking(true).unwrap();
            println!("== lan host: {} vs {} on {}, seed {}, listening on {} -- waiting for the joiner ==", race1, race2, map_name, seed, sock.local_addr().unwrap());
            // the joiner's first datagram (its bootstrap-turn commands) names it
            let mut buf = [0u8; 64];
            let from: SocketAddr = loop {
                if let Ok((_, from)) = sock.peek_from(&mut buf) {
                    break from;
                }
                std::thread::sleep(Duration::from_millis(20));
            };
            println!("   joiner at {from}");
            UdpLink::over(sock, from)
        }
        (1, _, Some(peer)) => {
            println!("== lan joiner: {} vs {} on {}, seed {}, host {} ==", race1, race2, map_name, seed, peer);
            UdpLink::open("0.0.0.0:0", peer).expect("bind a socket")
        }
        _ => {
            eprintln!("--lan-host PORT (plays player 0) or --lan-join HOST:PORT (plays player 1)");
            std::process::exit(2);
        }
    };
    let plans = [plan(race1), plan(race2)];
    let mut world = World::new(pack, load_map(map_name).0, [r1, r2], seed);
    let mut sess = Lockstep::new(cfg, side);
    let mut bot = Bot::default();
    let mut turn = 0u32;
    let mut checkpoints = 0u32;
    let max_turns = max_ticks / cfg.turn_len;
    let started = Instant::now();
    let mut last_progress = Instant::now();
    while turn < max_turns {
        let cmds = if turn % 6 == 0 {
            let mut v = Vec::new();
            bot_step(&world, side, &plans[side as usize], &mut bot, &mut v);
            v
        } else {
            Vec::new()
        };
        sess.submit_local(cmds);
        // wait for the peer's commands for this turn, pumping the link
        loop {
            link.pump(&mut sess, Instant::now());
            let mut advanced = false;
            while let Some((_t, merged)) = sess.next_turn() {
                advanced = true;
                world.step(&merged);
                for _ in 1..cfg.turn_len {
                    world.step(&[]);
                }
                let tk = world.tick;
                if tk % 448 == 0 {
                    sess.report_hash(tk, world.state_hash());
                    checkpoints += 1;
                }
            }
            if advanced {
                last_progress = Instant::now();
                break;
            }
            if last_progress.elapsed() > Duration::from_secs(20) {
                println!("   the peer has gone quiet for 20 s ({} unacked, {} resent); giving up", link.unacked(), link.resent);
                std::process::exit(1);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        link.pump(&mut sess, Instant::now());
        if let Some(t) = sess.desync {
            println!("   DESYNC at checkpoint tick {}", t);
            std::process::exit(1);
        }
        if world.defeated.iter().any(|d| *d) {
            break;
        }
        turn += 1;
    }
    // let the last acks and hashes cross before the sockets close
    let t_end = Instant::now();
    while t_end.elapsed() < Duration::from_millis(600) {
        link.pump(&mut sess, Instant::now());
        std::thread::sleep(Duration::from_millis(10));
    }
    let w = &world;
    let winner = match (w.defeated[0], w.defeated[1]) {
        (true, false) => Some((1usize, race2)),
        (false, true) => Some((0usize, race1)),
        _ => None,
    };
    match winner {
        Some((pw, name)) => println!("   winner: player {} ({}) at t{} ({}m{:02}s)", pw, name, w.tick, w.tick / 22 / 60, w.tick / 22 % 60),
        None => println!("   no winner by t{}", w.tick),
    }
    println!(
        "   {} command turns, {} hash checkpoints, desync: {}, {} datagram(s) resent, {} duplicate(s), {:.1}s wall, final state hash {:016x}",
        turn,
        checkpoints,
        match sess.desync { Some(t) => format!("at t{t}"), None => "none".into() },
        link.resent,
        link.duplicates,
        started.elapsed().as_secs_f32(),
        w.state_hash()
    );
    if sess.desync.is_some() {
        std::process::exit(1);
    }
}

fn assets_dir(sub: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data").join(sub)
}

/// -> (map, tileset name)
fn load_map(name: &str) -> (GameMap, String) {
    if name == "standard" {
        return (GameMap::standard(), "relic_wastes".into());
    }
    let def = MapDef::load(&assets_dir("maps"), name).expect("map def");
    let ts = def.tileset.clone();
    (GameMap::from_def(&def).expect("map builds"), ts)
}

fn load_tileset(name: &str) -> TilesetDef {
    TilesetDef::load(&assets_dir("tilesets"), name).expect("tileset")
}

fn order_tag(o: &echelon_sim::world::Order) -> i32 {
    use echelon_sim::world::Order::*;
    match o {
        Idle => 0,
        Move(_) => 1,
        AttackMove(_) => 2,
        Hold => 3,
        HarvestGo(_) => 4,
        HarvestReturn(_) => 5,
        BuildGo(_) => 6,
        Constructing(_) => 7,
    }
}

/// JSON-lines game server on stdio: the human drives player 0, the coach
/// bot drives player 1. One protocol seats the Deck, agents, and replays.
use echelon_sim::Cmd;

fn serve(race1: &str, race2: &str, map_name: &str, seed: u64, opponent: &str) {
    use std::io::{BufRead, Write};
    let pack = load_pack();
    let (map, _ts) = load_map(map_name);
    let r = [
        pack.race_index(race1).expect("race1"),
        pack.race_index(race2).expect("race2"),
    ];
    let mut w = World::new(pack, map, r, seed);
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut hgt = String::new();
    let mut ramp = String::new();
    let mut bridge = String::new();
    for y in 0..w.map.h {
        for x in 0..w.map.w {
            if !(x == 0 && y == 0) {
                hgt.push(',');
                ramp.push(',');
                bridge.push(',');
            }
            hgt.push_str(&w.map.hgt(x, y).to_string());
            ramp.push_str(&w.map.ramp(x, y).map(|k| k as i32).unwrap_or(-1).to_string());
            bridge.push_str(&w.map.bridge(x, y).to_string());
        }
    }
    let nodes: Vec<String> = w
        .map
        .nodes
        .iter()
        .map(|n| {
            format!(
                "[{},{},{},{}]",
                n.tx,
                n.ty,
                if n.kind == echelon_sim::map::ResKind::Flux { 1 } else { 0 },
                n.amount
            )
        })
        .collect();
    let defs: Vec<String> = w
        .pack
        .defs
        .iter()
        .enumerate()
        .map(|(i, (ri, d))| {
            let ids = |names: &Vec<String>| {
                names
                    .iter()
                    .filter_map(|n| w.pack.by_name.get(n))
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            };
            let abil = d
                .abil
                .iter()
                .map(|a| format!("\"{}\"", a))
                .collect::<Vec<_>>()
                .join(",");
            format!(
                concat!(
                    "{{\"id\":{},\"name\":\"{}\",\"race\":\"{}\",\"kind\":\"{}\",",
                    "\"fp\":{},\"air\":{},\"hp\":{},\"sh\":{},\"en\":{},",
                    "\"cost\":[{},{}],\"sup\":{},\"sight\":{},\"range\":{},",
                    "\"builds\":[{}],\"trains\":[{}],\"abil\":[{}]}}"
                ),
                i,
                d.name,
                w.pack.races[*ri].name,
                match d.kind {
                    Kind::Worker => "W",
                    Kind::Structure => "S",
                    _ => "U",
                },
                d.fp(),
                (d.mover() == echelon_data::Mover::Air) as i32,
                d.hp,
                d.sh,
                d.energy,
                d.cost.0,
                d.cost.1,
                d.supply2,
                d.sight_m,
                d.wpn.as_ref().map(|x| x.range_m).unwrap_or(0),
                ids(&d.builds),
                ids(&d.trains),
                abil
            )
        })
        .collect();
    let _ = writeln!(
        out,
        concat!(
            "{{\"type\":\"hello\",\"you\":0,\"w\":{},\"h\":{},",
            "\"starts\":[[{},{}],[{},{}]],",
            "\"hgt\":[{}],\"ramp\":[{}],\"bridge\":[{}],",
            "\"nodes\":[{}],\"defs\":[{}]}}"
        ),
        w.map.w,
        w.map.h,
        w.map.starts[0].0,
        w.map.starts[0].1,
        w.map.starts[1].0,
        w.map.starts[1].1,
        hgt,
        ramp,
        bridge,
        nodes.join(","),
        defs.join(",")
    );
    let _ = out.flush();
    let coach_plan = plan(race2);
    let mut coach = Bot::default();
    // "coach" (default) or "champion:/path/to/arena_state.json" — the
    // Deck can face the thing the league grew
    let genome = if let Some(path) = opponent.strip_prefix("champion:") {
        let txt = std::fs::read_to_string(path).expect("champion state");
        let key = "\"best\": [";
        let i = txt.find(key).expect("best vec") + key.len();
        let j = i + txt[i..].find(']').expect("vec end");
        let v: Vec<i32> = txt[i..j]
            .split(',')
            .map(|x| x.trim().parse().expect("gene"))
            .collect();
        echelon_bots::Genome::from_vec(&v)
    } else {
        echelon_bots::Genome::coach()
    };
    let stdin = std::io::stdin();
    let mut pending: Vec<PlayerCmd> = Vec::new();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.is_empty() {
            continue;
        }
        let pi = |k: usize| t.get(k).and_then(|x| x.parse::<i32>().ok()).unwrap_or(0);
        let pu = |k: usize| t.get(k).and_then(|x| x.parse::<u32>().ok()).unwrap_or(0);
        let ids_from = |k: usize| {
            t[k.min(t.len())..]
                .iter()
                .filter_map(|x| x.parse::<u32>().ok())
                .collect::<Vec<u32>>()
        };
        match t[0] {
            "quit" => break,
            "move" => pending.push(PlayerCmd {
                player: 0,
                cmd: Cmd::Move { ids: ids_from(3), mx: pi(1), my: pi(2) },
            }),
            "amove" => pending.push(PlayerCmd {
                player: 0,
                cmd: Cmd::Attack { ids: ids_from(3), mx: pi(1), my: pi(2) },
            }),
            "stop" => pending.push(PlayerCmd { player: 0, cmd: Cmd::Stop { ids: ids_from(1) } }),
            "hold" => pending.push(PlayerCmd { player: 0, cmd: Cmd::Hold { ids: ids_from(1) } }),
            "harvest" => {
                let node = pu(1);
                for id in ids_from(2) {
                    pending.push(PlayerCmd { player: 0, cmd: Cmd::Harvest { id, node } });
                }
            }
            "train" => pending.push(PlayerCmd {
                player: 0,
                cmd: Cmd::Train { structure: pu(1), unit: pu(2) as u16 },
            }),
            "build" => pending.push(PlayerCmd {
                player: 0,
                cmd: Cmd::Build { worker: pu(1), unit: pu(2) as u16, tx: pi(3), ty: pi(4) },
            }),
            "cast" => pending.push(PlayerCmd {
                player: 0,
                cmd: Cmd::Cast { caster: pu(1), slot: pu(2) as u8, target: pu(3), mx: pi(4), my: pi(5) },
            }),
            "rally" => pending.push(PlayerCmd {
                player: 0,
                cmd: Cmd::Rally { structure: pu(1), mx: pi(2), my: pi(3) },
            }),
            "step" => {
                let n = pi(1).max(1) as u32;
                for _ in 0..n {
                    let mut cmds = std::mem::take(&mut pending);
                    if w.tick % CADENCE == 0 {
                        echelon_bots::bot_step_g(
                            &w, 1, &coach_plan, &genome, &mut coach, &mut cmds,
                        );
                    }
                    w.step(&cmds);
                }
                let vis = w.visible_mask(0);
                let mut ents = String::new();
                let mut queues = String::new();
                for (i, e) in w.ents.iter().enumerate() {
                    if !vis[i] {
                        continue;
                    }
                    let d = w.pack.def(e.def);
                    if !ents.is_empty() {
                        ents.push(',');
                    }
                    let lv = if d.mover() == echelon_data::Mover::Air {
                        w.fly_level(i) as i32
                    } else {
                        0
                    };
                    ents.push_str(&format!(
                        "[{},{},{},{},{},{},{},{},{},{},{},{}]",
                        i,
                        e.def,
                        e.player,
                        e.pos.x.to_milli(),
                        e.pos.y.to_milli(),
                        e.hp.to_int_floor().max(0),
                        e.sh.to_int_floor().max(0),
                        e.energy.to_int_floor().max(0),
                        e.progress,
                        order_tag(&e.order),
                        lv,
                        (e.cloak_t > 0) as i32
                    ));
                    if e.player == 0 && !e.queue.is_empty() {
                        if !queues.is_empty() {
                            queues.push(',');
                        }
                        let q: Vec<String> = e.queue.iter().map(|qi| qi.def.to_string()).collect();
                        queues.push_str(&format!("\"{}\":[{}]", i, q.join(",")));
                    }
                }
                let winner = {
                    let alive = |p: u8| {
                        w.ents.iter().any(|e| {
                            e.alive
                                && e.player == p
                                && e.def != u16::MAX
                                && w.pack.def(e.def).kind == Kind::Structure
                        })
                    };
                    if w.tick > 600 && !alive(1) {
                        0
                    } else if w.tick > 600 && !alive(0) {
                        1
                    } else {
                        -1
                    }
                };
                let _ = writeln!(
                    out,
                    concat!(
                        "{{\"type\":\"snap\",\"tick\":{},\"res\":[{},{}],",
                        "\"sup\":[{},{}],\"winner\":{},\"ents\":[{}],\"q\":{{{}}}}}"
                    ),
                    w.tick,
                    w.res[0].0,
                    w.res[0].1,
                    w.used2[0],
                    w.cap2[0],
                    winner,
                    ents,
                    queues
                );
                let _ = out.flush();
            }
            _ => {}
        }
    }
}

fn load_pack() -> DataPack {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/data/races");
    DataPack::load_dir(&dir, &["genetron.ron", "xayid.ron", "keiron.ron"])
        .expect("data pack")
}

fn one(
    race1: &str,
    race2: &str,
    seed: u64,
    max_ticks: u32,
    verify: bool,
    verbose: bool,
    map_name: &str,
    svg_out: Option<(&str, u32)>,
    png_out: Option<(&str, u32)>,
) {
    let pack = load_pack();
    let r1 = pack.race_index(race1).expect("race1");
    let r2 = pack.race_index(race2).expect("race2");
    println!(
        "== {} vs {} on {}  (seed {}, data hash {:016x}) ==",
        race1, race2, map_name, seed, pack.hash
    );
    let res = run_match(load_pack(), [r1, r2], seed, max_ticks, verbose, map_name, svg_out, png_out);
    let mins = res.ticks / 22 / 60;
    let secs = res.ticks / 22 % 60;
    match res.winner {
        Some(pw) => println!(
            "   winner: player {} ({}) at t{} ({}m{:02}s)",
            pw,
            if pw == 0 { race1 } else { race2 },
            res.ticks,
            mins,
            secs
        ),
        None => println!("   no winner by t{} ({}m{:02}s)", res.ticks, mins, secs),
    }
    for pl in 0..2usize {
        let (a, wk, st, al, fl) = res.summary[pl];
        println!(
            "   p{}: army {} supply, {} workers, {} structures ({} gas), bank {}a/{}f",
            pl,
            a,
            wk,
            st / 100,
            st % 100,
            al,
            fl
        );
    }
    println!(
        "   commands: {}   final state hash: {:016x}",
        res.log.len(),
        res.final_hash
    );
    if verify {
        let (h2, f2) = replay(load_pack(), [r1, r2], seed, map_name, &res.log, res.ticks);
        let ok = h2 == res.hashes && f2 == res.final_hash;
        println!(
            "   verify (re-sim from command log): {}",
            if ok { "IDENTICAL — deterministic" } else { "MISMATCH!" }
        );
        if !ok {
            for (a, b) in res.hashes.iter().zip(h2.iter()) {
                if a != b {
                    println!("     first divergence at t{}", a.0);
                    break;
                }
            }
            std::process::exit(1);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut seed = 7u64;
    let mut max_ticks = 22 * 60 * 14;
    let mut verify = false;
    let mut verbose = false;
    let mut lockstep = false;
    let mut lan_host: Option<u16> = None;
    let mut lan_join: Option<std::net::SocketAddr> = None;
    let mut svg_base: Option<String> = None;
    let mut svg_every = 0u32;
    let mut png_base: Option<String> = None;
    let mut png_every = 0u32;
    let mut film: Option<(String, u32, u32, u32)> = None; // dir, from, frames, every
    let mut map_name = String::from("standard");
    let mut opponent = String::from("coach");
    let mut pos: Vec<String> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--verify" => verify = true,
            "--events" => verbose = true,
            "--lockstep" => lockstep = true,
            "--lan-host" => {
                i += 1;
                lan_host = Some(args[i].parse().expect("--lan-host PORT"));
            }
            "--lan-join" => {
                i += 1;
                lan_join = Some(args[i].parse().expect("--lan-join HOST:PORT"));
            }
            "--svg" => {
                i += 1;
                svg_base = Some(args[i].clone());
            }
            "--png" => {
                i += 1;
                png_base = Some(args[i].clone());
            }
            "--film" => {
                i += 1;
                let dir = args[i].clone();
                i += 1;
                let from: u32 = args[i].parse().unwrap();
                i += 1;
                let n: u32 = args[i].parse().unwrap();
                i += 1;
                let every: u32 = args[i].parse().unwrap();
                film = Some((dir, from, n, every));
            }
            "--png-every" => {
                i += 1;
                png_every = args[i].parse().unwrap();
            }
            "--map" => {
                i += 1;
                map_name = args[i].clone();
            }
            "--opponent" => {
                i += 1;
                opponent = args[i].clone();
            }
            "--svg-every" => {
                i += 1;
                svg_every = args[i].parse().unwrap();
            }
            "--seed" => {
                i += 1;
                seed = args[i].parse().unwrap();
            }
            "--ticks" => {
                i += 1;
                max_ticks = args[i].parse().unwrap();
            }
            s => pos.push(s.to_string()),
        }
        i += 1;
    }
    let svg_out = svg_base.as_deref().map(|b| (b, svg_every));
    if let Some((dir, from, n, every)) = &film {
        std::env::set_var("ECHELON_FILM", format!("{}:{}:{}:{}", dir, from, n, every));
        std::fs::create_dir_all(dir).ok();
    }
    let png_out = png_base.as_deref().map(|b| (b, png_every));
    if pos.first().map(|s| s.as_str()) == Some("serve") {
        let r1 = pos.get(1).expect("serve R1 R2").clone();
        let r2 = pos.get(2).expect("serve R1 R2").clone();
        serve(&r1, &r2, &map_name, seed, &opponent);
        return;
    }
    if pos.first().map(|s| s.as_str()) == Some("preview") {
        let mname = pos.get(1).expect("preview MAP OUT.png");
        let out = pos.get(2).expect("preview MAP OUT.png");
        let (m, ts) = load_map(mname);
        png::render(&m, None, &load_tileset(&ts), out).expect("write png");
        println!("wrote {} ({} tileset)", out, ts);
        return;
    }
    if pos.first().map(|s| s.as_str()) == Some("all") {
        for (a, b) in [
            ("Genetron", "Xayid"),
            ("Xayid", "Keiron"),
            ("Keiron", "Genetron"),
        ] {
            if lockstep {
                run_lockstep(a, b, seed, max_ticks, &map_name);
            } else {
                one(a, b, seed, max_ticks, verify, verbose, &map_name, None, None);
            }
        }
        return;
    }
    let r1 = pos.get(0).cloned().unwrap_or_else(|| "Genetron".into());
    let r2 = pos.get(1).cloned().unwrap_or_else(|| "Xayid".into());
    if lan_host.is_some() || lan_join.is_some() {
        let side = if lan_host.is_some() { 0 } else { 1 };
        run_lan(&r1, &r2, seed, max_ticks, &map_name, side, lan_host, lan_join);
    } else if lockstep {
        run_lockstep(&r1, &r2, seed, max_ticks, &map_name);
    } else {
        one(&r1, &r2, seed, max_ticks, verify, verbose, &map_name, svg_out, png_out);
    }
}
