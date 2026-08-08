//! The Proving Grounds — a CPU-honest AlphaStar-shaped league.
//!
//! Matches are pure functions of (genomes, races, map, seed), so fitness
//! is a measurement, not an estimate. Generation zero is the coach's own
//! genome plus mutations (imitation by initialization); every generation
//! is graded against the coach as a permanent fixed exploiter, plus a
//! rolling bench of frozen past champions (the league). The goal is a
//! number: `bench` must show the champion taking >=90% off the coach
//! across every race pairing, both maps, both colors, a spread of seeds.
//!
//!   echelon-arena time
//!   echelon-arena evolve <gens> [pop] [state.json]
//!   echelon-arena bench <state.json> [seeds]

use echelon_bots::{bot_step_g, plan, Bot, Genome, CADENCE};
use echelon_data::{DataPack, Kind};
use echelon_sim::map::GameMap;
use echelon_sim::{PlayerCmd, World};
use std::io::Write;

const MAX_TICKS: u32 = 14_000;
const MAPS: [&str; 2] = ["confluence", "cauldron"];
const RACES: [&str; 3] = ["Genetron", "Xayid", "Keiron"];

fn assets_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data")
}

fn load_pack() -> DataPack {
    DataPack::load_dir(
        &assets_dir().join("races"),
        &["genetron.ron", "xayid.ron", "keiron.ron"],
    )
    .expect("data pack")
}

fn load_map(name: &str) -> echelon_data::MapDef {
    let p = assets_dir().join("maps").join(format!("{}.ron", name));
    ron::from_str(&std::fs::read_to_string(p).expect("map file")).expect("map parse")
}

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9e3779b97f4a7c15) | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn i(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.below((hi - lo + 1) as u64) as i32)
    }
}

/// one deterministic match; score in [-1, 1] from p0's perspective
fn play(
    pack: &DataPack,
    maps: &[echelon_data::MapDef; 2],
    map_ix: usize,
    races: [usize; 2],
    gs: [&Genome; 2],
    seed: u64,
) -> (Option<usize>, u32, f32) {
    let map = GameMap::from_def(&maps[map_ix]).expect("map");
    let mut w = World::new((*pack).clone(), map, races, seed);
    let plans = [plan(&pack.races[races[0]].name), plan(&pack.races[races[1]].name)];
    let mut bots = [Bot::default(), Bot::default()];
    let mut winner = None;
    while w.tick < MAX_TICKS {
        let mut cmds: Vec<PlayerCmd> = Vec::new();
        if w.tick % CADENCE == 0 {
            for p in 0..2u8 {
                let (a, b) = bots.split_at_mut(1);
                let st = if p == 0 { &mut a[0] } else { &mut b[0] };
                bot_step_g(&w, p, &plans[p as usize], gs[p as usize], st, &mut cmds);
            }
        }
        w.step(&cmds);
        // deterministic whale guard: if the joint army count blows past
        // the budget, call it a draw at this tick (pure function of
        // state, so replays agree)
        if w.tick % 200 == 0 {
            let alive = w.ents.iter().filter(|e| e.alive && e.def != u16::MAX).count();
            if alive > 240 {
                break;
            }
        }
        if w.tick > 600 {
            let s0 = alive_structs(&w, 0);
            let s1 = alive_structs(&w, 1);
            if s0 == 0 || s1 == 0 {
                winner = Some(if s0 == 0 { 1 } else { 0 });
                break;
            }
        }
    }
    let score = match winner {
        Some(0) => 1.0 - 0.3 * (w.tick as f32 / MAX_TICKS as f32),
        Some(1) => -1.0 + 0.3 * (w.tick as f32 / MAX_TICKS as f32),
        _ => {
            // draw: economy + army differential, squashed
            let v = |p: u8| {
                w.army_supply2(p) as f32 * 1.5
                    + alive_structs(&w, p) as f32 * 12.0
                    + w.workers(p).len() as f32 * 3.0
                    + (w.res[p as usize].0 + w.res[p as usize].1) as f32 / 60.0
            };
            let d = (v(0) - v(1)) / 260.0;
            0.30 * d.clamp(-1.0, 1.0)
        }
    };
    (winner, w.tick, score)
}

fn alive_structs(w: &World, p: u8) -> usize {
    w.ents
        .iter()
        .filter(|e| {
            e.alive
                && e.player == p
                && e.def != u16::MAX
                && w.pack.def(e.def).kind == Kind::Structure
        })
        .count()
}

