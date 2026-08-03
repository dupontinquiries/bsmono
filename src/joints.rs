//! THE RIG'S JOINTS, as the viewer sees them.
//!
//! skydribble has never known about joints — they are fitted in foldview and
//! were only ever visible as a fold that looked right or did not. That is
//! not enough to talk about: "the arm comes off" and "joint 11 is parented
//! to the wrong side of the torso" are different conversations, and only the
//! second one can be fixed.
//!
//! So `foldview --joints-json` writes them beside the poses and this reads
//! them back. Deliberately a tiny hand-rolled parser rather than a serde
//! dependency: the file is written by a tool in this repo, the shape is
//! fixed, and adding a derive crate to read six fields would cost more than
//! it saves.
//!
//! Only joints that own geometry are in the file. The other 60-80% of the
//! tree has nothing to point at.

use glam::Vec3;

#[derive(Clone, Debug)]
pub struct Joint {
    /// index in the fitted rig — the number we both say out loud
    pub i: usize,
    /// -1 for a root
    pub parent: i64,
    /// how many parts hang off it
    pub parts: usize,
    pub pivot: Vec3,
    pub axis: Vec3,
    pub angle: f32,
    /// part-name prefixes, up to four, so a number has a word beside it
    pub names: Vec<String>,
    /// connects two things that do not touch
    pub misparented: bool,
    /// the parts are interior structure — a gyro in a hull, a rib inside a
    /// shell. Those are MEANT not to touch, so a large gap is the correct
    /// answer rather than a fault, and only the exterior ones are real.
    pub interior: bool,
    /// the joint it is actually adjacent to, when misparented
    pub should_parent: Option<usize>,
    /// gap to the declared parent, in units of the smaller joined part
    pub gap_now: f32,
    /// gap to the better candidate
    pub gap_best: f32,
}

impl Joint {
    /// One line for the readout.
    pub fn summary(&self) -> String {
        let who = if self.names.is_empty() {
            String::new()
        } else {
            format!(" {}", self.names.join("/"))
        };
        if self.misparented && self.interior {
            return format!(
                "J{} <- {}{}  interior, floats by design (nearest J{})",
                self.i,
                self.parent,
                who,
                self.should_parent.unwrap_or(0)
            );
        }
        if self.misparented {
            format!(
                "J{} <- {}{}  MISPARENTED: touches J{} ({:.1}x vs {:.1}x)",
                self.i,
                self.parent,
                who,
                self.should_parent.unwrap_or(0),
                self.gap_now,
                self.gap_best
            )
        } else {
            format!("J{} <- {}{}  {} parts", self.i, self.parent, who, self.parts)
        }
    }
}

fn num(s: &str, key: &str) -> Option<f32> {
    let i = s.find(&format!("\"{key}\":"))? + key.len() + 3;
    let rest = &s[i..];
    let end = rest.find([',', '}']).unwrap_or(rest.len());
    rest[..end].trim().parse().ok()
}

fn vec3(s: &str, key: &str) -> Option<Vec3> {
    let i = s.find(&format!("\"{key}\":["))? + key.len() + 4;
    let rest = &s[i..];
    let end = rest.find(']')?;
    let v: Vec<f32> = rest[..end].split(',').filter_map(|t| t.trim().parse().ok()).collect();
    (v.len() == 3).then(|| Vec3::new(v[0], v[1], v[2]))
}

/// Read a `joints.json` written by `foldview --joints-json`.
///
/// Returns an empty list rather than failing: a chassis with no joint file
/// is a chassis you cannot inspect, which is a smaller problem than a
/// viewer that will not open.
pub fn load(path: &std::path::Path) -> Vec<Joint> {
    let Ok(txt) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for chunk in txt.split("{\"i\":").skip(1) {
        let body = format!("{{\"i\":{chunk}");
        let Some(i) = num(&body, "i") else { continue };
        let names: Vec<String> = body
            .find("\"names\":[")
            .and_then(|p| {
                let rest = &body[p + 9..];
                rest.find(']').map(|e| {
                    rest[..e]
                        .split(',')
                        .map(|t| t.trim().trim_matches('"').to_string())
                        .filter(|t| !t.is_empty())
                        .collect()
                })
            })
            .unwrap_or_default();
        out.push(Joint {
            i: i as usize,
            parent: num(&body, "parent").unwrap_or(-1.0) as i64,
            parts: num(&body, "parts").unwrap_or(0.0) as usize,
            pivot: vec3(&body, "pivot").unwrap_or(Vec3::ZERO),
            axis: vec3(&body, "axis").unwrap_or(Vec3::Y),
            angle: num(&body, "angle").unwrap_or(0.0),
            names,
            misparented: body.contains("\"misparented\":true"),
            interior: body.contains("\"interior\":true"),
            should_parent: num(&body, "should_parent").map(|v| v as usize),
            gap_now: num(&body, "gap_now").unwrap_or(0.0),
            gap_best: num(&body, "gap_best").unwrap_or(0.0),
        });
    }
    out
}

