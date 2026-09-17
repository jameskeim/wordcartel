//! Save receipts grant cleanup authority over precisely one owned checkpoint.
use std::path::Path;
use crate::fsx::Fs;
use crate::save::FileFingerprint;
use super::RecoverySlot;

/// Recovery cleanup is secondary to an already successful ordinary save.
#[derive(Debug, PartialEq, Eq)]
pub enum CleanupOutcome {
    /// No owned checkpoint exists; no work or warning is needed.
    NoCheckpoint,
    /// Owned checkpoint removal and its directory sync succeeded.
    Cleaned,
    /// The checkpoint remains available; the reason can be shown with save success.
    Retained(String),
    /// A newer or divergent owned checkpoint is expected to remain available.
    RetainedByRule(String),
    /// Unlink succeeded but its directory sync failed; the saved destination is durable.
    Uncertain(String),
}
impl CleanupOutcome {
    /// Optional save-completion detail, including informational retention by rule.
    pub fn message(&self) -> Option<&str> {
        match self { Self::RetainedByRule(message) => Some(message), _ => self.warning() }
    }
    /// Optional warning to append to a successful save's status.
    pub fn warning(&self) -> Option<&str> {
        match self {
            Self::NoCheckpoint | Self::Cleaned | Self::RetainedByRule(_) => None,
            Self::Retained(message) | Self::Uncertain(message) => Some(message),
        }
    }
}

/// Verify durable saved bytes before retiring this slot's eligible checkpoint.
/// Call only after successful Saved or Unchanged in the same worker operation.
pub fn cleanup_saved(fs: &dyn Fs, slot: &RecoverySlot, generation_ceiling: u64,
    saved_version: u64, committed_path: &Path, committed_fingerprint: Option<FileFingerprint>,
    body: &str) -> CleanupOutcome
{
    cleanup_saved_with_policy(fs, slot, generation_ceiling, saved_version, committed_path,
        committed_fingerprint, body, AssociationPolicy::CurrentDestination)
}

/// Save As may cover this editing instance's pathless or captured previous association.
/// This changes only association eligibility, never the strict saved-byte receipt.
pub enum AssociationPolicy<'a> {
    /// An ordinary Save covers only the committed destination association.
    CurrentDestination,
    /// An accepted Save As also covers a pathless or captured previous association.
    SaveAs {
        /// Document path captured with this save's slot and content snapshot.
        previous: Option<&'a Path>,
    },
}

/// Apply an explicit association policy after a successful Saved or Unchanged write.
/// All policies require this slot's eligible record and strict same-handle destination proof.
// Receipt inputs remain explicit; the final policy only widens association eligibility.
#[allow(clippy::too_many_arguments)]
pub fn cleanup_saved_with_policy(fs: &dyn Fs, slot: &RecoverySlot, generation_ceiling: u64,
    saved_version: u64, committed_path: &Path, committed_fingerprint: Option<FileFingerprint>,
    body: &str, policy: AssociationPolicy<'_>) -> CleanupOutcome
{
    // Checkpoint mutations remain valid even when a caught worker panic poisons this mutex.
    let state = slot.0.worker.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(owner) = &state.owner else { return CleanupOutcome::NoCheckpoint; };
    let checkpoint_path = owner.dir.join("checkpoint.wcr");
    let mut checkpoint_file = match fs.open_regular_nofollow(&checkpoint_path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return CleanupOutcome::NoCheckpoint,
        Err(e) => return retained(e),
    };
    let bytes = match checkpoint_file.read_capped(super::MAX_RECORD_BYTES) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return retained("owned record exceeds its size limit"),
        Err(e) => return retained(e),
    };
    let (metadata, checkpoint_body) = match super::decode(&bytes) {
        Ok(decoded) => decoded,
        Err(e) => return retained(e),
    };
    let resolved = match fs.canonicalize_existing(committed_path) {
        Ok(path) => path,
        Err(e) => return retained(e),
    };
    let committed_path = resolved.as_path();
    let record = metadata.record();
    if metadata.owner() != owner.id { return retained("owned checkpoint identity mismatch"); }
    if record.generation() > generation_ceiling
        || record.edit_version() > saved_version || checkpoint_body != body
    { return retained_by_rule(); }
    let associated = match covered_association(fs, record.association(), committed_path, policy) {
        Ok(covered) => covered,
        Err(error) => return retained(error),
    };
    if !associated { return retained_by_rule(); }
    let receipt = match prove_saved(fs, owner, committed_path, committed_fingerprint, body) {
        Ok(receipt) => receipt,
        Err(e) => return retained(e),
    };
    // Closing the record read handle permits Windows unlink; the stable owner lease remains held.
    drop(checkpoint_file);
    receipt.consume(fs)
}