/// fitness battery for one genome vs the coach (+ one league champion)
fn evaluate(
    pack: &DataPack,
    maps: &[echelon_data::MapDef; 2],
    g: &Genome,
    coach: &Genome,
    league: &[Genome],
    gen: usize,
    seed_base: u64,
) -> (f32, u32, u32) {
    let mut score = 0.0f32;
    let mut wins = 0u32;
    let mut games = 0u32;
    let t0 = std::time::Instant::now();
    let mut duel = |opp: &Genome, map_ix: usize, pair: usize, flip: bool, seed: u64| {
        let (r_me, r_op) = (pair % 3, (pair + 1) % 3);
        let (races, gs, sgn) = if flip {
            ([r_op, r_me], [opp, g], -1.0f32)
        } else {
            ([r_me, r_op], [g, opp], 1.0f32)
        };
        eprintln!("duel map{} pair{} flip{} seed{}", map_ix, pair, flip, seed);
        let (winner, _t, s) = play(pack, maps, map_ix, races, gs, seed);
        games += 1;
        let my_side = if flip { 1 } else { 0 };
        if winner == Some(my_side) {
            wins += 1;
        }
        score += sgn * s;
    };
    // rotate configs per generation: a champion must beat the coach
    // everywhere, not memorize four matches (the gen-5 lesson)
    // full wheel: spin decouples map from color so all 12 bench slots
    // (pair x map x color) receive gradient across any 4 generations,
    // and seeds never repeat — the crown is held everywhere or not at all
    let abs_gen = ((seed_base.saturating_sub(11)) / 97) as usize;
    for pair in 0..3 {
        let spin = (pair + abs_gen) % 4;
        let map_ix = spin % 2;
        let flip = spin >= 2;
        duel(coach, map_ix, pair, flip, seed_base + pair as u64);
    }

    if std::env::var("ARENA_LEAGUE").is_ok() && !league.is_empty() {
        let opp = &league[gen % league.len()];
        for pair in 0..2 {
            duel(opp, pair, pair, pair == 1, 31 + pair as u64);
        }
    }
    eprint!("[{:.0}s]", t0.elapsed().as_secs_f32());
    (score / games as f32, wins, games)
}

fn mutate(g: &mut Genome, rng: &mut Rng, rate_pct: i32) {
    let mut v = g.to_vec();
    let mut steps = [2i32; 51];
    let head = [2, 2, 3, 1, 3, 80, 1, 1, 120, 10, 6, 15, 1500, 1500];
    steps[..14].copy_from_slice(&head);
    steps[50] = 25;
    for (i, x) in v.iter_mut().enumerate() {
        if (rng.below(100) as i32) < rate_pct {
            let s = steps[i];
            *x += rng.i(-s, s);
        }
    }
    *g = Genome::from_vec(&v);
}

fn crossover(a: &Genome, b: &Genome, rng: &mut Rng) -> Genome {
    let (va, vb) = (a.to_vec(), b.to_vec());
    let v: Vec<i32> = va
        .iter()
        .zip(vb.iter())
        .map(|(x, y)| if rng.below(2) == 0 { *x } else { *y })
        .collect();
    Genome::from_vec(&v)
}

fn genome_json(g: &Genome) -> String {
    let v: Vec<String> = g.to_vec().iter().map(|x| x.to_string()).collect();
    format!("[{}]", v.join(","))
}

fn save_state(path: &str, gen: usize, best: &Genome, fit: f32, wins: u32, games: u32, hist: &[String]) {
    let mut f = std::fs::File::create(path).expect("state file");
    let _ = writeln!(
        f,
        "{{\n  \"gen\": {},\n  \"best_fitness\": {:.4},\n  \"best_vs_coach\": \"{}/{}\",\n  \"best\": {},\n  \"coach\": {},\n  \"history\": [\n{}\n  ]\n}}",
        gen,
        fit,
        wins,
        games,
        genome_json(best),
        genome_json(&Genome::coach()),
        hist.iter()
            .map(|h| format!("    \"{}\"", h))
            .collect::<Vec<_>>()
            .join(",\n")
    );
}

