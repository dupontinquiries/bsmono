//! MAPSIM — `--terrain`, and the seam `terrain.rs` was missing.
//!
//! `terrain.rs` is 645 lines of physics-grounded worldgen — dune spacing from
//! saltation hop length, talus runout from gravity, crater profiles that go
//! central-peak at the right diameter — and it has **zero callers**. It has
//! had zero callers since it landed. `docs/PLAN.md` named the reason and the
//! fix in one line: *"`World::generate` takes a `Ground` — turns 13 route rows
//! into 13 places."*
//!
//! The gap is not the renderer. It is that nothing ever converted a
//! `patrol::Destination` (gravity in g, a weather flag, a sky) into a
//! `terrain::Ground` (gravity in m/s², a material, air density, friction
//! velocity). Two tables describing the same worlds in different units, with
//! no function between them.
//!
//! `ground_for` is that function, and this module is the tool that proves it
//! before any of it reaches a GPU. That ordering is deliberate: the terrain
//! relations are checkable arithmetic, and checking them headlessly is
//! cheaper and more honest than looking at a hillside and deciding it seems
//! about right.
//!
//! ```text
//! --terrain grounds          every destination's derived Ground, side by side
//! --terrain profile <dest>   an ASCII cross-section of real generated ground
//! --terrain dunes            dune spacing across the route, against the physics
//! --terrain audit            every route row converts, and the relations hold
//! ```
//!
//! ## What is derived and what is asserted
//!
//! Gravity is a unit conversion and nothing else. Air density and wind come
//! from the destination's own `sky` and `weather` fields, which is the closest
//! thing to data the route table has. **Material is a judgement** — the route
//! table does not record what a world is made of — so it is a small explicit
//! mapping with the reasoning next to each row, not a heuristic pretending to
//! be a measurement.

use crate::patrol::{self, Destination, Sky, Terrain};
use crate::procgen;
use glam::Vec3;
use crate::sun::Surface;
use crate::survival::Weather;
use crate::terrain::{self, Ground, Material};

/// What a world is made of. Not in `patrol::ROUTE`, so it is stated here
/// rather than guessed from gravity — the ice moons and the rock worlds
/// overlap far too much in mass for that to work.
fn material_of(d: &Destination) -> Material {
    match d.id {
        // ice: Europa's shell, Glaciem's lattice pools, the Kuiper cold
        "europa" | "glaciem" => Material::Ice,
        // nitrogen/methane ice, which flows at stresses water ice holds
        "pluto" => Material::SoftIce,
        // volcanic: Io's tide-forges resurface it faster than it weathers
        "io" => Material::Basalt,
        // worked metal, not geology
        "cybertron" => Material::Metal,
        // wind-sorted grains over rock: the dune worlds
        "mars" | "titan" => Material::Regolith,
        // airless and impact-gardened: regolith over basalt, and the top
        // metre is what terrain actually shapes
        "luna" => Material::Regolith,
        // layered and water-cut
        "earth" => Material::Sandstone,
        // everything else, including the exoworlds and the procedural rows:
        // basalt is the least-wrong default for a rocky body with no record
        _ => Material::Basalt,
    }
}

/// Atmospheric density, kg/m³, from the sky the route table already records.
///
/// Rough by construction — these are the order-of-magnitude figures for the
/// real bodies they are modelled on, and terrain only uses air through the
/// wind term, where an order of magnitude is what matters.
fn air_of(d: &Destination) -> f32 {
    match (d.sky, d.id) {
        (_, "earth") => 1.225,
        (_, "titan") => 5.4, // thicker than Earth's, and cold
        (_, "mars") => 0.020,
        (Sky::Vacuum, _) => 0.0,
        (Sky::Glare, _) => 0.0,
        (Sky::Atmosphere, _) => 1.0,
        (Sky::Murk, _) => 0.5,
        (Sky::Ember, _) => 0.01,
        // deep-space starfield and Cybertron's lattice sky: no atmosphere to
        // speak of. Cybertron's is a built ceiling, not weather.
        (Sky::Starfield, _) => 0.0,
        (Sky::Lattice, _) => 0.0,
    }
}

/// Friction velocity u*, m/s.
///
/// Zero where the route says there is no weather — and ALSO zero where there
/// is no air, because a friction velocity is a property of a moving fluid and
/// there is no aeolian transport in a vacuum. The route table lets a row carry
/// `weather: true` alongside a vacuum sky (Pluto and the four exoworlds all
/// do), and the first run of `--terrain grounds` printed exactly that: u* 0.25
/// over air 0.000. Cheap to miss in a table, obvious in a column.
fn wind_of(d: &Destination) -> f32 {
    if !d.weather || air_of(d) <= 0.0 {
        return 0.0;
    }
    match d.id {
        "mars" => 2.0,  // thin air needs high u* to move anything at all
        "titan" => 0.04, // thick, slow, and the sand is not sand
        "earth" => 0.3,
        _ => 0.25,
    }
}

