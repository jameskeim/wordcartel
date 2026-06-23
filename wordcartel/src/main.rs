#![forbid(unsafe_code)]
//! `wcartel` — thin binary entry point.
//!
//! Usage: `wcartel [file.md]`

use std::path::PathBuf;
use wordcartel::app;

fn main() {
    let path = std::env::args().nth(1).map(PathBuf::from);
    if let Err(e) = app::run(path) {
        eprintln!("wcartel: {e}");
        std::process::exit(1);
    }
}