/// Step through a joint list, optionally stopping only at the broken ones.
///
/// Cycling every joint on a 306-joint rig is not inspection, it is scrolling.
/// The faults are what we are looking at, so the viewer can filter to them
/// and still walk the whole set when you want the context.
pub fn step(joints: &[Joint], cur: usize, d: i32, only_bad: bool) -> usize {
    if joints.is_empty() {
        return 0;
    }
    let n = joints.len() as i32;
    let mut k = cur as i32;
    for _ in 0..n {
        k = (k + d).rem_euclid(n);
        // "bad" means a genuine fault: an EXTERIOR part that has come away.
        // Interior components are supposed to float, so stopping at them
        // turns a fault list into a parts list.
        if !only_bad || (joints[k as usize].misparented && !joints[k as usize].interior) {
            return k as usize;
        }
    }
    cur.min(joints.len() - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
 "joints": [
  {"i":0,"parent":-1,"parts":7,"pivot":[0.0,-0.26,-0.2],"axis":[1.0,0.0,0.0],"angle":-1.5,"names":["guts", "guts"],"misparented":false},
  {"i":1,"parent":0,"parts":2,"pivot":[-0.13,-0.04,-0.28],"axis":[1.0,0.0,0.0],"angle":0.06,"names":["muscle"],"misparented":true,"interior":false,"should_parent":2,"gap_now":1.10,"gap_best":0.25},
  {"i":3,"parent":0,"parts":1,"pivot":[0.0,0.0,0.0],"axis":[0.0,1.0,0.0],"angle":0.0,"names":["gyro"],"misparented":true,"interior":true,"should_parent":2,"gap_now":2.40,"gap_best":0.90},
  {"i":2,"parent":1,"parts":2,"pivot":[0.004,-0.04,-0.005],"axis":[1.0,0.0,0.0],"angle":0.05,"names":["core", "clad"],"misparented":false}
 ]
}"#;

    fn parsed() -> Vec<Joint> {
        let p = std::env::temp_dir().join("joints_test.json");
        std::fs::write(&p, SAMPLE).unwrap();
        load(&p)
    }

    #[test]
    fn it_reads_what_foldview_writes() {
        let j = parsed();
        assert_eq!(j.len(), 4);
        assert_eq!(j[0].i, 0);
        assert_eq!(j[0].parent, -1, "a root should report -1");
        assert_eq!(j[0].parts, 7);
        assert!((j[0].pivot.y + 0.26).abs() < 1e-4);
        assert_eq!(j[0].names, vec!["guts", "guts"]);
        assert!(!j[0].misparented);
    }

    #[test]
    fn a_misparented_joint_carries_its_whole_diagnosis() {
        // the point of the file: a number you can say out loud AND the
        // reason it is wrong
        let j = &parsed()[1];
        assert!(j.misparented);
        assert_eq!(j.should_parent, Some(2));
        assert!((j.gap_now - 1.10).abs() < 1e-4);
        assert!((j.gap_best - 0.25).abs() < 1e-4);
        assert!(j.summary().contains("MISPARENTED"));
        assert!(j.summary().contains("J1"));
        assert!(j.summary().contains("muscle"));
    }

    #[test]
    fn an_interior_part_says_it_floats_by_design() {
        let j = &parsed()[2];
        assert!(j.misparented && j.interior);
        let s = j.summary();
        assert!(s.contains("floats by design"), "{s}");
        assert!(!s.contains("MISPARENTED"), "an interior part was called a fault");
    }

    #[test]
    fn a_healthy_joint_reads_plainly() {
        let s = parsed()[3].summary();
        assert!(!s.contains("MISPARENTED"));
        assert!(s.contains("core/clad"));
    }

    #[test]
    fn a_missing_file_is_an_empty_list_not_a_failure() {
        // a chassis you cannot inspect beats a viewer that will not open
        assert!(load(std::path::Path::new("/nonexistent/joints.json")).is_empty());
        assert_eq!(step(&[], 0, 1, false), 0);
    }

    #[test]
    fn stepping_wraps_and_can_visit_only_the_broken_ones() {
        let j = parsed();
        assert_eq!(step(&j, 0, 1, false), 1);
        assert_eq!(step(&j, 3, 1, false), 0, "it did not wrap");
        assert_eq!(step(&j, 0, -1, false), 3);
        // filtered: only joint 1 is broken, so it is the only stop
        assert_eq!(step(&j, 0, 1, true), 1);
        assert_eq!(step(&j, 1, 1, true), 1, "filtered stepping left the only fault");
        assert_eq!(step(&j, 2, -1, true), 1);
        // the interior gyro is flagged but must NOT be a stop: it floats by
        // design, and stopping at it turns a fault list into a parts list
        assert_eq!(step(&j, 1, 1, true), 1, "filtered stepping stopped at an interior part");
    }
}
