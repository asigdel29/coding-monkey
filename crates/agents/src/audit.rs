/*
   File: crates/agents/src/audit.rs

   Purpose
   Append-only, hash-chained audit log used for SOC 2 evidence.

   On-disk format (one JSON object per line):
       { "ts": ISO-8601, "type": EventType, "fields": { … },
         "prev_hash": HEX(SHA-256 of previous line), "this_hash": HEX(SHA-256 of this line excluding this_hash) }

   Verification (`verify_audit_log`) walks every line and confirms
   prev_hash matches sha256(previous line). Any break is reported with
   the line number so an auditor can investigate.

   Concurrency: a single `AuditLogger` is intended per-session. If you
   need parallel writers, wrap it in `tokio::sync::Mutex<AuditLogger>`.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial Rust port from packages/agents/src/audit.ts
*/

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

/// Discrete event types tracked in the audit log. Add new variants here
/// when a new event class arises — verifiers reject unknown types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditEventType {
    /// Session started.
    SessionStart,
    /// Session ended (clean or via SIGINT).
    SessionEnd,
    /// Agent CLI was spawned.
    AgentSpawn,
    /// Agent CLI exited.
    AgentExit,
    /// Skill (review/cso/investigate/ship) ran.
    SkillRun,
    /// Pre-push pentest gate executed.
    PrePushPentest,
    /// User explicitly bypassed a gate (rare, recorded with reason).
    GateBypass,
    /// Compliance evidence bundle generated.
    EvidenceBundle,
    /// Generic note. Use sparingly.
    Note,
}

/// One line in the audit log. `this_hash` is computed over the
/// canonical-JSON serialization of the entry without `this_hash` itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Wall-clock UTC.
    pub ts: DateTime<Utc>,
    /// Event class.
    #[serde(rename = "type")]
    pub event_type: AuditEventType,
    /// Free-form structured fields.
    #[serde(default)]
    pub fields: serde_json::Value,
    /// Hex of sha256(previous line). For the first line, this is the
    /// hex of sha256("genesis").
    pub prev_hash: String,
    /// Hex of sha256(this entry without this_hash).
    pub this_hash: String,
}

/// Hash-chained writer. Each call to [`AuditLogger::log`] appends one
/// line to the configured path and updates the rolling hash.
#[derive(Debug)]
pub struct AuditLogger {
    path: PathBuf,
    last_hash: String,
}

impl AuditLogger {
    /// Open or create an audit log at `path`. If the file already exists,
    /// the chain is resumed from the last line so subsequent writes
    /// continue the existing chain.
    pub fn open(path: impl Into<PathBuf>) -> std::io::Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let last_hash = if path.exists() {
            last_hash_in_file(&path)?
        } else {
            genesis_hash()
        };
        // Touch the file so subsequent appends succeed even if no events
        // are written this session.
        OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self { path, last_hash })
    }

    /// Path the logger is writing to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one event to the chain.
    pub fn log(
        &mut self,
        event_type: AuditEventType,
        fields: serde_json::Value,
    ) -> std::io::Result<AuditEntry> {
        let mut entry = AuditEntry {
            ts: Utc::now(),
            event_type,
            fields,
            prev_hash: self.last_hash.clone(),
            this_hash: String::new(),
        };
        entry.this_hash = compute_this_hash(&entry);

        let mut file = OpenOptions::new().append(true).open(&self.path)?;
        let line = serde_json::to_string(&entry).map_err(io_other)?;
        writeln!(file, "{line}")?;
        self.last_hash = entry.this_hash.clone();
        Ok(entry)
    }
}

/// Walk an audit log end-to-end, confirming each line's `prev_hash`
/// matches `sha256(previous line)`. Returns `Ok(())` on a clean chain;
/// `Err` carries the first broken line number so an auditor can locate
/// the tampering.
pub fn verify_audit_log(path: &Path) -> Result<(), VerifyError> {
    let file = File::open(path).map_err(VerifyError::Io)?;
    let reader = BufReader::new(file);
    let mut prev_hash = genesis_hash();
    for (lineno, line) in reader.lines().enumerate() {
        let line = line.map_err(VerifyError::Io)?;
        if line.trim().is_empty() {
            continue;
        }
        let entry: AuditEntry = serde_json::from_str(&line).map_err(|e| VerifyError::Decode {
            line: lineno + 1,
            source: e,
        })?;
        if entry.prev_hash != prev_hash {
            return Err(VerifyError::ChainBreak {
                line: lineno + 1,
                expected_prev: prev_hash,
                actual_prev: entry.prev_hash,
            });
        }
        let recomputed = compute_this_hash(&entry);
        if recomputed != entry.this_hash {
            return Err(VerifyError::HashMismatch {
                line: lineno + 1,
                expected: entry.this_hash,
                computed: recomputed,
            });
        }
        prev_hash = entry.this_hash;
    }
    Ok(())
}

