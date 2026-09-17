#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::fsx::RealFs;
    use crate::test_support::{FaultAt, FaultFs, scratch_dir};
    fn record(slot: &RecoverySlot) -> CheckpointRecord {
        CheckpointRecord::new(slot.reserve_generation().expect("generation"), "lineage".into(), 3, None, None)
    }
    #[test]
    fn checkpoint_requires_all_barriers_and_consumes_failed_generation() {
        let root = scratch_dir("recovery-store-barriers");
        let slot = RecoverySlot::new();
        let fs = FaultFs::on_occurrence(FaultAt::StrictDirSync, 2);
        assert!(checkpoint(&fs, &root, &slot, &record(&slot), "old").is_err());
        let ack = checkpoint(&fs, &root, &slot, &record(&slot), "new").expect("retry");
        assert_eq!(ack.generation(), 2);
        let bytes = std::fs::read(ack.path()).expect("record");
        let (_, body) = decode(&bytes).expect("decode");
        assert_eq!(body, "new");
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn codec_rejects_trailing_body_and_preserves_lossless_paths() {
        let m = Metadata::new("abcdef0123456789".into(), record(&RecoverySlot::new()), 4).expect("metadata");
        let bytes = encode(&m, "text").expect("encode");
        assert_eq!(decode(&bytes).expect("decode").1, "text");
        let mut trailing = bytes; trailing.push(0);
        assert!(decode(&trailing).is_err());
        let _ = RealFs;
    }
    #[test]
    fn every_write_failure_preserves_previous_acknowledged_copy() {
        for fault in [FaultAt::Create, FaultAt::Write { after: 2 }, FaultAt::Flush,
            FaultAt::Sync, FaultAt::Rename] {
            let root = scratch_dir("recovery-store-old-copy");
            let slot = RecoverySlot::new();
            let ack = checkpoint(&RealFs, &root, &slot, &record(&slot), "original").expect("first");
            let before = std::fs::read(ack.path()).expect("before");
            assert!(checkpoint(&FaultFs::new(fault), &root, &slot, &record(&slot), "changed").is_err());
            assert_eq!(std::fs::read(ack.path()).expect("preserved"), before);
            let next = checkpoint(&RealFs, &root, &slot, &record(&slot), "retry").expect("retry");
            assert_eq!(next.generation(), 3);
            let _ = std::fs::remove_dir_all(root);
        }
    }
    #[test]
    fn every_ancestor_failure_retries_complete_obligation_on_same_slot() {
        let root = scratch_dir("recovery-store-ancestors");
        let count = root.join("recovery-v2").join("owner").ancestors().count();
        for boundary in 1..=count {
            let slot = RecoverySlot::new();
            let fs = FaultFs::on_occurrence(FaultAt::StrictDirSync, boundary);
            assert!(checkpoint(&fs, &root, &slot, &record(&slot), "source").is_err());
            let start = fs.path_operations().len();
            let ack = checkpoint(&fs, &root, &slot, &record(&slot), "source").expect("retry");
            let actual: Vec<_> = fs.path_operations()[start..].iter()
                .filter(|(op, _)| *op == FaultAt::StrictDirSync).map(|(_, p)| p.clone()).collect();
            let expected: Vec<_> = ack.path().parent().expect("owner").ancestors()
                .map(Path::to_owned).collect();
            assert_eq!(actual, expected);
        }
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn collision_tombstone_is_never_adopted_or_removed() {
        let root = scratch_dir("recovery-store-collision");
        let slot = RecoverySlot::new();
        let first = checkpoint_with_names(&RealFs, &root, &slot, &record(&slot), "one",
            || "1111111111111111".into()).expect("first");
        drop(slot);
        let slot = RecoverySlot::new();
        let mut names = ["1111111111111111", "2222222222222222"].into_iter();
        let second = checkpoint_with_names(&RealFs, &root, &slot, &record(&slot), "two",
            || names.next().expect("retry collision").into()).expect("second");
        assert_ne!(first.owner(), second.owner());
        assert_eq!(decode(&std::fs::read(first.path()).expect("first")).expect("decode").1, "one");
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn reserved_generation_replay_and_exhaustion_refuse_without_wrap() {
        let root = scratch_dir("recovery-store-generation");
        let slot = RecoverySlot::new();
        let captured = record(&slot);
        checkpoint(&RealFs, &root, &slot, &captured, "one").expect("first");
        assert!(checkpoint(&RealFs, &root, &slot, &captured, "different").is_err());
        slot.0.generation.store(u64::MAX, Ordering::Relaxed);
        assert!(matches!(slot.reserve_generation(), Err(RecoveryError::Exhausted)));
        assert_eq!(slot.0.generation.load(Ordering::Relaxed), u64::MAX);
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn full_body_cap_excludes_header_and_rejects_one_extra_byte() {
        let slot = RecoverySlot::new();
        let body = "x".repeat(crate::limits::MAX_OPEN_BYTES as usize);
        let metadata = Metadata::new("1234567890abcdef".into(), record(&slot), body.len() as u64)
            .expect("cap accepted");
        let bytes = encode(&metadata, &body).expect("encode at cap");
        assert_eq!(decode(&bytes).expect("decode at cap").1.len(), body.len());
        assert!(matches!(Metadata::new("1234567890abcdef".into(), record(&slot),
            body.len() as u64 + 1), Err(RecoveryError::TooLarge)));
    }
    #[cfg(unix)]
    #[test]
    fn raw_unix_paths_roundtrip_and_foreign_paths_cannot_target_local_files() {
        use std::os::unix::ffi::OsStringExt;
        let path = PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/nonutf8-\xff".to_vec()));
        let tagged = TaggedPath::from_path(&path);
        let rec = CheckpointRecord::new(1, "lineage".into(), 1, Some(tagged), None);
        let metadata = Metadata::new("1234567890abcdef".into(), rec, 0).expect("metadata");
        let bytes = encode(&metadata, "").expect("encode");
        assert_eq!(decode(&bytes).expect("decode").0.record().association()
            .expect("association").local_path(), Some(path));
        assert!(TaggedPath::Windows(vec![65, 0xd800]).local_path().is_none());
    }

    #[test]
    fn poison_retains_first_checkpoint_obligations_and_allows_retry() {
        let root = scratch_dir("recovery-store-poison");
        let slot = RecoverySlot::new();
        let fs = FaultFs::on_occurrence(FaultAt::StrictDirSync, 2);
        assert!(checkpoint(&fs, &root, &slot, &record(&slot), "initial").is_err());
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = slot.0.worker.lock().expect("unpoisoned");
            panic!("worker panicked after uncertain checkpoint");
        }));
        let start = fs.path_operations().len();
        let ack = checkpoint(&fs, &root, &slot, &record(&slot), "retry").expect("retry poison");
        assert_eq!(ack.generation(), 2);
        assert_eq!(fs.path_operations()[start..].iter()
            .filter(|(op, _)| *op == FaultAt::StrictDirSync).count(),
            ack.path().parent().expect("owner").ancestors().count());
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn maximum_header_and_corrupt_fields_are_validated_independently() {
        let slot = RecoverySlot::new();
        let meta = Metadata::new("1234567890abcdef".into(), record(&slot), 1).expect("meta");
        let encoded = encode(&meta, "x").expect("encode");
        let header_len = u32::from_le_bytes(encoded[21..25].try_into().expect("length")) as usize;
        let mut padded = encoded[..21].to_vec();
        padded.extend_from_slice(&(codec::MAX_METADATA as u32).to_le_bytes());
        padded.extend_from_slice(&encoded[25..25 + header_len]);
        padded.resize(25 + codec::MAX_METADATA, b' ');
        padded.push(b'x');
        assert!(decode(&padded).is_ok());
        padded[21..25].copy_from_slice(&((codec::MAX_METADATA + 1) as u32).to_le_bytes());
        assert!(matches!(decode(&padded), Err(RecoveryError::TooLarge)));
        for field in ["owner", "timestamp", "generation", "path"] {
            let mut json = serde_json::to_value(&meta).expect("json");
            match field {
                "owner" => json["owner"] = "../outside".into(),
                "timestamp" => json["record"]["timestamp_ms"] = u64::MAX.into(),
                "generation" => json["record"]["generation"] = 0.into(),
                "path" => json["record"]["association"] = serde_json::json!({"platform":"Alien","units":[]}),
                _ => unreachable!(),
            }
            let json = serde_json::to_vec(&json).expect("json bytes");
            let mut invalid = encoded[..21].to_vec();
            invalid.extend_from_slice(&(json.len() as u32).to_le_bytes());
            invalid.extend_from_slice(&json); invalid.push(b'x');
            assert!(decode(&invalid).is_err(), "{field}");
        }
    }

    #[test]
    fn foreground_identity_and_generation_do_not_wait_for_worker_mutex() {
        let slot = RecoverySlot::new();
        let clone = slot.clone();
        let _worker = slot.0.worker.lock().expect("worker owns state");
        assert!(slot.same_instance(&clone));
        assert!(!slot.same_instance(&RecoverySlot::new()));
        assert_eq!(slot.reserve_generation().expect("foreground reserve"), 1);
        assert!(format!("{slot:?}").starts_with("RecoverySlot("));
    }

}