#[allow(clippy::too_many_arguments)]
fn evolve(gens: usize, pop_n: usize, out: &str, init: Option<&str>, base_gen: usize) {
    let pack = load_pack();
    let maps = [load_map(MAPS[0]), load_map(MAPS[1])];
    let coach = Genome::coach();
    let mut rng = Rng::new(0xA1F45 ^ (base_gen as u64).wrapping_mul(0x9E37));
    let seed_g = init.map(parse_best).unwrap_or_else(|| coach.clone());
    // burst-friendly seeding: the champion, its mutants, AND the aggro
    // exploiters all land in the evaluated pool of every call — a burst
    // that only ever scores warm-start mutants tests nothing new
    let mut pop: Vec<Genome> = Vec::with_capacity(pop_n);
    pop.push(seed_g.clone());
    while pop.len() < pop_n {
        let slot = pop.len();
        let mut g = if slot % 3 == 0 {
            Genome::aggro()
        } else {
            seed_g.clone()
        };
        mutate(&mut g, &mut rng, if slot % 3 == 0 { 20 } else { 20 + 15 * (slot as i32 % 3) });
        pop.push(g);
    }
    let mut league: Vec<Genome> = Vec::new();
    let mut hist: Vec<String> = Vec::new();
    let threads = 1usize;
    for gen in 0..gens {
        let seed_base = 11 + ((base_gen + gen) as u64) * 97;
        let mut results: Vec<(f32, u32, u32)> = vec![(0.0, 0, 0); pop.len()];
        std::thread::scope(|sc| {
            for (ti, chunk) in pop.chunks(pop.len().div_ceil(threads)).enumerate() {
                let pack = &pack;
                let maps = &maps;
                let coach = &coach;
                let league = &league;
                let base = ti * pop.len().div_ceil(threads);
                let res_ptr: *mut (f32, u32, u32) = results.as_mut_ptr();
                let handle = unsafe { std::slice::from_raw_parts_mut(res_ptr.add(base), chunk.len()) };
                sc.spawn(move || {
                    for (i, g) in chunk.iter().enumerate() {
                        handle[i] =
                            evaluate(pack, maps, g, coach, league, gen, seed_base);
                    }
                });
            }
        });
        let mut idx: Vec<usize> = (0..pop.len()).collect();
        idx.sort_by(|&a, &b| results[b].0.partial_cmp(&results[a].0).unwrap());
        let best = idx[0];
        let mean: f32 = results.iter().map(|r| r.0).sum::<f32>() / pop.len() as f32;
        let line = format!(
            "gen {:>3}  best {:+.3}  mean {:+.3}  best-vs-coach {}/{}",
            base_gen + gen,
            results[best].0,
            mean,
            results[best].1,
            results[best].2
        );
        println!("{}", line);
        use std::io::Write as _;
        let _ = std::io::stdout().flush();
        hist.push(line);
        save_state(out, base_gen + gen, &pop[best], results[best].0, results[best].1, results[best].2, &hist);
        if gen % 3 == 2 {
            league.push(pop[best].clone());
            if league.len() > 4 {
                league.remove(0);
            }
        }
        // next generation: elitism + tournament + crossover + mutation,
        // plus one fresh aggro-exploiter immigrant every generation
        let mut next: Vec<Genome> = vec![pop[idx[0]].clone(), pop[idx[1]].clone()];
        {
            let mut imm = Genome::aggro();
            mutate(&mut imm, &mut rng, 25);
            next.push(imm);
        }
        while next.len() < pop.len() {
            let t = |rng: &mut Rng| {
                let mut b = idx[rng.below(pop.len() as u64) as usize];
                for _ in 0..2 {
                    let c = idx[rng.below(pop.len() as u64) as usize];
                    if results[c].0 > results[b].0 {
                        b = c;
                    }
                }
                b
            };
            let (pa, pb) = (t(&mut rng), t(&mut rng));
            let mut child = crossover(&pop[pa], &pop[pb], &mut rng);
            mutate(&mut child, &mut rng, 30);
            next.push(child);
        }
        pop = next;
    }
}

fn parse_best(path: &str) -> Genome {
    let s = std::fs::read_to_string(path).expect("state file");
    let key = "\"best\": [";
    let i = s.find(key).expect("best vec") + key.len();
    let j = i + s[i..].find(']').expect("vec end");
    let v: Vec<i32> = s[i..j]
        .split(',')
        .map(|x| x.trim().parse().expect("gene"))
        .collect();
    Genome::from_vec(&v)
}

