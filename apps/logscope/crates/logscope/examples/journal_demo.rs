fn main() {
    let path = std::env::args().nth(1).expect("usage: journal_demo <journal.log>");
    let text = std::fs::read_to_string(&path).expect("read");
    let mut r = logscope::diagreport::Report::new();
    r.absorb_journal(&text);
    print!("{}", r.render());
}