fn retained_by_rule() -> CleanupOutcome {
    CleanupOutcome::RetainedByRule("Recovery checkpoint retained: checkpoint is not covered by this save".into())
}
fn covered_association(fs: &dyn Fs, association: Option<&super::TaggedPath>, target: &Path,
    policy: AssociationPolicy<'_>) -> std::io::Result<bool>
{
    let local = association.and_then(super::TaggedPath::local_path);
    if local.as_deref() == Some(target) { return Ok(true); }
    let AssociationPolicy::SaveAs { previous } = policy else { return Ok(false); };
    if association.is_none() { return Ok(true); }
    let Some(previous) = previous else { return Ok(false); };
    let previous = crate::fsx::canonicalize_with_missing_suffix(fs, previous)?;
    Ok(local.as_deref() == Some(previous.as_path()))
}

fn retained(reason: impl std::fmt::Display) -> CleanupOutcome {
    CleanupOutcome::Retained(format!("Recovery checkpoint retained: {reason}"))
}

/// A non-Clone, operation-local proof; borrowing Owner keeps its enclosing guard live.
struct RecoverySaveReceipt<'a> { owner: &'a super::Owner }
impl RecoverySaveReceipt<'_> {
    fn consume(self, fs: &dyn Fs) -> CleanupOutcome {
        if let Err(e) = fs.remove_file(&self.owner.dir.join("checkpoint.wcr")) { return retained(e); }
        match fs.sync_dir_strict(&self.owner.dir) {
            Ok(()) => CleanupOutcome::Cleaned,
            Err(e) => CleanupOutcome::Uncertain(format!(
                "Recovery cleanup durability uncertain; saved file is durable: {e}")),
        }
    }
}

fn prove_saved<'a>(fs: &dyn Fs, owner: &'a super::Owner, target: &Path,
    committed: Option<FileFingerprint>, body: &str)
    -> Result<RecoverySaveReceipt<'a>, super::RecoveryError>
{
    use std::hash::Hasher as _;
    let expected = committed.ok_or(super::RecoveryError::Invalid("saved fingerprint unavailable"))?;
    let mut file = fs.open_regular_nofollow(target)?;
    let bytes = file.read_capped(crate::limits::MAX_OPEN_BYTES)?.ok_or(super::RecoveryError::TooLarge)?;
    if bytes != body.as_bytes() { return Err(super::RecoveryError::Invalid("saved bytes changed")); }
    let stat = file.stat()?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    hash.write(&bytes);
    let actual = FileFingerprint { mtime: stat.mtime, size: stat.len, hash: hash.finish() };
    if actual != expected { return Err(super::RecoveryError::Invalid("saved fingerprint changed")); }
    file.sync_all()?;
    let parent = target.parent().filter(|p| !p.as_os_str().is_empty())
        .ok_or(super::RecoveryError::Invalid("saved target must have resolved parent"))?;
    fs.sync_dir_strict(parent)?;
    Ok(RecoverySaveReceipt { owner })
}

#[cfg(test)]
#[path = "cleanup_tests.rs"]
mod cleanup_tests;
