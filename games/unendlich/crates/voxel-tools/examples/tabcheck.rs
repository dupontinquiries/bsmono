// Replays the keystrokes a player actually makes, against the real
// model names from a scanned directory.
fn main() {
    use voxel_tools::chatbox::{ChatBox, Key};
    use voxel_tools::model::ModelRegistry;
    let dir = std::env::args().nth(1).map(std::path::PathBuf::from)
        .unwrap_or_else(voxel_tools::model::default_models_dir);
    let reg = ModelRegistry::scan(dir);
    let mut b = ChatBox::new();
    b.set_dynamic_args("/model", reg.models.iter().map(|m| m.name.clone()).collect());
    println!("{} models loaded into the box", reg.models.len());

    for seq in [&["/model"][..], &["/model "][..], &["/model l"][..], &["/model 1b"][..]] {
        let mut b2 = ChatBox::new();
        b2.set_dynamic_args("/model", reg.models.iter().map(|m| m.name.clone()).collect());
        b2.key(Key::Enter);
        for c in seq[0].chars() { b2.key(Key::Char(c)); }
        let before = b2.buffer.clone();
        b2.key(Key::Tab);
        let after1 = b2.buffer.clone();
        b2.key(Key::Tab);
        println!("typed {before:22?}  Tab-> {after1:46?}  Tab-> {:?}", b2.buffer);
    }
}
