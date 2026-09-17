//! Bounded, lossless v2 wire format. Decoding grants no deletion authority.
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use super::RecoveryError;
const MAGIC: &[u8; 17] = b"WCARTEL-RECOVERY\0";
/// Maximum JSON header size, independent of body cap.
pub const MAX_METADATA: usize = 65536;
/// Full capped record read size including magic, lengths, header, and body.
pub const MAX_RECORD_BYTES: u64 = 25 + MAX_METADATA as u64 + crate::limits::MAX_OPEN_BYTES;
/// Maximum supported Unix timestamp: end of year 9999, in milliseconds.
const MAX_TIMESTAMP: u64 = 253402300799999;

/// Lossless platform-tagged pathname. Foreign paths never become local destinations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "platform", content = "units", deny_unknown_fields)]
pub enum TaggedPath {
    /// Unix native filename bytes.
    Unix(Vec<u8>),
    /// Windows native UTF-16 filename units.
    Windows(Vec<u16>),
}
impl TaggedPath {
    /// Capture local path units without lossy Unicode conversion.
    pub fn from_path(path: &Path) -> Self {
        #[cfg(unix)]
        { use std::os::unix::ffi::OsStrExt; Self::Unix(path.as_os_str().as_bytes().to_vec()) }
        #[cfg(windows)]
        { use std::os::windows::ffi::OsStrExt; Self::Windows(path.as_os_str().encode_wide().collect()) }
    }
    /// Return a destination only on the platform whose encoding this record carries.
    pub fn local_path(&self) -> Option<PathBuf> {
        match self {
            Self::Unix(units) => {
                #[cfg(unix)]
                { use std::os::unix::ffi::OsStringExt;
                    Some(std::ffi::OsString::from_vec(units.clone()).into()) }
                #[cfg(not(unix))]
                { let _ = units; None }
            }
            Self::Windows(units) => {
                #[cfg(windows)]
                { use std::os::windows::ffi::OsStringExt;
                    Some(std::ffi::OsString::from_wide(units).into()) }
                #[cfg(not(windows))]
                { let _ = units; None }
            }
        }
    }
    /// Losslessly escaped display, including foreign-platform names.
    pub fn escaped(&self) -> String {
        match self {
            Self::Unix(units) => units.iter().flat_map(|b| std::ascii::escape_default(*b))
                .map(char::from).collect(),
            Self::Windows(units) => char::decode_utf16(units.iter().copied()).map(|c| {
                match c {
                    Ok(c) => c.escape_default().to_string(),
                    Err(e) => format!("\\u{{{:04x}}}", e.unpaired_surrogate()),
                }
            }).collect(),
        }
    }
    fn validate(&self) -> Result<(), RecoveryError> {
        let invalid = match self {
            Self::Unix(units) => units.is_empty() || units.contains(&0),
            Self::Windows(units) => units.is_empty() || units.contains(&0),
        };
        if invalid { Err(RecoveryError::Invalid("empty or NUL pathname")) } else { Ok(()) }
    }
}

/// Snapshot metadata captured before dispatch, including its reserved generation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointRecord {
    generation: u64,
    lineage: String,
    edit_version: u64,
    association: Option<TaggedPath>,
    provenance: Option<TaggedPath>,
    timestamp_ms: Option<u64>,
    predecessor: Option<(String, u64)>,
}
impl CheckpointRecord {
    /// Capture a snapshot; unknown wall time is explicit. Validation occurs at encoding.
    pub fn new(generation: u64, lineage: String, edit_version: u64,
        association: Option<TaggedPath>, provenance: Option<TaggedPath>) -> Self
    {
        let timestamp_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .ok().and_then(|d| u64::try_from(d.as_millis()).ok()).filter(|&t| t <= MAX_TIMESTAMP);
        Self { generation, lineage, edit_version, association, provenance, timestamp_ms,
            predecessor: None }
    }
    /// Attach initial-import predecessor identity; this is metadata, not retirement authority.
    pub fn with_predecessor(mut self, owner: String, generation: u64) -> Self {
        self.predecessor = Some((owner, generation)); self
    }
    /// Reserved checkpoint generation.
    pub fn generation(&self) -> u64 { self.generation }
    /// Opaque document lineage hint.
    pub fn lineage(&self) -> &str { &self.lineage }
    /// Captured edit version.
    pub fn edit_version(&self) -> u64 { self.edit_version }
    /// Current filename association.
    pub fn association(&self) -> Option<&TaggedPath> { self.association.as_ref() }
    /// Original recovered filename context.
    pub fn provenance(&self) -> Option<&TaggedPath> { self.provenance.as_ref() }
    /// Validated wall-clock Unix milliseconds, if available.
    pub fn timestamp_ms(&self) -> Option<u64> { self.timestamp_ms }
    /// Original record hint for initial handoff.
    pub fn predecessor(&self) -> Option<(&str, u64)> {
        self.predecessor.as_ref().map(|(o, g)| (o.as_str(), *g))
    }
}