/// Errors raised by [`verify_audit_log`].
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    /// Underlying I/O failed.
    #[error("io: {0}")]
    Io(std::io::Error),
    /// A line was not valid JSON.
    #[error("decode error at line {line}: {source}")]
    Decode {
        /// 1-indexed line number.
        line: usize,
        /// Underlying serde error.
        source: serde_json::Error,
    },
    /// Hash chain broke at this line.
    #[error("chain break at line {line}: expected prev_hash={expected_prev}, got {actual_prev}")]
    ChainBreak {
        /// 1-indexed line number.
        line: usize,
        /// What `prev_hash` should have been (hash of preceding line).
        expected_prev: String,
        /// What we actually saw.
        actual_prev: String,
    },
    /// Line's own hash didn't match its content.
    #[error("hash mismatch at line {line}: stored={expected}, computed={computed}")]
    HashMismatch {
        /// 1-indexed line number.
        line: usize,
        /// Hash recorded in the file.
        expected: String,
        /// Hash recomputed over the line's content.
        computed: String,
    },
}

// ─── internals ──────────────────────────────────────────────────────────────

fn genesis_hash() -> String {
    hex::encode(Sha256::digest(b"genesis"))
}

fn compute_this_hash(entry: &AuditEntry) -> String {
    // Hash over the entry with `this_hash` cleared so the field can be
    // reconstructed and verified independently.
    let mut without = entry.clone();
    without.this_hash = String::new();
    let bytes = serde_json::to_vec(&without).expect("entry serializes");
    hex::encode(Sha256::digest(bytes))
}

fn last_hash_in_file(path: &Path) -> std::io::Result<String> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut last: Option<String> = None;
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let entry: AuditEntry = serde_json::from_str(&line).map_err(io_other)?;
        last = Some(entry.this_hash);
    }
    Ok(last.unwrap_or_else(genesis_hash))
}

fn io_other<E: std::fmt::Display>(e: E) -> std::io::Error {
    std::io::Error::other(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn fresh_logger_starts_with_genesis() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audit.log");
        let logger = AuditLogger::open(&path).unwrap();
        assert_eq!(logger.last_hash, genesis_hash());
    }

    #[test]
    fn log_then_verify_clean_chain() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audit.log");
        let mut logger = AuditLogger::open(&path).unwrap();
        logger
            .log(AuditEventType::SessionStart, json!({ "session": "s_001" }))
            .unwrap();
        logger
            .log(AuditEventType::AgentSpawn, json!({ "kind": "codex" }))
            .unwrap();
        logger.log(AuditEventType::SessionEnd, json!({})).unwrap();
        verify_audit_log(&path).expect("clean chain verifies");
    }

    #[test]
    fn tampered_line_is_detected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audit.log");
        let mut logger = AuditLogger::open(&path).unwrap();
        logger.log(AuditEventType::SessionStart, json!({})).unwrap();
        logger.log(AuditEventType::AgentSpawn, json!({})).unwrap();
        logger.log(AuditEventType::SessionEnd, json!({})).unwrap();

        // Tamper: rewrite the file with a modified middle line.
        let raw = std::fs::read_to_string(&path).unwrap();
        let mut lines: Vec<String> = raw.lines().map(|l| l.to_string()).collect();
        let mut e: AuditEntry = serde_json::from_str(&lines[1]).unwrap();
        e.fields = json!({ "kind": "MODIFIED" });
        lines[1] = serde_json::to_string(&e).unwrap();
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();

        let err = verify_audit_log(&path).expect_err("tampered chain must fail");
        match err {
            VerifyError::ChainBreak { line, .. } | VerifyError::HashMismatch { line, .. } => {
                assert!(line >= 2);
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn resume_continues_chain() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audit.log");
        {
            let mut a = AuditLogger::open(&path).unwrap();
            a.log(AuditEventType::SessionStart, json!({})).unwrap();
        }
        {
            let mut b = AuditLogger::open(&path).unwrap();
            b.log(AuditEventType::SessionEnd, json!({})).unwrap();
        }
        verify_audit_log(&path).expect("resumed chain still verifies");
    }
}
