//! Write-Ahead Journal — crash-safe persistence for AURA identity state.
//!
//! Entry format (binary, little-endian):
//! ```text
//! [length: u32][checksum: u32][timestamp_ms: u64][category: u8][payload: N bytes]
//! ```
//! where `length` = 8 (timestamp) + 1 (category) + payload.len().
//!
//! The journal is append-only.  `commit()` writes a commit marker (category=0xFF)
//! that groups preceding entries into a transaction.  On recovery, only
//! committed transactions are replayed; incomplete tails are discarded.
//!
//! ## Constraints
//! - Max file size: 1 MB.  `compact()` rewrites only committed entries.
//! - CRC32 (IEEE) on every entry.  Corrupt entries abort recovery at that point.
//! - `fsync` after every write.  No buffered I/O.
//! - No external crate dependencies — uses `std::fs` only.

use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Hard limit on journal file size (1 MB).
const MAX_JOURNAL_BYTES: u64 = 1_024 * 1_024;

/// Maximum number of entries returned from a single recovery pass.
/// The 1 MB file cap already bounds this in practice, but this enforces an
/// explicit, auditable limit so recovery Vecs are never unbounded in code.
const MAX_RECOVERY_ENTRIES: usize = 8_192;

/// Header size per entry: length(4) + checksum(4) = 8 bytes.
const ENTRY_HEADER_SIZE: usize = 8;

/// Minimum entry body: timestamp(8) + category(1) = 9 bytes.
const ENTRY_MIN_BODY: usize = 9;

/// Commit marker category byte.
const COMMIT_MARKER: u8 = 0xFF;

// ---------------------------------------------------------------------------
// CRC32 — IEEE polynomial, no external crate
// ---------------------------------------------------------------------------

/// CRC32 lookup table (IEEE 802.3 polynomial 0xEDB88320, reflected).
const CRC32_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0u32;
    while i < 256 {
        let mut crc = i;
        let mut j = 0;
        while j < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i as usize] = crc;
        i += 1;
    }
    table
};

/// Compute CRC32 (IEEE) of `data`.
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        let idx = ((crc ^ byte as u32) & 0xFF) as usize;
        crc = (crc >> 8) ^ CRC32_TABLE[idx];
    }
    crc ^ 0xFFFF_FFFF
}

// ---------------------------------------------------------------------------
// JournalCategory — what kind of state is being persisted
// ---------------------------------------------------------------------------

/// Category of a journal entry — tags what identity subsystem it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum JournalCategory {
    /// OCEAN personality trait change.
    Personality = 0,
    /// Per-user trust level change.
    Trust = 1,
    /// Episodic/semantic memory mutation.
    Memory = 2,
    /// Consent state change (privacy sovereignty).
    Consent = 3,
    /// Goal creation/update/completion.
    Goal = 4,
    /// Execution trace (action started/completed/failed).
    Execution = 5,
    /// Mood/affective state change.
    Mood = 6,
}

impl JournalCategory {
    /// Convert a raw byte to a category, returning `None` for unknown values
    /// (except `COMMIT_MARKER` which is handled separately).
    fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::Personality),
            1 => Some(Self::Trust),
            2 => Some(Self::Memory),
            3 => Some(Self::Consent),
            4 => Some(Self::Goal),
            5 => Some(Self::Execution),
            6 => Some(Self::Mood),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// JournalEntry — a single recovered entry
// ---------------------------------------------------------------------------

/// A recovered journal entry with its metadata.
#[derive(Debug, Clone)]
pub struct JournalEntry {
    /// Wall-clock timestamp (ms since UNIX epoch).
    pub timestamp_ms: u64,
    /// What subsystem this entry belongs to.
    pub category: JournalCategory,
    /// Opaque payload (category-specific serialization).
    pub payload: Vec<u8>,
}

// ---------------------------------------------------------------------------
// JournalError
// ---------------------------------------------------------------------------

