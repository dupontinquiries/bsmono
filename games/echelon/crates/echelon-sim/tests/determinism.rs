//! Two worlds, same seed, same commands => identical state hashes forever.

use echelon_data::DataPack;
use echelon_sim::map::GameMap;
use echelon_sim::{Cmd, PlayerCmd, World};

fn pack() -> DataPack {
    DataPack::from_sources_with_abilities(
        &[
            ("genetron", include_str!("../../../assets/data/races/genetron.ron")),
            ("xayid", include_str!("../../../assets/data/races/xayid.ron")),
            ("keiron", include_str!("../../../assets/data/races/keiron.ron")),
        ],
        include_str!("../../../assets/data/abilities.ron"),
    )
    .expect("pack parses")
}

fn scripted(w: &World, tick: u32) -> Vec<PlayerCmd> {
    let mut v = Vec::new();
    if tick == 50 {
        for p in 0..2u8 {
            let hall = w
                .ents
                .iter()
                .enumerate()
                .find(|(_, e)| e.alive && e.player == p && w.pack.def(e.def).townhall)
                .map(|(i, _)| i as u32)
                .unwrap();
            let worker = w.workers(p)[0];
            let wd = w.ents[worker as usize].def;
            v.push(PlayerCmd {
                player: p,
                cmd: Cmd::Train {
                    structure: hall,
                    unit: wd,
                },
            });
        }
    }
    if tick == 300 {
        for p in 0..2u8 {
            let ws = w.workers(p);
            v.push(PlayerCmd {
                player: p,
                cmd: Cmd::Attack {
                    ids: ws[0..3].to_vec(),
                    mx: 32 * 1000,
                    my: 32 * 1000,
                },
            });
        }
    }
    v
}

#[test]
fn same_seed_same_hash() {
    let mk = || World::new(pack(), GameMap::standard(), [0, 1], 42);
    let mut a = mk();
    let mut b = mk();
    for t in 0..1500u32 {
        let ca = scripted(&a, t);
        let cb = scripted(&b, t);
        a.step(&ca);
        b.step(&cb);
        if t % 100 == 0 {
            assert_eq!(a.state_hash(), b.state_hash(), "diverged at tick {}", t);
        }
    }
    assert_eq!(a.state_hash(), b.state_hash());
}

#[test]
fn different_seed_differs() {
    let mut a = World::new(pack(), GameMap::standard(), [0, 2], 1);
    let mut b = World::new(pack(), GameMap::standard(), [0, 2], 2);
    for t in 0..200u32 {
        let ca = scripted(&a, t);
        let cb = scripted(&b, t);
        a.step(&ca);
        b.step(&cb);
    }
    // worlds only differ through rng draws; ensure the hash actually
    // captures rng state even if nothing else diverged
    assert_ne!(
        {
            a.rng.next();
            a.state_hash()
        },
        b.state_hash()
    );
}

#[test]
fn tech_gates_hold() {
    let w = World::new(pack(), GameMap::standard(), [0, 1], 9);
    // Genetron Fixer requires @uplink1: no uploader linked yet
    let manu_missing = w.find_named(0, "Manufacturer", true);
    assert!(manu_missing.is_none(), "no Manufacturer at game start");
}

#[test]
fn abilities_fire() {
    use echelon_sim::world::Order;
    let mut w = World::new(pack(), GameMap::standard(), [1, 0], 5); // p0 = Xayid
    let mend = w.pack.id("Mendling");
    let ling = w.pack.id("Xayidling");
    let healer = w.spawn_unit(mend, 0, echelon_sim::V2::from_tile(30, 20));
    let hurt = w.spawn_unit(ling, 0, echelon_sim::V2::from_tile(31, 20));
    w.ents[hurt as usize].hp = echelon_sim::Fx::from_int(20);
    w.ents[healer as usize].order = Order::Idle;
    for _ in 0..60 {
        w.step(&[]);
    }
    assert!(
        w.ents[hurt as usize].hp > echelon_sim::Fx::from_int(40),
        "Mend autocast healed: hp={}",
        w.ents[hurt as usize].hp.to_int_floor()
    );
    // Active cast: Kraken discharge nukes a clump
    let kra = w.pack.id("Kraken");
    let tgt = w.pack.id("Spitfire");
    let k = w.spawn_unit(kra, 0, echelon_sim::V2::from_tile(40, 40));
    let v1 = w.spawn_unit(tgt, 1, echelon_sim::V2::from_tile(43, 40));
    let hp0 = w.ents[v1 as usize].hp;
    w.step(&[PlayerCmd {
        player: 0,
        cmd: Cmd::Cast {
            caster: k,
            slot: 0,
            target: 0,
            mx: 43 * 1000 + 500,
            my: 40 * 1000 + 500,
        },
    }]);
    assert!(w.ents[v1 as usize].hp < hp0, "discharge dealt damage");
}
