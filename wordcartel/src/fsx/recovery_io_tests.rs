#[cfg(test)]
mod tests {
use super::super::*;

#[cfg(unix)]
#[test]
fn recovery_root_alias_resolves_without_weakening_private_leaf_checks() {
    let root = tempfile::tempdir().unwrap();
    let real = root.path().join("real");
    let alias = root.path().join("alias");
    RealFs.create_dir_excl(&real, 0o700).unwrap();
    std::os::unix::fs::symlink(&real, &alias).unwrap();
    assert_eq!(RealFs.canonicalize_existing(&alias).unwrap(),
        RealFs.canonicalize_existing(&real).unwrap());
    assert!(RealFs.validate_private_dir(&alias).is_err());
    let missing = root.path().join("missing");
    assert_eq!(RealFs.canonicalize_existing(&missing).unwrap_err().kind(),
        std::io::ErrorKind::NotFound);
    let fault = crate::test_support::FaultFs::new(crate::test_support::FaultAt::Canonicalize);
    assert!(fault.canonicalize_existing(&real).is_err());
    assert_eq!(fault.path_operations(), vec![(crate::test_support::FaultAt::Canonicalize, real)]);
}

#[test]
fn exclusive_private_directory_and_strict_sync() {
    let root = tempfile::tempdir().expect("tempdir");
    let dir = root.path().join("owner");
    RealFs.create_dir_excl(&dir, 0o700).expect("create");
    assert_eq!(RealFs.create_dir_excl(&dir, 0o700).unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists);
    RealFs.validate_private_dir(&dir).expect("private");
    RealFs.sync_dir_strict(&dir).expect("strict sync");
    assert!(RealFs.sync_dir_strict(&dir.join("missing")).is_err());
}

#[test]
fn lock_is_exclusive_until_last_shared_guard_drops() {
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("lock");
    let lease: std::sync::Arc<dyn RecoveryLease> =
        RealFs.try_recovery_lock(&path, true).expect("new lock").into();
    let other = lease.clone();
    drop(lease);
    assert!(matches!(RealFs.try_recovery_lock(&path, false),
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
    drop(other);
    // Parallel child-spawn tests may briefly inherit the descriptor between fork and exec.
    // CLOEXEC closes it at exec; bounded retry still detects a leaked ownership guard.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match RealFs.try_recovery_lock(&path, false) {
            Ok(lease) => { drop(lease); break; }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
                && std::time::Instant::now() < deadline =>
                std::thread::sleep(std::time::Duration::from_millis(5)),
            Err(error) => panic!("lock was not released: {error}"),
        }
    }
    assert!(RealFs.try_recovery_lock(&path, true).is_err());
}

#[test]
fn opened_record_stays_on_original_inode_and_caps_reads() {
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("record");
    fs::write(&path, b"original").expect("seed");
    let mut handle = RealFs.open_regular_nofollow(&path).expect("open");
    fs::rename(&path, root.path().join("old")).expect("move");
    fs::write(&path, b"new").expect("replace");
    assert_eq!(handle.stat().expect("stat").len, 8);
    assert_eq!(handle.read_capped(8).expect("read"), Some(b"original".to_vec()));
    handle.sync_all().expect("sync original");
    assert!(RealFs.open_regular_nofollow(&path).expect("open").read_capped(2)
        .expect("oversized").is_none());
}

#[cfg(unix)]
#[test]
fn rejects_symlinks_special_files_and_public_directories() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let root = tempfile::tempdir().expect("tempdir");
    let file = root.path().join("file");
    fs::write(&file, b"x").expect("seed");
    let link = root.path().join("link");
    symlink(&file, &link).expect("symlink");
    assert!(RealFs.open_regular_nofollow(&link).is_err());
    assert!(RealFs.try_recovery_lock(&link, false).is_err());
    assert!(RealFs.open_regular_nofollow(root.path()).is_err());
    assert!(RealFs.open_regular_nofollow(Path::new("/dev/null")).is_err());
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).expect("chmod");
    assert!(RealFs.validate_private_dir(root.path()).is_err());
}

#[cfg(unix)]
#[test]
fn private_metadata_rejects_wrong_owner_even_with_private_mode() {
    assert!(validate_private_metadata(123, 0o700, 456).is_err());
    assert!(validate_private_metadata(123, 0o700, 123).is_ok());
    assert!(validate_private_metadata(123, 0o710, 123).is_err());
}