/// Errors from journal operations.
#[derive(Debug)]
pub enum JournalError {
    /// I/O error (open, read, write, fsync, rename).
    Io(std::io::Error),
    /// Journal file exceeds MAX_JOURNAL_BYTES.
    Full,
    /// Checksum mismatch during recovery.
    CorruptEntry {
        offset: u64,
        expected_crc: u32,
        actual_crc: u32,
    },
    /// Truncated entry (incomplete header or body).
    Truncated { offset: u64 },
    /// Payload too large for a single entry (> 64 KB).
    PayloadTooLarge { size: usize },
}

impl std::fmt::Display for JournalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "journal I/O error: {e}"),
            Self::Full => write!(f, "journal full (>{MAX_JOURNAL_BYTES} bytes)"),
            Self::CorruptEntry {
                offset,
                expected_crc,
                actual_crc,
            } => {
                write!(f, "corrupt entry at offset {offset}: expected CRC {expected_crc:#010X}, got {actual_crc:#010X}")
            },
            Self::Truncated { offset } => write!(f, "truncated entry at offset {offset}"),
            Self::PayloadTooLarge { size } => {
                write!(f, "payload too large: {size} bytes (max 65536)")
            },
        }
    }
}

impl std::error::Error for JournalError {}

impl From<std::io::Error> for JournalError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

// ---------------------------------------------------------------------------
// RecoveryReport — what happened during journal replay
// ---------------------------------------------------------------------------

/// Summary of a journal recovery pass.
#[derive(Debug, Clone)]
pub struct RecoveryReport {
    /// Number of fully committed transactions replayed.
    pub committed_transactions: usize,
    /// Total entries across all committed transactions.
    pub committed_entries: usize,
    /// Entries that were part of an uncommitted (incomplete) transaction.
    pub uncommitted_entries: usize,
    /// Whether any corruption was detected (stops replay at that point).
    pub corruption_detected: bool,
    /// File offset where corruption was first detected (0 if none).
    pub corruption_offset: u64,
}

// ---------------------------------------------------------------------------
// WriteAheadJournal
// ---------------------------------------------------------------------------

/// Append-only write-ahead journal for crash-safe identity persistence.
///
/// # Usage pattern
/// ```ignore
/// journal.append(JournalCategory::Personality, &payload)?;
/// journal.append(JournalCategory::Trust, &trust_payload)?;
/// journal.commit()?;  // makes the above two entries durable
/// ```
///
/// On startup, call `recover()` to replay only committed transactions.
pub struct WriteAheadJournal {
    /// Path to the journal file.
    path: PathBuf,
    /// Open file handle (append mode).
    file: File,
    /// Current file size (tracked to avoid repeated seeks).
    file_size: u64,
    /// Number of uncommitted entries since last `commit()`.
    pending_count: u32,
}

impl WriteAheadJournal {
    /// Open or create a journal file at `path`.
    ///
    /// If the file already exists, the cursor is positioned at the end
    /// for appending.  The caller should call `recover()` before
    /// appending new entries to replay any committed data.
    pub fn new(path: &Path) -> Result<Self, JournalError> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(path)?;

        let file_size = file.metadata()?.len();

        tracing::info!(
            path = %path.display(),
            size = file_size,
            "journal opened"
        );