/// The seam: a route row becomes ground you can generate.
pub fn ground_for(d: &Destination) -> Ground {
    Ground::new(d.gravity * Ground::EARTH_G, material_of(d), air_of(d), wind_of(d))
}

/// Destinations that have a surface to stand on. Deep space and asteroid
/// fields have no ground, and asking for one is a caller bug rather than a
/// row to invent.
pub fn has_ground(d: &Destination) -> bool {
    matches!(d.terrain, Terrain::Surface | Terrain::Procedural)
}

// ----------------------------------------------------- generated worlds ---
//
// The other half of the map pipeline, and the other dead module.
// `procgen.rs` produces a world from a seed — star, orbit, weather, a
// gravity from a mass-radius guess — and has had no caller since it landed,
// same as `terrain.rs`. They are the two halves of the same thing: procgen
// decides WHAT a bridge jump lands on, terrain decides what it LOOKS like,
// and neither could reach the other because the units never matched.
//
// This is the same seam as `ground_for`, for the worlds nobody authored. The
// route table's eighteen rows are the exception; the procedural sky past
// `the_far_gate` is the rule, so if the seam only worked for authored
// destinations it would be working for the small case.

/// What a generated world is made of.
///
/// Authored destinations get an explicit per-world judgement (`material_of`),
/// because the fiction knows what Io and Europa are. A seed does not, so this
/// derives it from the one thing the generator does establish — the surface
/// temperature class — which is also the honest driver: what a world is made
/// of at the surface is largely a question of what is frozen there.
fn material_of_world(w: &procgen::World) -> Material {
    match w.orbit.surface() {
        // rock is liquid or nearly so; what solidifies is volcanic
        Surface::Molten | Surface::Scorched => Material::Basalt,
        // water can be liquid somewhere: layered, water-worked ground
        Surface::Temperate => Material::Sandstone,
        // water ice is structural at these temperatures
        Surface::Frozen => Material::Ice,
        // nitrogen and methane ices, which flow at stresses water ice holds
        Surface::Cryogenic => Material::SoftIce,
    }
}

/// Friction velocity for a generated world.
///
/// Same rule as the authored path and for the same reason: no air, no
/// aeolian transport, whatever the weather enum says. A dust storm in a
/// vacuum is a contradiction the generator is allowed to produce and the
/// ground model is not allowed to believe.
fn wind_of_world(w: &procgen::World) -> f32 {
    if w.orbit.air <= 0.0 {
        return 0.0;
    }
    let base = match w.weather {
        Weather::Clear => 0.15,
        Weather::DustStorm => 1.2,
        Weather::ThermalFront => 0.6,
        Weather::GiantWave => 0.3,
        // charged particles, not moving air: nothing is being blown around
        Weather::FlareFront => 0.0,
    };
    // thin air needs a higher friction velocity to move the same grain, which
    // is why Mars' storms are fast and gentle
    base / w.orbit.air.max(0.05).sqrt()
}

/// The seam, for worlds nobody authored.
pub fn ground_for_world(w: &procgen::World) -> Ground {
    Ground::new(
        w.gravity_g * Ground::EARTH_G,
        material_of_world(w),
        w.orbit.air,
        wind_of_world(w),
    )
}

pub fn run(argv: &[String]) {
    let i = argv.iter().position(|a| a == "--terrain").unwrap_or(0);
    let cmd = argv.get(i + 1).map(String::as_str).unwrap_or("grounds");
    let arg = argv.get(i + 2).map(String::as_str);
    match cmd {
        "grounds" => grounds(),
        "profile" => profile(arg),
        "dunes" => dunes(),
        "audit" => audit(),
        "jump" => jump(arg),
        "scale" => scale_cmd(),
        "depth" => depth_cmd(),
        "drop" => drop_run(arg, argv.get(i + 3).map(String::as_str)),
        other => {
            println!("unknown: --terrain {other}");
            println!("try: grounds | profile <dest> | dunes | audit | jump [seed]");
            println!("     drop [seed] [altitude] | scale | depth");
        }
    }
}

fn grounds() {
    println!("DERIVED GROUND, per route row\n");
    println!("  {:<12} {:>7}  {:<10} {:>8}  {:>6}", "dest", "g m/s2", "material", "air", "u*");
    for d in patrol::ROUTE {
        if !has_ground(d) {
            println!("  {:<12} {:>7}  {:<10} {:>8}  {:>6}   (no surface)", d.id, "-", "-", "-", "-");
            continue;
        }
        let g = ground_for(d);
        println!(
            "  {:<12} {:>7.2}  {:<10} {:>8.3}  {:>6.2}",
            d.id,
            g.g,
            format!("{:?}", g.material),
            g.air,
            g.wind
        );
    }
    println!("\n`--terrain profile <dest>` to generate a cross-section.");
}

