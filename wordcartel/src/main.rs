#![forbid(unsafe_code)]
//! `wcartel` — thin binary entry point.
//!
//! Usage: `wcartel [--no-config] [--config <path>] [file.md]`

#[allow(clippy::print_stderr)] // fatal startup/exit errors go to stderr AFTER terminal restore — the correct channel
fn main() {
    let cli = wordcartel::config::parse_cli(std::env::args());
    match wordcartel::app::run(cli) {
        Ok(wordcartel::app::ExitReason::Normal) => {}
        Ok(wordcartel::app::ExitReason::InputLost) => {
            eprintln!("wcartel: input reader stopped — terminal may have closed; recovery written");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("wcartel: {e}");
            std::process::exit(1);
        }
    }
}
