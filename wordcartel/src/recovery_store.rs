//! Independently owned recovery records. All IO entry points run on workers.
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use crate::fsx::{Fs, RecoveryLease};

mod codec;
pub(crate) use codec::validate_owner;
mod cleanup;
pub use cleanup::{cleanup_saved, cleanup_saved_with_policy, AssociationPolicy, CleanupOutcome};
pub use codec::{CheckpointRecord, Metadata, TaggedPath, decode, encode, MAX_METADATA, MAX_RECORD_BYTES};

/// Recovery protocol failures; unsupported IO never creates a durability acknowledgement.
#[derive(Debug, thiserror::Error)]
pub enum RecoveryError {
    #[error("recovery IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid recovery record: {0}")]
    Invalid(&'static str),
    #[error("recovery record exceeds its size limit")]
    TooLarge,
    #[error("recovery generation exhausted")]
    Exhausted,
}

/// Memory-only identity, retained by queued jobs. Debug and identity comparison never lock.
#[derive(Clone, Default)]
pub struct RecoverySlot(Arc<SlotInner>);
#[derive(Default)]
struct SlotInner { generation: AtomicU64, worker: Mutex<SlotState> }
impl std::fmt::Debug for RecoverySlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("RecoverySlot").field(&Arc::as_ptr(&self.0)).finish()
    }
}
#[derive(Default)]
struct SlotState { owner: Option<Owner>, attempted: u64, acknowledged: Option<u64> }
struct Owner { id: String, dir: PathBuf, root: PathBuf, _lease: Box<dyn RecoveryLease> }
impl RecoverySlot {
    /// Reserve an identity without filesystem IO or locking.
    pub fn new() -> Self { Self::default() }
    /// Reserve a capture generation without locking; exhaustion never wraps.
    pub fn reserve_generation(&self) -> Result<u64, RecoveryError> {
        self.0.generation.fetch_update(Ordering::Relaxed, Ordering::Relaxed,
            |g| g.checked_add(1)).map(|g| g + 1).map_err(|_| RecoveryError::Exhausted)
    }
    /// Compare editing-instance identity without locking.
    pub fn same_instance(&self, other: &Self) -> bool { Arc::ptr_eq(&self.0, &other.0) }
}

/// Proof that this exact generation passed every required durability barrier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointAck { owner: String, generation: u64, path: PathBuf }
impl CheckpointAck {
    /// Opaque exclusively allocated owner component.
    pub fn owner(&self) -> &str { &self.owner }
    /// Attempt number; failed attempts are never reused.
    pub fn generation(&self) -> u64 { self.generation }
    /// Exact checkpoint record path.
    pub fn record_path(&self) -> &Path { &self.path }
    /// Record location, for diagnostics and discovery.
    pub fn path(&self) -> &Path { &self.path }
}

/// Persist a snapshot in its independent slot. Root must be an absolute resolved state directory.
/// Errors retain first-checkpoint ancestor obligations and never return an acknowledgement.
pub fn checkpoint(fs: &dyn Fs, root: &Path, slot: &RecoverySlot,
    record: &CheckpointRecord, body: &str) -> Result<CheckpointAck, RecoveryError>
{
    checkpoint_with_names(fs, root, slot, record, body, || crate::editor::DocumentId::mint().to_hex())
}

fn checkpoint_with_names(fs: &dyn Fs, root: &Path, slot: &RecoverySlot,
    record: &CheckpointRecord, body: &str, mut name: impl FnMut() -> String)
    -> Result<CheckpointAck, RecoveryError>
{
    // Every assignment preserves a valid state: attempt first, complete owner next, Ack last.
    // A caught worker panic may poison the mutex, but cannot discharge sync obligations.
    let mut state = slot.0.worker.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let generation = record.generation();
    if generation <= state.attempted || generation > slot.0.generation.load(Ordering::Relaxed) {
        return Err(RecoveryError::Invalid("unreserved or replayed generation"));
    }
    state.attempted = generation;
    if state.owner.is_none() { state.owner = Some(allocate(fs, root, &mut name)?); }
    let owner = state.owner.as_ref().expect("allocated owner");
    if owner.root != root { return Err(RecoveryError::Invalid("slot root changed")); }
    let metadata = Metadata::new(owner.id.clone(), record.clone(), body.len() as u64)?;
    let bytes = encode(&metadata, body)?;
    let path = owner.dir.join("checkpoint.wcr");
    let temp = owner.dir.join(format!("checkpoint-{generation}.tmp"));
    let mut created = false;
    let result = (|| {
        let mut file = fs.create_excl(&temp, 0o600)?;
        created = true;
        file.write_all(&bytes)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        fs.rename(&temp, &path)?;
        fs.sync_dir_strict(&owner.dir)?;
        if state.acknowledged.is_none() {
            for ancestor in owner.dir.ancestors().skip(1) { fs.sync_dir_strict(ancestor)?; }
        }
        Ok::<_, std::io::Error>(())
    })();
    if let Err(error) = result {
        if created { let _ = fs.remove_file(&temp); }
        return Err(error.into());
    }
    let ack = CheckpointAck { owner: owner.id.clone(), generation, path };
    state.acknowledged = Some(generation);
    Ok(ack)
}

fn allocate(fs: &dyn Fs, root: &Path, name: &mut impl FnMut() -> String)
    -> Result<Owner, RecoveryError>
{
    if !root.is_absolute() || root.components().any(|c| matches!(c,
        std::path::Component::ParentDir | std::path::Component::CurDir)) {
        return Err(RecoveryError::Invalid("state root must be resolved and absolute"));
    }
    provision(fs, root)?;
    let resolved = fs.canonicalize_existing(root)?;
    let protocol = resolved.join("recovery-v2");
    match fs.create_dir_excl(&protocol, 0o700) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e.into()),
    }
    fs.validate_private_dir(&protocol)?;
    for _ in 0..128 {
        let id = name();
        codec::validate_owner(&id)?;
        let dir = protocol.join(&id);
        match fs.create_dir_excl(&dir, 0o700) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
        fs.validate_private_dir(&dir)?;
        let lease = fs.try_recovery_lock(&dir.join("owner.lock"), true)?;
        return Ok(Owner { id, dir, root: root.to_owned(), _lease: lease });
    }
    Err(RecoveryError::Invalid("owner allocation collision limit"))
}

fn provision(fs: &dyn Fs, root: &Path) -> Result<(), RecoveryError> {
    for path in root.ancestors().collect::<Vec<_>>().into_iter().rev() {
        match fs.stat(path) {
            Ok(stat) if stat.is_dir => (),
            Ok(_) => return Err(RecoveryError::Invalid("unresolved state ancestor")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                match fs.create_dir_excl(path, 0o700) {
                    Ok(()) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        if !fs.stat(path)?.is_dir {
                            return Err(RecoveryError::Invalid("state ancestor is not a directory"));
                        }
                    }
                    Err(e) => return Err(e.into()),
                }
            },
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "recovery_store/tests.rs"]
mod store_tests;