        Ok(Self {
            path: path.to_path_buf(),
            file,
            file_size,
            pending_count: 0,
        })
    }

    /// Append a single entry to the journal.
    ///
    /// The entry is NOT durable until `commit()` is called.  However it IS
    /// written to disk and fsync'd immediately so that a crash between
    /// `append` and `commit` doesn't lose the data — it just won't be
    /// replayed (uncommitted tail is discarded on recovery).
    ///
    /// # Errors
    /// - `PayloadTooLarge` if `payload.len() > 65536`.
    /// - `Full` if writing this entry would exceed `MAX_JOURNAL_BYTES`.
    /// - `Io` on write/sync failure.
    pub fn append(
        &mut self,
        category: JournalCategory,
        payload: &[u8],
    ) -> Result<(), JournalError> {
        if payload.len() > 65536 {
            return Err(JournalError::PayloadTooLarge {
                size: payload.len(),
            });
        }

        let timestamp_ms = now_ms();
        let body_len = ENTRY_MIN_BODY + payload.len();

        // Check file size limit before writing.
        let entry_total = ENTRY_HEADER_SIZE as u64 + body_len as u64;
        if self.file_size + entry_total > MAX_JOURNAL_BYTES {
            return Err(JournalError::Full);
        }

        // Build the body: [timestamp: 8][category: 1][payload: N]
        let mut body = Vec::with_capacity(body_len);
        body.extend_from_slice(&timestamp_ms.to_le_bytes());
        body.push(category as u8);
        body.extend_from_slice(payload);

        let checksum = crc32(&body);
        let length = body_len as u32;

        // Seek to end, write header + body, fsync.
        self.file.seek(SeekFrom::End(0))?;
        self.file.write_all(&length.to_le_bytes())?;
        self.file.write_all(&checksum.to_le_bytes())?;
        self.file.write_all(&body)?;
        self.file.sync_all()?;

        self.file_size += entry_total;
        self.pending_count += 1;

        Ok(())
    }

    /// Write a commit marker, making all preceding uncommitted entries durable.
    ///
    /// The commit marker is a special entry with `category = 0xFF` and empty
    /// payload.  On recovery, entries are only replayed up to the last commit
    /// marker.
    pub fn commit(&mut self) -> Result<(), JournalError> {
        if self.pending_count == 0 {
            return Ok(()); // nothing to commit
        }

        let timestamp_ms = now_ms();
        let body_len = ENTRY_MIN_BODY; // no payload for commit marker

        let entry_total = ENTRY_HEADER_SIZE as u64 + body_len as u64;
        if self.file_size + entry_total > MAX_JOURNAL_BYTES {
            return Err(JournalError::Full);
        }

        let mut body = Vec::with_capacity(body_len);
        body.extend_from_slice(&timestamp_ms.to_le_bytes());
        body.push(COMMIT_MARKER);

        let checksum = crc32(&body);
        let length = body_len as u32;

        self.file.seek(SeekFrom::End(0))?;
        self.file.write_all(&length.to_le_bytes())?;
        self.file.write_all(&checksum.to_le_bytes())?;
        self.file.write_all(&body)?;
        self.file.sync_all()?;

        self.file_size += entry_total;

        tracing::debug!(
            committed = self.pending_count,
            file_size = self.file_size,
            "journal transaction committed"
        );

        self.pending_count = 0;
        Ok(())
    }

    /// Recover committed entries from the journal file.
    ///
    /// Reads all entries from the beginning of the file.  Only entries that
    /// precede a commit marker are included in the result.  If corruption
    /// is detected, recovery stops at the corrupt entry — everything before
    /// the last valid commit marker is still returned.
    ///
    /// After recovery, the journal is truncated to the end of the last
    /// committed transaction (discarding any uncommitted tail and corruption).
    pub fn recover(&mut self) -> Result<(Vec<JournalEntry>, RecoveryReport), JournalError> {
        self.file.seek(SeekFrom::Start(0))?;

        let mut all_entries: Vec<JournalEntry> = Vec::new();
        let mut pending_entries: Vec<JournalEntry> = Vec::new();
        let mut committed_transactions: usize = 0;
        let mut corruption_detected = false;
        let mut corruption_offset: u64 = 0;
        let mut offset: u64 = 0;
        let mut last_commit_offset: u64 = 0;

        loop {
            // Try to read entry header: length(4) + checksum(4).
            let mut header_buf = [0u8; ENTRY_HEADER_SIZE];
            match read_exact_or_eof(&mut self.file, &mut header_buf) {
                Ok(true) => {}, // got full header
                Ok(false) => {
                    // Clean EOF — no more entries.
                    break;
                },
                Err(e) => return Err(JournalError::Io(e)),
            }

            let length =
                u32::from_le_bytes([header_buf[0], header_buf[1], header_buf[2], header_buf[3]]);
            let stored_checksum =
                u32::from_le_bytes([header_buf[4], header_buf[5], header_buf[6], header_buf[7]]);

            // Sanity: length must be at least ENTRY_MIN_BODY and at most 65536 + 9.
            let body_len = length as usize;
            if body_len < ENTRY_MIN_BODY || body_len > 65536 + ENTRY_MIN_BODY {
                tracing::warn!(
                    offset,
                    length,
                    "journal: invalid entry length — treating as truncation"
                );
                corruption_detected = true;
                corruption_offset = offset;
                break;
            }

            // Read body.
            let mut body = vec![0u8; body_len];
            match read_exact_or_eof(&mut self.file, &mut body) {
                Ok(true) => {},
                Ok(false) => {
                    // Truncated entry — treat as incomplete write.
                    tracing::warn!(offset, "journal: truncated entry body");
                    corruption_detected = true;
                    corruption_offset = offset;
                    break;
                },
                Err(e) => return Err(JournalError::Io(e)),
            }

            // Verify checksum.
            let actual_checksum = crc32(&body);
            if actual_checksum != stored_checksum {
                tracing::warn!(
                    offset,
                    expected = stored_checksum,
                    actual = actual_checksum,
                    "journal: CRC mismatch — stopping recovery"
                );
                corruption_detected = true;
                corruption_offset = offset;
                break;
            }

            // Parse body: [timestamp: 8][category: 1][payload: rest]
            let timestamp_ms = u64::from_le_bytes([
                body[0], body[1], body[2], body[3], body[4], body[5], body[6], body[7],
            ]);
            let cat_byte = body[8];

            let entry_end = offset + ENTRY_HEADER_SIZE as u64 + body_len as u64;

            if cat_byte == COMMIT_MARKER {
                // Commit marker — promote pending entries to committed.
                // Cap total committed entries to MAX_RECOVERY_ENTRIES.
                let remaining = MAX_RECOVERY_ENTRIES.saturating_sub(all_entries.len());
                if pending_entries.len() > remaining {
                    pending_entries.truncate(remaining);
                }
                all_entries.append(&mut pending_entries);
                committed_transactions += 1;
                last_commit_offset = entry_end;
            } else if let Some(category) = JournalCategory::from_byte(cat_byte) {
                // Only buffer up to MAX_RECOVERY_ENTRIES pending entries.
                if all_entries.len() + pending_entries.len() < MAX_RECOVERY_ENTRIES {
                    pending_entries.push(JournalEntry {
                        timestamp_ms,
                        category,
                        payload: body[ENTRY_MIN_BODY..].to_vec(),
                    });
                }
            } else {
                tracing::warn!(
                    offset,
                    category = cat_byte,
                    "journal: unknown category — skipping"
                );
            }

            offset = entry_end;
        }

        let uncommitted_entries = pending_entries.len();
        let committed_entries = all_entries.len();

        // Truncate file to the end of the last committed transaction,
        // discarding any uncommitted tail or corruption.
        if last_commit_offset < self.file_size {
            tracing::info!(
                truncating_from = last_commit_offset,
                old_size = self.file_size,
                uncommitted = uncommitted_entries,
                "journal: truncating uncommitted tail"
            );
            self.file.set_len(last_commit_offset)?;
            self.file.sync_all()?;
            self.file_size = last_commit_offset;
        }

        self.pending_count = 0;

        let report = RecoveryReport {
            committed_transactions,
            committed_entries,
            uncommitted_entries,
            corruption_detected,
            corruption_offset,
        };

        tracing::info!(
            transactions = committed_transactions,
            entries = committed_entries,
            uncommitted = uncommitted_entries,
            corruption = corruption_detected,
            "journal recovery complete"
        );

        Ok((all_entries, report))
    }

    /// Compact the journal by rewriting only committed entries.
    ///
    /// This is useful when the journal approaches `MAX_JOURNAL_BYTES`.
    /// The caller provides the entries to keep (typically the latest state
    /// snapshot — one entry per category).
    ///
    /// Writes to a `.tmp` file, fsyncs, then atomically renames.
    pub fn compact(&mut self, entries: &[JournalEntry]) -> Result<(), JournalError> {
        let tmp_path = self.path.with_extension("wal.tmp");

        {
            let mut tmp = File::create(&tmp_path)?;

            for entry in entries {
                let payload = &entry.payload;
                let body_len = ENTRY_MIN_BODY + payload.len();

                let mut body = Vec::with_capacity(body_len);
                body.extend_from_slice(&entry.timestamp_ms.to_le_bytes());
                body.push(entry.category as u8);
                body.extend_from_slice(payload);

                let checksum = crc32(&body);
                let length = body_len as u32;

                tmp.write_all(&length.to_le_bytes())?;
                tmp.write_all(&checksum.to_le_bytes())?;
                tmp.write_all(&body)?;
            }

            // Write a commit marker after all entries.
            if !entries.is_empty() {
                let timestamp_ms = now_ms();
                let mut body = Vec::with_capacity(ENTRY_MIN_BODY);
                body.extend_from_slice(&timestamp_ms.to_le_bytes());
                body.push(COMMIT_MARKER);

                let checksum = crc32(&body);
                let length = ENTRY_MIN_BODY as u32;

                tmp.write_all(&length.to_le_bytes())?;
                tmp.write_all(&checksum.to_le_bytes())?;
                tmp.write_all(&body)?;
            }

            tmp.sync_all()?;
        }

        // Atomic rename: tmp → journal.
        std::fs::rename(&tmp_path, &self.path)?;

        // Reopen the file.
        self.file = OpenOptions::new().read(true).write(true).open(&self.path)?;
        self.file_size = self.file.metadata()?.len();
        self.pending_count = 0;

        tracing::info!(
            entries = entries.len(),
            new_size = self.file_size,
            "journal compacted"
        );

        Ok(())
    }

    /// Returns the current journal file size in bytes.
    pub fn file_size(&self) -> u64 {
        self.file_size
    }

    /// Returns the number of uncommitted entries.
    pub fn pending_count(&self) -> u32 {
        self.pending_count
    }

    /// Returns the journal file path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read exactly `buf.len()` bytes, returning `Ok(false)` on clean EOF