fn bench(state: &str, seeds: u64, seed_base: u64) {
    let pack = load_pack();
    let maps = [load_map(MAPS[0]), load_map(MAPS[1])];
    let champ = parse_best(state);
    let coach = Genome::coach();
    println!("champion genome: {}", genome_json(&champ));
    let mut wins = 0u32;
    let mut games = 0u32;
    let mut lines = Vec::new();
    for map_ix in 0..2 {
        for pair in 0..3 {
            for color in 0..2 {
                for s in 0..seeds {
                    let (r_me, r_op) = (pair % 3, (pair + 1) % 3);
                    let (races, gs, my_side) = if color == 1 {
                        ([r_op, r_me], [&coach, &champ], 1usize)
                    } else {
                        ([r_me, r_op], [&champ, &coach], 0usize)
                    };
                    let (winner, t, _s) =
                        play(&pack, &maps, map_ix, races, gs, seed_base + s);
                    games += 1;
                    let won = winner == Some(my_side);
                    if won {
                        wins += 1;
                    }
                    let line = format!(
                        "{} {} as {} vs {}: {} (t{})",
                        MAPS[map_ix],
                        if color == 0 { "p0" } else { "p1" },
                        RACES[r_me],
                        RACES[r_op],
                        if won { "WIN" } else { match winner { Some(_) => "loss", None => "draw" } },
                        t
                    );
                    println!("  {}", line);
                    use std::io::Write as _;
                    let _ = std::io::stdout().flush();
                    lines.push(line);
                }
            }
        }
    }
    println!(
        "champion vs coach: {}/{} = {:.1}%",
        wins,
        games,
        wins as f32 * 100.0 / games as f32
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("time") => {
            let pack = load_pack();
            let maps = [load_map(MAPS[0]), load_map(MAPS[1])];
            let g = Genome::coach();
            let t0 = std::time::Instant::now();
            let (w, t, s) = play(&pack, &maps, 0, [0, 1], [&g, &g], 11);
            println!("match: winner {:?} t{} score {:+.2} in {:.2?}", w, t, s, t0.elapsed());
        }
        Some("evolve") => {
            let gens = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(12);
            let pop = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(12);
            let out = args.get(4).map(|s| s.as_str()).unwrap_or("arena_state.json");
            let init = args.get(5).map(|s| s.as_str());
            let base = args.get(6).and_then(|s| s.parse().ok()).unwrap_or(0);
            evolve(gens, pop, out, init, base);
        }
        Some("play-one") => {
            // bsgames provider: one deterministic match, one RESULT line.
            // Entrant ids: <policy>@<Race> where policy is `coach` or
            // `champion:/path/to/state.json`.
            let get = |flag: &str| {
                args.iter()
                    .position(|a| a == flag)
                    .and_then(|i| args.get(i + 1))
                    .cloned()
                    .unwrap_or_default()
            };
            let parse_ent = |id: &str| -> (Genome, usize) {
                let (pol, race) = id.split_once('@').expect("entrant id policy@Race");
                let g = if let Some(path) = pol.strip_prefix("champion:") {
                    parse_best(path)
                } else {
                    Genome::coach()
                };
                let ri = RACES
                    .iter()
                    .position(|r| r.eq_ignore_ascii_case(race))
                    .expect("race");
                (g, ri)
            };
            let (ga, ra) = parse_ent(&get("--a"));
            let (gb, rb) = parse_ent(&get("--b"));
            let map = get("--map");
            let map_ix = MAPS.iter().position(|m| *m == map).expect("map");
            let seed: u64 = get("--seed").parse().expect("seed");
            let pack = load_pack();
            let maps = [load_map(MAPS[0]), load_map(MAPS[1])];
            let (winner, t, score) =
                play(&pack, &maps, map_ix, [ra, rb], [&ga, &gb], seed);
            println!(
                "RESULT winner={} ticks={} score={:+.2}",
                match winner {
                    Some(0) => "a",
                    Some(1) => "b",
                    _ => "draw",
                },
                t,
                score
            );
        }
        Some("bench") => {
            let state = args.get(2).expect("bench <state.json> [seeds] [seed_base]");
            let seeds = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(5);
            let base = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(100);
            bench(state, seeds, base);
        }
        _ => println!("echelon-arena time | evolve <gens> [pop] [out.json] | bench <state.json> [seeds]"),
    }
}
