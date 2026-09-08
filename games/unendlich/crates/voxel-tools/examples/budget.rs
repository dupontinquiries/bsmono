use voxel_tools::prompt::{Job, Tier};
fn main() {
    for t in [Tier::Tiny, Tier::Compact, Tier::Medium, Tier::Full] {
        println!("{t:?}: line {} answer {} plan {} program {}",
            Job::Line.tokens(t), Job::Answer.tokens(t), Job::Plan.tokens(t), Job::Program.tokens(t));
    }
}
