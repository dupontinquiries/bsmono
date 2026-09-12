// Proves a message goes chat -> provider -> reply -> plan -> blocks,
// against a REAL socket. A tiny server stands in for ollama and echoes
// a plan; everything else is the shipping code path.
use std::io::{Read, Write};
use std::net::TcpListener;

fn main() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let seen2 = seen.clone();

    std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let mut buf = [0u8; 65536];
        let n = sock.read(&mut buf).unwrap();
        let req = String::from_utf8_lossy(&buf[..n]).into_owned();
        *seen2.lock().unwrap() = req;
        // reply CHUNKED, the case that truncates naive clients
        let plan = r#"Sure!\n```\n# hut\ncylinder @0,0 r=3 h=5 block=oak_planks hollow\n```"#;
        let body = format!(r#"{{"response":"{plan}"}}"#);
        let mut out = String::from("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n");
        for c in body.as_bytes().chunks(17) {
            out.push_str(&format!("{:x}\r\n{}\r\n", c.len(), String::from_utf8_lossy(c)));
        }
        out.push_str("0\r\n\r\n");
        sock.write_all(out.as_bytes()).unwrap();
    });

    use voxel_tools::llm::{clean_reply, Completer, Dialect, HttpConfig, HttpProvider};
    let mut p = HttpProvider::new(HttpConfig {
        host: format!("{}:{}", addr.ip(), addr.port()),
        dialect: Dialect::Ollama, model: "test".into(), ..Default::default() });

    let system = voxel_tools::prompt::system_prompt();
    let raw = p.complete(&system, "FOCUS f1 | anchor (0, 65, 0)\n\nPASS: massing\nplayer: a hut\nplan:")
        .expect("provider answered");
    println!("1. provider replied: {} chars", raw.len());

    let req = seen.lock().unwrap().clone();
    let declared: usize = req.lines().find_map(|l| l.strip_prefix("Content-Length: "))
        .unwrap().trim().parse().unwrap();
    let sent_body = req.split_once("\r\n\r\n").unwrap().1;
    println!("2. request framing: Content-Length {} == body {} -> {}",
             declared, sent_body.len(), declared == sent_body.len());
    assert_eq!(declared, sent_body.len());
    assert!(req.contains("BUILD IN PASSES"), "the system prompt reached the server");
    assert!(req.contains("player: a hut"), "the user turn reached the server");
    println!("3. prompt arrived intact (system + user)");

    let ir = clean_reply(&raw);
    println!("4. cleaned to IR:\n{}", ir.trim());
    let (plan, issues) = voxel_tools::plan::parse_plan_at(&ir, Some((0, 65, 0)));
    assert!(issues.is_empty(), "{issues:?}");
    println!("5. parsed: '{}' with {} op(s)", plan.name, plan.ops.len());

    // and it builds
    use std::collections::BTreeMap;
    use voxel_tools::{VoxelTools, VoxelWorld};
    struct Flat(BTreeMap<(i32,i32,i32), u16>);
    impl VoxelWorld for Flat {
        fn get(&self, x: i32, y: i32, z: i32) -> u16 {
            if let Some(&b) = self.0.get(&(x,y,z)) { return b; }
            if y <= 64 { 1 } else { 0 }
        }
        fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.0.insert((x,y,z), b); true }
    }
    let mut w = Flat(BTreeMap::new());
    let run = { let mut t = VoxelTools::new(&mut w);
                voxel_tools::plan::run_plan(&mut t, &plan, 200, 32, false) };
    println!("6. built {} blocks; wall at (3,66,0) = {}",
             run.blocks_changed, w.get(3, 66, 0));
    assert!(run.blocks_changed > 0 && w.get(3, 66, 0) != 0);
    println!("\nmessage passed through end to end.");
}