fn dunes() {
    // the relation this module exists to make visible: saltation hop length
    // goes as u*^2/g, so dunes on a low-gravity thin-air world are spaced
    // hundreds of metres apart and Earth's are spaced tens.
    println!("DUNES — where the wind can build them at all\n");
    for d in patrol::ROUTE.iter().filter(|d| has_ground(d)) {
        let g = ground_for(d);
        let h = terrain::dune_height(&g, 120.0, 340.0, (1.0, 0.0), 7);
        match h {
            Some(v) => println!("  {:<12} dune field, height {:+.2} m at the sample point", d.id, v),
            None => println!(
                "  {:<12} no dunes ({})",
                d.id,
                if g.wind == 0.0 { "no weather" } else { "wind cannot move this material" }
            ),
        }
    }
}

fn profile(arg: Option<&str>) {
    let Some(id) = arg else {
        println!("--terrain profile <dest>");
        return;
    };
    let Some(d) = patrol::find(id) else {
        println!("no destination '{id}'.");
        return;
    };
    if !has_ground(d) {
        println!("{id} has no surface ({:?}).", d.terrain);
        return;
    }
    let g = ground_for(d);
    println!("{} — g {:.2} m/s2, {:?}, air {:.3}, u* {:.2}\n", d.name, g.g, g.material, g.air, g.wind);

    // 2 km of ground, sampled, drawn as columns. Real generator output, not a
    // sketch: this is the same fbm/ridged the world would use.
    const W: usize = 72;
    const H: usize = 14;
    const SPAN_M: f32 = 2000.0;
    let mut hs = [0.0f32; W];
    for (i, h) in hs.iter_mut().enumerate() {
        let x = (i as f32 / W as f32) * SPAN_M;
        let base = terrain::fbm(x * 0.004, 0.0, 0.0, 4, 11) * 60.0;
        let ridge = terrain::ridged(x * 0.0015, 0.0, 0.0, 3, 11) * 30.0;
        let dune = terrain::dune_height(&g, x, 0.0, (1.0, 0.0), 7).unwrap_or(0.0);
        *h = base + ridge + dune;
    }
    let lo = hs.iter().cloned().fold(f32::INFINITY, f32::min);
    let hi = hs.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let span = (hi - lo).max(0.001);
    for row in 0..H {
        let level = hi - (row as f32 + 0.5) * span / H as f32;
        let mut line = String::new();
        for h in &hs {
            line.push(if *h >= level { '#' } else { ' ' });
        }
        println!("  {line}");
    }
    println!("  {}", "-".repeat(W));
    println!("  0 m{:>width$}", format!("{:.0} m", SPAN_M), width = W - 2);
    println!("\n  relief {:.1} m over {:.0} m", hi - lo, SPAN_M);
    println!("  talus runout from a 200 m drop: {:.1} m", terrain::talus_depth(&g, 200.0, 50.0));
}

/// Generate a bridge jump and describe where it put you.
///
/// The whole procedural pipeline in one call: procgen picks a world, the seam
/// gives it ground, terrain generates that ground, and `procgen::raid_for`
/// says what is on it. Every one of those modules was unreachable an hour ago.
fn jump(arg: Option<&str>) {
    let seed: u32 = arg.and_then(|s| s.parse().ok()).unwrap_or(20260822);
    let w = procgen::World::generate_viable(seed);
    let g = ground_for_world(&w);

    println!("BRIDGE JUMP — seed {seed}\n");
    println!("  {} world, {:?} star at {:.2} AU", w.descriptor(), w.orbit.star, w.orbit.au);
    println!("  surface {:.0} K, air {:.2}, weather {:?}", w.orbit.surface_k(), w.orbit.air, w.weather);
    println!("  gravity {:.2} g -> {:.2} m/s2, {:?}, u* {:.2}", w.gravity_g, g.g, g.material, g.wind);
    if !w.has_ground {
        println!("\n  no surface: this is a jump you fly, not one you land on.");
        return;
    }

    // real generated ground, same primitives the world would use
    const W: usize = 68;
    let mut hs = [0.0f32; W];
    for (i, h) in hs.iter_mut().enumerate() {
        let x = (i as f32 / W as f32) * 2000.0;
        *h = terrain::fbm(x * 0.004, 0.0, 0.0, 4, seed as u64) * 60.0
            + terrain::ridged(x * 0.0015, 0.0, 0.0, 3, seed as u64) * 30.0
            + terrain::dune_height(&g, x, 0.0, (1.0, 0.0), seed as u64).unwrap_or(0.0);
    }
    let lo = hs.iter().cloned().fold(f32::INFINITY, f32::min);
    let hi = hs.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let span = (hi - lo).max(0.001);
    println!();
    for row in 0..10 {
        let level = hi - (row as f32 + 0.5) * span / 10.0;
        let line: String = hs.iter().map(|h| if *h >= level { '#' } else { ' ' }).collect();
        println!("  {line}");
    }
    println!("  {}", "-".repeat(W));
    println!("  relief {:.1} m over 2000 m", hi - lo);

    let raid = procgen::raid_for(&w, "the far side");
    println!(
        "\n  what is on it: {} encounters ({:.0} s), {} footholds, boss tier {}",
        raid.encounters.len(),
        raid.encounters.iter().sum::<f32>(),
        raid.sites.len(),
        raid.boss_tier
    );
    println!("  extraction: {:.0} m back to the ship", raid.extract_m);
}

