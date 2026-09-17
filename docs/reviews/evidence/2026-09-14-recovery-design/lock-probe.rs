// Design feasibility probe against the pinned toolchain; not a product test.
use std::{fs::{File, OpenOptions, TryLockError}, process::{Command, exit}};
fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    let path = &args[1];
    if args.len() > 2 {
        let f = OpenOptions::new().read(true).write(true).open(path).unwrap();
        match f.try_lock() {
            Ok(()) => exit(0),
            Err(TryLockError::WouldBlock) => exit(10),
            Err(e) => panic!("lock failed: {e}"),
        }
    }
    let f = OpenOptions::new().read(true).write(true).create_new(true).open(path).unwrap();
    f.try_lock().unwrap();
    let run = || Command::new(std::env::current_exe().unwrap()).arg(path).arg("child")
        .status().unwrap().code().unwrap();
    assert_eq!(run(), 10, "separate process cannot acquire held lease");
    drop(f);
    assert_eq!(run(), 0, "lease released when holder closes");
    File::open(std::path::Path::new(path).parent().unwrap()).unwrap().sync_all().unwrap();
    println!("PASS: cross-process contention, release on close, directory sync (Linux local filesystem)");
}
