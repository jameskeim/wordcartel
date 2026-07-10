#![forbid(unsafe_code)]
//! `wcartel` — thin binary entry point.
//!
//! Usage: `wcartel [--version|-V] [--no-config] [--config <path>] [file.md]`

// print_stdout: the `--version` line is pre-guard stdout — the conventional `--version` channel.
// print_stderr: fatal startup/exit errors go to stderr AFTER terminal restore — the correct channel.
#[allow(clippy::print_stdout, clippy::print_stderr)]
fn main() {
    let cli = wordcartel::config::parse_cli(std::env::args());
    if cli.version {
        // Printed BEFORE app::run installs the terminal guard, so stdout is safe.
        println!("wcartel {}", env!("CARGO_PKG_VERSION"));
        return;
    }
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