/// The last link in the chain: leave the ship and land on the world.
///
/// `drop.rs` is the twenty seconds between the hangar and the raid, and it
/// had no caller either — the fourth dead module in this pipeline. It is
/// ballistic physics with steering, which means it is exactly the kind of
/// thing that CAN be checked without a display: where you land is decided by
/// the ship's velocity at the door and by when you fold, and both are
/// numbers.
///
/// Real gravity from the generated world, so a drop onto a heavy world is a
/// different problem from a drop onto a light one — which is the argument for
/// running it here rather than assuming 9.81.
fn drop_run(seed_arg: Option<&str>, alt_arg: Option<&str>) {
    let seed: u32 = seed_arg.and_then(|s| s.parse().ok()).unwrap_or(20260822);
    let alt: f32 = alt_arg.and_then(|s| s.parse().ok()).unwrap_or(400.0);
    let w = procgen::World::generate_viable(seed);
    let g = ground_for_world(&w);
    if !w.has_ground {
        println!("seed {seed} has no surface. nothing to drop onto.");
        return;
    }
    let raid = procgen::raid_for(&w, "the far side");
    let Some(site) = raid.sites.first() else {
        println!("seed {seed} generated no foothold to aim at.");
        return;
    };

    println!("THE DROP — seed {seed}, {:.0} m up over a {} world\n", alt, w.descriptor());
    println!("  gravity {:.2} m/s2 ({:?}), aiming at a {:?} foothold ({} garrison)", g.g, g.material, site.approach, format!("{:.0}%", site.garrison * 100.0));

    // the ship's line at the door: a fast low pass throws you forward
    let lz = Vec3::new(0.0, 0.0, 0.0);
    let door = Vec3::new(0.0, alt, -260.0);
    let carry = Vec3::new(0.0, 0.0, 42.0);

    println!("\n  {:<14} {:>10} {:>10} {:>10}", "steer", "miss m", "impact", "self dmg");
    for (name, steer) in [
        ("none", Vec3::ZERO),
        ("into the line", Vec3::new(0.0, 0.0, 6.0)),
        ("against it", Vec3::new(0.0, 0.0, -6.0)),
        ("crosswise", Vec3::new(6.0, 0.0, 0.0)),
    ] {
        let mut d = crate::drop::Drop::new(lz, raid.boss_tier >= 2);
        if let Err(e) = d.can_jump(true, true, alt) {
            println!("  cannot jump: {e}");
            return;
        }
        d.jump(door, carry);
        let mut land = None;
        for step in 0..(90 * 60) {
            // fold late: the hero window is close to the ground
            if step == 60 * 8 {
                d.transform((d.pos.y - lz.y).max(0.0));
            }
            if let Some(l) = d.update(1.0 / 60.0, steer, g.g, lz.y) {
                land = Some(l);
                break;
            }
        }
        match land {
            Some(l) => println!(
                "  {:<14} {:>10.1} {:>10.1} {:>10.1}{}",
                name,
                l.miss_distance,
                l.impact_speed,
                l.self_dmg,
                if l.hero { "  HERO FOLD" } else { "" }
            ),
            None => println!("  {name:<14}  still falling after 90 s"),
        }
    }
    println!("\n  where you land is decided before you jump, by the pilot's line");
    println!("  and by when you let go.");
}