#[test]
fn recovery_faults_and_nth_sync_are_observable() {
    use crate::test_support::{FaultAt, FaultFs};
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("record");
    fs::write(&path, b"data").expect("seed");
    let injected = FaultFs::on_occurrence(FaultAt::StrictDirSync, 3);
    injected.sync_dir_strict(root.path()).expect("first");
    injected.sync_dir_strict(root.path()).expect("second");
    assert!(injected.sync_dir_strict(root.path()).is_err());
    injected.sync_dir_strict(root.path()).expect("fourth");
    assert!(injected.path_operations().iter().all(|(_, path)| path == root.path()));
    assert_eq!(injected.operations().iter().filter(|&&op| op == FaultAt::StrictDirSync)
        .count(), 4);
    for fault in [FaultAt::RecoveryRead, FaultAt::RecoveryStat, FaultAt::RecoverySync] {
        let injected = FaultFs::new(fault);
        let mut read = injected.open_regular_nofollow(&path).expect("open");
        let result = match fault {
            FaultAt::RecoveryRead => read.read_capped(100).map(|_| ()),
            FaultAt::RecoveryStat => read.stat().map(|_| ()),
            _ => read.sync_all(),
        };
        assert!(result.is_err(), "{fault:?}");
    }
    for fault in [FaultAt::RecoveryOpen, FaultAt::RecoveryLock,
        FaultAt::RecoveryLockBusy, FaultAt::StrictDirOpen, FaultAt::CreateDir,
        FaultAt::ValidatePrivateDir]
    {
        let injected = FaultFs::new(fault);
        let result = match fault {
            FaultAt::RecoveryOpen => injected.open_regular_nofollow(&path).map(|_| ()),
            FaultAt::RecoveryLock | FaultAt::RecoveryLockBusy =>
                injected.try_recovery_lock(&path, false).map(|_| ()),
            FaultAt::StrictDirOpen => injected.sync_dir_strict(root.path()),
            FaultAt::CreateDir => injected.create_dir_excl(&root.path().join("new"), 0o700),
            _ => injected.validate_private_dir(root.path()),
        };
        assert!(result.is_err(), "{fault:?}");
    }
}

#[cfg(unix)]
#[test]
fn fifo_child_probe() {
    let Some(fifo) = std::env::var_os("WORDCARTEL_FIFO_PROBE") else { return; };
    let fifo = std::path::PathBuf::from(fifo);
    assert!(RealFs.open_regular_nofollow(&fifo).is_err());
    assert!(RealFs.try_recovery_lock(&fifo, false).is_err());
    fs::write(fifo.with_extension("done"), b"both returned").expect("handshake");
}

#[cfg(unix)]
#[test]
fn fifo_open_returns_without_waiting_for_a_writer() {
    let root = tempfile::tempdir().expect("tempdir");
    let fifo = root.path().join("fifo");
    assert!(std::process::Command::new("mkfifo").arg(&fifo).status()
        .expect("mkfifo available").success());
    let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", "fsx::recovery_io_tests::tests::fifo_child_probe"])
        .env("WORDCARTEL_FIFO_PROBE", &fifo)
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .spawn().expect("child");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if std::time::Instant::now() < deadline =>
                std::thread::sleep(std::time::Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                child.wait().expect("reap child");
                break None;
            }
        }
    };
    assert!(status.is_some_and(|status| status.success()), "FIFO probe failed or blocked");
    assert_eq!(fs::read(fifo.with_extension("done")).expect("helper actually ran"),
        b"both returned");
}

#[test]
fn unsupported_capability_is_distinct_from_busy() {
    let unsupported = recovery_unsupported::<()>().expect_err("unsupported");
    assert_eq!(unsupported.kind(), std::io::ErrorKind::Unsupported);
    let root = tempfile::tempdir().expect("tempdir");
    let injected = crate::test_support::FaultFs::new(crate::test_support::FaultAt::RecoveryLockBusy);
    assert!(matches!(injected.try_recovery_lock(&root.path().join("lock"), true),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock));
    assert!(!root.path().join("lock").exists());
}

}
