#![forbid(unsafe_code)]
fn main() {
    let path = std::env::args().nth(1);
    match path {
        Some(p) => eprintln!("wcartel: would open {p} (loop wired in Task 12)"),
        None => eprintln!("usage: wcartel <file.md>"),
    }
}
