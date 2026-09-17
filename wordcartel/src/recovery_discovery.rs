//! Worker-side recovery discovery and nondestructive selection preparation.
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use crate::fsx::{Fs, RecoveryLease};
use crate::recovery_store::{decode, RecoveryError, TaggedPath, MAX_METADATA, MAX_RECORD_BYTES};

/// Contextual discovery matches the current association, or provenance when pathless.
#[derive(Clone, Debug)]
pub enum ScanScope { All, Associated(PathBuf) }
/// Concrete selection discriminator. Legacy identity is best-effort, never deletion authority.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SelectionToken {
    V2 { owner: String, generation: u64 },
    Legacy { path: PathBuf, len: u64, mtime: Option<SystemTime>, body_hash: u64 },
}
/// Civil checkpoint time or explicitly labelled filesystem fallback.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CandidateTime { Checkpoint(u64), LegacyMtime(Option<SystemTime>), Unknown }
/// Bounded display metadata; never retains a full body.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub source_path: PathBuf,
    pub token: Option<SelectionToken>,
    pub association: Option<TaggedPath>,
    pub provenance: Option<TaggedPath>,
    pub lineage: Option<String>,
    pub timestamp: CandidateTime,
    pub preview: String,
    pub unavailable: Option<String>,
    /// An actively leased source; automatic offers omit this row.
    pub busy: bool,
}
/// A selected body plus its held source ownership, for the combined successor transaction.
pub struct PreparedRecovery {
    pub body: String,
    pub candidate: Candidate,
    lease: Option<Box<dyn RecoveryLease>>,
}
impl PreparedRecovery {
    /// Consume this preparation without releasing ownership before handoff.
    pub(crate) fn into_parts(self) -> (Candidate, String, Option<Box<dyn RecoveryLease>>) {
        (self.candidate, self.body, self.lease)
    }
}
/// A changed row must be selected again; it is never silently imported.
pub enum PrepareOutcome { Ready(PreparedRecovery), Changed(Candidate) }