/// Validated wire metadata; constructors and decoder enforce bounds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata { owner: String, record: CheckpointRecord, body_bytes: u64 }
impl Metadata {
    /// Build metadata for an exclusively allocated owner.
    pub fn new(owner: String, record: CheckpointRecord, body_bytes: u64)
        -> Result<Self, RecoveryError>
    {
        let value = Self { owner, record, body_bytes }; value.validate()?; Ok(value)
    }
    /// Opaque owner component.
    pub fn owner(&self) -> &str { &self.owner }
    /// Snapshot context.
    pub fn record(&self) -> &CheckpointRecord { &self.record }
    /// Exact UTF-8 body byte count.
    pub fn body_bytes(&self) -> u64 { self.body_bytes }
    fn validate(&self) -> Result<(), RecoveryError> {
        validate_owner(&self.owner)?;
        if self.record.generation == 0 || self.record.lineage.is_empty() {
            return Err(RecoveryError::Invalid("generation or lineage"));
        }
        if self.body_bytes > crate::limits::MAX_OPEN_BYTES { return Err(RecoveryError::TooLarge); }
        if self.record.timestamp_ms.is_some_and(|t| t > MAX_TIMESTAMP) {
            return Err(RecoveryError::Invalid("timestamp range"));
        }
        for path in [&self.record.association, &self.record.provenance].into_iter().flatten() {
            path.validate()?;
        }
        if let Some((owner, generation)) = &self.record.predecessor {
            validate_owner(owner)?;
            if *generation == 0 { return Err(RecoveryError::Invalid("predecessor generation")); }
        }
        Ok(())
    }
}
pub(crate) fn validate_owner(owner: &str) -> Result<(), RecoveryError> {
    if owner.len() != 16 || !owner.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        Err(RecoveryError::Invalid("owner component"))
    } else { Ok(()) }
}

/// Encode one exact record. Body and header each have independent caps.
pub fn encode(metadata: &Metadata, body: &str) -> Result<Vec<u8>, RecoveryError> {
    metadata.validate()?;
    if metadata.body_bytes != body.len() as u64 { return Err(RecoveryError::Invalid("body count")); }
    let json = serde_json::to_vec(metadata).map_err(|_| RecoveryError::Invalid("metadata JSON"))?;
    if json.len() > MAX_METADATA { return Err(RecoveryError::TooLarge); }
    let length = 25usize.checked_add(json.len()).and_then(|n| n.checked_add(body.len()))
        .ok_or(RecoveryError::TooLarge)?;
    let mut bytes = Vec::with_capacity(length);
    bytes.extend_from_slice(MAGIC); bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.extend_from_slice(&(json.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&json); bytes.extend_from_slice(body.as_bytes()); Ok(bytes)
}

/// Decode an exact bounded record, rejecting malformed metadata, UTF-8 and trailing bytes.
pub fn decode(bytes: &[u8]) -> Result<(Metadata, &str), RecoveryError> {
    if bytes.len() < 25 || &bytes[..17] != MAGIC || bytes[17..21] != 2u32.to_le_bytes() {
        return Err(RecoveryError::Invalid("magic or format version"));
    }
    let length = u32::from_le_bytes(bytes[21..25].try_into().expect("four-byte prefix")) as usize;
    if length > MAX_METADATA { return Err(RecoveryError::TooLarge); }
    let end = 25usize.checked_add(length).ok_or(RecoveryError::TooLarge)?;
    let json = bytes.get(25..end).ok_or(RecoveryError::Invalid("truncated header"))?;
    let metadata: Metadata = serde_json::from_slice(json)
        .map_err(|_| RecoveryError::Invalid("metadata JSON"))?;
    metadata.validate()?;
    let body = bytes.get(end..).ok_or(RecoveryError::Invalid("truncated body"))?;
    if body.len() as u64 != metadata.body_bytes { return Err(RecoveryError::Invalid("body count")); }
    let body = std::str::from_utf8(body).map_err(|_| RecoveryError::Invalid("body UTF-8"))?;
    Ok((metadata, body))
}