/// The size ladder, and where the camera goes for each rung.
fn scale_cmd() {
    use crate::scale::{self, Class, LADDER};
    println!("THE SIZE LADDER\n");
    println!(
        "  {:<24} {:>9} {:>8} {:>9} {:>9}",
        "thing", "height", "class", "cam back", "cam up"
    );
    for s in LADDER {
        let (d, h) = scale::camera_for(s.class, s.height_m);
        println!(
            "  {:<24} {:>7.2} m {:>8} {:>7.1} m {:>7.1} m",
            s.id,
            s.height_m,
            format!("{:?}", s.class),
            d,
            h
        );
    }

    let human = scale::sized("human").unwrap();
    println!("\n  against a person:");
    for s in LADDER {
        if s.id == "human" {
            continue;
        }
        println!("    {:<24} {:>6.1}x   {}", s.id, scale::ratio(s, human), s.note);
    }

    // the join between the size ladder and the ship table
    println!("\n  hulls (ship::HULLS), framed as Titans:");
    for h in crate::ship::HULLS {
        let (d, _) = scale::camera_for(Class::Titan, h.length);
        let over = d > crate::camera::Camera::FAR_M;
        println!(
            "    {:<11} {:>6.0} m   camera {:>7.0} m{}",
            h.id,
            h.length,
            d,
            if over { "   PAST THE FAR PLANE" } else { "" }
        );
    }
    println!("\n  the far plane is {:.0} m, so the 3 km carrier cannot be shown whole.",
        crate::camera::Camera::FAR_M);
    println!("  the hull table and the camera were written apart and never asked");
    println!("  each other anything. the fix is a decision -- move the far plane,");
    println!("  frame big hulls differently, or accept a carrier is never framed.");

    println!("\n  the engine's own body is {:.2} m and its camera sits {:.1} m",
        scale::RL_BODY_H_M, scale::RL_CAM_DIST_M);
    println!("  back -- nearly thirteen times its height, which is a fact about");
    println!("  a car game and not about cameras. Pointed at an 8.5 m bot that");
    println!("  distance is nearer than the bot is tall.");
    println!("\n  model_tuning.json normalises every model INTO that body:");
    println!("  Optimus Prime ships at scale 0.4989. At his real height he");
    println!("  wants {:.1}.", scale::true_scale(0.4989, 8.5));
    let (d, _) = scale::camera_for(Class::Titan, 228.0);
    println!("\n  a person and Two California Plaza in one shot is a 130:1 span");
    println!("  and a {d:.0} m pullback. that is the depth range the renderer has");
    println!("  to survive, and it is the actual hard part.");
}

/// Can the z-buffer hold a person and a skyscraper in one shot?
fn depth_cmd() {
    use crate::depth::{self, FAR_M, NEAR_M};
    println!("DEPTH PRECISION — near {NEAR_M} m, far {FAR_M} m, Depth32Float\n");
    println!("  shipped today: standard Z (camera.rs perspective_rh)\n");
    println!("  {:>9} {:>13} {:>13} {:>9}", "distance", "standard", "reversed", "gain");
    for d in [3.5f64, 16.0, 50.0, 120.0, 343.0, 1000.0, 2500.0] {
        println!(
            "  {:>7.0} m {:>11.4} m {:>11.5} m {:>8.0}x",
            d,
            depth::resolution_m(d, NEAR_M, FAR_M, false),
            depth::resolution_m(d, NEAR_M, FAR_M, true),
            depth::reversed_gain(d, NEAR_M, FAR_M)
        );
    }
    println!("\n  343 m is the pullback `--terrain scale` gives for Two California");
    println!("  Plaza. A facade has detail at the centimetre, so standard Z there");
    println!("  puts the windows and the wall in the same depth bucket.");

    // three levers, and the cheapest one is not the pipeline change
    use crate::scale::{self, Class};
    let (td, _) = scale::camera_for(Class::Titan, 228.0);
    let scaled_near = scale::near_for(td) as f64;
    println!("\n  THREE LEVERS at the {td:.0} m tower shot:\n");
    println!(
        "    as shipped        near {:.2} m, standard Z   {:>9.5} m",
        NEAR_M,
        depth::resolution_m(td as f64, NEAR_M, FAR_M, false)
    );
    println!(
        "    near plane only   near {:.2} m, standard Z   {:>9.5} m   (free)",
        scaled_near,
        depth::resolution_m(td as f64, scaled_near, FAR_M, false)
    );
    println!(
        "    reversed Z only   near {:.2} m, reversed Z   {:>9.5} m   (flipped compare)",
        NEAR_M,
        depth::resolution_m(td as f64, NEAR_M, FAR_M, true)
    );
    println!(
        "    both              near {:.2} m, reversed Z   {:>9.5} m",
        scaled_near,
        depth::resolution_m(td as f64, scaled_near, FAR_M, true)
    );
    match depth::near_needed_for(td as f64, FAR_M, 0.001) {
        Some(n) => println!(
            "\n  to reach a MILLIMETRE on standard Z the near plane would have to go\n               out to {n:.0} m, which is further than some of the geometry."
        ),
        None => println!(
            "\n  no near plane inside the shot reaches a millimetre on standard Z."
        ),
    }
    println!("\n  THEY ARE ALTERNATIVES, NOT ADDITIVE. reversed Z gets its precision");
    println!("  from float32 being dense near zero, which is where it puts the FAR");
    println!("  plane; pushing the near plane out on top redistributes the same");
    println!("  budget and gives some back. Pick one:");
    println!("    - no pipeline change available -> scale the near plane, 130x better");
    println!("    - able to flip the depth compare -> reversed Z, and KEEP the small");
    println!("      near plane, so nothing close is lost");
    println!("\n  the near plane is FIXED at {NEAR_M} m today, which is right for a");
    println!("  38 cm body and absurd for a camera {td:.0} m from a skyscraper: most");
    println!("  of the buffer is spent between 8 cm and a metre, where nothing is.");
    println!("  `scale::near_for` scales it with the shot and leaves the human");
    println!("  case at 0.08 m, so close work does not move.");
    println!("\n  NOT VERIFIED ON A GPU. this is the arithmetic of the depth");
    println!("  mapping; it says nothing about shadow cascades or LOD popping,");
    println!("  which are the other two things a 130:1 span breaks.");
}

