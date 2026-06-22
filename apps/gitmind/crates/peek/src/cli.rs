//! CLI parsing
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "peek",
    about = "Fast line-range file viewer",
    version = "0.1.0"
)]
pub struct Args {
    #[arg(required = true)]
    pub files: Vec<String>,
    #[arg(short = 'h', long = "headers", group = "header_mode")]
    pub headers: bool,
    #[arg(short = 'H', long = "fullheaders", group = "header_mode")]
    pub full_headers: bool,
    #[arg(short = 'n', long = "numbered")]
    pub numbered: bool,
}