/// (zero bytes read) and `Err` on partial read.
fn read_exact_or_eof(file: &mut File, buf: &mut [u8]) -> std::io::Result<bool> {
    let mut total = 0;
    while total < buf.len() {
        match file.read(&mut buf[total..]) {
            Ok(0) => {
                if total == 0 {
                    return Ok(false); // clean EOF
                }
                // Partial read — file is truncated.
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!("read {total}/{} bytes before EOF", buf.len()),
                ));
            },
            Ok(n) => total += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

/// Current wall-clock time in milliseconds since UNIX epoch.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_journal() -> (tempfile::TempDir, WriteAheadJournal) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.wal");
        let journal = WriteAheadJournal::new(&path).expect("open journal");
        (dir, journal)
    }

    #[test]
    fn test_append_and_commit_then_recover() {
        let (_dir, mut journal) = temp_journal();

        // Write two entries and commit.
        journal
            .append(JournalCategory::Personality, b"ocean_update_1")
            .expect("append 1");
        journal
            .append(JournalCategory::Trust, b"trust_delta_user1")
            .expect("append 2");
        journal.commit().expect("commit");

        // Recover — should get both entries.
        let (entries, report) = journal.recover().expect("recover");
        assert_eq!(report.committed_transactions, 1);
        assert_eq!(report.committed_entries, 2);
        assert_eq!(report.uncommitted_entries, 0);
        assert!(!report.corruption_detected);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].category, JournalCategory::Personality);
        assert_eq!(entries[0].payload, b"ocean_update_1");
        assert_eq!(entries[1].category, JournalCategory::Trust);
    }

    #[test]
    fn test_uncommitted_entries_discarded_on_recover() {
        let (_dir, mut journal) = temp_journal();

        // Committed transaction.
        journal
            .append(JournalCategory::Personality, b"committed")
            .expect("append");
        journal.commit().expect("commit");

        // Uncommitted entries (simulating crash before commit).
        journal
            .append(JournalCategory::Trust, b"uncommitted_1")
            .expect("append");
        journal
            .append(JournalCategory::Memory, b"uncommitted_2")
            .expect("append");

        // Recover — should only get the committed entry.
        let (entries, report) = journal.recover().expect("recover");
        assert_eq!(report.committed_entries, 1);
        assert_eq!(report.uncommitted_entries, 2);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].payload, b"committed");
    }

    #[test]
    fn test_empty_journal_recovery() {
        let (_dir, mut journal) = temp_journal();

        let (entries, report) = journal.recover().expect("recover");
        assert_eq!(entries.len(), 0);
        assert_eq!(report.committed_transactions, 0);
        assert!(!report.corruption_detected);
    }

    #[test]
    fn test_compact_reduces_size() {
        let (_dir, mut journal) = temp_journal();

        // Write many entries across multiple transactions.
        for i in 0..20u8 {
            journal
                .append(JournalCategory::Personality, &[i; 100])
                .expect("append");
            journal.commit().expect("commit");
        }

        let size_before = journal.file_size();

        // Compact to just the last entry.
        let keep = vec![JournalEntry {
            timestamp_ms: now_ms(),
            category: JournalCategory::Personality,
            payload: vec![19u8; 100],
        }];
        journal.compact(&keep).expect("compact");

        assert!(journal.file_size() < size_before);

        // Verify recovery after compaction.
        let (entries, report) = journal.recover().expect("recover");
        assert_eq!(entries.len(), 1);
        assert_eq!(report.committed_transactions, 1);
    }

    #[test]
    fn test_payload_too_large_rejected() {
        let (_dir, mut journal) = temp_journal();

        let big_payload = vec![0u8; 65537];
        let result = journal.append(JournalCategory::Memory, &big_payload);
        assert!(matches!(result, Err(JournalError::PayloadTooLarge { .. })));
    }

    #[test]
    fn test_crc32_correctness() {
        // Known test vector: CRC32 of "123456789" = 0xCBF43926
        let result = crc32(b"123456789");
        assert_eq!(result, 0xCBF4_3926);
    }

    #[test]
    fn test_corruption_detected() {
        let (dir, mut journal) = temp_journal();

        journal
            .append(JournalCategory::Personality, b"good_entry")
            .expect("append");
        journal.commit().expect("commit");

        let path = dir.path().join("test.wal");
        let size = journal.file_size();

        // Corrupt a byte in the middle of the file.
        drop(journal);
        {
            let mut f = OpenOptions::new()
                .write(true)
                .open(&path)
                .expect("open for corruption");
            f.seek(SeekFrom::Start(size / 2)).expect("seek");
            f.write_all(&[0xFF]).expect("corrupt write");
            f.sync_all().expect("sync");
        }

        // Re-open and recover.
        let mut journal2 = WriteAheadJournal::new(&path).expect("reopen");
        let (entries, report) = journal2.recover().expect("recover");

        // The committed entry may or may not survive depending on which
        // byte was corrupted, but corruption must be detected.
        assert!(
            report.corruption_detected || entries.is_empty(),
            "corruption should be detected or no entries returned"
        );
    }

    #[test]
    fn test_multiple_transactions() {
        let (_dir, mut journal) = temp_journal();

        // Transaction 1
        journal
            .append(JournalCategory::Personality, b"tx1_a")
            .expect("append");
        journal
            .append(JournalCategory::Personality, b"tx1_b")
            .expect("append");
        journal.commit().expect("commit 1");

        // Transaction 2
        journal
            .append(JournalCategory::Trust, b"tx2_a")
            .expect("append");
        journal.commit().expect("commit 2");

        // Transaction 3 (uncommitted — should be discarded)
        journal
            .append(JournalCategory::Goal, b"tx3_uncommitted")
            .expect("append");

        let (entries, report) = journal.recover().expect("recover");
        assert_eq!(report.committed_transactions, 2);
        assert_eq!(report.committed_entries, 3);
        assert_eq!(report.uncommitted_entries, 1);
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn test_commit_without_pending_is_noop() {
        let (_dir, mut journal) = temp_journal();
        let size_before = journal.file_size();
        journal.commit().expect("commit noop");
        assert_eq!(journal.file_size(), size_before);
    }
}
