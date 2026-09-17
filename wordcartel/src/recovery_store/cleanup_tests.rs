#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::fsx::RealFs;
    use crate::recovery_store::{checkpoint, CheckpointRecord, TaggedPath};
    use crate::test_support::{FaultAt, FaultFs, scratch_dir};

    fn prepared(label: &str) -> (std::path::PathBuf, RecoverySlot,
        crate::recovery_store::CheckpointAck, std::path::PathBuf, FileFingerprint)
    {
        let root = scratch_dir(label);
        let target = root.join("document.md");
        std::fs::write(&target, "saved body").expect("saved file");
        let slot = RecoverySlot::new();
        let record = CheckpointRecord::new(slot.reserve_generation().expect("generation"),
            "lineage".into(), 4, Some(TaggedPath::from_path(&target)), None);
        let ack = checkpoint(&RealFs, &root, &slot, &record, "saved body").expect("checkpoint");
        let fp = crate::save::fingerprint_with_fs(&RealFs, &target).expect("fingerprint");
        (root, slot, ack, target, fp)
    }
    #[test]
    fn successful_save_receipt_removes_only_owned_checkpoint() {
        let (root, slot, ack, target, fp) = prepared("recovery-cleanup-success");
        let foreign = root.join("legacy-recovery.md");
        std::fs::write(&foreign, "legacy remains").expect("legacy");
        assert_eq!(cleanup_saved(&RealFs, &slot, 1, 4, &target, Some(fp), "saved body"),
            CleanupOutcome::Cleaned);
        assert!(!ack.path().exists());
        assert!(ack.path().parent().expect("owner").join("owner.lock").exists());
        assert_eq!(std::fs::read_to_string(&foreign).expect("legacy"), "legacy remains");
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn failed_target_sync_retains_recovery_after_successful_save() {
        let (root, slot, ack, target, fp) = prepared("recovery-cleanup-sync");
        let before = std::fs::read(ack.path()).expect("before");
        let outcome = cleanup_saved(&FaultFs::new(FaultAt::RecoverySync), &slot, 1, 4,
            &target, Some(fp), "saved body");
        assert!(matches!(outcome, CleanupOutcome::Retained(_)));
        assert_eq!(std::fs::read(ack.path()).expect("retained"), before);
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn every_pre_unlink_failure_retains_exact_record() {
        for (fault, nth) in [(FaultAt::RecoveryOpen, 1), (FaultAt::RecoveryOpen, 2),
            (FaultAt::RecoveryRead, 1), (FaultAt::RecoveryRead, 2), (FaultAt::RecoveryStat, 1),
            (FaultAt::Canonicalize, 1),
            (FaultAt::RecoverySync, 1), (FaultAt::StrictDirOpen, 1),
            (FaultAt::StrictDirSync, 1), (FaultAt::RemoveFile, 1)] {
            let (root, slot, ack, target, fp) = prepared("recovery-cleanup-faults");
            let before = std::fs::read(ack.path()).expect("before");
            let outcome = cleanup_saved(&FaultFs::on_occurrence(fault, nth), &slot, 1, 4,
                &target, Some(fp), "saved body");
            assert!(matches!(outcome, CleanupOutcome::Retained(_)), "{fault:?}: {outcome:?}");
            assert_eq!(std::fs::read(ack.path()).expect("retained"), before);
            assert_eq!(std::fs::read_to_string(&target).expect("saved"), "saved body");
            let _ = std::fs::remove_dir_all(root);
        }
    }
    #[test]
    fn uncertain_unlink_sync_is_backed_by_synced_saved_target() {
        let (root, slot, ack, target, fp) = prepared("recovery-cleanup-uncertain");
        let fs = FaultFs::on_occurrence(FaultAt::StrictDirSync, 2);
        let outcome = cleanup_saved(&fs, &slot, 1, 4, &target, Some(fp), "saved body");
        assert!(matches!(outcome, CleanupOutcome::Uncertain(_)));
        assert!(!ack.path().exists());
        let ops = fs.operations();
        let sync = ops.iter().position(|&o| o == FaultAt::RecoverySync).expect("same handle sync");
        let parent = ops.iter().position(|&o| o == FaultAt::StrictDirSync).expect("parent sync");
        let unlink = ops.iter().position(|&o| o == FaultAt::RemoveFile).expect("unlink");
        assert!(sync < parent && parent < unlink);
        assert_eq!(ops.iter().filter(|&&o| o == FaultAt::RecoveryOpen).count(), 2,
            "one checkpoint open and one target open, no reopen between compare and sync");
        assert_eq!(std::fs::read_to_string(&target).expect("saved"), "saved body");
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn divergent_newer_or_unassociated_records_cannot_be_retired() {
        for case in ["newer-generation", "newer-version", "body", "association", "no-fingerprint"] {
            let (root, slot, ack, target, fp) = prepared("recovery-cleanup-eligibility");
            let before = std::fs::read(ack.path()).expect("before");
            let generation = if case == "newer-generation" { 0 } else { 1 };
            let version = if case == "newer-version" { 3 } else { 4 };
            let body = if case == "body" { "different" } else { "saved body" };
            let path = if case == "association" { root.join("other.md") } else { target.clone() };
            let fp = (case != "no-fingerprint").then_some(fp);
            let outcome = cleanup_saved(&RealFs, &slot, generation, version, &path, fp, body);
            assert!(matches!(outcome, CleanupOutcome::Retained(_) | CleanupOutcome::RetainedByRule(_)), "{case}");
            assert_eq!(std::fs::read(ack.path()).expect("retained"), before);
            let _ = std::fs::remove_dir_all(root);
        }
    }
    #[test]
    fn changed_destination_bytes_or_fingerprint_retains_recovery() {
        for change_body in [true, false] {
            let (root, slot, ack, target, mut fp) = prepared("recovery-cleanup-target-changed");
            if change_body { std::fs::write(&target, "new writer").expect("external write"); }
            else { fp.hash ^= 1; }
            assert!(matches!(cleanup_saved(&RealFs, &slot, 1, 4, &target, Some(fp), "saved body"),
                CleanupOutcome::Retained(_)));
            assert!(ack.path().exists());
            let _ = std::fs::remove_dir_all(root);
        }
    }
    #[test]
    fn cleanup_does_not_discharge_failed_first_checkpoint_sync_obligations() {
        let root = scratch_dir("recovery-cleanup-first-ack");
        let target = root.join("document.md");
        std::fs::write(&target, "saved body").expect("saved");
        let slot = RecoverySlot::new();
        let rec = CheckpointRecord::new(slot.reserve_generation().expect("generation"),
            "lineage".into(), 4, Some(TaggedPath::from_path(&target)), None);
        assert!(checkpoint(&FaultFs::on_occurrence(FaultAt::StrictDirSync, 2), &root,
            &slot, &rec, "saved body").is_err());
        let fp = crate::save::fingerprint_with_fs(&RealFs, &target);
        assert_eq!(cleanup_saved(&RealFs, &slot, 1, 4, &target, fp, "saved body"), CleanupOutcome::Cleaned);
        let rec = CheckpointRecord::new(slot.reserve_generation().expect("generation"),
            "lineage".into(), 5, Some(TaggedPath::from_path(&target)), None);
        let fs = FaultFs::on_occurrence(FaultAt::StrictDirSync, usize::MAX);
        let ack = checkpoint(&fs, &root, &slot, &rec, "new edit").expect("next checkpoint");
        assert_eq!(fs.operations().iter().filter(|&&o| o == FaultAt::StrictDirSync).count(),
            ack.path().parent().expect("owner").ancestors().count());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ordinary_save_does_not_retire_unnamed_or_old_path_checkpoint() {
        for association in [None, Some(TaggedPath::from_path(Path::new("/old/document.md")))] {
            let root = scratch_dir("recovery-cleanup-save-as");
            let target = root.join("new.md");
            std::fs::write(&target, "saved body").expect("save as");
            let slot = RecoverySlot::new();
            let rec = CheckpointRecord::new(slot.reserve_generation().expect("generation"),
                "lineage".into(), 4, association, None);
            let ack = checkpoint(&RealFs, &root, &slot, &rec, "saved body").expect("checkpoint");
            let before = std::fs::read(ack.path()).expect("before");
            let fp = crate::save::fingerprint_with_fs(&RealFs, &target);
            assert!(matches!(cleanup_saved(&RealFs, &slot, 1, 4, &target, fp, "saved body"),
                CleanupOutcome::RetainedByRule(_)));
            assert_eq!(std::fs::read(ack.path()).expect("retained"), before);
            let _ = std::fs::remove_dir_all(root);
        }
    }
    #[test]
    fn memory_only_slot_cleanup_performs_no_filesystem_operations() {
        let fs = FaultFs::new(FaultAt::RecoveryOpen);
        assert_eq!(cleanup_saved(&fs, &RecoverySlot::new(), 0, 0, Path::new("/unused"), None, ""),
            CleanupOutcome::NoCheckpoint);
        assert!(fs.operations().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn committed_parent_alias_matches_normalized_checkpoint_association() {
        let (root, slot, ack, target, fp) = prepared("recovery-cleanup-alias");
        let alias = root.join("parent-alias");
        std::os::unix::fs::symlink(&root, &alias).expect("parent alias");
        let through_alias = alias.join(target.file_name().expect("filename"));
        assert_eq!(cleanup_saved(&RealFs, &slot, 1, 4, &through_alias, Some(fp), "saved body"),
            CleanupOutcome::Cleaned);
        assert!(!ack.path().exists());
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn committed_relative_path_matches_normalized_checkpoint_association() {
        let (root, slot, ack, target, fp) = prepared("recovery-cleanup-relative");
        let cwd = std::env::current_dir().expect("cwd");
        let mut relative = std::path::PathBuf::new();
        for component in cwd.components() {
            if matches!(component, std::path::Component::Normal(_)) { relative.push(".."); }
        }
        for component in target.components() {
            if let std::path::Component::Normal(part) = component { relative.push(part); }
        }
        assert_eq!(cleanup_saved(&RealFs, &slot, 1, 4, &relative, Some(fp), "saved body"),
            CleanupOutcome::Cleaned);
        assert!(!ack.path().exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn recovery_save_as_receipt_covers_pathless_or_captured_previous_association() {
        for named in [false,true] {
            let temp=tempfile::tempdir().unwrap(); let root=temp.path();
            let previous=root.join("old.md"); let target=root.join("new.md");
            std::fs::write(&target,"saved body").unwrap();
            let slot=RecoverySlot::new();
            let record=CheckpointRecord::new(slot.reserve_generation().unwrap(),"lineage".into(),4,
                named.then(|| TaggedPath::from_path(&previous)),None);
            let ack=checkpoint(&RealFs,root,&slot,&record,"saved body").unwrap();
            let fp=crate::save::fingerprint_with_fs(&RealFs,&target);
            assert_eq!(cleanup_saved_with_policy(&RealFs,&slot,1,4,&target,fp,"saved body",
                AssociationPolicy::SaveAs {previous: named.then_some(previous.as_path())}),CleanupOutcome::Cleaned);
            assert!(!ack.path().exists()); assert_eq!(std::fs::read_to_string(target).unwrap(),"saved body");
        }
    }
    #[test]
    fn recovery_save_as_receipt_keeps_owner_generation_body_and_association_guards() {
        for case in ["owner","generation","version","body","association"] {
            let (root,slot,ack,previous,_)=prepared("rekey-guards"); let target=root.join("new.md");
            std::fs::write(&target,"saved body").unwrap();
            if case=="owner" {
                let foreign=RecoverySlot::new();
                let record=CheckpointRecord::new(foreign.reserve_generation().unwrap(),"lineage".into(),4,None,None);
                let foreign_ack=checkpoint(&RealFs,&root,&foreign,&record,"saved body").unwrap();
                std::fs::write(ack.path(),std::fs::read(foreign_ack.path()).unwrap()).unwrap();
            }
            let fp=crate::save::fingerprint_with_fs(&RealFs,&target);
            let other=root.join("other.md");
            let outcome=cleanup_saved_with_policy(&RealFs,&slot,if case=="generation" {0}else{1},
                if case=="version" {3}else{4},&target,fp,if case=="body" {"different"}else{"saved body"},
                AssociationPolicy::SaveAs {previous:Some(if case=="association" {&other}else{&previous})});
            if case=="owner" { assert!(matches!(outcome,CleanupOutcome::Retained(_))); }
            else { assert!(matches!(outcome,CleanupOutcome::RetainedByRule(_))); assert!(outcome.warning().is_none()); }
            assert!(ack.path().exists());
        }
    }
    #[test]
    fn recovery_save_as_receipt_failures_retain_checkpoint_with_warning() {
        for fault in [FaultAt::RecoverySync,FaultAt::StrictDirSync,FaultAt::RecoveryStat,FaultAt::RemoveFile] {
            let (root,slot,ack,previous,_)=prepared("rekey-faults"); let target=root.join("new.md");
            std::fs::write(&target,"saved body").unwrap();
            let fp=crate::save::fingerprint_with_fs(&RealFs,&target);
            let outcome=cleanup_saved_with_policy(&FaultFs::new(fault),&slot,1,4,&target,fp,"saved body",
                AssociationPolicy::SaveAs {previous:Some(&previous)});
            assert!(matches!(outcome,CleanupOutcome::Retained(_)),"{fault:?}: {outcome:?}");
            assert!(outcome.warning().is_some()); assert!(ack.path().exists());
        }
    }

}
