fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match scribe::cli::run(&args) {
        Ok(out) => println!("{out}"),
        Err(e) => {
            eprintln!("scribe: {e}");
            std::process::exit(1);
        }
    }
}
