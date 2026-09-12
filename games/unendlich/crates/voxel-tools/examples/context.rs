// Does the prompt grow with the build?
use voxel_tools::{context, dossier};
fn main() {
    let (mut d, errs) = dossier::parse(
        "is a walled town with a market at its centre\n\
         part walls size=2000\n  A ring of stone wall with a gate to the south.\nend\n\
         part market size=800 after=walls\n  An open square of flagstones at the centre.\nend\n\
         part hall size=600 after=market\n  A timber hall on the north side of the square.\nend\n\
         part houses size=1400 after=market\n  Rows of small houses lining the streets.\nend\n\
         part well size=100 after=market\n  A stone well in the middle of the square.\nend\n\
         part temple size=500 after=walls\n  A small stone temple against the east wall.\nend\n\
         part docks size=700 after=walls\n  Timber quays outside the west gate.\nend\n\
         part gatehouse size=400 after=walls\n  A tower over the south gate.\nend\n");
    for e in &errs { println!("ERR {e}"); }
    println!("== dossier: {}", d.status());
    println!("\n   part                 dossier prompt   growing conversation");
    let mut chain = context::Chain::default();
    for i in 0..d.parts.len() {
        let p = d.part_prompt(i, "VOCABULARY (about 800 chars of shape language) ..............").unwrap();
        // what the same run would cost as one accumulating conversation
        chain.push(format!("build {}", d.parts[i].name), format!("{}\n(a program of about 40 lines)", d.parts[i].what), "built".to_string());
        let (folded, kept) = chain.pack(400, 200, 100_000);
        let convo = folded.map(|f| f.len()).unwrap_or(0)
            + kept.iter().map(|(a, b)| a.len() + b.len()).sum::<usize>();
        println!("   {:20} {:>10} ch   {:>10} ch", d.parts[i].name, p.len(), convo + p.len());
        d.finish(i, 900 + i * 30, "a stone mass");
    }
    println!("\n   {}", d.status());
}
