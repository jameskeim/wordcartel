#![forbid(unsafe_code)]
//! `wcartel` — thin binary entry point.
//!
//! Usage: `wcartel [--no-config] [--config <path>] [file.md]`

fn main() {
    let cli = wordcartel::config::parse_cli(std::env::args());
    if let Err(e) = wordcartel::app::run(cli) {
        eprintln!("wcartel: {e}");
        std::process::exit(1);
    }
}
