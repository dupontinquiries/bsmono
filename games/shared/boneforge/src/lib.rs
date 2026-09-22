pub mod glb;
pub mod island;
pub mod anatomy;
pub mod rig;
pub mod bot;
pub mod strip;
pub mod motion;
pub mod partpick;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anatomy::{Kind, Side};
    use crate::island::Island;
    use crate::rig::{along, solve, Bone, Pose};

    fn quad(c: [f32; 3], s: [f32; 3]) -> Island {
        let mut verts = Vec::new();
        for i in 0..8 {
            verts.push([c[0] + if i & 1 == 0 { -s[0] } else { s[0] },
                        c[1] + if i & 2 == 0 { -s[1] } else { s[1] },
                        c[2] + if i & 4 == 0 { -s[2] } else { s[2] }]);
        }
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for v in &verts { for a in 0..3 { lo[a] = lo[a].min(v[a]); hi[a] = hi[a].max(v[a]); } }
        Island { verts, tris: vec![[0, 1, 2], [1, 2, 3]], lo, hi }
    }

    #[test]
    fn along_bone_follows_the_rest_direction() {
        // The mistake that cost the most: `(0, t*len, 0)` is right for the
        // spine and backwards for every limb.
        let spine = Bone { name: "spine", parent: None, len: 100.0,
                           rest: [0.0, 1.0, 0.0], radius: 10.0 };
        let arm = Bone { name: "uparm_l", parent: None, len: 100.0,
                         rest: [0.0, -1.0, 0.0], radius: 10.0 };
        assert_eq!(along(&spine, 0.5)[1], 50.0);
        assert_eq!(along(&arm, 0.5)[1], -50.0, "a limb runs the other way");
    }

    #[test]
    fn bone_lengths_never_change() {
        let bones = bot::skeleton(400.0);
        let a = solve(&bones, &Pose::rest(bones.len()));
        let mut p = Pose::rest(bones.len());
        for ang in p.angles.iter_mut() { *ang = [31.0, -17.0, 44.0]; }
        let b = solve(&bones, &p);
        for (i, bone) in bones.iter().enumerate() {
            let l = |j: &rig::Joint| {
                let d = [j.tail[0]-j.head[0], j.tail[1]-j.head[1], j.tail[2]-j.head[2]];
                (d[0]*d[0]+d[1]*d[1]+d[2]*d[2]).sqrt()
            };
            assert!((l(&a[i]) - bone.len).abs() < 1e-3);
            assert!((l(&b[i]) - bone.len).abs() < 1e-3,
                    "{} changed length when posed", bone.name);
        }
    }

    #[test]
    fn root_roll_can_put_the_front_down() {
        // The root euler is ZYX, so after a lay-down in rz no rx can point the
        // body's front at the floor. `root_roll` is applied outermost.
        let bones = bot::skeleton(400.0);
        let mut p = Pose::rest(bones.len());
        p.root_rot = [0.0, 0.0, -90.0];
        let flat = solve(&bones, &p);
        p.root_roll = -90.0;
        let rolled = solve(&bones, &p);
        let ci = bot::bone_index(&bones, "chest").unwrap();
        // the chest's local -z, taken to world
        let front = |m: &rig::Mat3| [-m[0][2], -m[1][2], -m[2][2]];
        assert!(front(&flat[ci].mat)[1].abs() < 0.2, "without roll it faces sideways");
        assert!(front(&rolled[ci].mat)[1] < -0.8, "with roll it faces down");
    }

    #[test]
    fn islands_are_ordered_deterministically() {
        // Four near-identical wheels are the case that breaks a count-only
        // sort: ties fall to hash order and a cache disagrees with a recompute.
        let pos: Vec<[f32; 3]> = (0..4).flat_map(|k| {
            let x = k as f32 * 100.0;
            (0..3).map(move |j| [x + j as f32, 0.0, 0.0])
        }).collect();
        let idx: Vec<u32> = (0..4).flat_map(|k| {
            let b = k * 3;
            [b, b + 1, b + 2]
        }).collect();
        let a = island::split(&pos, &idx);
        let b = island::split(&pos, &idx);
        assert_eq!(a.len(), 4);
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.centre()[0], y.centre()[0], "order is not stable");
        }
        // and it is sorted, not incidental
        let xs: Vec<f32> = a.iter().map(|i| i.centre()[0]).collect();
        let mut s = xs.clone();
        s.sort_by(|p, q| p.partial_cmp(q).unwrap());
        assert_eq!(xs, s);
    }

    #[test]
    fn a_wheel_is_recognised_as_one() {
        // The fixture has to be UNAMBIGUOUS, not marginal. Its wheels were 21%
        // of the body length against a bound of 22%, so a legitimate tightening
        // of the wheel-size rule broke the test rather than the code.
        let mut isl = vec![
            quad([0.0, 20.0, 0.0], [200.0, 30.0, 70.0]),      // the body
        ];
        // four discs: round in two axes, thin in the third
        for (x, z) in [(130.0, -60.0), (130.0, 60.0), (-130.0, -60.0), (-130.0, 60.0)] {
            let mut w = quad([x, 30.0, z], [28.0, 28.0, 9.0]);
            // classify wants real vertex counts to tell a wheel from a greeble
            while w.verts.len() < 400 { let v = w.verts[0]; w.verts.push(v); }
            isl.push(w);
        }
        let body = anatomy::frame(&isl);
        let kinds: Vec<Kind> = isl.iter().map(|i| anatomy::classify(i, &body).kind).collect();
        assert_eq!(kinds.iter().filter(|k| **k == Kind::Wheel).count(), 4,
                   "four discs at the corners are four wheels");
    }

    #[test]
    fn a_wheel_assembly_counts_as_one_corner() {
        // A modelled wheel is a tyre, a rim, a disc, a hub and bolts -- every
        // one round and thin, so every one classifies as a wheel alone. That
        // gave 25 wheels on a four-wheeled car.
        let mut isl = vec![quad([0.0, 25.0, 0.0], [200.0, 25.0, 80.0])];
        for (x, z) in [(120.0, -70.0), (120.0, 70.0), (-120.0, -70.0), (-120.0, 70.0)] {
            // tyre, rim and disc at the same corner, slightly apart
            for (k, r) in [(0.0, 30.0), (4.0, 22.0), (7.0, 15.0)] {
                let mut w = quad([x, 30.0, z + k], [r, r, 9.0]);
                while w.verts.len() < 420 { let v = w.verts[0]; w.verts.push(v); }
                isl.push(w);
            }
        }
        let body = anatomy::frame(&isl);
        let parts: Vec<_> = isl.iter().map(|i| anatomy::classify(i, &body)).collect();
        let c = anatomy::corners(&isl, &parts, &body);
        assert_eq!(c.len(), 4, "twelve wheel parts are four corners, not twelve");
        for g in &c {
            assert!(g.len() >= 2, "a corner should gather its own fragments");
        }
    }

    #[test]
    fn a_round_thing_on_the_centreline_is_not_a_wheel() {
        // A fan, a spare, a steering wheel: round, wheel-sized, not a wheel.
        let mut isl = vec![quad([0.0, 25.0, 0.0], [200.0, 25.0, 80.0])];
        let mut fan = quad([0.0, 40.0, 0.0], [28.0, 28.0, 8.0]);
        while fan.verts.len() < 500 { let v = fan.verts[0]; fan.verts.push(v); }
        isl.push(fan);
        let body = anatomy::frame(&isl);
        let parts: Vec<_> = isl.iter().map(|i| anatomy::classify(i, &body)).collect();
        assert!(anatomy::corners(&isl, &parts, &body).is_empty(),
                "nothing on the centreline is a road wheel");
    }

    #[test]
    fn corner_clustering_does_not_depend_on_the_classifier_being_strict() {
        // Tightening what counts as a wheel once shrank the cluster reach --
        // which was derived from the surviving parts -- and split corners
        // apart, so the model got MORE corners by getting more correct. The
        // reach comes from the vehicle now, so this cannot recur.
        let mut isl = vec![quad([0.0, 25.0, 0.0], [200.0, 25.0, 80.0])];
        for (x, z) in [(120.0, -70.0), (120.0, 70.0), (-120.0, -70.0), (-120.0, 70.0)] {
            let mut w = quad([x, 30.0, z], [30.0, 30.0, 9.0]);
            while w.verts.len() < 420 { let v = w.verts[0]; w.verts.push(v); }
            isl.push(w);
        }
        let body = anatomy::frame(&isl);
        let full: Vec<_> = isl.iter().map(|i| anatomy::classify(i, &body)).collect();
        let n_full = anatomy::corners(&isl, &full, &body).len();
        // now pretend the classifier missed one fragment per corner
        let mut sparse = full.clone();
        sparse[1].kind = anatomy::Kind::Greeble;
        let n_sparse = anatomy::corners(&isl, &sparse, &body).len();
        assert!(n_sparse <= n_full,
                "recognising fewer parts must not produce more corners");
    }

    #[test]
    fn lateral_is_the_mirror_axis_wherever_the_model_sits() {
        // Two rules failed before this one: "the low end nearest zero" is a
        // fact about one exporter, and "the sparsest bottom" loses to a
        // lateral axis because those are sparse at both edges. Everything
        // that drives or walks is bilaterally symmetric and nothing is
        // symmetric top-to-bottom.
        //
        // Built centred on the origin, which is what broke the first rule,
        // and with the vertical deliberately lopsided: feet below, mass above.
        let mut isl = Vec::new();
        isl.push(quad([0.0, 40.0, 0.0], [160.0, 30.0, 50.0]));   // body, high
        for x in [-80.0, 80.0] {
            for z in [-35.0, 35.0] {
                isl.push(quad([x, -30.0, z], [8.0, 8.0, 8.0]));   // feet, low
            }
        }
        let body = anatomy::frame(&isl);
        assert_eq!(body.long_axis, 0, "length is x");
        assert_eq!(body.lat_axis, 2, "z mirrors; y does not, because feet are not a roof");
    }

    #[test]
    fn sides_come_from_position_not_from_names() {
        // Fixtures need a real vertical ASYMMETRY. A box is symmetric in every
        // axis, so a rule that finds left-right by mirror symmetry cannot
        // choose between two of them -- and every fixture here was a box
        // built under the old "low end nearest zero" rule. Real vehicles have
        // wheels at the bottom and a roof at the top.
        let mut isl = vec![
            quad([0.0, 40.0, 0.0], [120.0, 30.0, 60.0]),
            quad([0.0, 40.0, -50.0], [20.0, 10.0, 6.0]),
            quad([0.0, 40.0, 50.0], [20.0, 10.0, 6.0]),
        ];
        for x in [-80.0, 80.0] { for z in [-50.0, 50.0] {
            isl.push(quad([x, 0.0, z], [14.0, 14.0, 6.0]));
        }}
        let body = anatomy::frame(&isl);
        assert_eq!(anatomy::classify(&isl[1], &body).side, Side::L);
        assert_eq!(anatomy::classify(&isl[2], &body).side, Side::R);
    }

    #[test]
    fn plates_are_wider_than_they_are_long() {
        // A plate whose long axis runs UP the limb reads as a slat. This is
        // the one proportion that must hold on every bone.
        let bones = bot::skeleton(400.0);
        for (i, b) in bones.iter().enumerate() {
            for p in bot::plates(i, b, 0.0, 0.2, 0.6) {
                assert!(p.size[0] >= p.size[1] * 0.98,
                        "{}: plate {:.1} wide x {:.1} long", b.name, p.size[0], p.size[1]);
            }
        }
    }

    #[test]
    fn plates_seat_on_the_limb_rather_than_inside_it() {
        let bones = bot::skeleton(400.0);
        for (i, b) in bones.iter().enumerate() {
            for p in bot::plates(i, b, 0.0, 0.2, 0.6) {
                // distance from the bone's axis
                let d = b.rest;
                let t = p.pos[0]*d[0] + p.pos[1]*d[1] + p.pos[2]*d[2];
                let perp = [p.pos[0]-d[0]*t, p.pos[1]-d[1]*t, p.pos[2]-d[2]*t];
                let r = (perp[0].powi(2)+perp[1].powi(2)+perp[2].powi(2)).sqrt();
                assert!(r + p.size[2] >= b.radius * 0.94,
                        "{} plate is buried in the limb", b.name);
                assert!(r - p.size[2] <= b.radius * 1.75,
                        "{} plate stands off the limb", b.name);
            }
        }
    }

    #[test]
    fn neighbouring_plates_land_on_different_shells() {
        // Coplanar overlapping plates fight in the depth test. Adjacent ones
        // must differ in shell so the bias can separate them.
        let bones = bot::skeleton(400.0);
        let i = bot::bone_index(&bones, "thigh_l").unwrap();
        let ps = bot::plates(i, &bones[i], 0.0, 0.2, 0.6);
        let (rows, _, _) = (5usize, 0, 0);
        for w in ps.windows(2) {
            if w[0].bone == w[1].bone {
                // consecutive plates are neighbours around a row
                assert_ne!(w[0].shell, w[1].shell, "adjacent plates share a shell");
            }
        }
        let _ = rows;
    }

    #[test]
    fn the_wheels_tell_you_what_a_unit_is() {
        // One vehicle in the library is modelled in metres and the rest in
        // centimetres, and nothing in the geometry says which. A road wheel
        // is 0.65 m across whatever the file says, so it is the one physical
        // fact available -- and the same measurement fixes the engine's size
        // normalisation, which otherwise makes every bot identical.
        let make = |k: f32| {
            let mut isl = vec![quad([0.0, 70.0*k, 0.0], [200.0*k, 25.0*k, 80.0*k])];
            for (x, z) in [(130.0, -60.0), (130.0, 60.0), (-130.0, -60.0), (-130.0, 60.0)] {
                let mut w = quad([x*k, 28.0*k, z*k], [28.0*k, 28.0*k, 9.0*k]);
                while w.verts.len() < 420 { let v = w.verts[0]; w.verts.push(v); }
                isl.push(w);
            }
            bot::build(&isl)
        };
        let cm = make(1.0);
        let m = make(0.01);           // the same vehicle, modelled in metres
        let (a, b) = (cm.height_m().unwrap(), m.height_m().unwrap());
        assert!((a - b).abs() / a < 0.02,
                "the same vehicle in different units must give the same real height: {a} vs {b}");
        assert!((2.0..8.0).contains(&a), "a car-sized bot is metres tall, got {a}");
        assert!((cm.chassis_scale() - m.chassis_scale()).abs() < 0.05,
                "and therefore the same chassis scale");
    }

    #[test]
    fn no_wheels_means_no_invented_scale() {
        // A vehicle with nothing recognisable gives no reference. Guessing one
        // from the bounding box would be inventing a fact.
        let isl = vec![quad([0.0, 25.0, 0.0], [200.0, 25.0, 80.0])];
        let b = bot::build(&isl);
        assert!(b.units_per_m.is_none());
        assert_eq!(b.chassis_scale(), 1.0);
    }

    #[test]
    fn a_strip_satisfies_the_loaders_contract() {
        // blackstar-rs discovers a chassis by `p.is_dir() &&
        // p.join("scene.gltf").is_file()`, sorts the pose names, and takes
        // the first as the vehicle and the last as the robot. Everything here
        // is that contract, checked rather than remembered.
        let isl = vec![
            quad([0.0, 25.0, 0.0], [200.0, 25.0, 80.0]),
            quad([130.0, 30.0, -60.0], [28.0, 28.0, 9.0]),
            quad([130.0, 30.0, 60.0], [28.0, 28.0, 9.0]),
        ];
        let b = bot::build(&isl);
        let poses = bot::strip_poses(&b.bones, b.height, 5);
        assert!(poses.len() >= 2, "the loader needs two or more poses");

        let dir = std::env::temp_dir().join("boneforge_strip_test");
        let _ = std::fs::remove_dir_all(&dir);
        strip::write_chassis(&dir, "bf_test", &b, &poses, "test").unwrap();

        let c = dir.join("bf_test");
        assert!(c.join("chassis.json").is_file());
        let mut names: Vec<String> = std::fs::read_dir(&c).unwrap().flatten()
            .filter(|e| e.path().is_dir() && e.path().join("scene.gltf").is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names.len(), poses.len());
        assert_eq!(names[0], "f00", "sorting has to put the vehicle first");

        // the buffer the gltf names must exist and be the length it claims
        for n in &names {
            let g = std::fs::read_to_string(c.join(n).join("scene.gltf")).unwrap();
            let bin = std::fs::metadata(c.join(n).join("scene.bin")).unwrap().len();
            let claim: u64 = g.rsplit("\"byteLength\":").next().unwrap()
                .split(',').next().unwrap().trim_matches(|ch: char| !ch.is_ascii_digit())
                .parse().unwrap();
            assert_eq!(bin, claim, "{n}: scene.bin is not the length the gltf states");
            assert!(g.contains("\"min\":"), "{n}: POSITION needs min/max");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_mesh_is_closed_and_wound_outward() {
        // An SDF bake cares about two things this can get wrong. An OPEN mesh
        // leaks. An INVERTED one fills the world instead of the model -- and
        // a closed mesh with the wrong winding passes every structural check
        // there is, which is exactly how it shipped: every box was
        // inside-out, signed volume -1,865,069.
        use std::collections::HashMap;
        let isl = vec![quad([0.0, 25.0, 0.0], [200.0, 25.0, 80.0])];
        let b = bot::build(&isl);
        let poses = bot::strip_poses(&b.bones, b.height, 3);
        for (k, p) in poses.iter().enumerate() {
            let f = strip::frame(&b, p);
            let mut e: HashMap<(u32, u32), i32> = HashMap::new();
            for t in f.idx.chunks_exact(3) {
                for (a, c) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                    *e.entry((a.min(c), a.max(c))).or_insert(0) += 1;
                }
            }
            let open = e.values().filter(|v| **v != 2).count();
            assert_eq!(open, 0, "pose {k}: {open} open edges, an SDF bake would leak");

            let mut vol = 0.0f64;
            for t in f.idx.chunks_exact(3) {
                let (a, c, d) = (f.pos[t[0] as usize], f.pos[t[1] as usize],
                                 f.pos[t[2] as usize]);
                vol += (a[0] as f64 * (c[1] as f64 * d[2] as f64 - c[2] as f64 * d[1] as f64)
                      - a[1] as f64 * (c[0] as f64 * d[2] as f64 - c[2] as f64 * d[0] as f64)
                      + a[2] as f64 * (c[0] as f64 * d[1] as f64 - c[1] as f64 * d[0] as f64)) / 6.0;
            }
            assert!(vol > 0.0, "pose {k}: signed volume {vol:.0}, the hull is inside-out");
        }
    }

    #[test]
    fn the_strip_runs_vehicle_to_robot() {
        // The first pose has to be flat and the last upright, because the
        // engine names them that way and nothing checks.
        let isl = vec![quad([0.0, 25.0, 0.0], [200.0, 25.0, 80.0])];
        let b = bot::build(&isl);
        let poses = bot::strip_poses(&b.bones, b.height, 5);
        let extent = |p: &rig::Pose| {
            let js = rig::solve(&b.bones, p);
            let mut lo = [f32::MAX; 3];
            let mut hi = [f32::MIN; 3];
            for j in &js { for a in 0..3 {
                lo[a] = lo[a].min(j.head[a]).min(j.tail[a]);
                hi[a] = hi[a].max(j.head[a]).max(j.tail[a]);
            }}
            [hi[0]-lo[0], hi[1]-lo[1], hi[2]-lo[2]]
        };
        let v = extent(&poses[0]);
        let r = extent(&poses[poses.len()-1]);
        assert!(v[0] > v[1] * 2.0, "the vehicle form should be long and flat");
        assert!(r[1] > r[0] * 1.5, "the robot form should be tall");
    }

    #[test]
    fn the_getup_never_tips_back() {
        // Mixing the sign of the root tilt once rotated the body 176 degrees
        // through vertical between two frames: a front somersault performed
        // from inside a car.
        let b = bot::skeleton(400.0);
        let keys = bot::getup(&b, 400.0);
        let tilts: Vec<f32> = keys.iter().map(|p| p.root_rot[2]).collect();
        for w in tilts.windows(2) {
            assert!(w[1] >= w[0] - 1e-3,
                    "tilt went {} -> {}, which is a somersault", w[0], w[1]);
        }
        assert!(tilts[0] < -80.0 && tilts[tilts.len()-1].abs() < 1.0);
    }

    #[test]
    fn a_robot_form_specialises_away_from_what_the_vehicle_does() {
        // The vehicle owns speed. A fighting robot spends that leg mass on
        // reach and bracing instead -- and has to buy stability back with its
        // FEET, because moving mass into the chest and arms raises the centre
        // of mass, it does not lower it.
        use bot::{measure, Role};
        let g = measure(Role::Generic);
        let b = measure(Role::Brawler);
        let w = measure(Role::Gunner);
        assert!(b.legs < g.legs * 0.7, "a brawler has short legs");
        assert!(b.arm_to_leg > 1.5, "and arms longer than them");
        assert!(w.brace > g.brace * 1.5, "a gunner braces against recoil");
        assert!((b.com - g.com).abs() < 0.05,
                "short legs do NOT buy a low centre of mass once the mass goes upstairs");
        for r in [Role::Generic, Role::Brawler, Role::Gunner] {
            let bones = bot::skeleton_for(400.0, r);
            assert!(bones.iter().all(|x| x.len >= 0.0 && x.radius >= 0.0));
        }
    }

    #[test]
    fn a_vehicle_becomes_a_bot_of_the_right_height() {
        let isl = vec![
            quad([0.0, 25.0, 0.0], [200.0, 25.0, 80.0]),
            quad([120.0, 25.0, -70.0], [28.0, 28.0, 10.0]),
            quad([120.0, 25.0, 70.0], [28.0, 28.0, 10.0]),
        ];
        let bot = bot::build(&isl);
        let car_len = bot.body.size()[bot.body.long_axis];
        assert!((bot.height / car_len - bot::STAND_FRACTION).abs() < 1e-3);
        assert!(!bot.plates.is_empty(), "a bot with no armour is a skeleton");
        assert_eq!(bot.carried.len(), isl.len(), "every part goes somewhere");
    }

    #[test]
    fn mirrored_bones_are_clad_alike() {
        let isl = vec![
            quad([0.0, 25.0, 0.0], [200.0, 25.0, 80.0]),
            // a panel on one side only -- the vehicle is asymmetric
            quad([-60.0, 30.0, -60.0], [70.0, 20.0, 8.0]),
        ];
        let bot = bot::build(&isl);
        let count = |n: &str| {
            let i = bot::bone_index(&bot.bones, n).unwrap();
            bot.plates.iter().filter(|p| p.bone == i).count()
        };
        for stem in ["thigh", "shin", "uparm", "forearm", "clav"] {
            assert_eq!(count(&format!("{stem}_l")), count(&format!("{stem}_r")),
                       "{stem}: a machine has to be symmetric even when its donor is not");
        }
    }
}
