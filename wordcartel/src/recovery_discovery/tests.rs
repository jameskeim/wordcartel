#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::fsx::RealFs;
    use crate::recovery_store::{checkpoint, CheckpointRecord, RecoverySlot};
    use crate::test_support::scratch_dir;
    #[test]
    fn locked_owner_is_unavailable_then_empty_body_is_recoverable() {
        let root = scratch_dir("discovery-owner");
        let slot = RecoverySlot::new();
        let record = CheckpointRecord::new(slot.reserve_generation().unwrap(), "lineage".into(), 0, None, None);
        checkpoint(&RealFs, &root, &slot, &record, "").unwrap();
        let busy = scan(&RealFs, &root, &ScanScope::All).unwrap();
        assert_eq!(busy.len(), 1);
        assert!(busy[0].unavailable.is_some());
        drop(slot);
        let available = crate::test_support::recovery_scan_released(&RealFs, &root, &ScanScope::All, 1);
        assert!(available[0].unavailable.is_none());
        let PrepareOutcome::Ready(prepared) = crate::test_support::recovery_prepare_released(&RealFs, &root, &available[0]).unwrap() else { panic!("unchanged") };
        assert_eq!(prepared.body, "");
        assert!(available[0].source_path.exists());
    }
    #[test]
    fn legacy_changed_body_requires_reselection_and_preserves_source() {
        let root = scratch_dir("discovery-legacy");
        let path = root.join("recovered-test.md");
        std::fs::write(&path, "old").unwrap();
        let rows = scan(&RealFs, &root, &ScanScope::All).unwrap();
        std::fs::write(&path, "new").unwrap();
        let PrepareOutcome::Changed(row) = prepare(&RealFs, &root, &rows[0]).unwrap() else { panic!("changed") };
        assert_eq!(row.preview, "new");
        assert_eq!(std::fs::read_to_string(path).unwrap(), "new");
    }
    #[test]
    fn nonexistent_root_is_not_created() {
        let root = scratch_dir("discovery-no-create").join("missing");
        assert!(scan(&RealFs, &root, &ScanScope::All).unwrap().is_empty());
        assert!(!root.exists());
    }
    #[test]
    fn legacy_headers_discover_all_named_and_unnamed_records() {
        let root = scratch_dir("discovery-swap");
        for (name, realpath, body) in [("arbitrary.swp", Some("/missing/book.md".into()), "named"),
            ("other.swp", None, "unnamed")] {
            let header = crate::swap::SwapHeader { realpath, ..Default::default() };
            std::fs::write(root.join(name), crate::swap::serialize(&header, body)).unwrap();
        }
        let rows = scan(&RealFs, &root, &ScanScope::All).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.unavailable.is_none()));
        assert!(rows.iter().any(|r| r.association.is_some() && r.preview == "named"));
        assert!(rows.iter().all(|r| matches!(r.timestamp, CandidateTime::LegacyMtime(_))));
    }
    #[test]
    fn previews_are_bounded_unicode_and_corruption_is_visible() {
        let root = scratch_dir("discovery-bounds");
        std::fs::write(root.join("recovered-large.md"), "雪".repeat(1000)).unwrap();
        std::fs::write(root.join("broken.swp"), "not a header").unwrap();
        let rows = scan(&RealFs, &root, &ScanScope::All).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows.iter().find(|r| r.unavailable.is_none()).unwrap().preview.chars().count(), 240);
        assert!(rows.iter().any(|r| r.unavailable.is_some()));
    }
    #[test]
    fn selected_token_cannot_claim_another_owner_path() {
        let root = scratch_dir("discovery-path-token");
        for body in ["a", "b"] {
            let slot = RecoverySlot::new();
            let record = CheckpointRecord::new(slot.reserve_generation().unwrap(), "lineage".into(), 0, None, None);
            checkpoint(&RealFs, &root, &slot, &record, body).unwrap();
        }
        let rows = scan(&RealFs, &root, &ScanScope::All).unwrap();
        let mut forged = rows[0].clone();
        forged.source_path = rows[1].source_path.clone();
        assert!(prepare(&RealFs, &root, &forged).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn raw_legacy_filename_survives_scan_and_prepare() {
        use std::os::unix::ffi::OsStringExt;
        let root = scratch_dir("discovery-raw");
        let path = root.join(std::ffi::OsString::from_vec(b"recovered-\xff.md".to_vec()));
        std::fs::write(&path, "raw").unwrap();
        let rows = scan(&RealFs, &root, &ScanScope::All).unwrap();
        assert_eq!(rows[0].source_path, path);
        assert!(matches!(prepare(&RealFs, &root, &rows[0]).unwrap(), PrepareOutcome::Ready(_)));
    }

    #[test]
    fn owner_mismatch_is_disabled_without_deleting_bytes() {
        let root = scratch_dir("discovery-owner-mismatch");
        let slot = RecoverySlot::new();
        let record = CheckpointRecord::new(slot.reserve_generation().unwrap(), "lineage".into(), 0, None, None);
        let ack = checkpoint(&RealFs, &root, &slot, &record, "source").unwrap();
        drop(slot);
        let bytes = std::fs::read(ack.path()).unwrap();
        let (metadata, body) = crate::recovery_store::decode(&bytes).unwrap();
        let other = if ack.owner() == "1111111111111111" { "2222222222222222" } else { "1111111111111111" };
        let forged = crate::recovery_store::Metadata::new(other.into(), metadata.record().clone(), body.len() as u64).unwrap();
        let bytes = crate::recovery_store::encode(&forged, body).unwrap();
        std::fs::write(ack.path(), &bytes).unwrap();
        let rows = scan(&RealFs, &root, &ScanScope::All).unwrap();
        assert!(rows[0].unavailable.as_ref().unwrap().contains("mismatch"));
        assert_eq!(std::fs::read(ack.path()).unwrap(), bytes);
    }
    #[test]
    fn legacy_header_has_its_own_size_limit() {
        let root = scratch_dir("discovery-header-cap");
        let header = crate::swap::SwapHeader { realpath: Some("x".repeat(MAX_METADATA + 1)), ..Default::default() };
        let path = root.join("oversized.swp");
        std::fs::write(&path, crate::swap::serialize(&header, "small body")).unwrap();
        let rows = scan(&RealFs, &root, &ScanScope::All).unwrap();
        assert!(rows[0].unavailable.as_ref().unwrap().contains("size limit"));
        assert!(path.exists());
    }
    #[cfg(unix)]
    #[test]
    fn legacy_symlink_is_unavailable_and_target_is_preserved() {
        let root = scratch_dir("discovery-symlink");
        let target = root.join("target");
        std::fs::write(&target, "private").unwrap();
        std::os::unix::fs::symlink(&target, root.join("recovered-link.md")).unwrap();
        let rows = scan(&RealFs, &root, &ScanScope::All).unwrap();
        assert!(rows[0].unavailable.is_some());
        assert_eq!(std::fs::read_to_string(target).unwrap(), "private");
    }

    #[test]
    fn missing_relative_and_deleted_parent_paths_keep_resolved_context() {
        let cwd = RealFs.canonicalize_existing(Path::new(".")).unwrap();
        let missing = format!("wordcartel-missing-{}-{}.md", std::process::id(), crate::editor::DocumentId::mint().to_hex());
        assert_eq!(normalized(&RealFs, Path::new(&missing)), Some(cwd.join(&missing)));
        let root = scratch_dir("discovery-missing-parents");
        let missing = root.join("deleted-parent").join("subdir").join("book.md");
        assert_eq!(normalized(&RealFs, &missing), Some(missing));
    }
    #[test]
    fn lock_only_owner_is_omitted_only_after_lease_becomes_available() {
        let root = scratch_dir("discovery-tombstone-lease");
        let slot = RecoverySlot::new();
        let record = CheckpointRecord::new(slot.reserve_generation().unwrap(), "lineage".into(), 0, None, None);
        let ack = checkpoint(&RealFs, &root, &slot, &record, "source").unwrap();
        std::fs::remove_file(ack.path()).unwrap();
        let rows = scan(&RealFs, &root, &ScanScope::All).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].busy);
        drop(slot);
        assert!(scan(&RealFs, &root, &ScanScope::All).unwrap().is_empty());
        std::fs::write(ack.path().parent().unwrap().join("checkpoint-2.tmp"), "partial").unwrap();
        let rows = scan(&RealFs, &root, &ScanScope::All).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].unavailable.is_some());
        assert!(!rows[0].busy);
    }

}