fn unavailable(path: PathBuf, error: RecoveryError) -> Candidate {
    let busy = matches!(&error, RecoveryError::Io(e) if e.kind() == std::io::ErrorKind::WouldBlock);
    Candidate { source_path: path, token: None, association: None, provenance: None, lineage: None,
        timestamp: CandidateTime::Unknown, preview: String::new(), unavailable: Some(error.to_string()), busy }
}
use crate::recovery_store::validate_owner;
fn normalized(fs: &dyn Fs, path: &Path) -> Option<PathBuf> {
    crate::fsx::canonicalize_with_missing_suffix(fs, path).ok()
}
/// Scan existing storage without provisioning it. Corruption is a disabled row; root errors propagate.
pub fn scan(fs: &dyn Fs, root: &Path, scope: &ScanScope) -> Result<Vec<Candidate>, RecoveryError> {
    let root = match fs.canonicalize_existing(root) {
        Ok(root) => root,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let listing = fs.list_dir(&root, None)?;
    let mut rows = Vec::new();
    if listing.unreadable != 0 { rows.push(unavailable(root.clone(), RecoveryError::Invalid("Unreadable recovery directory entries"))); }
    for entry in listing.entries {
        let path = root.join(&entry.raw_name);
        if entry.raw_name == "recovery-v2" {
            if let Err(error) = fs.validate_private_dir(&path) { rows.push(unavailable(path, error.into())); continue; }
            let owners = match fs.list_dir(&path, None) {
                Ok(owners) => owners,
                Err(error) => { rows.push(unavailable(path, error.into())); continue; },
            };
            if owners.unreadable != 0 { rows.push(unavailable(path.clone(), RecoveryError::Invalid("Unreadable recovery owners"))); }
            for owner in owners.entries {
                let record = path.join(&owner.raw_name).join("checkpoint.wcr");
                match read_v2(fs, &root, &record, |row, _, _| row) {
                    Ok(Some(row)) => rows.push(row),
                    Ok(None) => (),
                    Err(error) => rows.push(unavailable(record, error)),
                }
            }
        } else if path.extension().is_some_and(|s| s == "swp") || is_dump(&path) {
            match read_legacy(fs, &path, |row, _, _| row) {
                Ok(row) => rows.push(row),
                Err(error) => rows.push(unavailable(path, error)),
            }
        }
    }
    if let ScanScope::Associated(path) = scope {
        let target = normalized(fs, path);
        rows.retain(|row| {
            // Surface errors with unknown association rather than pretending the scan was empty.
            (!row.busy && row.unavailable.is_some()) || target.as_ref().is_some_and(|target| row.association.as_ref().or(row.provenance.as_ref())
                .and_then(TaggedPath::local_path).and_then(|p| normalized(fs, &p)).as_ref() == Some(target))
        });
    }
    rows.sort_by(|a, b| time_key(&b.timestamp).cmp(&time_key(&a.timestamp))
        .then_with(|| a.source_path.cmp(&b.source_path)));
    Ok(rows)
}
fn time_key(time: &CandidateTime) -> Option<u128> {
    match time {
        CandidateTime::Checkpoint(ms) => Some(*ms as u128),
        CandidateTime::LegacyMtime(time) => time.and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map(|d| d.as_millis()),
        CandidateTime::Unknown => None,
    }
}
fn is_dump(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name.as_encoded_bytes().starts_with(b"recovered-"))
        && path.extension().is_some_and(|s| s == "md")
}
fn read_v2<T>(fs: &dyn Fs, root: &Path, path: &Path,
    consume: impl FnOnce(Candidate, &str, Option<Box<dyn RecoveryLease>>) -> T,
) -> Result<Option<T>, RecoveryError> {
    let dir = path.parent().ok_or(RecoveryError::Invalid("record parent"))?;
    let owner = dir.file_name().and_then(|s| s.to_str()).ok_or(RecoveryError::Invalid("owner encoding"))?;
    validate_owner(owner)?;
    if dir.parent() != Some(root.join("recovery-v2").as_path()) || path.file_name() != Some(std::ffi::OsStr::new("checkpoint.wcr")) {
        return Err(RecoveryError::Invalid("source outside recovery root"));
    }
    fs.validate_private_dir(&root.join("recovery-v2"))?;
    fs.validate_private_dir(dir)?;
    let lease = fs.try_recovery_lock(&dir.join("owner.lock"), false)?;
    let mut file = match fs.open_regular_nofollow(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Classify tombstones while holding the same source lease used by writers.
            // Unknown entries, partial checkpoints and enumeration failures stay visible.
            let contents = fs.list_dir(dir, None)?;
            if contents.unreadable == 0 && contents.entries.iter().all(|entry| entry.raw_name == "owner.lock") {
                return Ok(None);
            }
            return Err(error.into());
        },
        Err(error) => return Err(error.into()),
    };
    let bytes = file.read_capped(MAX_RECORD_BYTES)?.ok_or(RecoveryError::TooLarge)?;
    let (metadata, body) = decode(&bytes)?;
    if metadata.owner() != owner { return Err(RecoveryError::Invalid("owner directory mismatch")); }
    let row = Candidate { source_path: path.to_owned(), token: Some(SelectionToken::V2 {
        owner: owner.to_owned(), generation: metadata.record().generation() }),
        association: metadata.record().association().cloned(),
        provenance: metadata.record().provenance().cloned(), lineage: Some(metadata.record().lineage().to_owned()),
        timestamp: metadata.record().timestamp_ms().map(CandidateTime::Checkpoint).unwrap_or(CandidateTime::Unknown),
        preview: body.chars().take(240).collect(), unavailable: None, busy: false };
    Ok(Some(consume(row, body, Some(lease))))
}
fn read_legacy<T>(fs: &dyn Fs, path: &Path,
    consume: impl FnOnce(Candidate, &str, Option<Box<dyn RecoveryLease>>) -> T,
) -> Result<T, RecoveryError> {
    let swap = path.extension().is_some_and(|s| s == "swp");
    if !swap && !is_dump(path) { return Err(RecoveryError::Invalid("legacy source name")); }
    let mut file = fs.open_regular_nofollow(path)?;
    let stat = file.stat()?;
    let cap = crate::limits::MAX_OPEN_BYTES + if swap { MAX_METADATA as u64 + 5 } else { 0 };
    let bytes = file.read_capped(cap)?.ok_or(RecoveryError::TooLarge)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| RecoveryError::Invalid("legacy UTF-8"))?;
    let (association, lineage, body) = if swap {
        let (header, body) = text.split_once("\n---\n").ok_or(RecoveryError::Invalid("legacy header"))?;
        if header.len() > MAX_METADATA || body.len() as u64 > crate::limits::MAX_OPEN_BYTES { return Err(RecoveryError::TooLarge); }
        let (header, body) = crate::swap::parse_borrowed(text).ok_or(RecoveryError::Invalid("legacy header"))?;
        (header.realpath.map(|p| TaggedPath::from_path(Path::new(&p))), header.id, body)
    } else { (None, None, text) };
    let row = Candidate { source_path: path.to_owned(), token: Some(SelectionToken::Legacy {
        path: path.to_owned(), len: stat.len, mtime: stat.mtime, body_hash: crate::swap::fnv1a64(body.as_bytes()) }),
        association, provenance: None, lineage, timestamp: CandidateTime::LegacyMtime(stat.mtime),
        preview: body.chars().take(240).collect(), unavailable: None, busy: false };
    Ok(consume(row, body, None))
}
fn prepared_body(candidate: Candidate, body: &str, lease: Option<Box<dyn RecoveryLease>>) -> PreparedRecovery {
    PreparedRecovery { candidate, body: body.to_owned(), lease }
}
/// Reacquire and reread selected bytes. No path is removed by this operation.
pub fn prepare(fs: &dyn Fs, root: &Path, selected: &Candidate) -> Result<PrepareOutcome, RecoveryError> {
    let root = fs.canonicalize_existing(root)?;
    let prepared = match selected.token.as_ref().ok_or(RecoveryError::Invalid("unavailable selection"))? {
        SelectionToken::V2 { owner, .. } => {
            validate_owner(owner)?;
            let expected = root.join("recovery-v2").join(owner).join("checkpoint.wcr");
            if selected.source_path != expected { return Err(RecoveryError::Invalid("selection owner path mismatch")); }
            read_v2(fs, &root, &expected, prepared_body)?.ok_or(RecoveryError::Invalid("selected recovery source was removed"))?
        },
        SelectionToken::Legacy { .. } => {
            if selected.source_path.parent() != Some(root.as_path()) { return Err(RecoveryError::Invalid("legacy source outside root")); }
            read_legacy(fs, &selected.source_path, prepared_body)?
        },
    };
    if prepared.candidate.token != selected.token { return Ok(PrepareOutcome::Changed(prepared.candidate)); }
    Ok(PrepareOutcome::Ready(prepared))
}
#[cfg(test)]
#[path = "recovery_discovery/tests.rs"]
mod discovery_tests;
