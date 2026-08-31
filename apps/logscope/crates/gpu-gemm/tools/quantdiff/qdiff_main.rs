use std::io::Read;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (ty, n) = (a[1].clone(), a[2].parse::<usize>().unwrap());
    let mut raw = Vec::new();
    std::io::stdin().read_to_end(&mut raw).unwrap();
    let id: u32 = match ty.as_str() {
        "q2_k" => 10, "q3_k" => 11, "q4_k" => 12, "q5_k" => 13, "q6_k" => 14,
        "iq2_xxs" => 16, "iq2_xs" => 17, "iq3_xxs" => 18, "iq1_s" => 19,
        "iq4_nl" => 20, "iq3_s" => 21, "iq2_s" => 22, "iq4_xs" => 23, "iq1_m" => 29,
        _ => { eprintln!("unknown {ty}"); std::process::exit(2); }
    };
    match gpu_kernels::gguf::dequantize(id, &raw, n) {
        Ok(v) => for x in v { println!("{:.9}", x); },
        Err(e) => { eprintln!("ERR {e}"); std::process::exit(1); }
    }
}