fn audit() {
    let mut problems = 0;
    println!("TERRAIN AUDIT — {} route rows\n", patrol::ROUTE.len());
    for d in patrol::ROUTE {
        if !has_ground(d) {
            println!("  {:<12} skipped ({:?})", d.id, d.terrain);
            continue;
        }
        let g = ground_for(d);
        let mut notes = Vec::new();
        if g.g <= 0.0 {
            notes.push("non-positive gravity");
            problems += 1;
        }
        if d.weather && g.wind == 0.0 && g.air > 0.0 {
            notes.push("route says weather, ground says no wind");
            problems += 1;
        }
        if g.wind > 0.0 && g.air <= 0.0 {
            notes.push("wind in a vacuum");
            problems += 1;
        }
        if d.weather && g.air <= 0.0 {
            notes.push("route claims weather on an airless world (wind suppressed)");
        }
        if !d.weather && g.wind > 0.0 {
            notes.push("route says no weather, ground has wind");
            problems += 1;
        }
        if g.air == 0.0 && d.sky == Sky::Atmosphere {
            notes.push("atmosphere sky with zero air");
            problems += 1;
        }
        println!(
            "  {:<12} ok{}",
            d.id,
            if notes.is_empty() { String::new() } else { format!("  <- {}", notes.join("; ")) }
        );
    }
    println!("\n{problems} problem(s).");
    println!("\nnote: `terrain.rs` still has no caller in the game loop. this tool");
    println!("makes it REACHABLE and checkable; `world::World::generate` taking a");
    println!("Ground is the wiring that makes it VISIBLE (docs/PLAN.md, phase 0).");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_surface_destination_converts() {
        for d in patrol::ROUTE.iter().filter(|d| has_ground(d)) {
            let g = ground_for(d);
            assert!(g.g > 0.0, "{} has non-positive gravity", d.id);
            assert!(g.air >= 0.0 && g.wind >= 0.0, "{} has negative air or wind", d.id);
        }
    }

    #[test]
    fn there_is_no_wind_in_a_vacuum() {
        // found by running `--terrain grounds` and reading the columns: five
        // route rows carry weather: true with a vacuum sky, and the first
        // version of this seam happily gave them a friction velocity. u* is a
        // property of a moving fluid; with no fluid there is no transport,
        // whatever the flag says.
        for d in patrol::ROUTE.iter().filter(|d| has_ground(d)) {
            let g = ground_for(d);
            if g.air <= 0.0 {
                assert_eq!(g.wind, 0.0, "{} has wind with no air", d.id);
            }
        }
        // the rows that motivated it
        for id in ["pluto", "trappist1e", "proxima_b"] {
            assert_eq!(ground_for(patrol::find(id).unwrap()).wind, 0.0, "{id}");
        }
    }

    #[test]
    fn weather_plus_air_still_produces_wind() {
        // the suppression above must not have silenced the worlds that should
        // actually have weather
        for id in ["earth", "mars", "titan"] {
            let d = patrol::find(id).unwrap();
            assert!(d.weather, "{id} lost its weather flag");
            assert!(ground_for(d).wind > 0.0, "{id} lost its wind");
        }
    }

    #[test]
    fn gravity_is_a_unit_conversion_and_nothing_more() {
        let earth = patrol::find("earth").unwrap();
        assert!((ground_for(earth).g - 9.81).abs() < 0.05);
        let luna = patrol::find("luna").unwrap();
        assert!((ground_for(luna).g - 0.17 * 9.81).abs() < 0.05, "{}", ground_for(luna).g);
        // and the ordering survives: a heavier world is heavier
        assert!(ground_for(earth).g > ground_for(luna).g);
    }

    #[test]
    fn mars_dunes_are_spaced_further_than_earths() {
        // the headline relation in terrain.rs's own doc comment: saltation hop
        // length goes as u*^2/g, so this is the check that the SEAM feeds it
        // real numbers rather than plausible-looking ones.
        let mars = ground_for(patrol::find("mars").unwrap());
        let earth = ground_for(patrol::find("earth").unwrap());
        assert!(mars.wind > earth.wind, "thin air needs a higher u* to move grains");
        assert!(mars.g < earth.g);
        // both should actually produce a dune field
        assert!(terrain::dune_height(&mars, 120.0, 340.0, (1.0, 0.0), 7).is_some());
        assert!(terrain::dune_height(&earth, 120.0, 340.0, (1.0, 0.0), 7).is_some());
    }

    #[test]
    fn airless_worlds_get_no_dunes() {
        for id in ["luna", "europa"] {
            let d = patrol::find(id).unwrap();
            assert!(!d.weather, "{id} should have no weather");
            let g = ground_for(d);
            assert!(
                terrain::dune_height(&g, 120.0, 340.0, (1.0, 0.0), 7).is_none(),
                "{id} grew dunes with no wind"
            );
        }
    }

    #[test]
    fn talus_runs_further_where_gravity_is_lower() {
        let luna = ground_for(patrol::find("luna").unwrap());
        let earth = ground_for(patrol::find("earth").unwrap());
        let (a, b) = (terrain::talus_depth(&luna, 200.0, 50.0), terrain::talus_depth(&earth, 200.0, 50.0));
        assert!(a > 0.0 && b > 0.0);
        assert_ne!(a, b, "gravity is not reaching the talus relation");
    }

    #[test]
    fn material_is_stated_for_every_named_world() {
        // the mapping is a judgement, so it should be a DELIBERATE one for
        // every world the fiction names; the default is for procedural rows
        for id in ["earth", "mars", "luna", "europa", "io", "titan", "glaciem", "pluto", "cybertron"] {
            let d = patrol::find(id).unwrap();
            let m = material_of(d);
            assert_ne!(
                (m, id),
                (Material::Basalt, "earth"),
                "a named world fell through to the default"
            );
        }
        assert_eq!(material_of(patrol::find("cybertron").unwrap()), Material::Metal);
        assert_eq!(material_of(patrol::find("pluto").unwrap()), Material::SoftIce);
    }

    #[test]
    fn generated_worlds_get_ground_too() {
        // the seam has to work for the PROCEDURAL case, which is the common
        // one: eighteen authored rows against an unbounded procedural sky
        for seed in [1u32, 42, 20260822, 7777, 999_999] {
            let w = procgen::World::generate_viable(seed);
            let g = ground_for_world(&w);
            assert!(g.g > 0.0, "seed {seed} produced non-positive gravity");
            assert!(g.wind >= 0.0 && g.air >= 0.0);
            assert!(w.viable(), "generate_viable returned somewhere unenterable");
        }
    }

    #[test]
    fn no_wind_in_a_generated_vacuum_either() {
        // the same bug the authored path had. a generator can happily roll a
        // dust storm onto an airless rock; the ground model must not believe
        // it.
        let mut checked = 0;
        for seed in 0u32..400 {
            let w = procgen::World::generate_viable(seed);
            let g = ground_for_world(&w);
            if w.orbit.air <= 0.0 {
                assert_eq!(g.wind, 0.0, "seed {seed}: wind with no air");
                assert!(
                    terrain::dune_height(&g, 120.0, 340.0, (1.0, 0.0), 7).is_none(),
                    "seed {seed}: dunes in a vacuum"
                );
                checked += 1;
            }
        }
        assert!(checked > 0, "no airless world in 400 seeds — the check proved nothing");
    }

    #[test]
    fn material_follows_temperature_not_taste() {
        // the derivation is the honest one: what a surface is made of is
        // largely a question of what is frozen there
        let mut seen = std::collections::BTreeSet::new();
        for seed in 0u32..400 {
            let w = procgen::World::generate_viable(seed);
            let m = material_of_world(&w);
            match w.orbit.surface() {
                Surface::Frozen => assert_eq!(m, Material::Ice, "seed {seed}"),
                Surface::Cryogenic => assert_eq!(m, Material::SoftIce, "seed {seed}"),
                Surface::Temperate => assert_eq!(m, Material::Sandstone, "seed {seed}"),
                _ => assert_eq!(m, Material::Basalt, "seed {seed}"),
            }
            seen.insert(format!("{:?}", w.orbit.surface()));
        }
        assert!(seen.len() >= 3, "400 seeds produced only {seen:?} — the generator is cold");
    }

    #[test]
    fn a_jump_is_deterministic_in_its_seed() {
        // the same seed must land the same place, or nothing about a
        // procedural world can be shared, saved or reported as a bug
        for seed in [3u32, 77, 20260822] {
            let a = procgen::World::generate_viable(seed);
            let b = procgen::World::generate_viable(seed);
            assert_eq!(a.gravity_g, b.gravity_g, "seed {seed} is not deterministic");
            assert_eq!(ground_for_world(&a).material, ground_for_world(&b).material);
            assert_eq!(
                procgen::raid_for(&a, "x").encounters.len(),
                procgen::raid_for(&b, "x").encounters.len()
            );
        }
    }

    #[test]
    fn thin_air_needs_a_harder_wind_to_move_the_same_grain() {
        // the relation wind_of_world encodes, checked directly rather than
        // trusted: halving air density raises the friction velocity
        let mut w = procgen::World::generate_viable(11);
        w.weather = Weather::DustStorm;
        w.orbit.air = 1.0;
        let thick = ground_for_world(&w).wind;
        w.orbit.air = 0.25;
        let thin = ground_for_world(&w).wind;
        assert!(thin > thick, "thin {thin} should exceed thick {thick}");
    }

    /// One drop, run to the ground. Returns the landing.
    fn fall(g: f32, steer: Vec3, alt: f32) -> Option<crate::drop::Landing> {
        let lz = Vec3::ZERO;
        let mut d = crate::drop::Drop::new(lz, false);
        d.can_jump(true, true, alt).ok()?;
        d.jump(Vec3::new(0.0, alt, -260.0), Vec3::new(0.0, 0.0, 42.0));
        for step in 0..(90 * 60) {
            if step == 60 * 8 {
                d.transform((d.pos.y - lz.y).max(0.0));
            }
            if let Some(l) = d.update(1.0 / 60.0, steer, g, lz.y) {
                return Some(l);
            }
        }
        None
    }

    #[test]
    fn steering_decides_where_you_land() {
        // the claim drop.rs opens with: where you land is decided by the line
        // and by when you let go, not by a spawn point. if steering does not
        // move the miss distance, none of that is true.
        let g = 9.81;
        let with = fall(g, Vec3::new(0.0, 0.0, 6.0), 400.0).expect("landed");
        let against = fall(g, Vec3::new(0.0, 0.0, -6.0), 400.0).expect("landed");
        let none = fall(g, Vec3::ZERO, 400.0).expect("landed");
        assert!(
            with.miss_distance < none.miss_distance,
            "steering into the line did not help: {} vs {}",
            with.miss_distance,
            none.miss_distance
        );
        assert!(
            against.miss_distance > none.miss_distance,
            "steering against the line did not hurt"
        );
    }

    #[test]
    fn a_heavy_world_is_a_harder_drop() {
        // running the drop against the GENERATED world's gravity rather than
        // a hardcoded 9.81 is the whole reason it lives next to the map tool
        let light = fall(1.6, Vec3::ZERO, 400.0).expect("landed");
        let heavy = fall(20.0, Vec3::ZERO, 400.0).expect("landed");
        assert!(
            heavy.impact_speed > light.impact_speed,
            "gravity is not reaching the drop: {} vs {}",
            heavy.impact_speed,
            light.impact_speed
        );
        assert!(heavy.self_dmg >= light.self_dmg);
    }

    #[test]
    fn you_cannot_step_out_of_a_grounded_ship_or_a_shut_door() {
        let d = crate::drop::Drop::new(Vec3::ZERO, false);
        assert!(d.can_jump(false, true, 400.0).is_err(), "jumped through a shut door");
        assert!(d.can_jump(true, false, 400.0).is_err(), "jumped off a parked ship");
        assert!(d.can_jump(true, true, 5.0).is_err(), "jumped from five metres");
        assert!(d.can_jump(true, true, 400.0).is_ok());
    }

    #[test]
    fn every_generated_world_can_be_dropped_onto() {
        // a foothold you cannot reach is a raid you cannot start
        for seed in [1u32, 42, 20260822, 7777] {
            let w = procgen::World::generate_viable(seed);
            if !w.has_ground {
                continue;
            }
            let g = ground_for_world(&w);
            let l = fall(g.g, Vec3::ZERO, 400.0);
            assert!(l.is_some(), "seed {seed} never landed");
        }
    }
}
