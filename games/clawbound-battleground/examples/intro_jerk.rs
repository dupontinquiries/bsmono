// How smooth is the intro camera? Per-frame turn of the view direction and
// per-frame eye travel, over the whole intro, and where the worst ones are.
use clawbound::intro::{Intro, Shot};
use clawbound::consts::Team;
use glam::Vec3;
fn main() {
    for m in [clawbound::map::MapId::Jungle, clawbound::map::MapId::Pass] {
        clawbound::map::set_active(m);
        let n = m.team_size();
        let teams: Vec<Team> = (0..2 * n).map(|i| if i < n { Team::Blue } else { Team::Red }).collect();
        let p0 = Team::Blue.fountain();
        let play = Shot { eye: p0 + Vec3::new(0.0, 6.5, -9.0), target: p0 + Vec3::Y * 1.5, fov_y: 0.95 };
        let mut it = Intro::new(&teams, p0, play);
        let dt = 1.0 / 60.0;
        let mut prev = it.frame();
        let mut worst_turn = (0.0f32, 0.0f32, String::new());
        let mut worst_jump = (0.0f32, 0.0f32, String::new());
        while !it.done {
            it.advance(dt);
            let f = it.frame();
            let d0 = (prev.shot.target - prev.shot.eye).normalize();
            let d1 = (f.shot.target - f.shot.eye).normalize();
            let turn = d0.dot(d1).clamp(-1.0, 1.0).acos().to_degrees();
            let jump = (f.shot.eye - prev.shot.eye).length();
            if turn > worst_turn.0 { worst_turn = (turn, it.t, format!("{:?}", f.beat)); }
            if jump > worst_jump.0 { worst_jump = (jump, it.t, format!("{:?}", f.beat)); }
            prev = f;
        }
        println!("{m:?}: worst turn {:.1} deg/frame at t={:.2} ({}); worst eye jump {:.1} u/frame at t={:.2} ({})", worst_turn.0, worst_turn.1, worst_turn.2, worst_jump.0, worst_jump.1, worst_jump.2);
    }
}
